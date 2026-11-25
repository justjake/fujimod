//! Raw XP5 entry, serial NOR preparation, and optional post-DDR FAT32 loading.
use crate::image::{self, File, Result, Source, extent, put, require, word};
use crate::StartupStop;
use serde::{Deserialize, Serialize};
mod banks;
pub use banks::Banks;
mod card_boot;
pub use card_boot::CardBoot;
pub mod handoff_result;
pub use handoff_result::HandoffRange;
mod template {
    include!(concat!(env!("OUT_DIR"), "/xp5_sd_loader.rs"));
}
/// The card-resident setup hooks of this build are never installed, so their offsets go unused.
#[allow(dead_code)]
mod flash_template {
    include!(concat!(env!("OUT_DIR"), "/xp5_internal_flash_loader.rs"));
}

/// Where one compiled loader build keeps what the packager binds. `CARD` is the card-resident
/// loader (`template`); `INTERNAL_FLASH` is the same sources built for a startup that keeps the
/// installed flash attached (`flash_template`).
struct Loader {
    image: &'static [u8],
    prelude_stack_max: usize,
    setup_entry: usize,
    setup_configuration: usize,
    setup_return_branch: usize,
    setup_stack: usize,
    setup_stack_top: usize,
    sd_initialize: usize,
    sd_request: usize,
    sd_progress: usize,
    sd_board: usize,
    /// The host prelude, the root host setups, the stop-5 handoff ranges and the secondary-core
    /// release list: every build carries these tables, which stay empty unless the setup names them.
    sd_host_prelude: usize,
    sd_host_variants: usize,
    sd_handoff_ranges: usize,
    sd_secondary_release: usize,
    boot_ready: usize,
    boot_zero_gate: usize,
    externals: &'static [(usize, u32, &'static str)],
    /// Offset of the internal-flash binding; only the internal-flash build has one.
    internal_flash_binding: Option<usize>,
}
/// The XP5 redistributor window and the stride of its pages: an SGI frame is the second page of
/// a redistributor, `0x10000` above a `0x20000`-aligned base, and cores 1 to 3 follow each other.
/// A frame anywhere else (the flash aperture below all of it, the flash controller above) would
/// make the loader's interrupt-enable writes land on something that is not a GIC.
const GICR_WINDOW: std::ops::Range<u64> = 0xfa000000..0xfb000000;
const GICR_STRIDE: u64 = 0x20000;
fn valid_secondary_sgi(frames: &[u64; 3]) -> bool {
    frames
        .iter()
        .all(|&frame| GICR_WINDOW.contains(&frame) && frame % GICR_STRIDE == GICR_STRIDE / 2)
        && frames.windows(2).all(|pair| pair[1] == pair[0] + GICR_STRIDE)
}
const CARD: Loader = Loader {
    image: template::IMAGE,
    prelude_stack_max: template::PRELUDE_STACK_MAX,
    setup_entry: template::XP5_SETUP_ENTRY,
    setup_configuration: template::XP5_SETUP_CONFIGURATION,
    setup_return_branch: template::XP5_SETUP_RETURN_BRANCH,
    setup_stack: template::XP5_SETUP_STACK,
    setup_stack_top: template::XP5_SETUP_STACK_TOP,
    sd_initialize: template::SETUP_SD_INITIALIZE,
    sd_request: template::SETUP_SD_REQUEST,
    sd_progress: template::SETUP_SD_PROGRESS,
    sd_board: template::SETUP_SD_BOARD,
    sd_host_prelude: template::SETUP_SD_HOST_PRELUDE,
    sd_host_variants: template::SETUP_SD_HOST_VARIANTS,
    sd_handoff_ranges: template::SETUP_SD_HANDOFF_RANGES,
    sd_secondary_release: template::SETUP_SD_SECONDARY_RELEASE,
    boot_ready: template::A64_BOOT_READY,
    boot_zero_gate: template::A64_BOOT_ZERO_GATE,
    externals: template::EXTERNALS,
    internal_flash_binding: None,
};
const INTERNAL_FLASH: Loader = Loader {
    image: flash_template::IMAGE,
    prelude_stack_max: flash_template::PRELUDE_STACK_MAX,
    setup_entry: flash_template::XP5_SETUP_ENTRY,
    setup_configuration: flash_template::XP5_SETUP_CONFIGURATION,
    setup_return_branch: flash_template::XP5_SETUP_RETURN_BRANCH,
    setup_stack: flash_template::XP5_SETUP_STACK,
    setup_stack_top: flash_template::XP5_SETUP_STACK_TOP,
    sd_initialize: flash_template::SETUP_SD_INITIALIZE,
    sd_request: flash_template::SETUP_SD_REQUEST,
    sd_progress: flash_template::SETUP_SD_PROGRESS,
    sd_board: flash_template::SETUP_SD_BOARD,
    sd_host_prelude: flash_template::SETUP_SD_HOST_PRELUDE,
    sd_host_variants: flash_template::SETUP_SD_HOST_VARIANTS,
    sd_handoff_ranges: flash_template::SETUP_SD_HANDOFF_RANGES,
    sd_secondary_release: flash_template::SETUP_SD_SECONDARY_RELEASE,
    boot_ready: flash_template::A64_BOOT_READY,
    boot_zero_gate: flash_template::A64_BOOT_ZERO_GATE,
    externals: flash_template::EXTERNALS,
    internal_flash_binding: Some(flash_template::SETUP_SD_INTERNAL_FLASH),
};
/// Longest wait of the load-failure fallback; the loader refuses a binding above it.
const MAX_FALLBACK_WAIT_SECONDS: u32 = 86_400;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Operation {
    pub address: u64,
    pub mask: u32,
    pub value: u32,
    pub kind: u32,
}
/// One root SD host setup tried in order by a diagnostic loader. Times are
/// milliseconds; `clock_configuration_field` is 0-3 or `u32::MAX` to leave
/// the field unchanged.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostVariant {
    pub identification_divider: u32,
    pub clock_configuration_field: u32,
    pub power_off_ms: u32,
    pub power_on_ms: u32,
    pub clock_on_ms: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Board {
    pub host: u64,
    pub timeout_setting: u16,
    pub operations: Vec<Operation>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodeProof {
    pub address: usize,
    pub size: usize,
    pub sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceProof {
    pub source: Source,
    pub offset: usize,
    pub size: usize,
    pub sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisterWrite {
    pub address: u64,
    pub value: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Flash {
    pub controller: u64,
    pub aperture: u64,
    pub window_mask: u32,
    pub clock: u64,
    pub clock_commit: u64,
    pub pins: Vec<RegisterWrite>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lamp {
    pub pio_base: u64,
    pub clock_address: u64,
    pub clock_mask: u32,
    pub reset_address: u64,
    pub reset_mask: u32,
    pub green_port: u32,
    pub red_port: u32,
    pub pin: u32,
}
/// An extent of the RAM image that a stock-selector startup leaves zero.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cleared {
    pub address: usize,
    pub size: usize,
    pub what: String,
}
/// A startup that keeps the installed flash attached and runs the reset stage with the update's
/// own selector (1), not the preloaded one (0). The stock stage then loads the section table and
/// its sections from the installed flash as on a flash boot. What the card supplies is the reset
/// stage and whatever `ram_edits` keep the stage from loading over. A recipe names this when the
/// preloaded selector does not boot its body.
///
/// The loader sends the cores the boot ROM parks no wake. Under the preloaded selector the reset
/// stage's secondary-core entry waits for one; under the update's selector that entry returns at
/// once and the core goes on into the stage's own work, so the stage wakes the cores itself when
/// it is ready for them. A wake at the hand-off runs them beside core 0 too early.
///
/// The loader keeps the card host as the boot ROM left it after reading the loader from this
/// card: none of the recipe's board operations run. Those operations are the stock driver's
/// bring-up of a slot it finds unconfigured. On a body with a UHS-II slot they also enable the
/// slot's UHS-II interface, which the stock driver turns off again before its first command on
/// the UHS-I host and the loader does not.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StockSelector {
    /// SD clock divider the loader reads the card with, in place of the loader's own divider 1:
    /// the divider the body's stock driver programs for a default-speed card.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transfer_clock_divider: Option<u8>,
    /// Word edits of the RAM image, checked against it.
    pub ram_edits: Vec<image::Edit>,
    /// Extents of the RAM image after the reset stage that the card carries as zeros: data the
    /// running firmware loads from the installed flash itself.
    pub cleared: Vec<Cleared>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Setup {
    pub prefix: Source,
    pub low_length: usize,
    pub length_flags: u32,
    pub base: u64,
    pub stack: u64,
    pub bootstrap_offset: usize,
    pub initialize_offset: usize,
    pub gate_offset: usize,
    pub continuation: u64,
    pub initializer: CodeProof,
    pub delay: CodeProof,
    pub reset_dat: usize,
    pub reset_copy_size: usize,
    pub selector: usize,
    pub flash: Flash,
    pub lamp: Lamp,
    pub board: Board,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub host_prelude: Vec<Operation>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub host_variants: Vec<HostVariant>,
    /// Ranges of 32-bit words that stop 5 reads after the CRC match and
    /// writes to the card's [`handoff_result::NAME`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub handoff_ranges: Vec<HandoffRange>,
    /// Aff0 of each core (1 to 3) that the loader wakes with Group 0 SGI 0
    /// once the image is in place, before the boot core runs it. A boot ROM
    /// that parks the other cores branches them to physical 0 on that SGI.
    /// Card boot releases the cores it holds itself and takes no list.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secondary_release: Vec<u8>,
    pub proofs: Vec<SourceProof>,
    pub banks: Banks,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card_boot: Option<CardBoot>,
    /// Word edits of the RAM image for a startup that keeps the installed flash attached
    /// (`StartupOptions::internal_flash`). The image selects the reset stage's preloaded mode,
    /// whose other differences from the update's own selector have to be undone here.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub internal_flash_ram_edits: Vec<image::Edit>,
    /// When present, `Recipe::build_startup` builds the stock-selector startup.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stock_selector: Option<StockSelector>,
}
/// Request ABI 3: six words, then the count and offset/length pairs of the
/// RAM prefix's zero runs, which the loader fills instead of reading.
const REQUEST_BYTES: usize = 56 + ZERO_RUNS * 16;
const ZERO_RUNS: usize = 16;
const ZERO_BLOCK: usize = 0x10000;

/// The prefix's all-zero 64 KiB blocks after its first, merged into runs; when
/// there are more runs than the request holds, the longest are kept.
fn zero_runs(prefix: &[u8]) -> Vec<(u64, u64)> {
    let mut runs: Vec<(u64, u64)> = Vec::new();
    for start in (ZERO_BLOCK..prefix.len() / ZERO_BLOCK * ZERO_BLOCK).step_by(ZERO_BLOCK) {
        if prefix[start..start + ZERO_BLOCK].iter().all(|v| *v == 0) {
            match runs.last_mut() {
                Some((offset, bytes)) if *offset + *bytes == start as u64 => {
                    *bytes += ZERO_BLOCK as u64
                }
                _ => runs.push((start as u64, ZERO_BLOCK as u64)),
            }
        }
    }
    if runs.len() > ZERO_RUNS {
        runs.sort_by_key(|run| std::cmp::Reverse(run.1));
        runs.truncate(ZERO_RUNS);
        runs.sort_unstable();
    }
    runs
}
fn put64(data: &mut [u8], at: usize, value: u64) -> Result<()> {
    extent(data, at, 8)?;
    data[at..at + 8].copy_from_slice(&value.to_le_bytes());
    Ok(())
}
fn branch(at: u64, target: u64, link: bool) -> Result<u32> {
    let delta = i128::from(target) - i128::from(at);
    require(
        delta % 4 == 0 && (-(1 << 27)..(1 << 27)).contains(&delta),
        "AArch64 branch outside immediate range",
    )?;
    Ok((if link { 0x94000000 } else { 0x14000000 }) | (((delta >> 2) as u32) & 0x03ffffff))
}
fn mmio(address: u64) -> bool {
    (0xf0000000..0xfffff000).contains(&address) && address.is_multiple_of(4)
}
fn mmio_halfword(address: u64) -> bool {
    (0xf0000000..0xfffff000).contains(&address) && address.is_multiple_of(2)
}
/// Whether the loader runs `op` as a board operation: 0 writes a word, 1 changes the masked bits
/// of a word, 2 waits `value` (or the startup delay when 0), 3 changes the masked bits of a
/// halfword, 4 polls a halfword until its masked bits equal `value`, 5 reads a word, changes its
/// masked bits and writes it back as a halfword, and 6 waits `value`.
fn board_operation(op: &Operation) -> bool {
    match op.kind {
        0 | 1 => mmio(op.address),
        2 => true,
        3 | 4 => {
            mmio_halfword(op.address) && op.mask <= u16::MAX.into() && op.value & !op.mask == 0
        }
        5 => mmio(op.address) && op.mask <= u16::MAX.into() && op.value & !op.mask == 0,
        6 => op.address == 0 && op.mask == 0 && (1..=1000).contains(&op.value),
        _ => false,
    }
}
/// Whether the loader runs `op` in the host prelude: a board operation, or 7 sets `mask` in a
/// halfword and polls its ready bit 1 up to `value` times, 8 records the word at `address` as
/// host observation `value`, or 9 continues the selected root host's setup, gated by the word at
/// `mask`, for up to `value` polls.
fn host_prelude_operation(op: &Operation, host: u64) -> bool {
    board_operation(op)
        || match op.kind {
            7 => {
                mmio_halfword(op.address)
                    && op.mask != 0
                    && op.mask <= u16::MAX.into()
                    && (1..=10_000).contains(&op.value)
            }
            8 => mmio(op.address) && op.mask == 0 && op.value < 3,
            9 => {
                op.address == host
                    && op.mask != 0
                    && op.mask & 3 == 0
                    && (1..=1000).contains(&op.value)
            }
            _ => false,
        }
}
impl Setup {
    fn verify(&self, inverted: &[u8]) -> Result<Vec<u8>> {
        let prefix = self.prefix.read(inverted)?;
        require(
            prefix.len() >= 0x800
                && self.low_length >= 4
                && self.low_length <= prefix.len() - 0x800,
            "XP5 prefix extent differs",
        )?;
        require(
            self.base.is_multiple_of(0x4000) && (0xff000000..0xffff0000).contains(&self.base),
            "Invalid XP5 SRAM base",
        )?;
        require(
            self.length_flags & !0xc0000000 == 0
                && word(&prefix, 0x10)? == self.length_flags | self.low_length as u32,
            "XP5 setup length field differs",
        )?;
        require(
            self.bootstrap_offset >= 0x800
                && self
                    .bootstrap_offset
                    .checked_add(48)
                    .is_some_and(|end| end <= self.low_length + 0x800)
                && self.gate_offset == self.bootstrap_offset + 40
                && self.gate_offset >= 0x800
                && self
                    .gate_offset
                    .checked_add(12)
                    .is_some_and(|end| end <= self.low_length + 0x800),
            "XP5 bootstrap escapes setup",
        )?;
        let mut sources = std::collections::BTreeMap::new();
        for proof in &self.proofs {
            let key = serde_json::to_string(&proof.source)
                .map_err(|e| image::Error::Invalid(e.to_string()))?;
            if !sources.contains_key(&key) {
                sources.insert(key.clone(), proof.source.read(inverted)?);
            }
            image::hash(
                extent(&sources[&key], proof.offset, proof.size)?,
                &proof.sha256,
            )?;
        }
        require(
            !self.proofs.is_empty(),
            "XP5 device bindings lack source proofs",
        )?;
        for proof in [&self.initializer, &self.delay] {
            let offset = (proof.address as u64)
                .checked_sub(self.base)
                .ok_or_else(|| image::Error::Invalid("XP5 code proof precedes setup".into()))?
                as usize;
            require(
                proof.size > 0
                    && offset
                        .checked_add(proof.size)
                        .is_some_and(|end| end <= self.low_length),
                "XP5 code proof escapes setup",
            )?;
            image::hash(extent(&prefix, offset + 0x800, proof.size)?, &proof.sha256)?;
        }
        require(
            self.initialize_offset >= 0x800
                && self.initializer.address as u64
                    == self.base + self.initialize_offset as u64 - 0x800,
            "XP5 initializer binding differs",
        )?;
        Ok(prefix)
    }
}
/// Card entry replays its checked header without probing installed flash.
/// Stage one probes installed flash before retaining its stock continuation.
pub fn package(
    inverted: &[u8],
    setup: &Setup,
    ram: Option<Vec<u8>>,
    success_hold_units: u64,
    stop: StartupStop,
    prepare_installed_flash: bool,
) -> Result<Vec<File>> {
    package_with_storage(
        inverted,
        setup,
        ram,
        success_hold_units,
        stop,
        prepare_installed_flash,
        None,
    )
}

/// `stop` ends the load at a diagnostic point instead of running the image, and
/// `prepare_installed_flash` makes the prelude set up and scan the installed flash as stage one
/// does; neither changes the SD image or its CRC.
pub(crate) fn package_with_storage(
    inverted: &[u8], setup: &Setup, ram: Option<Vec<u8>>, success_hold_units: u64,
    stop: StartupStop, prepare_installed_flash: bool, storage: Option<&[u8]>,
) -> Result<Vec<File>> {
    package_inner(
        inverted, setup, ram, success_hold_units, stop, prepare_installed_flash, storage, None, None,
    )
}

/// Startup with the installed flash left attached: the card supplies the RAM
/// image only. The setup prelude initializes and scans the installed flash as
/// stage one does, and the card-resident setup tables, namespace and unit
/// cache are not used, so no per-unit storage image is involved.
///
/// The loader is the internal-flash build of the loader sources. Throughout the load it keeps
/// the cores the boot ROM parks held (their interrupt enables masked), RAM 0 gated and the
/// image's reset entry back, and it wakes the cores, which the image's reset stage expects to
/// be awake, only once the CRC has passed. With `fallback_wait_seconds`, a failed FFBOOT load
/// (no file, a length or CRC mismatch, a read error) also waits that long, puts the stock reset
/// image back from the installed flash and continues the stock boot there instead of halting.
/// The recipe's card-resident setup supplies the cores' interrupt frames and is otherwise not
/// used.
pub(crate) fn package_internal_flash(
    inverted: &[u8], setup: &Setup, ram: Vec<u8>, success_hold_units: u64,
    fallback_wait_seconds: Option<u32>,
) -> Result<Vec<File>> {
    let mut setup = setup.clone();
    let ram_edits = std::mem::take(&mut setup.internal_flash_ram_edits);
    let secondary_sgi = setup
        .card_boot
        .take()
        .map(|card| card.secondary_sgi)
        .ok_or_else(|| {
            image::Error::Invalid(
                "Internal-flash startup needs the parked cores' interrupt frames, which the \
                 recipe's card-resident setup names"
                    .into(),
            )
        })?;
    // The loader wakes the cores it holds itself, once the CRC has passed.
    setup.secondary_release = Vec::new();
    setup.banks = Banks {
        copies: Vec::new(),
        edits: Vec::new(),
    };
    package_inner(
        inverted,
        &setup,
        Some(ram),
        success_hold_units,
        StartupStop::Continue,
        false,
        None,
        Some(InternalFlash {
            fallback_wait_seconds,
            ram_edits,
            secondary_sgi,
        }),
        None,
    )
}

/// The recipe's stock-selector startup (`Setup::stock_selector`): the card loader, with the
/// prelude's installed-flash setup and scan as in stage one and without the card start's setup
/// tables in SRAM, and a RAM image changed after the reset-copy comparison. The preloaded
/// selector goes back to the update's own word, the recipe's edits are applied and its cleared
/// extents are zeroed; the CRC the loader checks covers the result. The loader holds, gates and
/// wakes no core. `stop` is the card startup's diagnostic stop.
pub(crate) fn package_stock_selector(
    inverted: &[u8],
    setup: &Setup,
    ram: Vec<u8>,
    success_hold_units: u64,
    stop: StartupStop,
) -> Result<Vec<File>> {
    let mut setup = setup.clone();
    let stock = setup.stock_selector.take().ok_or_else(|| {
        image::Error::Invalid("The recipe names no stock-selector startup".into())
    })?;
    require(
        setup.card_boot.is_none(),
        "A stock-selector startup excludes a card-resident setup",
    )?;
    setup.internal_flash_ram_edits = Vec::new();
    setup.banks = Banks {
        copies: Vec::new(),
        edits: Vec::new(),
    };
    // The boot ROM read the loader from this card: its card routing stays.
    setup.board.operations = Vec::new();
    // The reset stage wakes the parked cores itself; a wake at the hand-off is too early.
    setup.secondary_release = Vec::new();
    package_inner(
        inverted,
        &setup,
        Some(ram),
        success_hold_units,
        stop,
        false,
        None,
        None,
        Some(&stock),
    )
}


/// How an internal-flash startup differs from a card-resident one.
#[derive(Clone)]
struct InternalFlash {
    fallback_wait_seconds: Option<u32>,
    /// SGI frames of cores 1 to 3, held during the load.
    secondary_sgi: [u64; 3],
    /// Word edits of the RAM image, checked against it after the reset-copy comparison.
    ram_edits: Vec<image::Edit>,
}

/// Zero the cleared extents of a stock-selector image. Each is whole words after the reset
/// stage and inside the image; none overlaps another or an edit.
fn clear_ram(
    ram: &mut [u8], cleared: &[Cleared], edits: &[image::Edit], reset_copy_size: usize,
) -> Result<()> {
    let mut taken: Vec<(usize, usize)> = edits.iter().map(|edit| (edit.address, edit.address + 4)).collect();
    for extent in cleared {
        let end = extent.address.checked_add(extent.size).filter(|end| {
            extent.size != 0
                && extent.address.is_multiple_of(4)
                && extent.size.is_multiple_of(4)
                && extent.address >= reset_copy_size
                && *end <= ram.len()
        });
        let end = end.filter(|end| taken.iter().all(|&(low, high)| *end <= low || extent.address >= high));
        let end = end.ok_or_else(|| {
            image::Error::Invalid("Invalid cleared extent of the XP5 RAM image".into())
        })?;
        taken.push((extent.address, end));
    }
    for extent in cleared {
        ram[extent.address..extent.address + extent.size].fill(0);
    }
    Ok(())
}

/// Give the loader's card read the SD clock divider `divider` in place of its own divider 1.
/// The loader sets its transfer clock in one place: it writes clock control (host `+0x2c`) with
/// the divider in bits 15..8 and the internal clock enable, `mov w1, #0x101` and, within the next
/// eight instructions, `strh w1, [xN, #0x2c]`. The loader must hold that word exactly once and
/// the store after it, or nothing changes.
fn bind_transfer_clock(program: &mut [u8], divider: u8) -> Result<()> {
    const MOV_W1_DIVIDER_1: u32 = 0x52802021;
    require(
        (1..=0x80).contains(&divider) && divider.is_power_of_two(),
        "Invalid XP5 transfer clock divider",
    )?;
    let words = || (0..program.len() / 4).map(|n| n * 4);
    let sites: Vec<usize> =
        words().filter(|&at| word(program, at).is_ok_and(|w| w == MOV_W1_DIVIDER_1)).collect();
    let stored = sites.len() == 1
        && (1..=8).any(|n| {
            // strh w1, [xN, #0x2c]
            word(program, sites[0] + 4 * n).is_ok_and(|w| w & 0xfffffc1f == 0x79005801)
        });
    require(stored, "XP5 loader transfer clock differs")?;
    // mov w1, #(divider << 8 | 1)
    put(program, sites[0], 0x52800001 | (u32::from(divider) << 8 | 1) << 5)
}

/// Apply checked word edits to the RAM image: every old word must match, none may repeat or
/// touch the preloaded selector, and nothing changes unless all of them pass.
fn edit_ram(ram: &mut [u8], edits: &[image::Edit], selector: usize) -> Result<()> {
    let mut addresses = std::collections::BTreeSet::new();
    for edit in edits {
        require(
            edit.address.is_multiple_of(4)
                && !(selector..selector + 8).contains(&edit.address)
                && addresses.insert(edit.address)
                && word(ram, edit.address)? == edit.old,
            "XP5 RAM edit differs from source",
        )?;
    }
    for edit in edits {
        put(ram, edit.address, edit.new)?;
    }
    Ok(())
}

/// Fill the internal-flash loader's binding: the interrupt frames it holds during the load and,
/// with a fallback, where the stock reset image sits in the flash aperture and how long to wait
/// before putting it back. That image is the one the stock setup copied to RAM 0, so the loader
/// restores exactly what it overwrote (the gate stub and whatever of FFBOOT it had streamed).
fn bind_internal_flash(
    program: &mut [u8], loader: &Loader, setup: &Setup, flash: &InternalFlash, ram: Option<&[u8]>,
    inverted: &[u8],
) -> Result<()> {
    let at = loader.internal_flash_binding.ok_or_else(|| {
        image::Error::Invalid("This XP5 loader build has no internal-flash binding".into())
    })?;
    // Inputs, then the three words the loader writes (its fallback status and two CRCs).
    let empty = [1u64, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0].map(u64::to_le_bytes).concat();
    require(
        extent(program, at, 96)? == empty,
        "XP5 internal-flash binding template changed",
    )?;
    let page = |address: u64| address & !0xfff..(address & !0xfff) + 0x1000;
    let (aperture, window) = (setup.flash.aperture, u64::from(!setup.flash.window_mask) + 1);
    require(
        valid_secondary_sgi(&flash.secondary_sgi)
            && flash.secondary_sgi.iter().all(|&frame| {
                // Not the flash aperture and not the flash controller's page: a write there is
                // a flash command, not an interrupt-enable.
                !(aperture..aperture + window).contains(&frame)
                    && !page(setup.flash.controller).contains(&frame)
            }),
        "Invalid parked-core interrupt frames",
    )?;
    let (mut fallback, mut wait, mut source, mut bytes, mut crc) = (0u64, 0u64, 0u64, 0u64, 0u64);
    if let Some(seconds) = flash.fallback_wait_seconds {
        let restore_offset = setup
            .reset_dat
            .checked_sub(setup.prefix.dat_offset)
            .ok_or_else(|| {
                image::Error::Invalid("XP5 reset image precedes the update".into())
            })?;
        let window = u64::from(!setup.flash.window_mask) + 1;
        require(
            seconds <= MAX_FALLBACK_WAIT_SECONDS
                && setup.reset_copy_size > 0
                && setup.reset_copy_size % 8 == 0
                && restore_offset % 8 == 0
                && (restore_offset as u64)
                    .checked_add(setup.reset_copy_size as u64)
                    .is_some_and(|end| end <= window)
                && ram.is_some_and(|ram| setup.reset_copy_size <= ram.len()),
            "XP5 reset image is not a flash-aperture extent, or the wait is over a day",
        )?;
        (fallback, wait) = (1, u64::from(seconds));
        source = setup.flash.aperture + restore_offset as u64;
        bytes = setup.reset_copy_size as u64;
        // The loader checks the aperture against this before it gates RAM 0 and RAM 0 against it
        // after a restore, so a flash holding anything but this update's reset image (another
        // firmware, an unreadable aperture) never restores.
        crc = u64::from(crate::xp4::crc32(extent(inverted, setup.reset_dat, setup.reset_copy_size)?));
    }
    for (n, value) in [1, fallback, wait, source, bytes, crc]
        .into_iter()
        .chain(flash.secondary_sgi)
        .enumerate()
    {
        put64(program, at + n * 8, value)?;
    }
    Ok(())
}

fn package_inner(
    inverted: &[u8], setup: &Setup, ram: Option<Vec<u8>>, success_hold_units: u64,
    stop: StartupStop, prepare_installed_flash: bool, storage: Option<&[u8]>,
    internal_flash: Option<InternalFlash>,
    stock_selector: Option<&StockSelector>,
) -> Result<Vec<File>> {
    require(
        stop == StartupStop::Continue || ram.is_some(),
        "Diagnostic stop requires an SD image",
    )?;
    require(
        stock_selector.is_none()
            || (internal_flash.is_none()
                && ram.is_some()
                && storage.is_none()
                && setup.card_boot.is_none()),
        "The stock-selector startup is a card-loader startup without card-resident setup",
    )?;
    let prefix = setup.verify(inverted)?;
    let loader = if internal_flash.is_some() {
        &INTERNAL_FLASH
    } else {
        &CARD
    };
    let copied_end = setup.base + setup.low_length as u64;
    // XP5 raw-entry SRAM reserves its upper workspace for the boot service.
    let workspace = setup.base + 0x38800;
    let address = (copied_end.max(setup.stack + 0x100) + 0x3fff) & !0x3fff;
    let capacity = workspace.saturating_sub(address);
    require(
        copied_end < setup.stack && setup.stack < workspace && capacity >= 0x4000,
        "No XP5 extension above the stock stack and below boot workspace",
    )?;
    let count = word(&prefix, 0x14)? as usize;
    require(count <= 253, "XP5 header operations exceed header")?;
    let header_size = 0x800;
    let mut program = loader.image.to_vec();
    let header_at = program.len();
    program.extend_from_slice(extent(&prefix, 0, header_size)?);
    if internal_flash.is_some() {
        require(
            ram.is_some() && storage.is_none() && setup.card_boot.is_none(),
            "The internal-flash startup needs a startup without card-resident setup",
        )?;
    }
    let mut resident_end = setup
        .banks
        .end(setup.low_length, (workspace - setup.base) as usize)?;
    if let Some(card) = &setup.card_boot {
        resident_end = resident_end.max(card.resident_end()?);
    }
    let end = (address + program.len() as u64).max(
        setup.base
            + if ram.is_some() {
                resident_end as u64
            } else {
                0
            },
    );
    let end = (end + 0x1ff) & !0x1ff;
    require(
        end <= address + capacity && end <= workspace,
        "XP5 SD loader exceeds SRAM workspace",
    )?;
    require(
        loader.prelude_stack_max + 512 <= loader.setup_stack_top - loader.setup_stack,
        "XP5 prelude exceeds private stack",
    )?;
    let cfg = loader.setup_configuration;
    let mut empty = vec![0; 216];
    empty[0] = 2;
    require(
        extent(&program, cfg, 216)? == empty,
        "XP5 setup configuration template changed",
    )?;
    let flash = &setup.flash;
    let lamp = &setup.lamp;
    require(
        [
            flash.controller,
            flash.aperture,
            flash.clock,
            flash.clock_commit,
            lamp.pio_base,
            lamp.clock_address,
            lamp.reset_address,
        ]
        .into_iter()
        .all(mmio)
            && flash.pins.len() == 6
            && flash.pins.iter().all(|p| mmio(p.address))
            && flash.window_mask != 0
            && lamp.clock_mask != 0
            && lamp.reset_mask != 0
            && lamp.green_port < 16
            && lamp.red_port < 16
            && lamp.green_port != 3
            && lamp.red_port != 3
            && lamp.green_port != lamp.red_port
            && lamp.pin < 8,
        "Invalid XP5 flash/lamp binding",
    )?;
    let stock = extent(&prefix, 0x800, setup.low_length)?;
    let first = word(stock, 0)?;
    let jump = branch(
        setup.base,
        address + loader.setup_entry as u64,
        false,
    )?;
    for (n, value) in [
        2,
        flash.controller,
        flash.aperture,
        u64::from(flash.window_mask),
        flash.clock,
        flash.clock_commit,
        1000000,
        setup.base,
        u64::from(first),
        u64::from(jump),
        lamp.pio_base,
        lamp.clock_address,
        u64::from(lamp.clock_mask),
        lamp.reset_address,
        u64::from(lamp.reset_mask),
    ]
    .into_iter()
    .enumerate()
    {
        put64(&mut program, cfg + n * 8, value)?;
    }
    for (n, value) in [lamp.green_port, lamp.red_port, lamp.pin, count as u32]
        .into_iter()
        .enumerate()
    {
        put(&mut program, cfg + 120 + n * 4, value)?;
    }
    for (n, pin) in flash.pins.iter().enumerate() {
        put(&mut program, cfg + 136 + n * 8, pin.address as u32)?;
        put(&mut program, cfg + 140 + n * 8, pin.value)?;
    }
    put64(&mut program, cfg + 184, address + header_at as u64)?;
    put64(
        &mut program,
        cfg + 192,
        // Each unit executes 1,000,000 four-NOP iterations. Callers that ask
        // for seconds pass 2,048 units per second (>= 1.024 s even at two
        // billion iterations per second).
        success_hold_units,
    )?;
    put64(
        &mut program,
        cfg + 200,
        // The prelude sets up and scans the installed flash whenever it stays in use, and when
        // a card startup asks for it.
        u64::from(
            ram.is_none()
                || prepare_installed_flash
                || internal_flash.is_some()
                || stock_selector.is_some(),
        ),
    )?;
    put64(
        &mut program,
        cfg + 208,
        u64::from(stop != StartupStop::Continue),
    )?;
    if let Some(flash) = &internal_flash {
        bind_internal_flash(&mut program, loader, setup, flash, ram.as_deref(), inverted)?;
    }
    let restore = loader.setup_return_branch;
    require(
        word(&program, restore)? == 0x14000000,
        "XP5 restore branch template changed",
    )?;
    put(
        &mut program,
        restore,
        branch(address + restore as u64, setup.base, false)?,
    )?;
    let call = setup.base + setup.bootstrap_offset as u64 - 0x800 + 36;
    let gate = setup.base + setup.gate_offset as u64 - 0x800;
    for &(at, kind, name) in loader.externals {
        let target = match name {
            "boot_setup_initialize" => setup.initializer.address as u64,
            "boot_setup_delay" => setup.delay.address as u64,
            _ => {
                return Err(image::Error::Invalid(
                    "Unexpected XP5 SD loader external".into(),
                ));
            }
        };
        put(
            &mut program,
            at,
            branch(address + at as u64, target, kind == 283)?,
        )?;
    }
    let mut raw = vec![0xff; (end - setup.base) as usize];
    raw[..stock.len()].copy_from_slice(stock);
    put(&mut raw, 0, jump)?;
    let mut files = Vec::new();
    if let Some(mut ram) = ram {
        require(
            !ram.is_empty()
                && ram.len() < 1 << 30
                && setup
                    .selector
                    .checked_add(8)
                    .is_some_and(|end| end <= setup.reset_copy_size)
                && setup.reset_copy_size <= ram.len(),
            "XP5 payload/reset extent differs",
        )?;
        let mut reset = extent(inverted, setup.reset_dat, setup.reset_copy_size)?.to_vec();
        require(
            word(&reset, setup.selector)? == 1 && word(&reset, setup.selector + 4)? == 0,
            "XP5 stock reset selector differs",
        )?;
        put64(&mut reset, setup.selector, 0)?;
        require(
            reset == ram[..setup.reset_copy_size],
            "XP5 reset copy differs beyond preloaded selector",
        )?;
        if let Some(flash) = &internal_flash {
            edit_ram(&mut ram, &flash.ram_edits, setup.selector)?;
        }
        if let Some(stock) = stock_selector {
            clear_ram(&mut ram, &stock.cleared, &stock.ram_edits, setup.reset_copy_size)?;
            edit_ram(&mut ram, &stock.ram_edits, setup.selector)?;
            // Checked above: the stock word is 1 and the plan's image carries 0.
            put64(&mut ram, setup.selector, 1)?;
        }
        require(
            word(&prefix, setup.bootstrap_offset + 36)?
                == branch(call, setup.initializer.address as u64, true)?,
            "XP5 initializer call differs",
        )?;
        let load = word(&prefix, setup.gate_offset)?;
        require(
            load & 0xff00001f == 0x5800001e && word(&prefix, setup.gate_offset + 4)? == 0xd65f03c0,
            "XP5 bootstrap continuation differs",
        )?;
        let displacement = (((load >> 5) & 0x7ffff) as i64 ^ 0x40000) - 0x40000;
        let literal = (setup.gate_offset as i64 + displacement * 4)
            .try_into()
            .map_err(|_| image::Error::Invalid("XP5 zero literal escapes prefix".into()))?;
        require(
            extent(&prefix, literal, 8)? == setup.continuation.to_le_bytes(),
            "XP5 continuation literal differs",
        )?;
        let board = &setup.board;
        let cfg = loader.sd_board;
        require(
            board.operations.len() <= 48 && board.timeout_setting <= 14 && mmio(board.host),
            "Invalid XP5 SD binding",
        )?;
        require(
            extent(&program, cfg, 16 + 48 * 24)?.iter().all(|v| *v == 0),
            "XP5 board template changed",
        )?;
        put64(&mut program, cfg, board.host)?;
        put(&mut program, cfg + 8, u32::from(board.timeout_setting))?;
        put(&mut program, cfg + 12, board.operations.len() as u32)?;
        for (n, op) in board.operations.iter().enumerate() {
            require(board_operation(op), "Invalid XP5 SD operation")?;
            let at = cfg + 16 + n * 24;
            put64(&mut program, at, op.address)?;
            put(&mut program, at + 8, op.mask)?;
            put(&mut program, at + 12, op.value)?;
            put(&mut program, at + 16, op.kind)?;
        }
        if let Some(divider) = stock_selector.and_then(|stock| stock.transfer_clock_divider) {
            bind_transfer_clock(&mut program, divider)?;
        }
        let host_prelude = loader.sd_host_prelude;
        require(
            setup.host_prelude.len() <= 16,
            "XP5 host prelude exceeds operation limit",
        )?;
        require(
            extent(&program, host_prelude, 8 + 16 * 24)?
                .iter()
                .all(|v| *v == 0),
            "XP5 host prelude template changed",
        )?;
        put(&mut program, host_prelude, setup.host_prelude.len() as u32)?;
        for (n, op) in setup.host_prelude.iter().enumerate() {
            require(
                host_prelude_operation(op, setup.board.host),
                "Invalid XP5 SD host prelude operation",
            )?;
            let at = host_prelude + 8 + n * 24;
            put64(&mut program, at, op.address)?;
            put(&mut program, at + 8, op.mask)?;
            put(&mut program, at + 12, op.value)?;
            put(&mut program, at + 16, op.kind)?;
        }
        let variants = loader.sd_host_variants;
        require(
            setup.host_variants.len() <= 8,
            "XP5 host variants exceed their limit",
        )?;
        require(
            extent(&program, variants, 8 + 8 * 24)?
                .iter()
                .all(|v| *v == 0),
            "XP5 host variant template changed",
        )?;
        put(&mut program, variants, setup.host_variants.len() as u32)?;
        for (n, variant) in setup.host_variants.iter().enumerate() {
            require(
                (1..=0xff).contains(&variant.identification_divider)
                    && (variant.clock_configuration_field <= 3
                        || variant.clock_configuration_field == u32::MAX)
                    && [
                        variant.power_off_ms,
                        variant.power_on_ms,
                        variant.clock_on_ms,
                    ]
                    .iter()
                    .all(|ms| *ms <= 2000),
                "Invalid XP5 host variant",
            )?;
            let at = variants + 8 + n * 24;
            for (k, value) in [
                variant.identification_divider,
                variant.clock_configuration_field,
                variant.power_off_ms,
                variant.power_on_ms,
                variant.clock_on_ms,
            ]
            .into_iter()
            .enumerate()
            {
                put(&mut program, at + 4 * k, value)?;
            }
        }
        let ranges = loader.sd_handoff_ranges;
        handoff_result::check(&setup.handoff_ranges)?;
        require(
            setup.handoff_ranges.is_empty()
                || matches!(stop, StartupStop::AfterCrcWithErrorPackets),
            "XP5 handoff ranges require the after-CRC packet stop",
        )?;
        require(
            extent(&program, ranges, 8 + handoff_result::RANGES * 16)?
                .iter()
                .all(|v| *v == 0),
            "XP5 handoff range template changed",
        )?;
        if !setup.handoff_ranges.is_empty() {
            put(&mut program, ranges, setup.handoff_ranges.len() as u32)?;
            put(&mut program, ranges + 4, handoff_result::BYTES as u32)?;
        }
        for (n, range) in setup.handoff_ranges.iter().enumerate() {
            put64(&mut program, ranges + 8 + n * 16, range.address)?;
            put(&mut program, ranges + 16 + n * 16, range.words)?;
        }
        let release = loader.sd_secondary_release;
        require(
            setup.secondary_release.is_empty()
                || (setup.card_boot.is_none()
                    && setup.secondary_release.is_sorted_by(|a, b| a < b)
                    && setup
                        .secondary_release
                        .iter()
                        .all(|core| (1..=3).contains(core))),
            "Invalid XP5 secondary release",
        )?;
        require(
            extent(&program, release, 8)?.iter().all(|v| *v == 0),
            "XP5 secondary release template changed",
        )?;
        let targets = setup
            .secondary_release
            .iter()
            .fold(0u64, |mask, core| mask | 1 << core);
        put64(&mut program, release, targets)?;
        let request = loader.sd_request;
        let mut empty = vec![0; REQUEST_BYTES];
        empty[0] = 3;
        require(
            extent(&program, request, REQUEST_BYTES)? == empty,
            "XP5 request template changed",
        )?;
        let (file_len, file_crc) = (ram.len(), crate::xp4::crc32(&ram));
        for (n, value) in [
            3,
            file_len as u64,
            u64::from(file_crc),
            100,
            100000,
            stop as u64,
        ]
        .into_iter()
        .enumerate()
        {
            put64(&mut program, request + n * 8, value)?;
        }
        put(
            &mut raw,
            (call - setup.base) as usize,
            branch(call, address + loader.sd_initialize as u64, true)?,
        )?;
        put(
            &mut raw,
            (gate - setup.base) as usize,
            branch(gate, address + loader.boot_zero_gate as u64, false)?,
        )?;
        if let Some(card) = &setup.card_boot {
            card.copy_unit_cache(storage, &mut raw, setup.low_length,
                (setup.stack-setup.base) as usize,
                (address-setup.base) as usize..(address-setup.base) as usize+program.len(),
                &setup.banks)?;
            card.install(inverted, storage, card_boot::Installation {
                base: setup.base, stock, program_address: address,
                raw: &mut raw, program: &mut program, ram: &mut ram,
            })?;
        } else {
            require(storage.is_none(), "Setup has no card storage binding")?;
        }
        // Runs come from the final file: card boot patches the RAM prefix above.
        let file = ram;
        let bytes = u64::from_le_bytes(extent(&program, request + 8, 8)?.try_into().unwrap());
        let head = extent(&file, 0, bytes as usize)?;
        require(
            word(&program, request + 16)? == crate::xp4::crc32(head),
            "XP5 request CRC differs from the file it reads",
        )?;
        let runs = zero_runs(head);
        put64(&mut program, request + 48, runs.len() as u64)?;
        for (n, (offset, length)) in runs.into_iter().enumerate() {
            put64(&mut program, request + 56 + n * 16, offset)?;
            put64(&mut program, request + 64 + n * 16, length)?;
        }
        files.push(File::new("ffboot.bin", file)?);
        if !setup.handoff_ranges.is_empty() {
            files.push(File::new(handoff_result::NAME, handoff_result::pattern())?);
        }
    }
    require(
        extent(&program, loader.sd_progress, 64)?
            .iter()
            .all(|v| *v == 0)
            && word(&program, loader.boot_ready)? == 0,
        "XP5 handoff is already published",
    )?;
    let offset = (address - setup.base) as usize;
    raw[offset..offset + program.len()].copy_from_slice(&program);
    if !files.is_empty() {
        setup.banks.apply(
            &prefix,
            &mut raw,
            setup.low_length,
            (setup.stack - setup.base) as usize,
            offset,
            offset + program.len(),
        )?;
    }
    files.insert(0, File::new("marble_init.bin", raw)?);
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use card_boot::Payload;
    pub(super) fn source(bytes: &[u8]) -> Source {
        Source {
            dat_offset: 0,
            size: bytes.len(),
            compression: image::Compression::Raw,
            decoded_size: bytes.len(),
            sha256: fujimod_firmware::sha256(bytes),
            initialized_prefix: None,
        }
    }
    pub(super) fn fixture() -> (Vec<u8>, Setup, Vec<u8>) {
        let base = 0xff800000;
        let mut data = vec![0xff; 0x41000];
        for (at, value) in [
            (0x10, 0x4000),
            (0x14, 1),
            (0x18, 0xff900000),
            (0x1c, 7),
            (0x800, 0xd503201f),
            (0xa00, 0xd503201f),
            (0xb00, 0xd503201f),
            (0x924, branch(base + 0x124, base + 0x200, true).unwrap()),
            (0x928, 0x5800005e),
            (0x92c, 0xd65f03c0),
        ] {
            put(&mut data, at, value).unwrap();
        }
        put64(&mut data, 0x930, base + 0x3000).unwrap();
        put64(&mut data, 0x40040, 1).unwrap();
        let prefix = source(&data[..0x40000]);
        let setup = Setup {
            prefix: prefix.clone(),
            low_length: 0x4000,
            length_flags: 0,
            base,
            stack: base + 0x8000,
            bootstrap_offset: 0x900,
            initialize_offset: 0xa00,
            gate_offset: 0x928,
            continuation: base + 0x3000,
            initializer: CodeProof {
                address: (base + 0x200) as usize,
                size: 4,
                sha256: fujimod_firmware::sha256(&data[0xa00..0xa04]),
            },
            delay: CodeProof {
                address: (base + 0x300) as usize,
                size: 4,
                sha256: fujimod_firmware::sha256(&data[0xb00..0xb04]),
            },
            reset_dat: 0x40000,
            reset_copy_size: 0x1000,
            selector: 0x40,
            flash: Flash {
                controller: 0xff900000,
                aperture: 0xf0000000,
                window_mask: 0xf8000000,
                clock: 0xffa10000,
                clock_commit: 0xffa10040,
                pins: (0..6)
                    .map(|n| RegisterWrite {
                        address: 0xff901000 + n * 4,
                        value: 0,
                    })
                    .collect(),
            },
            lamp: Lamp {
                pio_base: 0xff902000,
                clock_address: 0xff903000,
                clock_mask: 1,
                reset_address: 0xff904000,
                reset_mask: 2,
                green_port: 4,
                red_port: 5,
                pin: 1,
            },
            board: Board {
                host: 0xff905000,
                timeout_setting: 14,
                operations: vec![Operation {
                    address: 0xff906000,
                    mask: 0,
                    value: 1,
                    kind: 0,
                }],
            },
            host_prelude: vec![],
            host_variants: vec![],
            handoff_ranges: vec![],
            secondary_release: vec![],
            proofs: vec![SourceProof {
                source: prefix,
                offset: 0,
                size: 4,
                sha256: fujimod_firmware::sha256(&data[..4]),
            }],
            banks: Banks {
                copies: vec![],
                edits: vec![],
            },
            card_boot: None,
            internal_flash_ram_edits: vec![],
            stock_selector: None,
        };
        let mut ram = data[0x40000..].to_vec();
        put64(&mut ram, 0x40, 0).unwrap();
        (data, setup, ram)
    }
    /// The recipe's card-resident setup, reduced to what an internal-flash startup reads from it:
    /// the interrupt frames of the parked cores.
    fn with_frames(mut setup: Setup) -> Setup {
        let payload = || Payload {
            source: source(&[0; 2]),
            file_offset: 0,
            destinations: [0xfb0a0000, 0xfb4a0000],
        };
        setup.card_boot = Some(CardBoot {
            hooks: vec![],
            dma: CodeProof {
                address: 0,
                size: 0,
                sha256: String::new(),
            },
            layout_marker: 0,
            jedec_id: 1,
            secondary_sgi: [0xfa0b0000, 0xfa0d0000, 0xfa0f0000],
            payloads: [payload(), payload(), payload(), payload()],
            namespace: None,
            unit_cache: None,
            runtime: None,
            ram_edits: vec![],
        });
        setup
    }

    #[test]
    fn zero_runs_merge_whole_zero_blocks_after_the_first() {
        let block = ZERO_BLOCK;
        let mut prefix = vec![0; 5 * block + 0x800];
        prefix[3 * block + 7] = 1;
        assert_eq!(
            zero_runs(&prefix),
            [
                (block as u64, 2 * block as u64),
                (4 * block as u64, block as u64)
            ]
        );
        // More runs than the request holds: the longest stay, in order.
        let mut many = vec![1; 90 * block];
        for n in 0..20 {
            let length = if n == 7 { 3 } else { 1 };
            many[(2 + 4 * n) * block..(2 + 4 * n + length) * block].fill(0);
        }
        let runs = zero_runs(&many);
        assert_eq!(runs.len(), ZERO_RUNS);
        assert!(runs.contains(&((30 * block) as u64, 3 * block as u64)));
        assert!(runs.is_sorted());
    }
    #[test]
    fn the_request_lists_the_ram_prefix_zero_runs() {
        let (data, s, mut ram) = fixture();
        ram.resize(5 * ZERO_BLOCK, 0);
        ram[3 * ZERO_BLOCK] = 9;
        let files = package(
            &data,
            &s,
            Some(ram.clone()),
            0,
            StartupStop::Continue,
            false,
        )
        .unwrap();
        let request = 0xc000 + template::SETUP_SD_REQUEST;
        let raw = &files[0].bytes;
        let value =
            |at: usize| u64::from_le_bytes(raw[request + at..request + at + 8].try_into().unwrap());
        assert_eq!((value(0), value(8), value(48)), (3, ram.len() as u64, 2));
        assert_eq!(
            [value(56), value(64), value(72), value(80)],
            [0x10000, 0x20000, 0x40000, 0x10000]
        );
        assert!(
            raw[request + 88..request + REQUEST_BYTES]
                .iter()
                .all(|v| *v == 0)
        );
    }
    #[test]
    fn startup_skips_installed_flash_probe_while_stage_one_preserves_it() {
        let (data, s, ram) = fixture();
        for payload in [None, Some(ram.clone())] {
            let combined = payload.is_some();
            let files = package(&data, &s, payload, 0, StartupStop::Continue, false).unwrap();
            let raw = &files[0].bytes;
            assert_eq!(files.len(), if combined { 2 } else { 1 });
            assert_eq!(files[0].name, "marble_init.bin");
            let source = 0xc000 + template::XP5_SETUP_CONFIGURATION + 200;
            assert_eq!(
                u64::from_le_bytes(raw[source..source + 8].try_into().unwrap()),
                u64::from(!combined)
            );
            let address = s.base + 0xc000;
            assert_eq!(
                word(raw, 0).unwrap(),
                branch(s.base, address + template::XP5_SETUP_ENTRY as u64, false).unwrap()
            );
            let mut restored = raw[..s.low_length].to_vec();
            put(&mut restored, 0, word(&data, 0x800).unwrap()).unwrap();
            if combined {
                assert_eq!(files[1].name, "ffboot.bin");
                assert_eq!(files[1].bytes, ram);
                for at in [0x124, 0x128] {
                    put(&mut restored, at, word(&data, at + 0x800).unwrap()).unwrap();
                }
                let request = 0xc000 + template::SETUP_SD_REQUEST;
                assert_eq!(
                    u64::from_le_bytes(raw[request + 8..request + 16].try_into().unwrap()),
                    ram.len() as u64
                );
                assert_eq!(word(raw, request + 16).unwrap(), crate::xp4::crc32(&ram));
            }
            assert_eq!(restored, data[0x800..0x800 + s.low_length]);
            let header = 0xc000 + template::IMAGE.len();
            assert_eq!(&raw[header..header + 0x800], &data[..0x800]);
            assert!(raw.len() <= 0x38800);
            assert_eq!(
                word(raw, 0xc000 + template::XP5_SETUP_RETURN_BRANCH).unwrap(),
                branch(
                    address + template::XP5_SETUP_RETURN_BRANCH as u64,
                    s.base,
                    false
                )
                .unwrap()
            );
        }
    }
    #[test]
    fn internal_flash_startup_probes_flash_like_stage_one_and_drops_card_resident_tables() {
        let (data, mut s, ram) = fixture();
        let plain = s.clone();
        s.banks = Banks {
            copies: vec![banks::Copy {
                source_offset: 0x1000,
                destination_offset: 0x20000,
                size: 16,
                sha256: fujimod_firmware::sha256(&data[0x1000..0x1010]),
            }],
            edits: vec![],
        };
        let flag = |raw: &[u8], configuration: usize| {
            let source = 0xc000 + configuration + 200;
            u64::from_le_bytes(raw[source..source + 8].try_into().unwrap())
        };
        let card = package(&data, &s, Some(ram.clone()), 0, StartupStop::Continue, false).unwrap();
        let flash = package_internal_flash(&data, &with_frames(s.clone()), ram.clone(), 0, None).unwrap();
        assert_eq!(
            flash.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
            ["marble_init.bin", "ffboot.bin"]
        );
        assert_eq!(flash[1].bytes, ram);
        assert_eq!(
            (
                flag(&card[0].bytes, template::XP5_SETUP_CONFIGURATION),
                flag(&flash[0].bytes, flash_template::XP5_SETUP_CONFIGURATION)
            ),
            (0, 1)
        );
        // The card start relocates the setup tables into SRAM; the flash start does not.
        assert_eq!(&card[0].bytes[0x20000..0x20010], &data[0x1000..0x1010]);
        assert!(flash[0].bytes.len() < 0x20000);
        // Against a card start with no extras, the flash-probe word differs, and so does the
        // loader build (checked in the next test).
        let bare = package(&data, &plain, Some(ram), 0, StartupStop::Continue, false).unwrap();
        assert_eq!(bare[1].bytes, flash[1].bytes);
        assert_eq!(flag(&bare[0].bytes, template::XP5_SETUP_CONFIGURATION), 0);
    }

    /// A recipe that names a stock-selector startup, and a RAM image with data after the reset
    /// stage (the fixture's image is the reset stage alone).
    fn stock_selector_fixture() -> (Vec<u8>, Setup, Vec<u8>) {
        let (data, mut setup, mut ram) = fixture();
        ram.extend(std::iter::repeat_n(0x5a, 0x2000));
        setup.stock_selector = Some(StockSelector {
            transfer_clock_divider: None,
            ram_edits: vec![image::Edit {
                address: 0x100,
                old: 0xffff_ffff,
                new: 0xd2a4_0000,
                what: "decode the section to scratch DRAM".into(),
            }],
            cleared: vec![Cleared {
                address: 0x2000,
                size: 0x800,
                what: "loaded from the installed flash".into(),
            }],
        });
        (data, setup, ram)
    }

    #[test]
    fn stock_selector_startup_keeps_the_updates_selector_and_the_installed_flash() {
        let (data, setup, ram) = stock_selector_fixture();
        let files =
            package_stock_selector(&data, &setup, ram.clone(), 0, StartupStop::Continue).unwrap();
        assert_eq!(
            files.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
            ["marble_init.bin", "ffboot.bin"]
        );
        // The image: the update's own selector, the edit, the cleared extent, nothing else.
        let mut expected = ram.clone();
        put64(&mut expected, setup.selector, 1).unwrap();
        put(&mut expected, 0x100, 0xd2a4_0000).unwrap();
        expected[0x2000..0x2800].fill(0);
        assert_eq!(files[1].bytes, expected);
        assert_eq!(&files[1].bytes[..0x1000], {
            let mut reset = data[0x40000..0x41000].to_vec();
            put(&mut reset, 0x100, 0xd2a4_0000).unwrap();
            reset
        });
        // The loader is the card loader of a plain card start of the same recipe. Two things
        // differ in its file: the prelude sets up and scans the installed flash, and the request
        // carries the changed image's CRC.
        let mut plain = setup.clone();
        plain.stock_selector = None;
        let card = package(
            &data,
            &plain,
            Some(ram.clone()),
            0,
            StartupStop::Continue,
            false,
        )
        .unwrap();
        let (raw, stock) = (&card[0].bytes, &files[0].bytes);
        assert_eq!(raw.len(), stock.len());
        let source = 0xc000 + template::XP5_SETUP_CONFIGURATION + 200;
        let request = 0xc000 + template::SETUP_SD_REQUEST;
        assert_eq!((word(raw, source).unwrap(), word(stock, source).unwrap()), (0, 1));
        assert_eq!(word(stock, request + 8).unwrap() as usize, expected.len());
        assert_eq!(word(stock, request + 16).unwrap(), crate::xp4::crc32(&expected));
        // The stock-selector loader runs none of the recipe's board operations: the boot ROM's
        // card routing stays. The plain card start keeps them.
        let board = 0xc000 + template::SETUP_SD_BOARD;
        let operations = 16 + 24 * plain.board.operations.len();
        assert!(!plain.board.operations.is_empty());
        assert_eq!(word(raw, board + 12).unwrap() as usize, plain.board.operations.len());
        assert_eq!(word(stock, board + 12).unwrap(), 0);
        assert!(stock[board + 16..board + operations].iter().all(|b| *b == 0));
        assert_eq!(stock[board..board + 12], raw[board..board + 12]);
        let differing: Vec<usize> = (0..raw.len())
            .filter(|&at| raw[at] != stock[at] && !(board + 12..board + operations).contains(&at))
            .map(|at| at & !7)
            .collect();
        assert!(differing.iter().all(|at| [source, request + 16].contains(at)), "{differing:x?}");
        // It is not the internal-flash build, which holds and wakes the parked cores.
        let flash = package_internal_flash(&data, &with_frames(plain), ram.clone(), 0, None).unwrap();
        assert_ne!(flash[0].bytes.len(), stock.len());
    }

    /// A recipe's transfer clock divider changes one instruction of the loader, the divider
    /// of its clock control write, and a loader without that one write refuses.
    #[test]
    fn stock_selector_startup_binds_the_recipes_transfer_clock_divider() {
        let (data, mut setup, ram) = stock_selector_fixture();
        let own = package_stock_selector(&data, &setup, ram.clone(), 0, StartupStop::Continue).unwrap();
        setup.stock_selector.as_mut().unwrap().transfer_clock_divider = Some(4);
        let bound = package_stock_selector(&data, &setup, ram.clone(), 0, StartupStop::Continue).unwrap();
        assert_eq!(own[1].bytes, bound[1].bytes);
        let (own, bound) = (&own[0].bytes, &bound[0].bytes);
        let differing: Vec<usize> = (0..own.len()).filter(|&at| own[at] != bound[at]).map(|at| at & !3).collect();
        assert!(!differing.is_empty() && differing.iter().all(|at| *at == differing[0]), "{differing:x?}");
        // mov w1, #0x101 becomes mov w1, #0x401.
        assert_eq!(word(own, differing[0]).unwrap(), 0x52802021);
        assert_eq!(word(bound, differing[0]).unwrap(), 0x52808021);
        for divider in [0, 3, 0xff] {
            setup.stock_selector.as_mut().unwrap().transfer_clock_divider = Some(divider);
            assert!(package_stock_selector(&data, &setup, ram.clone(), 0, StartupStop::Continue).is_err(), "{divider}");
        }
        let mut program = template::IMAGE.to_vec();
        bind_transfer_clock(&mut program, 4).unwrap();
        // The bound loader no longer holds the divider-1 write.
        assert!(bind_transfer_clock(&mut program, 4).is_err());
        assert!(bind_transfer_clock(&mut [0; 64], 4).is_err());
    }

    #[test]
    fn stock_selector_startup_refuses_what_it_cannot_check() {
        let (data, setup, ram) = stock_selector_fixture();
        let with = |change: &dyn Fn(&mut StockSelector)| {
            let mut setup = setup.clone();
            change(setup.stock_selector.as_mut().unwrap());
            package_stock_selector(&data, &setup, ram.clone(), 0, StartupStop::Continue)
        };
        assert!(with(&|_| {}).is_ok());
        // An edit whose old word differs, or one of the selector.
        assert!(with(&|s| s.ram_edits[0].old = 0x1234_5678).is_err());
        let selector = setup.selector;
        assert!(with(&|s| s.ram_edits[0].address = selector).is_err());
        // A cleared extent inside the reset stage, past the image, unaligned, empty or overlapping.
        assert!(with(&|s| s.cleared[0].address = 0x800).is_err());
        assert!(with(&|s| s.cleared[0].size = 0x2000).is_err());
        assert!(with(&|s| s.cleared[0].address = 0x2002).is_err());
        assert!(with(&|s| s.cleared[0].size = 0).is_err());
        assert!(with(&|s| {
            let again = s.cleared[0].clone();
            s.cleared.push(again);
        })
        .is_err());
        assert!(with(&|s| s.ram_edits[0].address = 0x2400).is_err());
        // A recipe without the startup, or with a card-resident setup as well.
        let mut plain = setup.clone();
        plain.stock_selector = None;
        assert!(
            package_stock_selector(&data, &plain, ram.clone(), 0, StartupStop::Continue).is_err()
        );
        assert!(
            package_stock_selector(
                &data,
                &with_frames(setup.clone()),
                ram.clone(),
                0,
                StartupStop::Continue
            )
            .is_err()
        );
        // Nothing of it reaches the other startups of the same recipe.
        let card = package(
            &data,
            &setup,
            Some(ram.clone()),
            0,
            StartupStop::Continue,
            false,
        )
        .unwrap();
        assert_eq!(card[1].bytes, ram);
    }

    #[test]
    fn internal_flash_applies_the_recipes_ram_edits_checked_and_in_the_crc() {
        let (data, s, ram) = fixture();
        let edit = |address: usize, old: u32| image::Edit {
            address,
            old,
            new: 0x1400_0068,
            what: "branch to the retained-metadata path".into(),
        };
        let with = |edits: Vec<image::Edit>| {
            let mut setup = with_frames(s.clone());
            setup.internal_flash_ram_edits = edits;
            setup
        };
        let plain = package_internal_flash(&data, &with_frames(s.clone()), ram.clone(), 0, None).unwrap();
        let edited =
            package_internal_flash(&data, &with(vec![edit(0x100, 0xffff_ffff)]), ram.clone(), 0, None)
                .unwrap();
        let mut expected = ram.clone();
        put(&mut expected, 0x100, 0x1400_0068).unwrap();
        assert_eq!(edited[1].bytes, expected);
        assert_eq!(plain[1].bytes, ram);
        let request = 0xc000 + flash_template::SETUP_SD_REQUEST;
        assert_eq!(word(&edited[0].bytes, request + 16).unwrap(), crate::xp4::crc32(&expected));
        assert_ne!(
            word(&edited[0].bytes, request + 16).unwrap(),
            word(&plain[0].bytes, request + 16).unwrap()
        );
        assert_eq!(edited[0].bytes.len(), plain[0].bytes.len());
        // A card start does not use them.
        let mut card_setup = s.clone();
        card_setup.internal_flash_ram_edits = vec![edit(0x100, 0xffff_ffff)];
        let card = package(
            &data,
            &card_setup,
            Some(ram.clone()),
            0,
            StartupStop::Continue,
            false,
        )
        .unwrap();
        assert_eq!(card[1].bytes, ram);
        // A changed old word, a repeat or the selector refuses.
        for edits in [
            vec![edit(0x100, 0x1234_5678)],
            vec![edit(0x100, 0xffff_ffff), edit(0x100, 0xffff_ffff)],
            vec![edit(s.selector, 0)],
        ] {
            assert!(package_internal_flash(&data, &with(edits), ram.clone(), 0, None).is_err());
        }
    }

    #[test]
    fn ram_edits_change_nothing_unless_every_one_passes() {
        let mut ram = vec![0xff; 0x20];
        let edit = |address: usize, old: u32| image::Edit {
            address,
            old,
            new: 0,
            what: "edit".into(),
        };
        let before = ram.clone();
        assert!(edit_ram(&mut ram, &[edit(0, 0xffff_ffff), edit(4, 1)], 0x40).is_err());
        assert_eq!(ram, before);
        assert!(edit_ram(&mut ram, &[edit(2, 0xffff_ffff)], 0x40).is_err());
        assert!(edit_ram(&mut ram, &[edit(0x20, 0xffff_ffff)], 0x40).is_err());
        edit_ram(&mut ram, &[edit(0, 0xffff_ffff), edit(8, 0xffff_ffff)], 0x40).unwrap();
        assert_eq!(&ram[..12], &[0, 0, 0, 0, 255, 255, 255, 255, 0, 0, 0, 0]);
    }

    #[test]
    fn boot_files_never_include_the_update_and_internal_flash_takes_no_unit_storage() {
        use crate::StartupOptions;
        let (data, s, _) = fixture();
        let update = fujimod_firmware::invert(&data);
        let mut recipe = crate::Recipe::bundled("x100vi-131").unwrap().unwrap();
        recipe.source_size = update.len();
        recipe.source_sha256 = fujimod_firmware::sha256(&update);
        recipe.updater = None;
        recipe.plan = Some(image::Plan {
            size: 0x1000,
            entry: 0,
            regions: vec![image::Region {
                name: "reset".into(),
                address: 0,
                source: Source {
                    dat_offset: 0x40000,
                    size: 0x1000,
                    compression: image::Compression::Raw,
                    decoded_size: 0x1000,
                    sha256: fujimod_firmware::sha256(&data[0x40000..0x41000]),
                    initialized_prefix: None,
                },
            }],
            bss: vec![],
            edits: vec![image::Edit {
                address: 0x40,
                old: 1,
                new: 0,
                what: "preloaded selector".into(),
            }],
        });
        recipe.xp5 = Some(with_frames(s));
        let options = StartupOptions {
            internal_flash: true,
            ..StartupOptions::default()
        };
        let names = |files: &[File]| files.iter().map(|f| f.name.clone()).collect::<Vec<_>>();
        let everything = recipe.build_startup_with_options(&update, options).unwrap();
        assert_eq!(names(&everything), ["marble_init.bin", "ffboot.bin", "FPUPDATE.DAT"]);
        let boot = recipe.build_boot_files(&update, options).unwrap();
        assert_eq!(names(&boot), ["marble_init.bin", "ffboot.bin"]);
        assert_eq!(boot[0].bytes, everything[0].bytes);
        assert_eq!(boot[1].bytes, everything[1].bytes);
        assert!(recipe.build_startup_with_storage(&update, &[0; 8], options).is_err());
        let mut stage_one = recipe.clone();
        stage_one.generation = crate::Generation::Xp4;
        assert!(stage_one.build_boot_files(&update, options).is_err());
    }

    /// Every word of the internal-flash loader the packager may touch: the words of every build
    /// and its own binding.
    fn bound_ranges() -> Vec<std::ops::Range<usize>> {
        let mut ranges = shared_bound_ranges(&INTERNAL_FLASH);
        ranges.push(
            flash_template::SETUP_SD_INTERNAL_FLASH..flash_template::SETUP_SD_INTERNAL_FLASH + 96,
        );
        ranges
    }

    /// The words every loader build lets the packager touch: the setup configuration, the SD
    /// board, host prelude, root host setup, handoff range, secondary release and request tables,
    /// the restore branch and the calls into the stock setup.
    fn shared_bound_ranges(loader: &Loader) -> Vec<std::ops::Range<usize>> {
        let mut ranges = vec![
            loader.setup_configuration..loader.setup_configuration + 216,
            loader.sd_board..loader.sd_board + 16 + 48 * 24,
            loader.sd_host_prelude..loader.sd_host_prelude + 8 + 16 * 24,
            loader.sd_host_variants..loader.sd_host_variants + 8 + 8 * 24,
            loader.sd_handoff_ranges..loader.sd_handoff_ranges + 8 + handoff_result::RANGES * 16,
            loader.sd_secondary_release..loader.sd_secondary_release + 8,
            loader.sd_request..loader.sd_request + REQUEST_BYTES,
            loader.setup_return_branch..loader.setup_return_branch + 4,
        ];
        ranges.extend(loader.externals.iter().map(|&(at, _, _)| at..at + 4));
        ranges
    }

    #[test]
    fn the_internal_flash_loader_binds_its_frames_and_the_fallback_only_on_request() {
        let (data, s, ram) = fixture();
        let setup = with_frames(s.clone());
        let at = 0xc000 + flash_template::SETUP_SD_INTERNAL_FLASH;
        let field = |raw: &[u8], n: usize| {
            u64::from_le_bytes(raw[at + 8 * n..at + 8 * n + 8].try_into().unwrap())
        };
        let gate = (s.gate_offset - 0x800) as usize;
        let address = s.base + 0xc000;
        for wait in [None, Some(7), Some(0), Some(MAX_FALLBACK_WAIT_SECONDS)] {
            let files = package_internal_flash(&data, &setup, ram.clone(), 0, wait).unwrap();
            let raw = &files[0].bytes;
            assert_eq!(files[1].bytes, ram);
            assert!(raw.len() <= 0x38800);
            assert_eq!(field(raw, 0), 1);
            let restore = s.flash.aperture + (s.reset_dat - s.prefix.dat_offset) as u64;
            let measured = u64::from(crate::xp4::crc32(
                &data[s.reset_dat..s.reset_dat + s.reset_copy_size],
            ));
            let expected = match wait {
                None => [0, 0, 0, 0, 0],
                Some(wait) => [1, u64::from(wait), restore, s.reset_copy_size as u64, measured],
            };
            assert_eq!((1..6).map(|n| field(raw, n)).collect::<Vec<_>>(), expected);
            assert_eq!(
                (6..9).map(|n| field(raw, n)).collect::<Vec<_>>(),
                [0xfa0b0000, 0xfa0d0000, 0xfa0f0000]
            );
            // The loader's own status words start empty.
            assert_eq!((9..12).map(|n| field(raw, n)).collect::<Vec<_>>(), [0, 0, 0]);
            // The bootstrap's gate is the loader's own.
            assert_eq!(
                word(raw, gate).unwrap(),
                branch(
                    s.base + gate as u64,
                    address + flash_template::A64_BOOT_ZERO_GATE as u64,
                    false
                )
                .unwrap()
            );
            // The loader is the compiled internal-flash build: nothing outside the bound ranges
            // differs from it, and nothing is appended.
            let bound = bound_ranges();
            let loader = &raw[0xc000..0xc000 + flash_template::IMAGE.len()];
            for at in (0..loader.len()).step_by(4) {
                if !bound.iter().any(|range| range.contains(&at)) {
                    assert_eq!(word(loader, at).unwrap(), word(flash_template::IMAGE, at).unwrap(), "{at:#x}");
                }
            }
            let header = 0xc000 + flash_template::IMAGE.len();
            assert!(raw[header + 0x800..].iter().all(|&byte| byte == 0xff));
        }
    }

    #[test]
    fn the_internal_flash_loader_needs_frames_the_fallback_a_flash_extent_and_a_bounded_wait() {
        let (data, s, ram) = fixture();
        // No card-resident setup in the recipe: no interrupt frames to hold.
        assert!(package_internal_flash(&data, &s, ram.clone(), 0, None).is_err());
        let setup = with_frames(s.clone());
        assert!(package_internal_flash(&data, &setup, ram.clone(), 0, Some(1)).is_ok());
        let frames = |change: &dyn Fn(&mut [u64; 3])| {
            let mut setup = setup.clone();
            change(&mut setup.card_boot.as_mut().unwrap().secondary_sgi);
            setup
        };
        for bad in [
            frames(&|sgi| sgi[1] = sgi[0]),
            frames(&|sgi| sgi[2] = 0xfa0f0010),
            frames(&|sgi| sgi[0] = 0x1000_0000),
            frames(&|sgi| sgi[0] = 0),
            // The flash aperture, the flash controller's page, a frame outside the
            // redistributor window and frames that are not one stride apart are refused.
            frames(&|sgi| sgi[0] = 0xf0010000),
            frames(&|sgi| sgi[0] = 0xf7ff0000),
            frames(&|sgi| sgi[1] = 0xff910000),
            frames(&|sgi| sgi[2] = 0xfa110000),
            frames(&|sgi| *sgi = [0xfb010000, 0xfb030000, 0xfb050000]),
            // Correctly strided, second-page frames that lie wholly in the flash aperture: only
            // the window clause refuses them.
            frames(&|sgi| *sgi = [0xf0010000, 0xf0030000, 0xf0050000]),
            frames(&|sgi| *sgi = [0xfa0b0000, 0xfa0f0000, 0xfa0d0000]),
        ] {
            assert!(package_internal_flash(&data, &bad, ram.clone(), 0, None).is_err());
        }
        // The wait is bounded at a day.
        assert!(package_internal_flash(&data, &setup, ram.clone(), 0, Some(MAX_FALLBACK_WAIT_SECONDS + 1)).is_err());
        // The stock reset image has to be a flash-aperture extent that the RAM image covers.
        let mut narrow = setup.clone();
        narrow.flash.window_mask = 0xffff_f000;
        assert!(package_internal_flash(&data, &narrow, ram.clone(), 0, None).is_ok());
        assert!(package_internal_flash(&data, &narrow, ram.clone(), 0, Some(1)).is_err());
        let mut odd = setup.clone();
        odd.reset_copy_size = 0xffc;
        assert!(package_internal_flash(&data, &odd, ram.clone(), 0, None).is_ok());
        assert!(package_internal_flash(&data, &odd, ram.clone(), 0, Some(1)).is_err());
    }

    #[test]
    fn the_internal_flash_loader_asset_keeps_its_binding_template_and_the_default_loader_has_none() {
        // The packager refuses a loader whose binding is not the empty ABI-1 template.
        assert_eq!(flash_template::SETUP_SD_INTERNAL_FLASH % 8, 0);
        assert_eq!(
            &flash_template::IMAGE[flash_template::SETUP_SD_INTERNAL_FLASH
                ..flash_template::SETUP_SD_INTERNAL_FLASH + 96],
            [1u64, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0].map(u64::to_le_bytes).concat()
        );
        assert_eq!(INTERNAL_FLASH.internal_flash_binding, Some(flash_template::SETUP_SD_INTERNAL_FLASH));
        assert_eq!(CARD.internal_flash_binding, None);
        // The default loader is the card-resident build: its layout names no such binding.
        let layout = include_str!("../assets/sd-boot-v1-xp5-loader.generated.json");
        assert!(!layout.contains("setup_sd_internal_flash"));
        assert_ne!(template::IMAGE, flash_template::IMAGE);
        // Both builds share the prelude the packager bounds and configures.
        assert_eq!(template::PRELUDE_STACK_MAX, flash_template::PRELUDE_STACK_MAX);
        assert_eq!(
            template::XP5_SETUP_RETURN_BRANCH,
            flash_template::XP5_SETUP_RETURN_BRANCH
        );
    }

    #[test]
    fn success_hold_is_optional_and_changes_only_its_configuration() {
        let (data, s, ram) = fixture();
        assert_eq!(crate::StartupOptions::default().success_hold_seconds, 0);
        for payload in [None, Some(ram)] {
            let normal =
                package(&data, &s, payload.clone(), 0, StartupStop::Continue, false).unwrap();
            let at = 0xc000 + template::XP5_SETUP_CONFIGURATION + 192;
            assert_eq!(&normal[0].bytes[at..at + 8], &[0; 8]);
            for units in [1, 32, 6_144, u64::from(u32::MAX) * 2_048] {
                let held = package(
                    &data,
                    &s,
                    payload.clone(),
                    units,
                    StartupStop::Continue,
                    false,
                )
                .unwrap();
                let mut expected = normal[0].bytes.clone();
                put64(&mut expected, at, units).unwrap();
                assert_eq!(expected, held[0].bytes);
                if normal.len() == 2 {
                    assert_eq!(normal[1].bytes, held[1].bytes);
                }
            }
        }
    }
    #[test]
    fn diagnostic_stops_change_only_the_stop_request() {
        let (data, setup, ram) = fixture();
        let normal = package(
            &data,
            &setup,
            Some(ram.clone()),
            0,
            StartupStop::Continue,
            false,
        )
        .unwrap();
        let at = 0xc000 + template::SETUP_SD_REQUEST + 40;
        for stop in [
            StartupStop::AfterCardInit,
            StartupStop::AfterCardInitWithErrorBlink,
            StartupStop::AfterSetup,
            StartupStop::AfterCrc,
        ] {
            assert!(package(&data, &setup, None, 0, stop, false).is_err());
            let checked = package(&data, &setup, Some(ram.clone()), 0, stop, false).unwrap();
            let mut expected = normal[0].bytes.clone();
            put64(
                &mut expected,
                0xc000 + template::XP5_SETUP_CONFIGURATION + 208,
                1,
            )
            .unwrap();
            put64(&mut expected, at, stop as u64).unwrap();
            assert_eq!(checked[0].bytes, expected);
            assert_eq!(checked[1].bytes, normal[1].bytes);
        }
    }
    #[test]
    fn installed_flash_preparation_changes_only_the_entry_source() {
        let (data, setup, ram) = fixture();
        let normal = package(
            &data,
            &setup,
            Some(ram.clone()),
            0,
            StartupStop::AfterCrc,
            false,
        )
        .unwrap();
        let prepared = package(&data, &setup, Some(ram), 0, StartupStop::AfterCrc, true).unwrap();
        let mut expected = normal[0].bytes.clone();
        put64(
            &mut expected,
            0xc000 + template::XP5_SETUP_CONFIGURATION + 200,
            1,
        )
        .unwrap();
        assert_eq!(prepared[0].bytes, expected);
        assert_eq!(prepared[1].bytes, normal[1].bytes);
    }

    #[test]
    fn startup_accepts_a_shorter_checked_setup_prefix() {
        let (data, mut setup, ram) = fixture();
        setup.prefix = source(&data[..0x28000]);
        setup.proofs[0].source = setup.prefix.clone();
        assert!(package(&data, &setup, Some(ram), 0, StartupStop::Continue, false).is_ok());
        setup.prefix = source(&data[..0x800]);
        setup.proofs[0].source = setup.prefix.clone();
        assert!(package(&data, &setup, None, 0, StartupStop::Continue, false).is_err());
    }

    #[test]
    fn changed_source_and_incomplete_bindings_refuse() {
        let (data, s, ram) = fixture();
        let mut changed = data.clone();
        changed[0x1000] ^= 1;
        assert!(package(&changed, &s, None, 0, StartupStop::Continue, false).is_err());
        let mut changed = s.clone();
        changed.flash.pins.clear();
        assert!(package(&data, &changed, None, 0, StartupStop::Continue, false).is_err());
        let mut changed = s.clone();
        changed.initializer.sha256 = "0".repeat(64);
        assert!(package(&data, &changed, None, 0, StartupStop::Continue, false).is_err());
        let mut changed = s.clone();
        changed.board.operations[0].kind = 7;
        assert!(
            package(
                &data,
                &changed,
                Some(ram.clone()),
                0,
                StartupStop::Continue,
                false
            )
            .is_err()
        );
        let mut changed = s.clone();
        changed.gate_offset = 0x940;
        assert!(package(&data, &changed, Some(ram), 0, StartupStop::Continue, false).is_err());
        let mut changed = s.clone();
        changed.stack = s.base + 0x38000;
        assert!(package(&data, &changed, None, 0, StartupStop::Continue, false).is_err());
    }

    #[test]
    fn board_operations_accept_checked_halfword_widths() {
        let (data, mut setup, ram) = fixture();
        setup.board.operations = vec![
            Operation {
                address: 0xff906002,
                mask: 0x1100,
                value: 0x1100,
                kind: 3,
            },
            Operation {
                address: 0xff906004,
                mask: 1,
                value: 1,
                kind: 5,
            },
        ];
        package(
            &data,
            &setup,
            Some(ram.clone()),
            0,
            StartupStop::Continue,
            false,
        )
        .unwrap();
        setup.board.operations[0].mask = 0x10000;
        assert!(
            package(
                &data,
                &setup,
                Some(ram.clone()),
                0,
                StartupStop::Continue,
                false
            )
            .is_err()
        );
        setup.board.operations[0].mask = 0x1100;
        setup.board.operations[1].address += 2;
        assert!(package(&data, &setup, Some(ram), 0, StartupStop::Continue, false).is_err());
    }
    #[test]
    fn host_prelude_serializes_separately_and_enforces_its_limit() {
        let (data, mut setup, ram) = fixture();
        setup.host_prelude = vec![Operation {
            address: 0xff907000,
            mask: 0x10,
            value: 0x10,
            kind: 1,
        }];
        let files = package(
            &data,
            &setup,
            Some(ram.clone()),
            0,
            StartupStop::Continue,
            false,
        )
        .unwrap();
        let at = 0xc000 + template::SETUP_SD_HOST_PRELUDE;
        assert_eq!(word(&files[0].bytes, at).unwrap(), 1);
        assert_eq!(word(&files[0].bytes, at + 4).unwrap(), 0);
        assert_eq!(
            u64::from_le_bytes(files[0].bytes[at + 8..at + 16].try_into().unwrap()),
            0xff907000
        );
        assert_eq!(word(&files[0].bytes, at + 16).unwrap(), 0x10);
        assert_eq!(word(&files[0].bytes, at + 20).unwrap(), 0x10);
        assert_eq!(word(&files[0].bytes, at + 24).unwrap(), 1);

        setup.host_prelude = vec![
            Operation {
                address: 0xff907000,
                mask: 0,
                value: 0,
                kind: 0,
            };
            17
        ];
        assert!(package(&data, &setup, Some(ram), 0, StartupStop::Continue, false).is_err());
    }
    #[test]
    fn host_variants_serialize_in_order_and_are_bounded() {
        let (data, mut setup, ram) = fixture();
        let variant = HostVariant {
            identification_divider: 0x3e,
            clock_configuration_field: u32::MAX,
            power_off_ms: 500,
            power_on_ms: 250,
            clock_on_ms: 10,
        };
        setup.host_variants = vec![variant.clone(); 2];
        setup.host_variants[0].identification_divider = 0xfa;
        let build = |setup: &Setup| {
            package(
                &data,
                setup,
                Some(ram.clone()),
                0,
                StartupStop::Continue,
                false,
            )
        };
        let files = build(&setup).unwrap();
        let at = 0xc000 + template::SETUP_SD_HOST_VARIANTS;
        let words: Vec<u32> = (0..14)
            .map(|n| word(&files[0].bytes, at + 4 * n).unwrap())
            .collect();
        assert_eq!(
            words,
            [
                2,
                0,
                0xfa,
                u32::MAX,
                500,
                250,
                10,
                0,
                0x3e,
                u32::MAX,
                500,
                250,
                10,
                0
            ]
        );
        for broken in [
            HostVariant {
                identification_divider: 0,
                ..variant.clone()
            },
            HostVariant {
                identification_divider: 0x100,
                ..variant.clone()
            },
            HostVariant {
                clock_configuration_field: 4,
                ..variant.clone()
            },
            HostVariant {
                power_off_ms: 2001,
                ..variant.clone()
            },
        ] {
            setup.host_variants = vec![broken];
            assert!(build(&setup).is_err());
        }
        setup.host_variants = vec![variant; 9];
        assert!(build(&setup).is_err());
    }
    #[test]
    fn handoff_ranges_serialize_and_add_the_result_file_for_the_after_crc_stop() {
        let (data, mut setup, ram) = fixture();
        setup.handoff_ranges = vec![
            HandoffRange {
                address: 0xff90e104,
                words: 3,
            },
            HandoffRange {
                address: 0xfa0b0100,
                words: 1,
            },
        ];
        let build = |setup: &Setup, stop| package(&data, setup, Some(ram.clone()), 0, stop, false);
        let files = build(&setup, StartupStop::AfterCrcWithErrorPackets).unwrap();
        let at = 0xc000 + template::SETUP_SD_HANDOFF_RANGES;
        let words: Vec<u32> = (0..10)
            .map(|n| word(&files[0].bytes, at + 4 * n).unwrap())
            .collect();
        assert_eq!(
            words,
            [2, 1 << 20, 0xff90e104, 0, 3, 0, 0xfa0b0100, 0, 1, 0]
        );
        let names: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(
            names,
            ["marble_init.bin", "ffboot.bin", handoff_result::NAME]
        );
        assert_eq!(files[2].bytes, handoff_result::pattern());
        assert!(build(&setup, StartupStop::Continue).is_err());
        setup.handoff_ranges[1].address = 0xfa0b0102;
        assert!(build(&setup, StartupStop::AfterCrcWithErrorPackets).is_err());
        setup.handoff_ranges = vec![];
        let plain = build(&setup, StartupStop::AfterCrcWithErrorPackets).unwrap();
        assert_eq!(plain.len(), 2);
        assert_eq!(word(&plain[0].bytes, at).unwrap(), 0);
    }
    #[test]
    fn secondary_release_writes_the_sgi_target_list() {
        let (data, mut setup, ram) = fixture();
        let build = |setup: &Setup| {
            package(
                &data,
                setup,
                Some(ram.clone()),
                0,
                StartupStop::Continue,
                false,
            )
        };
        let at = 0xc000 + template::SETUP_SD_SECONDARY_RELEASE;
        let plain = build(&setup).unwrap();
        assert_eq!(word(&plain[0].bytes, at).unwrap(), 0);
        setup.secondary_release = vec![1, 2, 3];
        let files = build(&setup).unwrap();
        assert_eq!(word(&files[0].bytes, at).unwrap(), 0xe);
        assert_eq!(word(&files[0].bytes, at + 4).unwrap(), 0);
        for broken in [vec![0], vec![4], vec![2, 1], vec![1, 1]] {
            setup.secondary_release = broken;
            assert!(build(&setup).is_err());
        }
    }
    #[test]
    fn startups_that_wake_no_core_at_the_hand_off_drop_the_secondary_release() {
        let (data, mut setup, ram) = stock_selector_fixture();
        setup.secondary_release = vec![1, 2, 3];
        let release = |raw: &[u8], at: usize| word(raw, 0xc000 + at).unwrap();
        let stock =
            package_stock_selector(&data, &setup, ram.clone(), 0, StartupStop::Continue).unwrap();
        assert_eq!(
            release(&stock[0].bytes, template::SETUP_SD_SECONDARY_RELEASE),
            0
        );
        let flash =
            package_internal_flash(&data, &with_frames(setup.clone()), ram.clone(), 0, None)
                .unwrap();
        assert_eq!(
            release(&flash[0].bytes, flash_template::SETUP_SD_SECONDARY_RELEASE),
            0
        );
        setup.stock_selector = None;
        let card = package(&data, &setup, Some(ram), 0, StartupStop::Continue, false).unwrap();
        assert_eq!(
            release(&card[0].bytes, template::SETUP_SD_SECONDARY_RELEASE),
            0xe
        );
    }
    #[test]
    fn the_stock_selector_startup_takes_the_diagnostic_stop() {
        let (data, setup, ram) = stock_selector_fixture();
        let stopped =
            package_stock_selector(&data, &setup, ram.clone(), 0, StartupStop::AfterCrc).unwrap();
        let raw = &stopped[0].bytes;
        let stop = 0xc000 + template::XP5_SETUP_CONFIGURATION + 208;
        assert_eq!(word(raw, stop).unwrap(), 1);
        assert_eq!(
            word(raw, 0xc000 + template::SETUP_SD_REQUEST + 40).unwrap(),
            2
        );
        let plain = package_stock_selector(&data, &setup, ram, 0, StartupStop::Continue).unwrap();
        assert_eq!(word(&plain[0].bytes, stop).unwrap(), 0);
    }
    #[test]
    fn host_prelude_handshake_serializes_and_bounds_attempts() {
        let (data, mut setup, ram) = fixture();
        setup.host_prelude = vec![
            Operation {
                address: 0xff9b222c,
                mask: 1,
                value: 1000,
                kind: 7,
            },
            Operation {
                address: 0xff9b2244,
                mask: 0,
                value: 0,
                kind: 8,
            },
        ];
        let files = package(
            &data,
            &setup,
            Some(ram.clone()),
            0,
            StartupStop::AfterCrcWithErrorPackets,
            false,
        )
        .unwrap();
        let at = 0xc000 + template::SETUP_SD_HOST_PRELUDE;
        assert_eq!(word(&files[0].bytes, at).unwrap(), 2);
        assert_eq!(word(&files[0].bytes, at + 16).unwrap(), 1);
        assert_eq!(word(&files[0].bytes, at + 20).unwrap(), 1000);
        assert_eq!(word(&files[0].bytes, at + 24).unwrap(), 7);
        assert_eq!(word(&files[0].bytes, at + 8 + 24 + 12).unwrap(), 0);
        assert_eq!(word(&files[0].bytes, at + 8 + 24 + 16).unwrap(), 8);
        setup.host_prelude[1].value = 3;
        assert!(
            package(
                &data,
                &setup,
                Some(ram.clone()),
                0,
                StartupStop::AfterCrcWithErrorPackets,
                false
            )
            .is_err()
        );
        setup.host_prelude[1].value = 0;
        setup.host_prelude[0].value = 0;
        assert!(
            package(
                &data,
                &setup,
                Some(ram.clone()),
                0,
                StartupStop::AfterCrcWithErrorPackets,
                false
            )
            .is_err()
        );
        setup.host_prelude[0].value = 1000;
        setup.host_prelude[0].address += 1;
        assert!(
            package(
                &data,
                &setup,
                Some(ram),
                0,
                StartupStop::AfterCrcWithErrorPackets,
                false
            )
            .is_err()
        );
    }

    #[test]
    fn host_continuation_requires_selected_root_and_bounded_gate() {
        let (data, mut setup, ram) = fixture();
        setup.host_prelude = vec![Operation {
            address: setup.board.host,
            mask: 0x205c344,
            value: 1000,
            kind: 9,
        }];
        assert!(
            package(
                &data,
                &setup,
                Some(ram.clone()),
                0,
                StartupStop::AfterCrcWithErrorPackets,
                false
            )
            .is_ok()
        );
        setup.host_prelude[0].address += 4;
        assert!(
            package(
                &data,
                &setup,
                Some(ram.clone()),
                0,
                StartupStop::AfterCrcWithErrorPackets,
                false
            )
            .is_err()
        );
        setup.host_prelude[0].address = setup.board.host;
        setup.host_prelude[0].mask |= 1;
        assert!(
            package(
                &data,
                &setup,
                Some(ram.clone()),
                0,
                StartupStop::AfterCrcWithErrorPackets,
                false
            )
            .is_err()
        );
        setup.host_prelude[0].mask &= !1;
        setup.host_prelude[0].value = 1001;
        assert!(
            package(
                &data,
                &setup,
                Some(ram),
                0,
                StartupStop::AfterCrcWithErrorPackets,
                false
            )
            .is_err()
        );
    }
    #[test]
    fn oversized_header_and_changed_payload_refuse() {
        let (mut data, mut s, mut ram) = fixture();
        ram[0x20] ^= 1;
        assert!(package(&data, &s, Some(ram), 0, StartupStop::Continue, false).is_err());
        put(&mut data, 0x14, 254).unwrap();
        s.prefix = source(&data[..0x40000]);
        s.proofs[0].source = s.prefix.clone();
        assert!(package(&data, &s, None, 0, StartupStop::Continue, false).is_err());
    }
}
