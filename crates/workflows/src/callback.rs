//! Temporary command callback and zero-filled executable reservation borrowing.
use crate::{
    Limits,
    bulk::{self, BulkError, Qualified},
};
use fujimod_usb::{
    ff80::{
        SessionState,
        commands::{
            diagnostic::CommandStatus,
            research::{CommandCode, Request},
        },
    },
    operations::checks::sha256_bytes,
};
use serde::{Deserialize, Serialize};
use std::num::NonZeroU32;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Range {
    pub address: u64,
    pub size: u64,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CodeCheck {
    pub address: u64,
    pub size: u64,
    pub sha256: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WordCheck {
    pub address: u64,
    pub size: u64,
    pub value: u64,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Transport {
    pub callback: u64,
    pub instruction_cache_sync: u64,
    pub registered_scratch: Range,
    pub configuration_pointer: u64,
    pub configuration_bytes: u64,
    pub guard_offset: u32,
    pub debug_offset: u32,
    pub checks: Vec<CodeCheck>,
    /// Includes the service mode, translation root and executable descriptors.
    pub words: Vec<WordCheck>,
}

pub(crate) fn require(condition: bool, message: impl Into<String>) -> bulk::Result<()> {
    if condition {
        Ok(())
    } else {
        Err(BulkError::Check(message.into()))
    }
}
pub(crate) fn word(bytes: &[u8]) -> u64 {
    bytes
        .iter()
        .enumerate()
        .fold(0, |n, (i, &b)| n | (u64::from(b) << (i * 8)))
}
pub(crate) async fn write(
    reader: &mut Qualified<'_>,
    address: u64,
    bytes: &[u8],
) -> bulk::Result<()> {
    reader.access.write(address, bytes).await?;
    require(
        reader.access.raw(address, bytes.len() as u64).await? == bytes,
        "RAM write readback differs",
    )
}
pub(crate) async fn check_code(
    reader: &mut Qualified<'_>,
    checks: &[CodeCheck],
) -> bulk::Result<()> {
    for check in checks {
        let bytes = reader.access.raw(check.address, check.size).await?;
        require(
            bulk::hex(&sha256_bytes(&bytes)) == check.sha256,
            format!("Stock code differs at {:#x}", check.address),
        )?;
    }
    Ok(())
}

/// Invoke once, restoring only a recognized callback value. A completed error
/// reply permits cleanup; unknown completion forbids every subsequent command.
pub(crate) async fn invoke(
    reader: &mut Qualified<'_>,
    slot: u64,
    entry: u64,
    require_success: bool,
) -> bulk::Result<()> {
    require(
        reader.access.raw(slot, 8).await? == [0; 8],
        "Service callback is occupied",
    )?;
    let replacement = entry.to_le_bytes();
    let outcome = async {
        write(reader, slot, &replacement).await?;
        let text = b"CMD PROBE\n";
        let mut body: Vec<u8> = [5u32, 0, 0, 0, text.len() as u32]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect();
        body.extend(text);
        let request = Request::new(CommandCode(0x200001), body, NonZeroU32::new(256).unwrap())?;
        let reply = reader.access.session.research_command(&request).await?;
        require(
            reply.body().len() >= 4 && word(&reply.body()[..4]) == 5,
            "Service callback operation echo differs",
        )?;
        require(
            !require_success || reply.status == CommandStatus::Success,
            "Service callback returned an error",
        )
    }
    .await;
    let restored = async {
        require(
            reader.access.session.state() == SessionState::Ready,
            "Completion unknown; callback restoration withheld",
        )?;
        let current = reader.access.raw(slot, 8).await?;
        require(
            current == [0; 8] || current == replacement,
            "Callback ownership changed; restoration withheld",
        )?;
        if current != [0; 8] {
            write(reader, slot, &[0; 8]).await?;
        }
        Ok(())
    }
    .await;
    if restored.is_err() {
        reader.access.session.invalidate_completion();
    }
    bulk::finish(outcome, restored)
}

/// Ownership accepts mutable objects but pins every code byte and unused tail.
pub(crate) fn owns_image(
    observed: &[u8],
    image: &[u8],
    capacity: usize,
    mutable: &[Range],
    base: u64,
    uploaded: bool,
) -> bool {
    if observed.len() != capacity
        || image.len() > capacity
        || observed[image.len()..].iter().any(|&b| b != 0)
    {
        return false;
    }
    observed[..image.len()].iter().enumerate().all(|(i, &b)| {
        mutable
            .iter()
            .any(|r| base + (i as u64) >= r.address && base + (i as u64) < r.address + r.size)
            || b == image[i]
            || (!uploaded && b == 0)
    })
}

pub(crate) struct Arena<'a> {
    pub address: u64,
    pub capacity: u64,
    pub image: &'a [u8],
    pub mutable: &'a [Range],
    pub uploaded: bool,
    pub touched: bool,
}
impl Arena<'_> {
    pub(crate) async fn upload(
        &mut self,
        reader: &mut Qualified<'_>,
        limits: Limits,
    ) -> bulk::Result<()> {
        let bytes = reader
            .read(fujimod_catalog_bulk::BulkRange {
                address: self.address,
                size: self.capacity,
            })
            .await?;
        require(
            bytes.iter().all(|&b| b == 0),
            "Executable reservation is occupied",
        )?;
        self.touched = true;
        for (index, piece) in self
            .image
            .chunks(limits.memory.write.get() as usize)
            .enumerate()
        {
            write(
                reader,
                self.address + (index * limits.memory.write.get() as usize) as u64,
                piece,
            )
            .await?;
        }
        require(
            reader
                .read(fujimod_catalog_bulk::BulkRange {
                    address: self.address,
                    size: self.image.len() as u64,
                })
                .await?
                == self.image,
            "DMA readback of uploaded code differs",
        )?;
        self.uploaded = true;
        Ok(())
    }
    pub(crate) async fn restore(&self, reader: &mut Qualified<'_>, slot: u64) -> bulk::Result<()> {
        if !self.touched {
            return Ok(());
        }
        require(
            reader.access.session.state() == SessionState::Ready,
            "Completion unknown; executable reservation retained",
        )?;
        require(
            reader.access.raw(slot, 8).await? == [0; 8],
            "Callback is still published; executable reservation retained",
        )?;
        let bytes = reader
            .read(fujimod_catalog_bulk::BulkRange {
                address: self.address,
                size: self.capacity,
            })
            .await?;
        require(
            owns_image(
                &bytes,
                self.image,
                self.capacity as usize,
                self.mutable,
                self.address,
                self.uploaded,
            ),
            "Executable reservation ownership changed; restoration withheld",
        )?;
        write(reader, self.address, &vec![0; self.image.len()]).await?;
        require(
            reader
                .read(fujimod_catalog_bulk::BulkRange {
                    address: self.address,
                    size: self.capacity,
                })
                .await?
                .iter()
                .all(|&b| b == 0),
            "Executable reservation restoration differs",
        )
    }
}
