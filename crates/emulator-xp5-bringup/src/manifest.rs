//! Declarative EL1 handoff. Addresses and register values are TOML integers
//! (0x hexadecimal is accepted).
use serde::{Deserialize, Serialize};
use fujimod_emulator_devices::graph::DeviceWindow;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BootState {
    #[serde(default = "version")]
    pub version: u32,
    pub firmware: String,
    #[serde(default = "yes")]
    pub load_firmware: bool,
    pub ram: Vec<RamRegion>,
    /// DRAM extent for this run, and where the body's bus-drain peripheral moves so that it
    /// does not overlap the larger RAM, when the firmware's use of DRAM differs from the
    /// catalog body's. Absent: the catalog's values. The firmware's own drain routine
    /// (FUN_0159dd8c) still reads 0x68000000, 0x80000000 and 0x1c0000000.
    #[serde(default)]
    pub dram: Option<Dram>,
    #[serde(default)]
    pub preload: Vec<Preload>,
    #[serde(default)]
    pub device: Vec<DeviceWindow>,
    /// Card media of each SD slot, keyed by its UHS-I host base. Presence and
    /// write protection are run inputs; the card is formatted blank.
    #[serde(default)]
    pub sd: Vec<fujimod_emulator_xp5_board::board::sd::MediaInputs>,
    pub cpu: Vec<CpuState>,
    pub gic: Option<Gic>,
}
fn version() -> u32 {
    1
}
fn yes() -> bool {
    true
}
fn cpsr() -> u64 {
    0x3c5
}
fn stride() -> u64 {
    0x20000
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Dram {
    /// Exclusive end of the DRAM address space, page aligned.
    pub limit: u64,
    /// Address the body's four-byte bus-drain peripheral answers at; at or past `limit`, so
    /// that it does not overlap the DRAM.
    pub bus_drain: u64,
}
impl Dram {
    /// Refuse an override that the run could not honor. `run_sets_limit` is whether the run
    /// configuration also sets `dram_address_space`: two sources for one limit.
    pub fn validate(&self, run_sets_limit: bool) -> Result<(), String> {
        if self.limit == 0 || self.limit % 4096 != 0 {
            return Err("manifest DRAM limit must be page-aligned".into());
        }
        if self.bus_drain % 4 != 0 {
            return Err(format!(
                "manifest DRAM bus_drain {:#x} must be four-byte aligned",
                self.bus_drain
            ));
        }
        if self.bus_drain < self.limit {
            return Err(format!(
                "manifest DRAM bus_drain {:#x} lies inside the DRAM below limit {:#x}",
                self.bus_drain, self.limit
            ));
        }
        if run_sets_limit {
            return Err(
                "manifest [dram] and the run configuration's dram_address_space both set the \
                 DRAM limit; use one"
                    .into(),
            );
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RamRegion {
    pub base: u64,
    pub size: u64,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Preload {
    pub address: u64,
    /// XOR offset in the update file; length and sha256 are mandatory.
    pub dat_offset: Option<u64>,
    pub length: Option<u64>,
    pub sha256: Option<String>,
    /// Hex byte string, spaces accepted. Exactly one of bytes/value/dat_offset.
    pub bytes: Option<String>,
    pub value: Option<u64>,
    /// Little-endian value width (1,2,4,8), required with value.
    pub size: Option<u8>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeviceFragment {
    device: Vec<DeviceWindow>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CpuState {
    pub id: usize,
    pub mpidr: u64,
    pub pc: u64,
    pub sp: u64,
    #[serde(default = "cpsr")]
    pub cpsr: u64,
    #[serde(default)]
    pub running: bool,
    /// Start a parked reset core when stock primary writes its shared-RAM flag.
    pub release_when: Option<ReleaseWord>,
    #[serde(default)]
    pub regs: BTreeMap<String, u64>,
    #[serde(default)]
    pub sysregs: BTreeMap<String, u64>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseWord {
    pub address: u64,
    pub value: u32,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Gic {
    pub distributor: u64,
    pub redistributor: u64,
    #[serde(default = "stride")]
    pub redistributor_stride: u64,
    pub spi_max: Option<u32>,
    /// Distributor register offsets and values left by the skipped boot stage.
    #[serde(default)]
    pub registers: BTreeMap<String, u64>,
}
/// A firmware's boot manifest and stream layout, compiled into the crate.
pub(crate) struct Bundled {
    pub firmware: &'static str,
    pub manifest: &'static str,
    pub fragments: &'static [&'static str],
    pub streams: &'static str,
}

include!(concat!(env!("OUT_DIR"), "/bundled.rs"));

pub(crate) fn bundled(firmware: &str) -> Result<&'static Bundled, String> {
    BUNDLED
        .iter()
        .find(|bundled| bundled.firmware == firmware)
        .ok_or_else(|| format!("no XP5 boot manifest is bundled for {firmware}"))
}

impl BootState {
    /// The bundled manifest of `firmware`. `FUJIMOD_XP5_MANIFEST=FILE` replaces it with a manifest
    /// file (and its sibling `.d` directory of device fragments) for experiments and tests.
    pub fn for_firmware(firmware: &str) -> Result<Self, String> {
        let state = match std::env::var_os("FUJIMOD_XP5_MANIFEST") {
            Some(path) => Self::load(Path::new(&path))?,
            None => {
                let bundled = bundled(firmware)?;
                Self::parse(bundled.manifest, bundled.fragments)?
            }
        };
        if state.firmware != firmware {
            return Err(format!("boot manifest is for {}, not {firmware}", state.firmware));
        }
        Ok(state)
    }

    fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let directory = path.with_extension("d");
        let mut names = match std::fs::read_dir(&directory) {
            Ok(entries) => entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|fragment| fragment.extension().is_some_and(|extension| extension == "toml"))
                .collect::<Vec<_>>(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(format!("{}: {e}", directory.display())),
        };
        names.sort();
        let fragments = names
            .iter()
            .map(|fragment| {
                std::fs::read_to_string(fragment).map_err(|e| format!("{}: {e}", fragment.display()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Self::parse(&text, &fragments.iter().map(String::as_str).collect::<Vec<_>>())
            .map_err(|error| format!("{}: {error}", path.display()))
    }

    fn parse(text: &str, fragments: &[&str]) -> Result<Self, String> {
        let mut s: Self = toml::from_str(text).map_err(|e| e.to_string())?;
        for fragment in fragments {
            let parsed: DeviceFragment = toml::from_str(fragment).map_err(|e| e.to_string())?;
            s.device.extend(parsed.device);
        }
        if s.version != 1 {
            return Err(format!("unsupported boot state version {}", s.version));
        }
        if s.cpu.is_empty() || s.cpu.iter().enumerate().any(|(i, c)| c.id != i) {
            return Err("CPU ids must be contiguous from zero".into());
        }
        if s.cpu.iter().filter(|c| c.running).count() != 1 {
            return Err("exactly one boot CPU must have running=true".into());
        }
        Ok(s)
    }
}

impl Preload {
    pub fn load_bytes(&self, firmware: &crate::firmware::Firmware) -> Result<Vec<u8>, String> {
        let count = usize::from(self.bytes.is_some())
            + usize::from(self.value.is_some())
            + usize::from(self.dat_offset.is_some());
        if count != 1 {
            return Err("preload requires exactly one of bytes/value/dat_offset".into());
        }
        let bytes = if let Some(offset) = self.dat_offset {
            return firmware.xor_dat_range(
                offset,
                self.length.ok_or("DAT preload requires length")?,
                self.sha256
                    .as_deref()
                    .ok_or("DAT preload requires sha256")?,
            );
        } else if let Some(hex) = &self.bytes {
            let hex: String = hex.chars().filter(|c| !c.is_whitespace()).collect();
            if !hex.is_ascii() || hex.len() % 2 != 0 {
                return Err("invalid preload hex bytes".into());
            }
            (0..hex.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).map_err(|e| e.to_string()))
                .collect::<Result<Vec<_>, _>>()?
        } else {
            let size = self.size.ok_or("value preload requires size")?;
            if ![1, 2, 4, 8].contains(&size) {
                return Err("invalid value preload width".into());
            }
            let value = self.value.unwrap();
            if size < 8 && value >= 1 << (size * 8) {
                return Err("value does not fit preload width".into());
            }
            value.to_le_bytes()[..size as usize].to_vec()
        };
        if let Some(hash) = &self.sha256 {
            if crate::firmware::sha256_hex(&bytes) != *hash {
                return Err("preload hash mismatch".into());
            }
        }
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::Dram;

    #[test]
    fn dram_override_refuses_what_the_run_could_not_honor() {
        let dram = |limit, bus_drain| Dram { limit, bus_drain };
        assert!(dram(0x8000_0000, 0x8000_0000).validate(false).is_ok());
        assert!(dram(0x8000_0000, 0x8000_0002).validate(false).unwrap_err().contains("aligned"));
        assert!(dram(0x8000_0000, 0x6800_0000).validate(false).unwrap_err().contains("inside"));
        assert!(dram(0x8000_0000, 0x8000_0000).validate(true).unwrap_err().contains("dram_address_space"));
    }
}
