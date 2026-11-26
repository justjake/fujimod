//! Emulator-only guest-memory patches for diagnostic runs.
//!
//! A patch file lists byte replacements at physical addresses and the guest time (milliseconds
//! after the run starts) at which each is applied. Every entry names the bytes it expects to
//! replace; a run whose guest holds other bytes stops instead of writing. Patches touch guest RAM
//! only, for the length of one run, and never write a firmware update file.
use crate::host::Memory;
use serde::Deserialize;
use std::{collections::VecDeque, ops::Range, path::Path};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    #[serde(default)]
    patch: Vec<Entry>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    address: u64,
    original_hex: String,
    patched_hex: String,
    /// Guest milliseconds after the run starts; the code must not have run yet.
    at_ms: u64,
    reason: String,
}

pub(crate) struct Patch {
    address: u64,
    original: Vec<u8>,
    patched: Vec<u8>,
    at_ms: u64,
    reason: String,
}

fn bytes(text: &str) -> Result<Vec<u8>, String> {
    let digits: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    if !digits.is_ascii() {
        return Err(format!("non-ASCII hex string {text:?}"));
    }
    if !digits.len().is_multiple_of(2) {
        return Err(format!("odd hex string {text:?}"));
    }
    (0..digits.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&digits[i..i + 2], 16).map_err(|e| format!("{text:?}: {e}")))
        .collect()
}

fn check_range(ranges: &[Range<u64>], address: u64, length: u64) -> Result<(), String> {
    if let Some(end) = address.checked_add(length)
        && (ranges.is_empty()
            || ranges
                .iter()
                .any(|range| range.contains(&address) && end <= range.end))
    {
        return Ok(());
    }
    Err(format!(
        "patch at {address:#x}+{length:#x} is outside configured RAM windows"
    ))
}

/// Ordered checked byte replacements relative to the current run.
#[derive(Default)]
pub struct Schedule {
    patches: VecDeque<Patch>,
}

impl Schedule {
    pub fn from_path(path: Option<&Path>, allowed: &[Range<u64>]) -> Result<Self, String> {
        match path {
            Some(path) => Ok(Self {
                patches: load(path, allowed)?.into(),
            }),
            None => Ok(Self::default()),
        }
    }

    /// Apply at the stopped boundary and return the next guest deadline.
    pub fn advance(
        &mut self,
        memory: &dyn Memory,
        now: u64,
        start: u64,
        ticks_per_second: u64,
    ) -> Result<Option<u64>, String> {
        let deadline = |patch: &Patch| start.saturating_add(patch.at_ticks(ticks_per_second));
        while let Some(patch) = self.patches.front() {
            let at = deadline(patch);
            if now < at {
                return Ok(Some(at));
            }
            patch.apply(memory, now)?;
            self.patches.pop_front();
        }
        Ok(None)
    }
}

/// Read a patch file; entries are ordered by their time. Ticks are not needed until the run starts.
fn load(path: &Path, allowed: &[Range<u64>]) -> Result<Vec<Patch>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let file: File = toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut patches = file
        .patch
        .into_iter()
        .map(|entry| {
            let original = bytes(&entry.original_hex)?;
            let patched = bytes(&entry.patched_hex)?;
            if original.is_empty()
                || original.len() != patched.len()
                || entry.reason.trim().is_empty()
            {
                return Err(format!(
                    "patch at {:#x} needs equal nonempty byte strings and a reason",
                    entry.address
                ));
            }
            check_range(allowed, entry.address, original.len() as u64)?;
            Ok(Patch {
                address: entry.address,
                original,
                patched,
                at_ms: entry.at_ms,
                reason: entry.reason,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    patches.sort_by_key(|patch| patch.at_ms);
    Ok(patches)
}

impl Patch {
    /// Guest ticks after the run starts at which the patch applies.
    pub(crate) fn at_ticks(&self, ticks_per_second: u64) -> u64 {
        (u128::from(self.at_ms) * u128::from(ticks_per_second) / 1000).min(u128::from(u64::MAX))
            as u64
    }

    pub(crate) fn apply(&self, memory: &dyn Memory, now: u64) -> Result<(), String> {
        let mut current = vec![0u8; self.original.len()];
        memory
            .read(self.address, &mut current)
            .map_err(|e| format!("patch at {:#x}: {e}", self.address))?;
        if current != self.original {
            return Err(format!(
                "patch at {:#x} expects {:02x?} but the guest holds {:02x?}",
                self.address, self.original, current
            ));
        }
        memory
            .write(self.address, &self.patched)
            .map_err(|e| format!("patch at {:#x}: {e}", self.address))?;
        eprintln!(
            "FUJIMOD patch: {:#x} {:02x?} -> {:02x?} at {now}: {}",
            self.address, self.original, self.patched, self.reason
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const RANGES: [Range<u64>; 2] = [0..0x8000_0000, 0xfa20_0000..0xfa30_0000];

    #[test]
    fn hex_strings_decode_and_odd_ones_are_refused() {
        assert_eq!(bytes("f9 d0").unwrap(), [0xf9, 0xd0]);
        assert_eq!(bytes("00bf").unwrap(), [0x00, 0xbf]);
        assert!(bytes("f9d").is_err());
        assert!(bytes("zz").is_err());
    }

    #[test]
    fn patches_may_name_companion_sram_or_dram_and_nothing_else() {
        assert!(check_range(&RANGES, 0xfa20_0000, 2).is_ok());
        assert!(check_range(&RANGES, 0x100, 4).is_ok());
        assert!(check_range(&RANGES, 0x3000_0100, 4).is_ok());
        assert!(check_range(&RANGES, 0x7fff_fffc, 4).is_ok());
        assert!(check_range(&RANGES, 0x7fff_fffe, 4).is_err());
        assert!(check_range(&RANGES, 0xf000_0000, 4).is_err());
        assert!(check_range(&RANGES, 0xff90_7200, 4).is_err());
        assert!(check_range(&RANGES, u64::MAX, 4).is_err());
    }

    fn sample(name: &str) -> std::path::PathBuf {
        let path =
            std::env::temp_dir().join(format!("patch-sample-{name}-{}.toml", std::process::id()));
        std::fs::write(
            &path,
            "[[patch]]\naddress = 0xfa2057f6\noriginal_hex = \"0000\"\npatched_hex = \"00bf\"\nat_ms = 5\nreason = \"x\"\n\
             [[patch]]\naddress = 0x30000100\noriginal_hex = \"00000000\"\npatched_hex = \"01000000\"\nat_ms = 9\nreason = \"y\"\n",
        )
        .unwrap();
        path
    }

    #[test]
    fn a_patch_file_loads_its_entries() {
        let path = sample("entries");
        let patches = load(&path, &RANGES).unwrap();
        std::fs::remove_file(&path).ok();
        assert_eq!(patches.len(), 2);
    }

    #[test]
    fn patch_times_convert_to_ticks_when_the_run_starts() {
        let path = sample("ticks");
        let patches = load(&path, &RANGES).unwrap();
        std::fs::remove_file(&path).ok();
        assert_eq!(patches[0].at_ticks(1_000), 5);
        assert_eq!(patches[0].at_ticks(2_000), 10);
    }

    #[test]
    fn patches_outside_the_companion_sram_and_dram_are_refused() {
        assert!(check_range(&RANGES, 0xfa20_57f6, 2).is_ok());
        assert!(check_range(&RANGES, 0xfa2f_fffe, 2).is_ok());
        assert!(check_range(&RANGES, 0xfa2f_ffff, 2).is_err());
        assert!(check_range(&RANGES, 0x8000_0000, 2).is_err());
        assert!(check_range(&RANGES, u64::MAX, 2).is_err());
    }
}
