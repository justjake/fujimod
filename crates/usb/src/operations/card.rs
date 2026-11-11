//! Card files over the debug link. All addresses come from a selected capability.
//! The installed-service protocol is shared with the report file service.
use crate::{
    Error, Result,
    ff80::{
        Session,
        commands::{
            info::InfoSelector,
            memory::{Address, DebugMemoryLimits, MemoryRange},
        },
    },
    operations::memory::{read_range, write_range},
};
pub use fujimod_catalog_card::{FirmwareCard, Mechanism};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    num::NonZeroU32,
    time::{Duration, Instant},
};

/// Bound one request while the backend streams through its owned buffer.
pub const MAX_REQUEST_BYTES: u32 = 8 * 1024 * 1024;

/// Caller-qualified buffer transports keep file protocols independent of USB
/// acceleration. Reads bound DMA to an admitted aperture; uploads own their padding.
pub trait BufferIo: Send {
    /// Admit an owned DMA upload. Cached-buffer callers clean/invalidate before
    /// delegation, then synchronize again before the device consumes the data.
    fn admits_write(&self, _address: u32, _size: u32, _capacity: u32) -> bool {
        false
    }

    /// Return true after uploading only within this caller-owned buffer. False
    /// selects bounded CPU writes. Control words always use the CPU writer.
    fn write_owned<'a>(
        &'a mut self,
        _session: &'a mut Session,
        _address: u32,
        _data: &'a [u8],
        _capacity: u32,
    ) -> crate::emulator::EndpointFuture<'a, Result<bool>> {
        Box::pin(async { Ok(false) })
    }
    fn admits(&self, address: u32, size: u32) -> bool;
    fn read<'a>(
        &'a mut self,
        session: &'a mut Session,
        address: u32,
        size: u32,
    ) -> crate::emulator::EndpointFuture<'a, Result<Vec<u8>>>;
}

mod mailbox;
/// A client-owned service entered through a temporarily borrowed query slot.
#[derive(Clone, Copy, Debug)]
pub struct QueryTrigger {
    pub slot: u32,
    pub stock: u32,
    pub outputs: u32,
    pub entry: u32,
    pub argument: u32,
}
#[cfg(test)]
mod protocol_tests;
mod sdk;
mod volume;
pub use volume::Volume;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub bytes: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DirectoryEntry {
    pub name: String,
    pub bytes: u32,
    pub directory: bool,
    pub read_only: bool,
}

/// File contents remain binary and are never serialized into the capability.
pub enum Request {
    List {
        pattern: String,
    },
    Read {
        path: String,
        max_bytes: u32,
    },
    Write {
        path: String,
        data: Vec<u8>,
    },
    Delete {
        path: String,
    },
    ReadDirectory {
        path: String,
    },
    ReadAt {
        path: String,
        offset: u32,
        size: u32,
    },
    WriteAt {
        path: String,
        offset: u32,
        data: Vec<u8>,
    },
    Create {
        path: String,
    },
    /// Close an existing file with a nonzero byte limit.
    Shrink {
        path: String,
        size: u32,
    },
}
pub enum Response {
    Entries(Vec<Entry>),
    Bytes(Vec<u8>),
    Done,
    Directory(Vec<DirectoryEntry>),
}

fn validate_request(request: &Request) -> Result<()> {
    match request {
        Request::List { pattern } => path(pattern, true),
        Request::ReadDirectory { path: p } => {
            if p == "A:\\" {
                Ok(())
            } else {
                path(p, false)
            }
        }
        Request::Read { path: p, .. }
        | Request::Write { path: p, .. }
        | Request::Delete { path: p }
        | Request::ReadAt { path: p, .. }
        | Request::WriteAt { path: p, .. }
        | Request::Create { path: p }
        | Request::Shrink { path: p, .. } => path(p, false),
    }?;
    match request {
        Request::Shrink { size, .. } => require(*size != 0, "Use Create to truncate to zero"),
        Request::ReadAt { offset, size, .. } => require(
            *size <= MAX_REQUEST_BYTES && offset.checked_add(*size).is_some(),
            "Invalid bounded card read",
        ),
        Request::WriteAt { offset, data, .. } => require(
            data.len() <= MAX_REQUEST_BYTES as usize
                && u64::from(*offset) + data.len() as u64 <= u64::from(u32::MAX),
            "Invalid bounded card write",
        ),
        _ => Ok(()),
    }
}

fn fail(message: impl Into<String>) -> Error {
    Error::Card(message.into())
}
fn require(ok: bool, message: &str) -> Result<()> {
    if ok { Ok(()) } else { Err(fail(message)) }
}
fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().expect("checked field"))
}
fn words(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}
fn range(address: u32, size: u32) -> Result<MemoryRange> {
    MemoryRange::new(
        Address(address),
        NonZeroU32::new(size).ok_or(Error::InvalidLength)?,
    )
}
fn path(value: &str, listing: bool) -> Result<()> {
    let b = value.as_bytes();
    require(
        b.len() >= 4 && b.len() < 48 && b[0] == b'A' && &b[1..3] == b":\\",
        "Only SD0 A-drive paths are allowed",
    )?;
    require(
        b[3..]
            .iter()
            .all(|c| is_short_name_byte(*c) || *c == b'\\' || listing && b"*?".contains(c)),
        "Invalid card path",
    )?;
    require(
        !value[3..]
            .split('\\')
            .any(|s| s.is_empty() || s == "." || s == ".."),
        "Invalid card path component",
    )
}

/// ASCII characters accepted in FAT short names, including generated aliases.
pub fn is_short_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b".$%'-_@~`!(){}^#&".contains(&byte)
}

struct Host<'a> {
    session: &'a mut Session,
    limits: DebugMemoryLimits,
    io: Option<&'a mut dyn BufferIo>,
}
impl Host<'_> {
    async fn read(&mut self, address: u32, size: u32) -> Result<Vec<u8>> {
        if let Some(reader) = self.io.as_mut().filter(|r| r.admits(address, size)) {
            return reader.read(self.session, address, size).await;
        }
        read_range(self.session, range(address, size)?, self.limits).await
    }
    async fn write(&mut self, address: u32, raw: &[u8]) -> Result<()> {
        write_range(self.session, Address(address), raw, self.limits).await
    }
    async fn write_owned(&mut self, address: u32, raw: &[u8], capacity: u32) -> Result<()> {
        if let Some(io) = self
            .io
            .as_mut()
            .filter(|io| io.admits_write(address, raw.len() as u32, capacity))
            && io.write_owned(self.session, address, raw, capacity).await?
        {
            return Ok(());
        }
        self.write(address, raw).await
    }
    async fn word(&mut self, address: u32) -> Result<u32> {
        Ok(u32_at(&self.read(address, 4).await?, 0))
    }
    async fn put(&mut self, address: u32, value: u32) -> Result<()> {
        self.write(address, &value.to_le_bytes()).await
    }
    async fn check(&mut self, address: u32, raw: &[u8]) -> Result<()> {
        require(
            self.read(
                address,
                raw.len().try_into().map_err(|_| Error::InvalidLength)?,
            )
            .await?
                == raw,
            "Borrowed RAM changed",
        )
    }
}

/// Runs one operation with exclusive session ownership, including preparation
/// and cleanup. Timeouts retain outstanding buffers and fence the session.
pub async fn execute(
    session: &mut Session,
    limits: DebugMemoryLimits,
    capability: &FirmwareCard,
    request: Request,
) -> Result<Response> {
    capability.validate().map_err(fail)?;
    require(
        capability.schema == 1 && capability.schema_uri == fujimod_catalog_card::SCHEMA_URI,
        "Unsupported card capability",
    )?;
    validate_request(&request)?;
    require(
        session
            .device_info(InfoSelector::Product)
            .await?
            .text
            .trim()
            == capability.product
            && session
                .device_info(InfoSelector::InternalVersion)
                .await?
                .text
                == capability.internal_version,
        "Card capability identifies another firmware",
    )?;
    let mut host = Host {
        session,
        limits,
        io: None,
    };
    for c in &capability.code_checks {
        let bytes = host
            .read(
                c.address.try_into().map_err(|_| Error::AddressOverflow)?,
                c.size.try_into().map_err(|_| Error::InvalidLength)?,
            )
            .await?;
        require(
            format!("{:x}", Sha256::digest(bytes)) == c.sha256,
            "Stock card instructions differ",
        )?;
    }
    match &capability.mechanism {
        Mechanism::Mailbox {
            search_address,
            search_bytes,
            address_alias,
            build,
        } => {
            mailbox::execute(
                host,
                *search_address,
                *search_bytes,
                *address_alias,
                *build,
                request,
            )
            .await
        }
        Mechanism::StockSdk(profile) => sdk::execute(host, profile, request).await,
    }
}

fn decode_entries(raw: &[u8]) -> Result<Vec<Entry>> {
    require(raw.len().is_multiple_of(24), "Malformed directory page")?;
    raw.as_chunks::<24>()
        .0
        .iter()
        .map(|row| {
            let n = row[4..]
                .iter()
                .position(|&v| v == 0)
                .ok_or_else(|| fail("Unterminated directory name"))?;
            let name = &row[4..4 + n];
            require(
                !name.is_empty()
                    && row[4 + n..].iter().all(|&v| v == 0)
                    && name
                        .iter()
                        .all(|b| b.is_ascii_graphic() && !b"\\/:".contains(b))
                    && name != b"."
                    && name != b"..",
                "Invalid directory entry",
            )?;
            Ok(Entry {
                name: String::from_utf8(name.to_vec())
                    .map_err(|_| fail("Invalid directory name"))?,
                bytes: u32_at(row, 0),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn card_paths_cannot_escape_to_another_drive_or_parent() {
        for p in [
            "B:\\X.BIN",
            "A:\\..\\X.BIN",
            "A:\\X\\.\\Y.BIN",
            "A:\\X\0.BIN",
            "A:\\X*.BIN",
        ] {
            assert!(path(p, false).is_err(), "{p}");
        }
        assert!(path("A:\\FUJIMOD\\REPORTS\\R*.TAR", true).is_ok());
    }
    #[test]
    fn directory_records_are_exact_and_have_safe_names() {
        let mut raw = vec![0; 24];
        raw[..4].copy_from_slice(&100u32.to_le_bytes());
        raw[4..9].copy_from_slice(b"X.BIN");
        assert_eq!(
            decode_entries(&raw).unwrap(),
            vec![Entry {
                name: "X.BIN".into(),
                bytes: 100
            }]
        );
        raw[23] = 1;
        assert!(decode_entries(&raw).is_err());
        assert!(decode_entries(&raw[..23]).is_err());
    }
}
