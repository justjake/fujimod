//! The stock user profile of an XP5 loader start.
//!
//! A run that names camera settings (`setup=done`, `language=en`) boots from the
//! user profile the firmware's catalog settings layout describes, built by
//! `fujimod_settings` from the caller's update file at run time:
//!
//! - the eight-byte profile header goes into the loader's hand-off block at the
//!   layout's `loader_offset`, which stock code reads on the first profile load,
//!   and into the power-controller registers from `power_register`, which it
//!   reads on later loads;
//! - each sealed journal record is seeded into the flash storage at its erase block
//!   (64 KiB on serial NOR, 256 KiB on serial NAND), where the stock journal reader
//!   finds it.
//!
//! No firmware bytes are stored: the records start from the update's own
//! defaults and the catalog only names addresses, sizes and hashes.

use super::{firmware::Firmware, manifest::{BootState, Preload}};
use fujimod_emulator_devices::graph::DeviceWindow;
use fujimod_emulator_xp5_storage::nand_storage;
use fujimod_catalog_resolution::ResolvedConfiguration;
use std::collections::BTreeMap;

/// One serial NOR flash unit of a profile record's `block` number.
const NOR_UNIT: u64 = 0x1_0000;
/// One serial NAND erase block of a profile record's `block` number.
const NAND_UNIT: u64 = nand_storage::PAGE as u64 * nand_storage::PAGES_PER_BLOCK as u64;

/// A profile ready for the board: the requested settings (part of the run's
/// identity) and the sealed records as `(flash offset, bytes)`.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Placed {
    pub requested: BTreeMap<String, String>,
    pub records: Vec<(u64, Vec<u8>)>,
}

/// Build the profile for `requested` and place its header in `state`.
pub(crate) fn place(
    state: &mut BootState,
    firmware: &Firmware,
    resolved: &ResolvedConfiguration,
    requested: &BTreeMap<String, String>,
) -> Result<Placed, String> {
    let layout = resolved
        .reference
        .settings
        .as_ref()
        .ok_or_else(|| format!("{} has no settings layout for camera settings", firmware.id))?;
    let located = layout
        .profile
        .as_ref()
        .ok_or_else(|| format!("{} has no located user profile", firmware.id))?;
    let loader_offset = located
        .loader_offset
        .ok_or("an XP5 user profile needs the header's hand-off offset")?;
    let flash = update_payload(state, firmware)?;
    let unit = match medium(state)? {
        "nor" => NOR_UNIT,
        "serial-nand" => NAND_UNIT,
        other => return Err(format!("unknown flash storage medium {other}")),
    };
    let built = fujimod_settings::build(layout, &flash, requested, None)?;
    let profile = built.profile.ok_or("the settings layout built no user profile")?;
    let header: [u8; 8] = profile
        .header
        .as_slice()
        .try_into()
        .map_err(|_| "an XP5 profile header has eight bytes")?;
    // A start whose startup profile names a hand-off block (the X-T5 loader start, x21) gets
    // the header preloaded there. A start without one (the X-H2S 7.30 loader start runs stage
    // one, which builds the hand-off block itself from the power-controller registers) gets the
    // header only through those registers, so it must name them.
    match handoff_block(resolved) {
        Ok(handoff) => state.preload.push(Preload {
            address: handoff + loader_offset,
            value: Some(u64::from_le_bytes(header)),
            size: Some(8),
            dat_offset: None,
            length: None,
            sha256: None,
            word_size: None,
            words: None,
        }),
        Err(_) if located.power_register.is_some() => {}
        Err(error) => return Err(error),
    }
    if let Some(first) = located.power_register {
        set_power_registers(state, &power_words(first, &header))?;
    }
    let records = profile
        .records
        .into_iter()
        .map(|record| (u64::from(record.block) * unit, record.bytes))
        .collect();
    Ok(Placed { requested: requested.clone(), records })
}

/// The medium of the manifest's flash storage window, `nor` unless it names another.
fn medium(state: &BootState) -> Result<&str, String> {
    let window = flash_window(state)?;
    Ok(window.options.get("medium").and_then(toml::Value::as_str).unwrap_or("nor"))
}

fn flash_window(state: &BootState) -> Result<&DeviceWindow, String> {
    state
        .device
        .iter()
        .find(|window| window.kind == "flash-storage")
        .ok_or_else(|| "a user profile needs the flash storage window".into())
}

/// The original update payload the manifest's flash storage serves.
fn update_payload(state: &BootState, firmware: &Firmware) -> Result<Vec<u8>, String> {
    let window = flash_window(state)?;
    let number = |key: &str| {
        window
            .options
            .get(key)
            .and_then(toml::Value::as_integer)
            .ok_or_else(|| format!("flash storage requires {key}"))
    };
    let origin = usize::try_from(number("dat_origin")?).map_err(|error| error.to_string())?;
    let xor = u8::try_from(number("xor")?).map_err(|error| error.to_string())?;
    firmware.dat_payload(origin, xor)
}

/// The loader's hand-off block: stock kernel entry passes the boot core's x21
/// to the copy at 0x5b308 (X-T5 4.31 `mov x1, x21` at 0x58528).
fn handoff_block(resolved: &ResolvedConfiguration) -> Result<u64, String> {
    let profile_id = resolved
        .configuration
        .preset
        .as_ref()
        .and_then(|preset| preset.firmware_profile_id.as_deref())
        .ok_or("run configuration has no firmware startup profile")?;
    resolved
        .reference
        .firmware
        .layouts
        .as_ref()
        .and_then(|layouts| layouts.profiles.get(profile_id))
        .and_then(|profile| profile.cpu.as_ref())
        .and_then(|cpus| cpus.iter().find(|cpu| cpu.id == 0))
        .map(|cpu| cpu.regs.x21)
        .filter(|&address| address != 0)
        .ok_or_else(|| "the startup profile names no hand-off block".into())
}

/// The header as 16-bit power-controller registers from `first`, little-endian.
fn power_words(first: u8, header: &[u8; 8]) -> Vec<(u8, u16)> {
    header
        .chunks_exact(2)
        .zip(first..)
        .map(|(pair, register)| (register, u16::from_le_bytes([pair[0], pair[1]])))
        .collect()
}

/// Set initial registers on every power-controller endpoint of the manifest.
fn set_power_registers(state: &mut BootState, words: &[(u8, u16)]) -> Result<(), String> {
    let mut found = false;
    for window in state.device.iter_mut().filter(|window| window.kind == "spi-controller") {
        let Some(toml::Value::Array(endpoints)) = window.options.get_mut("endpoints") else {
            continue;
        };
        for endpoint in endpoints {
            let Some(part) = endpoint.get_mut("part").and_then(toml::Value::as_table_mut) else {
                continue;
            };
            if part.get("kind").and_then(toml::Value::as_str) != Some("power-controller") {
                continue;
            }
            let registers = part
                .entry("registers")
                .or_insert_with(|| toml::Value::Table(toml::Table::new()))
                .as_table_mut()
                .ok_or("power-controller registers must be a table")?;
            for (register, value) in words {
                registers.insert(format!("{register:#x}"), toml::Value::Integer(i64::from(*value)));
            }
            found = true;
        }
    }
    if found {
        Ok(())
    } else {
        Err("the user profile header needs a power-controller endpoint".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_words_fill_four_registers_little_endian() {
        let header = [0, 0, 0, 0, 0x0e, 0, 0xaa, 0x48];
        assert_eq!(
            power_words(0x1f, &header),
            vec![(0x1f, 0), (0x20, 0), (0x21, 0x000e), (0x22, 0x48aa)]
        );
    }

    #[test]
    fn power_registers_merge_into_the_power_controller_endpoint() {
        let mut state: BootState = toml::from_str(
            r#"
            firmware = "xt5-431"
            ram = []
            [[device]]
            kind = "spi-controller"
            base = 0xff950000
            size = 0x100
            [[device.options.endpoints]]
            select = { port = 15, pin = 15, active_high = true }
            [device.options.endpoints.part]
            kind = "power-controller"
            rtc_seconds = 1
            rtc_valid = true
            registers = { "0x0" = 128 }
            "#,
        )
        .unwrap();
        set_power_registers(&mut state, &[(0x1f, 1), (0x22, 0x48aa)]).unwrap();
        let part = &state.device[0].options["endpoints"][0]["part"];
        let registers = part["registers"].as_table().unwrap();
        assert_eq!(registers["0x0"].as_integer(), Some(128));
        assert_eq!(registers["0x1f"].as_integer(), Some(1));
        assert_eq!(registers["0x22"].as_integer(), Some(0x48aa));
        let mut bare: BootState = toml::from_str("firmware = \"xt5-431\"\nram = []\n").unwrap();
        assert!(set_power_registers(&mut bare, &[(0x1f, 1)]).is_err());
    }

    /// The catalog layout of X-T5 4.31 seeds a set-up profile: header byte 4
    /// carries the clock, language and performance bits, the main record lands in
    /// journal block 0x3c0 (NOR 0x7800000) with an English language byte, and the
    /// system record in block 0x3b4 (NOR 0x7680000).
    /// Needs the stock update file in `FUJIMOD_XT5_431_DAT`; skipped without it.
    #[test]
    fn xt5_431_profile_is_set_up_and_english() {
        let Some(path) = std::env::var_os("FUJIMOD_XT5_431_DAT") else {
            eprintln!("skipped: FUJIMOD_XT5_431_DAT is not set");
            return;
        };
        let resolved = ResolvedConfiguration::with_overlays("xt5-431", &[]).unwrap();
        let firmware = Firmware::from_update("xt5-431", std::fs::read(path).unwrap(), true).unwrap();
        let mut state = BootState::for_firmware("xt5-431", None).unwrap().with_catalog_windows(&resolved).unwrap();
        let before = state.preload.len();
        let requested = BTreeMap::from([
            ("setup".to_owned(), "done".to_owned()),
            ("language".to_owned(), "en".to_owned()),
        ]);
        let placed = place(&mut state, &firmware, &resolved, &requested).unwrap();
        let header = &state.preload[before];
        assert_eq!(header.address, 0x18650 + 0x158);
        let bytes = header.value.unwrap().to_le_bytes();
        assert_eq!(bytes[4], 0x0e);
        assert_eq!(bytes[6], 0xaa);
        assert_eq!(bytes.iter().fold(0u8, |sum, b| sum.wrapping_add(*b)), 0);
        // Records in catalog order: main (journal block 0x3c0), then system (block 0x3b4).
        let [(offset, record), (system_offset, system)] = placed.records.as_slice() else {
            panic!("main and system records");
        };
        assert_eq!(*offset, 0x780_0000);
        assert_eq!((*system_offset, system.len()), (0x768_0000, 0x2000));
        assert_eq!(record.len(), 0x8000);
        // UI store offsets: language 0x30e, pairing 0x677, app introduction 0x676,
        // power-off temperature 0x6840; record offset = store offset - 8 + 0x104.
        for store in [0x30e_usize, 0x676, 0x677, 0x6840] {
            assert_eq!(record[store - 8 + 0x104], 1, "store offset {store:#x}");
        }
        assert_eq!(&record[0x7bf8..0x7bfa], &[4, 0x31]);
        let mut checksum = 0u32;
        for (index, word) in record[..0x7bfc].chunks_exact(4).enumerate() {
            let word = u32::from_le_bytes(word.try_into().unwrap());
            checksum ^= word;
            assert_eq!((record[0x7bfc + index / 8] >> (index % 8)) & 1, (word.count_ones() & 1) as u8);
        }
        assert_eq!(&record[0x7ffc..], &checksum.to_le_bytes());
    }
}
