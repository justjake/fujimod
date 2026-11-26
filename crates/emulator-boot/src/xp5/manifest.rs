//! Declarative EL1 handoff. Addresses and register values are TOML integers
//! (0x hexadecimal is accepted).
use fujimod_emulator_devices::graph::DeviceWindow;
use serde::{Deserialize, Serialize};
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
    /// Address windows the companion core sees in addition to the SoC's own, for a body whose
    /// companion firmware reaches DRAM the SoC windows do not cover.
    #[serde(default)]
    pub companion_window: Vec<CompanionWindow>,
    #[serde(default)]
    pub device: Vec<DeviceWindow>,
    /// Card media of each SD slot, keyed by its UHS-I host base. Presence and
    /// write protection are run inputs; the card is formatted blank.
    #[serde(default)]
    pub sd: Vec<fujimod_emulator_xp5_board::board::sd::MediaInputs>,
    /// Run the M-profile processors in block execution instead of under the instruction
    /// observer with its loop windows. Both reach the same guest state; this only chooses the
    /// faster host path for the body's M-profile work, so it is not part of the wiring that a
    /// snapshot binds to. `FUJIMOD_MPROFILE_INSTRUCTIONS` in the environment overrides it.
    #[serde(default, skip_serializing)]
    pub mprofile_block_execution: bool,
}
fn version() -> u32 {
    1
}
fn yes() -> bool {
    true
}
/// One window of the companion core's address space onto the application cores' physical memory.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CompanionWindow {
    /// Address the companion core uses.
    pub address: u64,
    /// Physical address it reaches.
    pub physical: u64,
    pub size: u64,
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
        if self.limit == 0 || !self.limit.is_multiple_of(4096) {
            return Err("manifest DRAM limit must be page-aligned".into());
        }
        if !self.bus_drain.is_multiple_of(4) {
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
pub use fujimod_catalog::firmware::Preload;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeviceFragment {
    device: Vec<DeviceWindow>,
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
    /// Load the bundled manifest or a caller-selected manifest and its sibling device fragments.
    pub fn for_firmware(firmware: &str, manifest: Option<&Path>) -> Result<Self, String> {
        let state = match manifest {
            Some(path) => Self::load(path)?,
            None => {
                let bundled = bundled(firmware)?;
                Self::parse(bundled.manifest, bundled.fragments)?
            }
        };
        if state.firmware != firmware {
            return Err(format!(
                "boot manifest is for {}, not {firmware}",
                state.firmware
            ));
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
                .filter(|fragment| {
                    fragment
                        .extension()
                        .is_some_and(|extension| extension == "toml")
                })
                .collect::<Vec<_>>(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(format!("{}: {e}", directory.display())),
        };
        names.sort();
        let fragments = names
            .iter()
            .map(|fragment| {
                std::fs::read_to_string(fragment)
                    .map_err(|e| format!("{}: {e}", fragment.display()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Self::parse(
            &text,
            &fragments.iter().map(String::as_str).collect::<Vec<_>>(),
        )
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
        Ok(s)
    }
}

pub(crate) fn preload_bytes(
    preload: &Preload,
    firmware: &super::firmware::Firmware,
) -> Result<Vec<u8>, String> {
    let count = usize::from(preload.words.is_some())
        + usize::from(preload.value.is_some())
        + usize::from(preload.dat_offset.is_some());
    if count != 1 {
        return Err("preload requires exactly one of words, value or DAT offset".into());
    }
    if let Some(offset) = preload.dat_offset {
        return firmware.xor_dat_range(
            offset,
            preload.length.ok_or("DAT preload requires length")?,
            preload
                .sha256
                .as_deref()
                .ok_or("DAT preload requires SHA-256")?,
        );
    }
    let (words, width) = match &preload.words {
        Some(words) => (
            words.as_slice(),
            preload.word_size.ok_or("word preload requires word_size")?,
        ),
        None => (
            std::slice::from_ref(preload.value.as_ref().unwrap()),
            preload.size.ok_or("value preload requires size")?,
        ),
    };
    if ![1, 2, 4, 8].contains(&width) {
        return Err("invalid preload word width".into());
    }
    let mut bytes = Vec::with_capacity(words.len() * width as usize);
    for &word in words {
        if width < 8 && word >= 1 << (width * 8) {
            return Err("word does not fit preload width".into());
        }
        bytes.extend_from_slice(&word.to_le_bytes()[..width as usize]);
    }
    if let Some(hash) = &preload.sha256
        && super::firmware::sha256_hex(&bytes) != *hash
    {
        return Err("preload hash mismatch".into());
    }
    Ok(bytes)
}

pub(crate) fn preload_length(preload: &Preload) -> Result<u64, String> {
    if let Some(words) = &preload.words {
        return (words.len() as u64)
            .checked_mul(preload.word_size.ok_or("word preload requires word_size")?)
            .ok_or_else(|| "preload length overflows".into());
    }
    preload
        .length
        .or(preload.size)
        .ok_or_else(|| "preload has no length".into())
}

#[cfg(test)]
mod tests {
    use super::Dram;

    #[test]
    fn dram_override_refuses_what_the_run_could_not_honor() {
        let dram = |limit, bus_drain| Dram { limit, bus_drain };
        assert!(dram(0x8000_0000, 0x8000_0000).validate(false).is_ok());
        assert!(
            dram(0x8000_0000, 0x8000_0002)
                .validate(false)
                .unwrap_err()
                .contains("aligned")
        );
        assert!(
            dram(0x8000_0000, 0x6800_0000)
                .validate(false)
                .unwrap_err()
                .contains("inside")
        );
        assert!(
            dram(0x8000_0000, 0x8000_0000)
                .validate(true)
                .unwrap_err()
                .contains("dram_address_space")
        );
    }
}
