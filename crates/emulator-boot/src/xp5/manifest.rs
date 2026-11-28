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
    /// The run's RAM. Regions marked `dram` are its DRAM: the span from the SoC's body-DRAM
    /// base is the DRAM address space, every DRAM region is in the SD hosts' DMA reach, and a
    /// bus drain inside DRAM is RAM.
    pub ram: Vec<RamRegion>,
    /// The serial flash as the stages before the hand-off leave it.
    #[serde(default)]
    pub flash: Option<FlashHandoff>,
    #[serde(default)]
    pub preload: Vec<Preload>,
    /// Address windows the companion core sees in addition to the SoC's own, for a body whose
    /// companion firmware reaches DRAM the SoC windows do not cover.
    #[serde(default)]
    pub companion_window: Vec<CompanionWindow>,
    /// The run's device windows, built from the catalog (`with_catalog_windows`); a
    /// manifest declares none.
    #[serde(default)]
    pub device: Vec<DeviceWindow>,
    /// A control experiment's catalog windows to leave out, by base.
    #[serde(default)]
    pub remove_windows: Vec<u64>,
    /// A control experiment's option changes to the catalog window at each base.
    #[serde(default)]
    pub window_options: Vec<WindowControl>,
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
/// Serial-flash state at the loader hand-off.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FlashHandoff {
    /// Dummy cycles the part's volatile configuration register selects.
    pub volatile_dummy_cycles: u8,
}
/// Options a control experiment sets on, and removes from, the catalog window at `base`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WindowControl {
    pub base: u64,
    #[serde(default)]
    pub set: toml::Table,
    #[serde(default)]
    pub unset: Vec<String>,
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
pub struct RamRegion {
    pub base: u64,
    pub size: u64,
    /// The region is DRAM, not companion SRAM, DDR-PHY register space or boot SRAM.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dram: bool,
}
pub use fujimod_catalog::firmware::Preload;

/// A firmware's boot manifest and stream layout, compiled into the crate.
pub(crate) struct Bundled {
    pub firmware: &'static str,
    pub manifest: &'static str,
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
    /// Load the bundled manifest or a caller-selected manifest.
    pub fn for_firmware(firmware: &str, manifest: Option<&Path>) -> Result<Self, String> {
        let state = match manifest {
            Some(path) => Self::load(path)?,
            None => Self::parse(bundled(firmware)?.manifest)?,
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
        Self::parse(&text).map_err(|error| format!("{}: {error}", path.display()))
    }

    fn parse(text: &str) -> Result<Self, String> {
        let s: Self = toml::from_str(text).map_err(|e| e.to_string())?;
        if !s.device.is_empty() {
            return Err("device windows come from the catalog, not the boot manifest".into());
        }
        if s.version != 1 {
            return Err(format!("unsupported boot state version {}", s.version));
        }
        Ok(s)
    }
}

impl BootState {
    /// The manifest's RAM, checked.
    pub fn memory(&self) -> Result<fujimod_emulator_xp5_board::boot::RunMemory, String> {
        fujimod_emulator_xp5_board::boot::RunMemory::new(self.ram.iter().map(|region| {
            fujimod_emulator_xp5_board::boot::RunRam {
                base: region.base,
                size: region.size,
                dram: region.dram,
            }
        }))
    }

    /// Build the run's device windows from the catalog and the manifest's RAM.
    pub fn with_catalog_windows(
        mut self,
        resolved: &fujimod_catalog_resolution::ResolvedConfiguration,
    ) -> Result<Self, String> {
        let memory = self.memory()?;
        self.device = fujimod_emulator_xp5_board::boot::device_windows(resolved, &memory)?;
        if let Some(flash) = &self.flash {
            let controller = self
                .device
                .iter_mut()
                .find(|window| window.kind == "flash-storage-controller")
                .ok_or("the manifest's flash hand-off needs a serial-flash controller")?;
            controller.options.insert(
                "inherited_dummy_cycles".into(),
                toml::Value::Integer(i64::from(flash.volatile_dummy_cycles)),
            );
        }
        for base in &self.remove_windows {
            let before = self.device.len();
            self.device.retain(|window| window.base != *base);
            if self.device.len() == before {
                return Err(format!("the manifest removes absent window {base:#x}"));
            }
        }
        for options in &self.window_options {
            let window = self
                .device
                .iter_mut()
                .find(|window| window.base == options.base)
                .ok_or_else(|| {
                    format!(
                        "the manifest sets options on absent window {:#x}",
                        options.base
                    )
                })?;
            for key in &options.unset {
                window.options.remove(key).ok_or_else(|| {
                    format!(
                        "the manifest removes absent option {key} of window {:#x}",
                        options.base
                    )
                })?;
            }
            window.options.extend(options.set.clone());
        }
        Ok(self)
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
    use super::BootState;

    fn xh2s_windows() -> Vec<fujimod_emulator_devices::graph::DeviceWindow> {
        BootState::for_firmware("xh2s-730", None)
            .unwrap()
            .with_catalog_windows(
                &fujimod_catalog_resolution::ResolvedConfiguration::bundled("xh2s-730").unwrap(),
            )
            .unwrap()
            .device
    }

    #[test]
    fn xh2s_viewfinder_writes_use_spi_mode_zero() {
        let windows = xh2s_windows();
        let spi = windows
            .iter()
            .find(|device| device.base == 0xff91_5000)
            .unwrap();
        let endpoints = spi.options["endpoints"].as_array().unwrap();
        let evf = endpoints
            .iter()
            .find(|endpoint| {
                endpoint["select"]["port"].as_integer() == Some(12)
                    && endpoint["select"]["pin"].as_integer() == Some(10)
            })
            .unwrap();
        assert_eq!(evf["part"]["bits"].as_integer(), Some(8));
        assert_eq!(evf["part"]["mode"].as_integer(), Some(0));
    }

    #[test]
    fn xh2s_proximity_sensor_raises_its_body_interrupt() {
        let windows = xh2s_windows();
        let bus = windows
            .iter()
            .find(|device| device.base == 0xff92_5000)
            .unwrap();
        let targets = bus.options["targets"].as_array().unwrap();
        let part = |address| {
            &targets
                .iter()
                .find(|target| target["address"].as_integer() == Some(address))
                .unwrap()["part"]
        };
        assert_eq!(part(0x39)["kind"].as_str(), Some("tmd2772-proximity"));
        assert_eq!(part(0x39)["interrupt"].as_integer(), Some(122));
        assert!(part(0x36).get("interrupt").is_none());
    }

    #[test]
    fn xh2s_mode_transitions_include_the_mixer_and_filter_scaler() {
        let state = BootState::for_firmware("xh2s-730", None)
            .unwrap()
            .with_catalog_windows(
                &fujimod_catalog_resolution::ResolvedConfiguration::bundled("xh2s-730").unwrap(),
            )
            .unwrap();
        for (base, kind, irq) in [
            (0xffd2_2000, "completion-stub", 213),
            (0xff98_3000, "imaging-filter-scaler", 204),
        ] {
            let window = state
                .device
                .iter()
                .find(|window| window.base == base)
                .unwrap();
            assert_eq!(window.kind, kind);
            assert_eq!(window.options["irq"].as_integer(), Some(irq));
            let start = &window.options["starts"].as_array().unwrap()[0];
            assert_eq!(start["offset"].as_integer(), Some(4));
            assert_eq!(start["mask"].as_integer(), Some(1));
        }
    }

    #[test]
    fn xh2s_boot_attaches_the_x_mount_lens_to_ssp0() {
        let state = BootState::for_firmware("xh2s-730", None)
            .unwrap()
            .with_catalog_windows(
                &fujimod_catalog_resolution::ResolvedConfiguration::bundled("xh2s-730").unwrap(),
            )
            .unwrap();
        let lens = state
            .device
            .iter()
            .find(|window| window.base == 0xff91_0000)
            .expect("X-H2S lens bus");
        assert_eq!(lens.kind, "pl022");
        assert_eq!(lens.options["receive_request"].as_integer(), Some(0));
        assert_eq!(lens.options["transmit_request"].as_integer(), Some(1));
        let wire = &lens.options["wire"];
        assert_eq!(wire["kind"].as_str(), Some("x-mount-lens"));
        assert_eq!(wire["lens"].as_str(), Some("xf18-55"));
        assert_eq!(wire["ready_high_line"].as_integer(), Some(115));
    }

    #[test]
    fn every_bundled_manifest_marks_its_dram_from_zero() {
        for (firmware, end) in [
            ("x100vi-131", 0x8000_0000),
            ("xe5-112", 0x8000_0000),
            ("xh2-530", 0xf000_0000),
            ("xh2s-730", 0x8000_0000),
            ("xm5-130", 0x6800_0000),
            ("xt5-431", 0xc000_0000),
        ] {
            let memory = BootState::for_firmware(firmware, None)
                .unwrap()
                .memory()
                .unwrap();
            assert_eq!(memory.dram_end_from(0), Ok(end), "{firmware}");
        }
    }

    #[test]
    fn xh2s_movie_start_selects_the_dial_position_and_routes_recorder_dma() {
        let windows = BootState::for_firmware("xh2s-730", None)
            .unwrap()
            .with_catalog_windows(
                &fujimod_catalog_resolution::ResolvedConfiguration::bundled("xh2s-730").unwrap(),
            )
            .unwrap()
            .device;
        let adc = windows.iter().find(|window| window.kind == "adc").unwrap();
        // ADC channel 3 reads the mode dial; 3774 is its Movie position.
        assert_eq!(adc.options["samples"][3].as_integer(), Some(3774));
        let dma = windows
            .iter()
            .find(|window| window.kind == "general-dma")
            .unwrap();
        // Recorder DDR training waits for the channel-1 completion on INTID 238.
        assert_eq!(dma.options["a53_irq_base"].as_integer(), Some(237));
    }

    fn xh2s_catalog_state() -> BootState {
        BootState::for_firmware("xh2s-730", None)
            .unwrap()
            .with_catalog_windows(
                &fujimod_catalog_resolution::ResolvedConfiguration::bundled("xh2s-730").unwrap(),
            )
            .unwrap()
    }

    #[test]
    fn xh2s_sd_dma_can_reach_the_recorder_dram_window() {
        let state = xh2s_catalog_state();
        assert!(state.ram.iter().any(|region| region.dram
            && region.base == 0x1_8000_4000
            && region.base + region.size == 0x2_0000_0000));
        for (base, address_high) in [(0xff9b_0000, 0xff90_e7d8), (0xff9b_4000, 0xff90_e7e4)] {
            let host = state
                .device
                .iter()
                .find(|window| window.base == base && window.kind == "sdhci")
                .unwrap();
            let controller = &host.options["controller"];
            assert_eq!(controller["address_high"].as_integer(), Some(address_high));
            let banks = controller["ram_banks"].as_array().unwrap();
            for (bank_base, bank_size) in [
                (0x8000_4000, 0x6fff_c000),
                (0x1_0000_0000, 0x8000_0000),
                (0x1_8000_4000, 0x7fff_c000),
            ] {
                assert!(
                    banks.iter().any(|bank| {
                        let bank = bank.as_array().unwrap();
                        bank[0].as_integer() == Some(bank_base)
                            && bank[1].as_integer() == Some(bank_size)
                    }),
                    "{bank_base:#x}"
                );
            }
        }
    }

    #[test]
    fn xh2s_movie_stream_starts_in_ram() {
        // The H.264 stream starts at 0x100001000, inside the bus-drain port's first granule:
        // the whole aperture is RAM and no device serves its base.
        let state = xh2s_catalog_state();
        assert!(state.ram.iter().any(|region| {
            region.dram && region.base == 0x1_0000_0000 && region.base + region.size == 0x1_8000_0000
        }));
        assert!(
            state
                .device
                .iter()
                .all(|device| !(device.base..device.base + device.size).contains(&0x1_0000_1000))
        );
    }

    #[test]
    fn xh2s_first_sd_host_reads_the_slot_two_contact() {
        // The SD socket (labeled slot 2) is the first host; its card detect is GPIO 11.12,
        // active low. The second host has no socket contact.
        let state = xh2s_catalog_state();
        let host = |base: u64| {
            state
                .device
                .iter()
                .find(|window| window.base == base && window.kind == "sdhci")
                .unwrap()
        };
        let detect = &host(0xff9b_0000).options["detect_gpio"];
        assert_eq!(detect["port"].as_integer(), Some(11));
        assert_eq!(detect["pin"].as_integer(), Some(12));
        assert_eq!(detect["active_high"].as_bool(), Some(false));
        assert!(!host(0xff9b_4000).options.contains_key("detect_gpio"));
    }

    #[test]
    fn xh2s_movie_statistics_are_in_boot_sram() {
        // The Movie exposure task reads its statistics banks from boot SRAM (first read at
        // 0xff801060): the manifest declares that RAM, so the statistics window is RAM-backed.
        let state = BootState::for_firmware("xh2s-730", None)
            .unwrap()
            .with_catalog_windows(
                &fujimod_catalog_resolution::ResolvedConfiguration::bundled("xh2s-730").unwrap(),
            )
            .unwrap();
        let address = 0xff80_1060;
        assert!(
            state
                .ram
                .iter()
                .any(|region| (region.base..region.base + region.size).contains(&address))
        );
        let statistics = state
            .device
            .iter()
            .find(|window| (window.base..window.base + window.size).contains(&address))
            .unwrap();
        assert_eq!(statistics.kind, "imaging-statistics");
        assert_eq!(
            statistics.options.get("ram").and_then(toml::Value::as_bool),
            Some(true)
        );
    }

    #[test]
    fn statistics_banks_in_boot_sram_are_ram_backed() {
        for (firmware, ram) in [
            ("x100vi-131", true),
            ("xe5-112", true),
            ("xh2-530", true),
            ("xt5-431", true),
            ("xh2s-730", true),
            ("xm5-130", false),
        ] {
            let windows = BootState::for_firmware(firmware, None)
                .unwrap()
                .with_catalog_windows(
                    &fujimod_catalog_resolution::ResolvedConfiguration::bundled(firmware).unwrap(),
                )
                .unwrap()
                .device;
            let statistics = windows
                .iter()
                .find(|window| window.kind == "imaging-statistics")
                .unwrap();
            assert_eq!(
                statistics.options.get("ram").and_then(toml::Value::as_bool),
                ram.then_some(true),
                "{firmware}"
            );
        }
    }

    #[test]
    fn the_retired_dram_table_and_overlapping_dram_are_refused() {
        let manifest = |extra: &str| {
            BootState::parse(&format!(
                "firmware = \"xt5-431\"\n[[ram]]\nbase = 0\nsize = 0x40000000\ndram = true\n{extra}"
            ))
        };
        assert!(manifest("").unwrap().memory().is_ok());
        assert!(manifest("[dram]\nlimit = 0x80000000\nbus_drain = 0x80000000\n").is_err());
        let error = manifest("[[ram]]\nbase = 0x3ffff000\nsize = 0x2000\ndram = true\n")
            .unwrap()
            .memory()
            .unwrap_err();
        assert!(error.contains("overlap"), "{error}");
    }
}
