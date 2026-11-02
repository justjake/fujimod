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

mod mailbox;
#[cfg(test)]
mod protocol_tests;
mod sdk;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub bytes: u32,
}

/// File contents remain binary and are never serialized into the capability.
pub enum Request {
    List { pattern: String },
    Read { path: String, max_bytes: u32 },
    Write { path: String, data: Vec<u8> },
    Delete { path: String },
}
pub enum Response {
    Entries(Vec<Entry>),
    Bytes(Vec<u8>),
    Done,
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
        b[3..].iter().all(|c| {
            c.is_ascii_alphanumeric() || b"._-\\".contains(c) || listing && b"*?".contains(c)
        }),
        "Invalid card path",
    )?;
    require(
        !value[3..]
            .split('\\')
            .any(|s| s.is_empty() || s == "." || s == ".."),
        "Invalid card path component",
    )
}

struct Host<'a> {
    session: &'a mut Session,
    limits: DebugMemoryLimits,
}
impl Host<'_> {
    async fn read(&mut self, address: u32, size: u32) -> Result<Vec<u8>> {
        read_range(self.session, range(address, size)?, self.limits).await
    }
    async fn write(&mut self, address: u32, raw: &[u8]) -> Result<()> {
        write_range(self.session, Address(address), raw, self.limits).await
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
    match &request {
        Request::List { pattern } => path(pattern, true)?,
        Request::Read { path: p, .. }
        | Request::Write { path: p, .. }
        | Request::Delete { path: p } => path(p, false)?,
    }
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
    let mut host = Host { session, limits };
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
    raw.chunks_exact(24)
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
