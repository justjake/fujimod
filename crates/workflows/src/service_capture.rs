//! Optional service access and streaming RAM capture from explicit located facts.
//!
//! Physical entry and battery removal are caller actions. This module never
//! changes the stored configuration, injects keys, resets USB or powers off.
use fujimod_usb::{
    ff80::{
        Session,
        commands::{
            configuration::ConfigurationView,
            diagnostic::{CommandStatus, DiagnosticTag},
            info::InfoSelector,
            memory::{Address, DebugMemoryLimits, MemoryRange},
        },
    },
    operations::{
        checks, configuration,
        data::{DataReader, DataWindow, OffsetMode},
        memory::{MemoryReader, read_range},
    },
};
use serde::{Deserialize, Serialize};
use std::num::NonZeroU32;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Usb(#[from] fujimod_usb::Error),
    #[error("unlocated or invalid service capture facts: {0}")]
    Facts(String),
    #[error("connected device does not match the located {0}")]
    Identity(&'static str),
    #[error("service capture verification failed: {0}; remove the battery before normal startup")]
    Verification(&'static str),
}
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct UsbIds {
    pub vendor_id: u16,
    pub product_id: u16,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Region {
    pub address: u32,
    pub size: u32,
}
impl Region {
    fn range(&self) -> Result<MemoryRange> {
        Ok(MemoryRange::new(
            Address(self.address),
            nonzero(self.size)?,
        )?)
    }
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CodeCheck {
    pub region: Region,
    pub sha256: [u8; 32],
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    /// Firmware key names, in the order recovered from the boot selector.
    pub physical_keys: Vec<String>,
    /// Complete located boot-source condition, suitable for printing to an operator.
    pub usb_condition: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReadGate {
    pub offset: u32,
    pub enabled_value: u8,
    /// This capability accepts only a located RAM-only, nonpersistent setter.
    pub ram_only: bool,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Bulk {
    pub tag: u32,
    pub base: u32,
    /// Exact positive diagnostic capacities. A zero-capacity path cannot read.
    pub capacities: Vec<u32>,
    pub wrapping_offsets: bool,
    pub callback_pointer: u32,
    pub callback_offset: u32,
    pub callback: u32,
    pub source_state: u32,
    pub probes: Vec<Region>,
    pub mapped_probe: CodeCheck,
    /// Host transfer bound; independent of device capacity or RAM density.
    pub chunk_bytes: u32,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReadMode {
    Bulk,
    Slow,
}

/// Partial documents can be reviewed offline. Normal USB IDs are optional
/// recognition data; service access requires an explicit finite read range.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Facts {
    pub format_version: u32,
    pub firmware_id: String,
    pub firmware_sha256: String,
    /// Optional native OS descriptor recognition; never a service prerequisite.
    pub normal_usb: Option<UsbIds>,
    pub service_usb: Option<UsbIds>,
    pub product: Option<String>,
    pub internal_version: Option<String>,
    pub entry: Option<Entry>,
    pub read_gate: Option<ReadGate>,
    pub slow_read_bytes: Option<u32>,
    pub read_mode: ReadMode,
    /// Explicit bounded read range. A slow probe may test an unknown extent;
    /// only successfully returned bytes establish readability. Never infer this
    /// range from a diagnostic capacity or assume it describes all physical RAM.
    pub ram: Option<Region>,
    pub bulk: Option<Bulk>,
    pub code_checks: Vec<CodeCheck>,
}

#[derive(Debug, Serialize)]
pub struct Review {
    pub ready: bool,
    pub blockers: Vec<String>,
    pub steps: Vec<String>,
}
fn nonzero(n: u32) -> Result<NonZeroU32> {
    NonZeroU32::new(n).ok_or_else(|| Error::Facts("zero size".into()))
}
impl Facts {
    pub fn from_json(json: &str) -> Result<Self> {
        serde_json::from_str(json).map_err(|e| Error::Facts(e.to_string()))
    }
    pub fn review(&self) -> Review {
        let mut blockers = Vec::new();
        macro_rules! require {
            ($condition:expr, $name:expr) => {
                if !$condition {
                    blockers.push($name.to_string());
                }
            };
        }
        require!(self.format_version == 1, "format_version");
        require!(
            !self.firmware_id.is_empty()
                && self.firmware_sha256.len() == 64
                && self.firmware_sha256.bytes().all(|b| b.is_ascii_hexdigit()),
            "firmware identity"
        );
        require!(
            self.service_usb
                .as_ref()
                .is_some_and(|i| i.vendor_id != 0 && i.product_id != 0),
            "service_usb"
        );
        require!(
            self.product.as_ref().is_some_and(|s| !s.is_empty()),
            "product"
        );
        require!(
            self.internal_version
                .as_ref()
                .is_some_and(|s| !s.is_empty()),
            "internal_version"
        );
        require!(
            self.entry
                .as_ref()
                .is_some_and(|e| !e.physical_keys.is_empty()
                    && e.physical_keys.iter().all(|k| !k.is_empty())
                    && !e.usb_condition.is_empty()),
            "entry inputs and USB condition"
        );
        require!(
            self.read_gate
                .as_ref()
                .is_some_and(|g| g.ram_only && g.enabled_value != 0),
            "RAM-only read gate"
        );
        require!(
            self.slow_read_bytes.is_some_and(|n| n != 0),
            "slow read path"
        );
        require!(
            self.ram.as_ref().is_some_and(|r| r.range().is_ok()),
            "explicit bounded read range"
        );
        require!(
            !self.code_checks.is_empty()
                && self.code_checks.iter().all(|c| c.region.range().is_ok()),
            "stock code checks"
        );
        if self.read_mode == ReadMode::Bulk {
            require!(
                self.bulk.as_ref().is_some_and(|b| self.valid_bulk(b)),
                "bulk read path and probes"
            );
        }
        let ids = |value: &Option<UsbIds>| {
            value.as_ref().map_or_else(
                || "unlocated".into(),
                |i| format!("{:04x}:{:04x}", i.vendor_id, i.product_id),
            )
        };
        let mut steps = vec![format!(
            "Power on holding {}; USB boot condition: {}",
            self.entry
                .as_ref()
                .map_or_else(|| "unlocated".into(), |e| e.physical_keys.join(" + ")),
            self.entry
                .as_ref()
                .map_or("unlocated", |e| e.usb_condition.as_str())
        )];
        steps.push(format!(
            "Connect native service USB {}, product {}, internal version {}",
            ids(&self.service_usb),
            self.product.as_deref().unwrap_or("unlocated"),
            self.internal_version.as_deref().unwrap_or("unlocated")
        ));
        steps.push(format!(
            "Read configuration RAM; enable only {}",
            self.read_gate.as_ref().map_or_else(
                || "the unlocated read gate".into(),
                |g| format!("RAM[page=0, offset={:#x}]={:#x}", g.offset, g.enabled_value)
            )
        ));
        steps.push(
            if self.read_mode == ReadMode::Bulk {
                "Verify stock code, callback, diagnostic source/capacity and bulk/slow probes"
            } else {
                "Verify stock code through the slow reader"
            }
            .into(),
        );
        steps.push(format!(
            "Stream {} through the {:?} reader",
            self.ram.as_ref().map_or_else(
                || "an unspecified range (refused)".into(),
                |r| format!("bounded range {:#x}+{:#x}", r.address, r.size)
            ),
            self.read_mode
        ));
        steps.push("Remove battery; disconnect USB; reinsert battery before normal startup".into());
        Review {
            ready: blockers.is_empty(),
            blockers,
            steps,
        }
    }
    fn valid_bulk(&self, b: &Bulk) -> bool {
        let Ok(()) = (|| -> Result<()> {
            let ram = self
                .ram
                .as_ref()
                .ok_or_else(|| Error::Facts("RAM".into()))?
                .range()?;
            let window = DataWindow::new(
                Address(b.base),
                ram,
                nonzero(b.chunk_bytes)?,
                if b.wrapping_offsets {
                    OffsetMode::Wrapping
                } else {
                    OffsetMode::Checked
                },
            )?;
            if b.capacities.is_empty() || b.capacities.contains(&0) || b.probes.is_empty() {
                return Err(Error::Facts("bulk probes or capacity".into()));
            }
            for probe in b
                .probes
                .iter()
                .chain(std::iter::once(&b.mapped_probe.region))
            {
                window.validate(probe.range()?)?;
            }
            MemoryRange::new(Address(b.callback_pointer), NonZeroU32::new(4).unwrap())?;
            let table_limit = b
                .callback_offset
                .checked_add(4)
                .ok_or_else(|| Error::Facts("callback offset".into()))?;
            nonzero(table_limit)?;
            MemoryRange::new(Address(b.source_state), NonZeroU32::new(8).unwrap())?;
            Ok(())
        })() else {
            return false;
        };
        true
    }
    pub fn validate(self) -> Result<Plan> {
        let review = self.review();
        if !review.ready {
            return Err(Error::Facts(review.blockers.join(", ")));
        }
        Ok(Plan(self))
    }
}

/// Constructed only from a complete validated facts document.
pub struct Plan(Facts);
enum Reader<'a> {
    Bulk(DataReader<&'a mut Session>),
    Slow(MemoryReader<&'a mut Session>),
}
/// One serialized reader owns the session borrow for the entire capture.
pub struct CaptureReader<'a>(Reader<'a>);
impl CaptureReader<'_> {
    pub fn bytes_completed(&self) -> u32 {
        match &self.0 {
            Reader::Bulk(r) => r.bytes_completed(),
            Reader::Slow(r) => r.bytes_completed(),
        }
    }
    pub async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>> {
        Ok(match &mut self.0 {
            Reader::Bulk(r) => r.next_chunk().await?,
            Reader::Slow(r) => r.next_chunk().await?,
        })
    }
}
impl Plan {
    /// Inspect a discovered USB descriptor without opening its interface.
    pub fn recognize_device(&self, device: &fujimod_usb::connection::Device) -> Result<&Entry> {
        self.recognize_normal(&UsbIds {
            vendor_id: device.vendor_id(),
            product_id: device.product_id(),
        })
    }
    /// Normal descriptors authorize the physical entry instruction only.
    pub fn recognize_normal(&self, ids: &UsbIds) -> Result<&Entry> {
        if Some(ids) != self.0.normal_usb.as_ref() {
            return Err(Error::Identity("normal USB ids"));
        }
        Ok(self.0.entry.as_ref().expect("validated entry"))
    }
    /// Check service identity before the sole possible write. No retries or exit
    /// commands are issued on success or failure. The owner ends the boot by
    /// battery removal even when verification or transfer fails.
    pub async fn prepare<'a>(
        &self,
        session: &'a mut Session,
        ids: &UsbIds,
    ) -> Result<CaptureReader<'a>> {
        let facts = &self.0;
        if Some(ids) != facts.service_usb.as_ref() {
            return Err(Error::Identity("service USB ids"));
        }
        if session.device_info(InfoSelector::Product).await?.text
            != *facts.product.as_ref().expect("validated product")
        {
            return Err(Error::Identity("product"));
        }
        if session
            .device_info(InfoSelector::InternalVersion)
            .await?
            .text
            != *facts.internal_version.as_ref().expect("validated version")
        {
            return Err(Error::Identity("internal version"));
        }
        let gate = facts.read_gate.as_ref().expect("validated gate");
        let before = configuration::read_byte(
            session,
            ConfigurationView::Ram,
            gate.offset,
            NonZeroU32::MIN,
        )
        .await?;
        if before == 0 {
            configuration::write_byte_verified(
                session,
                ConfigurationView::Ram,
                gate.offset,
                gate.enabled_value,
                NonZeroU32::MIN,
            )
            .await?;
        } else if before != gate.enabled_value {
            return Err(Error::Verification("unexpected read gate value"));
        }
        let limits = DebugMemoryLimits {
            read: nonzero(facts.slow_read_bytes.expect("validated slow reader"))?,
            write: NonZeroU32::MIN,
        };
        for check in &facts.code_checks {
            if !checks::region_matches(session, check.region.range()?, limits, check.sha256).await?
            {
                return Err(Error::Verification("stock code"));
            }
        }
        if facts.read_mode == ReadMode::Slow {
            return Ok(CaptureReader(Reader::Slow(MemoryReader::new(
                session,
                facts.ram.as_ref().expect("validated RAM").range()?,
                limits,
            ))));
        }
        let bulk = facts.bulk.as_ref().expect("validated bulk");
        let word = |data: &[u8]| u32::from_le_bytes(data.try_into().expect("four-byte read"));
        let pointer = read_range(
            session,
            Region {
                address: bulk.callback_pointer,
                size: 4,
            }
            .range()?,
            limits,
        )
        .await?;
        let slot = word(&pointer)
            .checked_add(bulk.callback_offset)
            .ok_or(Error::Verification("callback overflow"))?;
        let callback = read_range(
            session,
            Region {
                address: slot,
                size: 4,
            }
            .range()?,
            limits,
        )
        .await?;
        if word(&callback) != bulk.callback {
            return Err(Error::Verification("stock callback"));
        }
        let query = session
            .diagnostic_query(DiagnosticTag::Research(bulk.tag))
            .await?;
        if query.status != CommandStatus::Success {
            return Err(Error::Verification("diagnostic query status"));
        }
        let source = read_range(
            session,
            Region {
                address: bulk.source_state,
                size: 8,
            }
            .range()?,
            limits,
        )
        .await?;
        let capacity = word(&source[4..]);
        if word(&source[..4]) != bulk.base
            || !bulk.capacities.contains(&capacity)
            || query.fields[3] != capacity
        {
            return Err(Error::Verification("diagnostic source/capacity"));
        }
        let window = DataWindow::new(
            Address(bulk.base),
            facts.ram.as_ref().expect("validated RAM").range()?,
            nonzero(bulk.chunk_bytes.min(capacity))?,
            if bulk.wrapping_offsets {
                OffsetMode::Wrapping
            } else {
                OffsetMode::Checked
            },
        )?;
        for probe in &bulk.probes {
            let expected = read_range(session, probe.range()?, limits).await?;
            let mut reader = DataReader::new(&mut *session, window, probe.range()?)?;
            let mut at = 0;
            while let Some(bytes) = reader.next_chunk().await? {
                if expected[at..at + bytes.len()] != bytes {
                    return Err(Error::Verification("bulk/slow probe"));
                }
                at += bytes.len();
            }
        }
        // This larger independent code hash detects agreement on a stale or
        // wrong runtime mapping that a short bulk/slow comparison could miss.
        let mut reader = DataReader::new(&mut *session, window, bulk.mapped_probe.region.range()?)?;
        let mut bytes = Vec::new();
        while let Some(chunk) = reader.next_chunk().await? {
            bytes.extend(chunk);
        }
        use fujimod_usb::operations::checks::sha256_bytes;
        if sha256_bytes(&bytes) != bulk.mapped_probe.sha256 {
            return Err(Error::Verification("mapped bulk probe"));
        }
        Ok(CaptureReader(Reader::Bulk(DataReader::new(
            session,
            window,
            facts.ram.as_ref().expect("validated RAM").range()?,
        )?)))
    }
}

#[cfg(test)]
mod tests;
