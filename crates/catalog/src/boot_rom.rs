//! Located boot-ROM card facts, keyed by the ROM's SHA-256.
//!
//! One document describes one ROM image: the FAT mount, open and read entries,
//! the low-mirror SD command table the loader relocates, the power-hold
//! register and the file-system pointer word. Firmware documents reference the
//! ROM by hash instead of repeating these addresses.
use serde::{Deserialize, Serialize};

pub const BOOT_ROM_SCHEMA_URI: &str = "https://fujimod.com/schemas/boot-rom.generated.schema.json";

/// Card facts for one boot ROM image.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(missing_as_null)
)]
pub struct BootRom {
    /// Schema identifying this generated document.
    #[serde(rename = "$schema")]
    pub schema_uri: String,
    /// Document version.
    pub schema: u32,
    /// Stable document identity.
    pub id: String,
    /// SHA-256 of the ROM image.
    pub sha256: String,
    /// FAT mount, open and read entries.
    pub routines: BootRomRoutines,
    /// The low-mirror SD command table the loader relocates.
    pub command_table: BootRomCommandTable,
    /// The ROM's file-system pointer word.
    pub filesystem_pointer: BootRomFilesystemPointer,
    /// The power-hold register the loader reasserts.
    pub power_hold: BootRomPowerHold,
    /// RAM address the boot ROM hands control to.
    pub load_address: u64,
}

/// The three ROM file routines the NAND bootloader calls.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct BootRomRoutines {
    /// Mount entry.
    pub mount: u64,
    /// Open entry.
    pub open: u64,
    /// Read entry.
    pub read: u64,
}

/// SD command table copied over stage-1 bytes while the ROM runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct BootRomCommandTable {
    /// High-mirror source address.
    pub source: u64,
    /// Low-mirror destination address.
    pub destination: u64,
    /// Bytes copied.
    pub bytes: u64,
}

/// The word the ROM file service uses to publish its current volume.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct BootRomFilesystemPointer {
    /// Address of the pointer word.
    pub address: u64,
    /// Stock value at stage-1 entry.
    pub entry_value: u32,
}

/// Power-hold register asserted before any ROM card access.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct BootRomPowerHold {
    /// Register address.
    pub address: u64,
    /// Bit held high.
    pub bit: u32,
}

impl BootRom {
    /// Validate identity, hash and relocated state.
    pub fn validate(&self) -> Result<(), String> {
        fujimod_catalog_identity::validate_id(&self.id)?;
        if self.schema_uri != BOOT_ROM_SCHEMA_URI
            || self.schema != 1
            || self.sha256.len() != 64
            || !self.sha256.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(format!("boot ROM {}: invalid schema or hash", self.id));
        }
        if self.routines.mount == 0 || self.routines.open == 0 || self.routines.read == 0 {
            return Err("boot-ROM file routines are required".into());
        }
        if self.command_table.bytes == 0 || self.command_table.bytes % 8 != 0 {
            return Err("invalid command table".into());
        }
        if self.power_hold.bit >= 32 {
            return Err("invalid power-hold bit".into());
        }
        if self.load_address % 4 != 0 {
            return Err("invalid boot-ROM handoff address".into());
        }
        Ok(())
    }
}
