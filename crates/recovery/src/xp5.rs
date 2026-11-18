//! Raw XP5 entry, serial NOR preparation, and optional post-DDR FAT32 loading.
use crate::image::{self, File, Result, Source, extent, put, require, word};
use serde::{Deserialize, Serialize};
mod banks;
pub use banks::Banks;
mod card_boot;
pub use card_boot::CardBoot;
mod redirect;
mod template {
    include!(concat!(env!("OUT_DIR"), "/xp5_sd_loader.rs"));
}
/// The card-resident setup hooks of this build are never installed, so their offsets go unused.
#[allow(dead_code)]
mod flash_template {
    include!(concat!(env!("OUT_DIR"), "/xp5_internal_flash_loader.rs"));
}

/// The read-only startup build never installs its card-resident hooks or its gate either.
#[allow(dead_code)]
mod read_only_template {
    include!(concat!(env!("OUT_DIR"), "/xp5_read_only_loader.rs"));
}

/// The checked-redirect build: the read-only build plus one proven redirect of the stock reset
/// stage's section load. It never installs its card-resident hooks or its gate either.
#[allow(dead_code)]
mod checked_redirect_template {
    include!(concat!(env!("OUT_DIR"), "/xp5_checked_redirect_loader.rs"));
}

/// The copy-stub build: the checked-redirect build whose redirect is a stub of its own, written into
/// dead padding of the stock reset image. It never installs its card-resident hooks or its gate.
#[allow(dead_code)]
mod copy_stub_template {
    include!(concat!(env!("OUT_DIR"), "/xp5_copy_stub_loader.rs"));
}

/// The stub's own image (position independent code and a zero parameter block) and the offsets of
/// the parameters in it.
#[allow(dead_code)]
mod stub_template {
    include!(concat!(env!("OUT_DIR"), "/xp5_copy_stub.rs"));
}

/// Where one compiled loader build keeps what the packager binds. `CARD` is the card-resident
/// loader (`template`); `INTERNAL_FLASH` is the same sources built for a startup that keeps the
/// installed flash attached (`flash_template`); `READ_ONLY` only reads the RAM image into spare
/// DRAM and checks it (`read_only_template`).
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
    boot_ready: usize,
    boot_zero_gate: usize,
    externals: &'static [(usize, u32, &'static str)],
    /// Offset of the internal-flash binding; only the internal-flash build has one.
    internal_flash_binding: Option<usize>,
    /// Offset of the read-only binding; only the read-only and checked-redirect builds have one.
    read_only_binding: Option<usize>,
    /// Offset of the checked-redirect binding; only the checked-redirect and copy-stub builds have one.
    redirect_binding: Option<usize>,
    /// Offset of the copy-stub binding; only the copy-stub build has one.
    copy_stub_binding: Option<usize>,
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
    boot_ready: template::A64_BOOT_READY,
    boot_zero_gate: template::A64_BOOT_ZERO_GATE,
    externals: template::EXTERNALS,
    internal_flash_binding: None,
    read_only_binding: None,
    redirect_binding: None,
    copy_stub_binding: None,
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
    boot_ready: flash_template::A64_BOOT_READY,
    boot_zero_gate: flash_template::A64_BOOT_ZERO_GATE,
    externals: flash_template::EXTERNALS,
    internal_flash_binding: Some(flash_template::SETUP_SD_INTERNAL_FLASH),
    read_only_binding: None,
    redirect_binding: None,
    copy_stub_binding: None,
};
const READ_ONLY: Loader = Loader {
    image: read_only_template::IMAGE,
    prelude_stack_max: read_only_template::PRELUDE_STACK_MAX,
    setup_entry: read_only_template::XP5_SETUP_ENTRY,
    setup_configuration: read_only_template::XP5_SETUP_CONFIGURATION,
    setup_return_branch: read_only_template::XP5_SETUP_RETURN_BRANCH,
    setup_stack: read_only_template::XP5_SETUP_STACK,
    setup_stack_top: read_only_template::XP5_SETUP_STACK_TOP,
    sd_initialize: read_only_template::SETUP_SD_INITIALIZE,
    sd_request: read_only_template::SETUP_SD_REQUEST,
    sd_progress: read_only_template::SETUP_SD_PROGRESS,
    sd_board: read_only_template::SETUP_SD_BOARD,
    boot_ready: read_only_template::A64_BOOT_READY,
    boot_zero_gate: read_only_template::A64_BOOT_ZERO_GATE,
    externals: read_only_template::EXTERNALS,
    internal_flash_binding: None,
    read_only_binding: Some(read_only_template::SETUP_SD_READ_ONLY),
    redirect_binding: None,
    copy_stub_binding: None,
};
const CHECKED_REDIRECT: Loader = Loader {
    image: checked_redirect_template::IMAGE,
    prelude_stack_max: checked_redirect_template::PRELUDE_STACK_MAX,
    setup_entry: checked_redirect_template::XP5_SETUP_ENTRY,
    setup_configuration: checked_redirect_template::XP5_SETUP_CONFIGURATION,
    setup_return_branch: checked_redirect_template::XP5_SETUP_RETURN_BRANCH,
    setup_stack: checked_redirect_template::XP5_SETUP_STACK,
    setup_stack_top: checked_redirect_template::XP5_SETUP_STACK_TOP,
    sd_initialize: checked_redirect_template::SETUP_SD_INITIALIZE,
    sd_request: checked_redirect_template::SETUP_SD_REQUEST,
    sd_progress: checked_redirect_template::SETUP_SD_PROGRESS,
    sd_board: checked_redirect_template::SETUP_SD_BOARD,
    boot_ready: checked_redirect_template::A64_BOOT_READY,
    boot_zero_gate: checked_redirect_template::A64_BOOT_ZERO_GATE,
    externals: checked_redirect_template::EXTERNALS,
    internal_flash_binding: None,
    read_only_binding: Some(checked_redirect_template::SETUP_SD_READ_ONLY),
    redirect_binding: Some(checked_redirect_template::SETUP_SD_REDIRECT),
    copy_stub_binding: None,
};
const COPY_STUB: Loader = Loader {
    image: copy_stub_template::IMAGE,
    prelude_stack_max: copy_stub_template::PRELUDE_STACK_MAX,
    setup_entry: copy_stub_template::XP5_SETUP_ENTRY,
    setup_configuration: copy_stub_template::XP5_SETUP_CONFIGURATION,
    setup_return_branch: copy_stub_template::XP5_SETUP_RETURN_BRANCH,
    setup_stack: copy_stub_template::XP5_SETUP_STACK,
    setup_stack_top: copy_stub_template::XP5_SETUP_STACK_TOP,
    sd_initialize: copy_stub_template::SETUP_SD_INITIALIZE,
    sd_request: copy_stub_template::SETUP_SD_REQUEST,
    sd_progress: copy_stub_template::SETUP_SD_PROGRESS,
    sd_board: copy_stub_template::SETUP_SD_BOARD,
    boot_ready: copy_stub_template::A64_BOOT_READY,
    boot_zero_gate: copy_stub_template::A64_BOOT_ZERO_GATE,
    externals: copy_stub_template::EXTERNALS,
    internal_flash_binding: None,
    read_only_binding: Some(copy_stub_template::SETUP_SD_READ_ONLY),
    redirect_binding: Some(copy_stub_template::SETUP_SD_REDIRECT),
    copy_stub_binding: Some(copy_stub_template::SETUP_SD_COPY_STUB),
};
/// The DRAM a read-only startup may fill: the same two numbers as `READ_ONLY_FLOOR` and
/// `READ_ONLY_CEILING` in the loader source `read_only.h`, which refuses anything else. It is the
/// gap that a boot of the X100VI 1.31 firmware to its shooting screen never touches (found in the
/// emulator); the X-T5's `0x20000000` is below it, among the firmware's own buffers.
pub const READ_ONLY_FLOOR: u64 = 0x2445_9000;
pub const READ_ONLY_CEILING: u64 = 0x3e40_8000;
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
    pub proofs: Vec<SourceProof>,
    pub banks: Banks,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card_boot: Option<CardBoot>,
    /// Word edits of the RAM image for a startup that keeps the installed flash attached
    /// (`StartupOptions::internal_flash`). The image selects the reset stage's preloaded mode,
    /// whose other differences from the update's own selector have to be undone here.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub internal_flash_ram_edits: Vec<image::Edit>,
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
) -> Result<Vec<File>> {
    package_with_storage(inverted, setup, ram, success_hold_units, None)
}

pub(crate) fn package_with_storage(
    inverted: &[u8], setup: &Setup, ram: Option<Vec<u8>>, success_hold_units: u64,
    storage: Option<&[u8]>,
) -> Result<Vec<File>> {
    package_inner(inverted, setup, ram, success_hold_units, storage, None, None)
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
    setup.banks = Banks {
        copies: Vec::new(),
        edits: Vec::new(),
    };
    package_inner(
        inverted,
        &setup,
        Some(ram),
        success_hold_units,
        None,
        Some(InternalFlash {
            fallback_wait_seconds,
            ram_edits,
            secondary_sgi,
        }),
        None,
    )
}

/// A startup that only reads: the loader streams `ffboot.bin` into spare DRAM at `destination`,
/// checks its length and CRC-32, reads the DRAM back and checks it again, and goes back to the
/// stock bootstrap, which then starts from the installed flash as on a normal boot. The loader
/// writes no RAM 0, holds and gates no core and executes nothing it read; it halts if RAM 0 changed
/// while it ran. `ffboot.bin` is the RAM image as the stock update installs it: the recipe's plan
/// with its preloaded-selector edit undone (selector 1) and no RAM edits, so its reset-image prefix
/// is byte for byte the stock reset image in the flash. The stock continuation is left in place.
/// A failed read is shown on the lamp and the stock boot goes on all the same.
pub(crate) fn package_read_only(
    inverted: &[u8], setup: &Setup, ram: Vec<u8>, success_hold_units: u64, destination: u64,
) -> Result<Vec<File>> {
    let (setup, ram) = read_only_inputs(inverted, setup, ram)?;
    package_inner(
        inverted,
        &setup,
        Some(ram),
        success_hold_units,
        None,
        None,
        Some(ReadOnly { destination, redirect: None, stub: None }),
    )
}

/// What a read-only startup of the stock update is built from: the setup without the
/// card-resident parts and the RAM image with the stock selector.
fn read_only_inputs(inverted: &[u8], setup: &Setup, ram: Vec<u8>) -> Result<(Setup, Vec<u8>)> {
    let mut setup = setup.clone();
    // The RAM edits are for a card that runs the image; this one only reads it.
    setup.internal_flash_ram_edits = Vec::new();
    // Nothing of the card-resident setup is used: no tables, no unit cache, no interrupt frames.
    setup.card_boot = None;
    setup.banks = Banks {
        copies: Vec::new(),
        edits: Vec::new(),
    };
    // The plan's only edit of the reset image is the preloaded selector; give the stock word back.
    let mut ram = ram;
    let stock = extent(inverted, setup.reset_dat, setup.reset_copy_size)?;
    require(
        setup.selector.checked_add(8).is_some_and(|end| end <= setup.reset_copy_size)
            && setup.reset_copy_size <= ram.len()
            && word(&ram, setup.selector)? == 0
            && word(&ram, setup.selector + 4)? == 0,
        "XP5 RAM image does not carry the preloaded selector",
    )?;
    ram[setup.selector..setup.selector + 8].copy_from_slice(&stock[setup.selector..setup.selector + 8]);
    Ok((setup, ram))
}

/// Whether the loader changes RAM 0 once the redirect is proven.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CheckedRedirect {
    /// Prove the redirect (the DRAM copy equals the flash) and report it; write nothing to RAM 0.
    VerifyOnly,
    /// Also change the one word of the stock reset image that sends the stage to the DRAM copy.
    Apply,
}

/// A read-only startup plus one checked redirect: the loader proves, against the installed flash,
/// that a section of the DRAM copy it just read equals what the flash holds, and with `Apply`
/// changes the one word of the stock reset image in RAM 0 that makes the stock reset stage copy
/// that section from the DRAM copy instead of from the flash (`redirect.rs`). `ffboot.bin` is the
/// unedited image cut after that section, so the card holds only what the redirect names and the
/// file is read in a fraction of a second. Nothing the loader does can change what the stock stage
/// copies, only where it copies it from; an unproven redirect is never made.
pub(crate) fn package_checked_redirect(
    inverted: &[u8], setup: &Setup, ram: Vec<u8>, success_hold_units: u64, destination: u64,
    mode: CheckedRedirect,
) -> Result<Vec<File>> {
    let (setup, mut ram) = read_only_inputs(inverted, setup, ram)?;
    let bias = setup
        .reset_dat
        .checked_sub(redirect::RESET_FLASH)
        .ok_or_else(|| image::Error::Invalid("The reset image precedes its flash offset".into()))?;
    let reset = extent(&ram, 0, setup.reset_copy_size)?.to_vec();
    let plan = redirect::locate(&reset, inverted, bias, &ram)?;
    let prefix = (plan.dst as usize + plan.bytes + 0xfff) & !0xfff;
    require(prefix <= ram.len(), "The redirected section lies outside the RAM image")?;
    ram.truncate(prefix);
    let binding = redirect::binding(
        &plan,
        &reset,
        inverted,
        bias,
        setup.flash.aperture,
        destination,
        setup.reset_copy_size,
        ram.len(),
        mode == CheckedRedirect::Apply,
    )?;
    package_inner(
        inverted,
        &setup,
        Some(ram),
        success_hold_units,
        None,
        None,
        Some(ReadOnly { destination, redirect: Some(binding), stub: None }),
    )
}

/// The checked redirect with a copy stub in place of the one-word source change. The loader
/// proves the same things (tables, the flash range, the DRAM copy equal to it, the stock reset
/// image) and, with `Apply`, writes a stub into the nop padding of the stock reset image and
/// replaces the stage's copy call by a call to it. The stub checks the DRAM copy and the destination
/// at the moment of use and copies from the flash on any difference (`redirect.rs`, `copy_stub.S`).
/// `ffboot.bin` is the same cut image as the checked redirect's.
pub(crate) fn package_copy_stub(
    inverted: &[u8], setup: &Setup, ram: Vec<u8>, success_hold_units: u64, destination: u64,
    mode: CheckedRedirect,
) -> Result<Vec<File>> {
    let (setup, mut ram) = read_only_inputs(inverted, setup, ram)?;
    let bias = setup
        .reset_dat
        .checked_sub(redirect::RESET_FLASH)
        .ok_or_else(|| image::Error::Invalid("The reset image precedes its flash offset".into()))?;
    let reset = extent(&ram, 0, setup.reset_copy_size)?.to_vec();
    let plan = redirect::locate(&reset, inverted, bias, &ram)?;
    let prefix = (plan.dst as usize + plan.bytes + 0xfff) & !0xfff;
    require(prefix <= ram.len(), "The redirected section lies outside the RAM image")?;
    ram.truncate(prefix);
    let lamp = &setup.lamp;
    let binding = redirect::stub_binding(
        &plan,
        &reset,
        inverted,
        bias,
        setup.flash.aperture,
        destination,
        setup.reset_copy_size,
        ram.len(),
        mode == CheckedRedirect::Apply,
        &redirect::StubLamp {
            pio_base: lamp.pio_base,
            green_port: lamp.green_port,
            red_port: lamp.red_port,
            pin: lamp.pin,
        },
        stub_template::IMAGE,
    )?;
    package_inner(
        inverted,
        &setup,
        Some(ram),
        success_hold_units,
        None,
        None,
        Some(ReadOnly { destination, redirect: Some(redirect::Binding(binding.redirect.0)), stub: Some(binding) }),
    )
}

/// How a read-only startup differs from a card-resident one.
struct ReadOnly {
    /// DRAM address the loader streams the file to.
    destination: u64,
    /// The checked redirect the loader proves (and, if bound to, makes) once the file is read.
    redirect: Option<redirect::Binding>,
    /// The copy stub of the copy-stub build: with it, `redirect` is the stub build's binding of the
    /// copy call, not of the source translator.
    stub: Option<redirect::StubBinding>,
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

/// Fill the read-only loader binding: where in DRAM the file goes, and what the RAM-0 guard
/// covers (the stock reset image's extent and its selector word). The loader refuses a bound
/// outside its own floor and ceiling, and so does the packager, before any file is written. The
/// last five words are the loader's own outputs.
fn bind_read_only(
    program: &mut [u8], loader: &Loader, setup: &Setup, read_only: &ReadOnly, bytes: usize,
) -> Result<()> {
    let at = loader.read_only_binding.ok_or_else(|| {
        image::Error::Invalid("This XP5 loader build has no read-only binding".into())
    })?;
    require(
        extent(program, at, 72)? == [1u64, 0, 0, 0, 0, 0, 0, 0, 0].map(u64::to_le_bytes).concat(),
        "XP5 read-only binding template changed",
    )?;
    let destination = read_only.destination;
    require(
        destination % 0x1000 == 0
            && (READ_ONLY_FLOOR..READ_ONLY_CEILING).contains(&destination)
            && bytes as u64 <= READ_ONLY_CEILING - destination,
        "The read-only destination is outside the DRAM the loader may fill",
    )?;
    require(
        setup.reset_copy_size >= 8
            && setup.reset_copy_size <= 0x10_0000
            && setup.reset_copy_size % 4 == 0
            && setup.selector % 4 == 0
            && setup.selector + 8 <= setup.reset_copy_size,
        "The read-only guard does not fit the stock reset image",
    )?;
    for (n, value) in [destination, setup.reset_copy_size as u64, setup.selector as u64]
        .into_iter()
        .enumerate()
    {
        put64(program, at + 8 + n * 8, value)?;
    }
    Ok(())
}

fn package_inner(
    inverted: &[u8], setup: &Setup, ram: Option<Vec<u8>>, success_hold_units: u64,
    storage: Option<&[u8]>, internal_flash: Option<InternalFlash>, read_only: Option<ReadOnly>,
) -> Result<Vec<File>> {
    require(
        internal_flash.is_none() || read_only.is_none(),
        "An XP5 startup is internal-flash or read-only, not both",
    )?;
    let prefix = setup.verify(inverted)?;
    let loader = if let Some(read_only) = &read_only {
        if read_only.stub.is_some() {
            &COPY_STUB
        } else if read_only.redirect.is_some() {
            &CHECKED_REDIRECT
        } else {
            &READ_ONLY
        }
    } else if internal_flash.is_some() {
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
    if internal_flash.is_some() || read_only.is_some() {
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
    let mut empty = vec![0; 208];
    empty[0] = 2;
    require(
        extent(&program, cfg, 208)? == empty,
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
        u64::from(ram.is_none() || internal_flash.is_some() || read_only.is_some()),
    )?;
    if let Some(flash) = &internal_flash {
        bind_internal_flash(&mut program, loader, setup, flash, ram.as_deref(), inverted)?;
    }
    if let Some(read_only) = &read_only {
        let bytes = ram.as_ref().map_or(0, Vec::len);
        bind_read_only(&mut program, loader, setup, read_only, bytes)?;
        if let Some(binding) = &read_only.redirect {
            let at = loader.redirect_binding.ok_or_else(|| {
                image::Error::Invalid("This XP5 loader build has no checked-redirect binding".into())
            })?;
            redirect::bind(&mut program, at, binding)?;
        }
        if let Some(stub) = &read_only.stub {
            let at = loader.copy_stub_binding.ok_or_else(|| {
                image::Error::Invalid("This XP5 loader build has no copy-stub binding".into())
            })?;
            redirect::bind_stub(&mut program, at, stub)?;
        }
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
        // The read-only startup keeps the stock selector in its image.
        if read_only.is_none() {
            put64(&mut reset, setup.selector, 0)?;
        }
        require(
            reset == ram[..setup.reset_copy_size],
            "XP5 reset copy differs beyond preloaded selector",
        )?;
        if let Some(flash) = &internal_flash {
            edit_ram(&mut ram, &flash.ram_edits, setup.selector)?;
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
            board.operations.len() <= 32 && board.timeout_setting <= 14 && mmio(board.host),
            "Invalid XP5 SD binding",
        )?;
        require(
            extent(&program, cfg, 16 + 32 * 24)?.iter().all(|v| *v == 0),
            "XP5 board template changed",
        )?;
        put64(&mut program, cfg, board.host)?;
        put(&mut program, cfg + 8, u32::from(board.timeout_setting))?;
        put(&mut program, cfg + 12, board.operations.len() as u32)?;
        for (n, op) in board.operations.iter().enumerate() {
            let valid = match op.kind {
                0 | 1 => mmio(op.address),
                2 => true,
                3 | 4 => (0xf0000000..0xfffff000).contains(&op.address)
                    && op.address.is_multiple_of(2)
                    && op.mask <= u16::MAX.into()
                    && op.value & !op.mask == 0,
                5 => mmio(op.address) && op.mask <= u16::MAX.into()
                    && op.value & !op.mask == 0,
                6 => op.address == 0 && op.mask == 0 && (1..=1000).contains(&op.value),
                _ => false,
            };
            require(
                valid,
                "Invalid XP5 SD operation",
            )?;
            let at = cfg + 16 + n * 24;
            put64(&mut program, at, op.address)?;
            put(&mut program, at + 8, op.mask)?;
            put(&mut program, at + 12, op.value)?;
            put(&mut program, at + 16, op.kind)?;
        }
        let request = loader.sd_request;
        require(
            extent(&program, request, 40)? == [1u64, 0, 0, 0, 0].map(u64::to_le_bytes).concat(),
            "XP5 request template changed",
        )?;
        for (n, value) in [
            1,
            ram.len() as u64,
            u64::from(crate::xp4::crc32(&ram)),
            100,
            100000,
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
        // The read-only startup leaves the stock continuation alone (the bootstrap's
        // `ldr x30, =0xf0027110; ret` into the flash, whose Aff0-zero path loads zero and
        // returns to RAM 0): nothing is written to RAM 0, so there is nothing to gate.
        if read_only.is_none() {
            put(
                &mut raw,
                (gate - setup.base) as usize,
                branch(gate, address + loader.boot_zero_gate as u64, false)?,
            )?;
        }
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
        files.push(File::new("ffboot.bin", ram)?);
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
    fn startup_skips_installed_flash_probe_while_stage_one_preserves_it() {
        let (data, s, ram) = fixture();
        for payload in [None, Some(ram.clone())] {
            let combined = payload.is_some();
            let files = package(&data, &s, payload, 0).unwrap();
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
        let card = package(&data, &s, Some(ram.clone()), 0).unwrap();
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
        let bare = package(&data, &plain, Some(ram), 0).unwrap();
        assert_eq!(bare[1].bytes, flash[1].bytes);
        assert_eq!(flag(&bare[0].bytes, template::XP5_SETUP_CONFIGURATION), 0);
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
        let card = package(&data, &card_setup, Some(ram.clone()), 0).unwrap();
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

    /// Every word of the internal-flash loader the packager may touch: the setup configuration,
    /// the SD board and request tables, its own binding, the restore branch and the calls into
    /// the stock setup.
    fn bound_ranges() -> Vec<std::ops::Range<usize>> {
        let mut ranges = vec![
            flash_template::XP5_SETUP_CONFIGURATION..flash_template::XP5_SETUP_CONFIGURATION + 208,
            flash_template::SETUP_SD_BOARD..flash_template::SETUP_SD_BOARD + 16 + 32 * 24,
            flash_template::SETUP_SD_REQUEST..flash_template::SETUP_SD_REQUEST + 40,
            flash_template::SETUP_SD_INTERNAL_FLASH..flash_template::SETUP_SD_INTERNAL_FLASH + 96,
            flash_template::XP5_SETUP_RETURN_BRANCH..flash_template::XP5_SETUP_RETURN_BRANCH + 4,
        ];
        ranges.extend(flash_template::EXTERNALS.iter().map(|&(at, _, _)| at..at + 4));
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
            let normal = package(&data, &s, payload.clone(), 0).unwrap();
            let at = 0xc000 + template::XP5_SETUP_CONFIGURATION + 192;
            assert_eq!(&normal[0].bytes[at..at + 8], &[0; 8]);
            for units in [1, 32, 6_144, u64::from(u32::MAX) * 2_048] {
                let held = package(&data, &s, payload.clone(), units).unwrap();
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
    fn startup_accepts_a_shorter_checked_setup_prefix() {
        let (data, mut setup, ram) = fixture();
        setup.prefix = source(&data[..0x28000]);
        setup.proofs[0].source = setup.prefix.clone();
        assert!(package(&data, &setup, Some(ram), 0).is_ok());
        setup.prefix = source(&data[..0x800]);
        setup.proofs[0].source = setup.prefix.clone();
        assert!(package(&data, &setup, None, 0).is_err());
    }

    #[test]
    fn changed_source_and_incomplete_bindings_refuse() {
        let (data, s, ram) = fixture();
        let mut changed = data.clone();
        changed[0x1000] ^= 1;
        assert!(package(&changed, &s, None, 0).is_err());
        let mut changed = s.clone();
        changed.flash.pins.clear();
        assert!(package(&data, &changed, None, 0).is_err());
        let mut changed = s.clone();
        changed.initializer.sha256 = "0".repeat(64);
        assert!(package(&data, &changed, None, 0).is_err());
        let mut changed = s.clone();
        changed.board.operations[0].kind = 7;
        assert!(package(&data, &changed, Some(ram.clone()), 0).is_err());
        let mut changed = s.clone();
        changed.gate_offset = 0x940;
        assert!(package(&data, &changed, Some(ram), 0).is_err());
        let mut changed = s.clone();
        changed.stack = s.base + 0x38000;
        assert!(package(&data, &changed, None, 0).is_err());
    }
    #[test]
    fn oversized_header_and_changed_payload_refuse() {
        let (mut data, mut s, mut ram) = fixture();
        ram[0x20] ^= 1;
        assert!(package(&data, &s, Some(ram), 0).is_err());
        put(&mut data, 0x14, 254).unwrap();
        s.prefix = source(&data[..0x40000]);
        s.proofs[0].source = s.prefix.clone();
        assert!(package(&data, &s, None, 0).is_err());
    }

    /// Every word of the read-only loader the packager may touch.
    fn read_only_bound_ranges() -> Vec<std::ops::Range<usize>> {
        use read_only_template as t;
        let mut ranges = vec![
            t::XP5_SETUP_CONFIGURATION..t::XP5_SETUP_CONFIGURATION + 208,
            t::SETUP_SD_BOARD..t::SETUP_SD_BOARD + 16 + 32 * 24,
            t::SETUP_SD_REQUEST..t::SETUP_SD_REQUEST + 40,
            t::SETUP_SD_READ_ONLY..t::SETUP_SD_READ_ONLY + 72,
            t::XP5_SETUP_RETURN_BRANCH..t::XP5_SETUP_RETURN_BRANCH + 4,
        ];
        ranges.extend(t::EXTERNALS.iter().map(|&(at, _, _)| at..at + 4));
        ranges
    }

    #[test]
    fn the_read_only_startup_binds_a_destination_and_leaves_the_stock_continuation_alone() {
        let (data, s, ram) = fixture();
        let at = 0xc000 + read_only_template::SETUP_SD_READ_ONLY;
        let gate = (s.gate_offset - 0x800) as usize;
        let call = (s.bootstrap_offset - 0x800 + 36) as usize;
        let address = s.base + 0xc000;
        // The recipe's card-resident setup (interrupt frames) is not needed and not used.
        for setup in [s.clone(), with_frames(s.clone())] {
            for destination in [READ_ONLY_FLOOR, 0x3000_0000, READ_ONLY_CEILING - 0x1000] {
                let files = package_read_only(&data, &setup, ram.clone(), 0, destination).unwrap();
                assert_eq!(
                    files.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
                    ["marble_init.bin", "ffboot.bin"]
                );
                let raw = &files[0].bytes;
                // The RAM image as the update installs it: the stock selector, no edits, so its
                // reset-image prefix is the stock reset image byte for byte.
                assert_eq!(files[1].bytes, data[0x40000..0x41000].to_vec());
                assert_ne!(files[1].bytes, ram);
                assert!(raw.len() <= 0x38800);
                let field = |n: usize| u64::from_le_bytes(raw[at + 8 * n..at + 8 * n + 8].try_into().unwrap());
                // The binding: the destination, then what the RAM-0 guard covers (the stock reset
                // image's extent and its selector word); the loader's five output words start empty.
                assert_eq!(
                    (0..4).map(field).collect::<Vec<_>>(),
                    [1, destination, s.reset_copy_size as u64, s.selector as u64]
                );
                assert!((4..9).all(|n| field(n) == 0));
                // The stock initializer's call goes to the loader; the stock continuation
                // (`ldr x30, =0xf0027110; ret`, into the flash) is untouched, so no gate waits
                // and nothing goes to RAM 0.
                assert_eq!(
                    word(raw, call).unwrap(),
                    branch(
                        s.base + call as u64,
                        address + read_only_template::SETUP_SD_INITIALIZE as u64,
                        true
                    )
                    .unwrap()
                );
                assert_eq!(word(raw, gate).unwrap(), word(&data, gate + 0x800).unwrap());
                assert_eq!(word(raw, gate + 4).unwrap(), word(&data, gate + 0x804).unwrap());
                // It probes the installed flash like stage one and requests the whole file.
                let flag = 0xc000 + read_only_template::XP5_SETUP_CONFIGURATION + 200;
                assert_eq!(u64::from_le_bytes(raw[flag..flag + 8].try_into().unwrap()), 1);
                let request = 0xc000 + read_only_template::SETUP_SD_REQUEST;
                assert_eq!(
                    u64::from_le_bytes(raw[request + 8..request + 16].try_into().unwrap()),
                    ram.len() as u64
                );
                assert_eq!(word(raw, request + 16).unwrap(), crate::xp4::crc32(&files[1].bytes));
                // The loader is the compiled read-only build: nothing outside the bound ranges
                // differs from it, and nothing is appended.
                let bound = read_only_bound_ranges();
                let loader = &raw[0xc000..0xc000 + read_only_template::IMAGE.len()];
                for at in (0..loader.len()).step_by(4) {
                    if !bound.iter().any(|range| range.contains(&at)) {
                        assert_eq!(
                            word(loader, at).unwrap(),
                            word(read_only_template::IMAGE, at).unwrap(),
                            "{at:#x}"
                        );
                    }
                }
                let header = 0xc000 + read_only_template::IMAGE.len();
                assert!(raw[header + 0x800..].iter().all(|&byte| byte == 0xff));
            }
        }
        // Unlike an internal-flash startup's image, which carries the preloaded selector and the
        // recipe's edits so that it could run, this one differs from it by exactly those.
        let flash = package_internal_flash(&data, &with_frames(s.clone()), ram.clone(), 0, None).unwrap();
        let only = package_read_only(&data, &s, ram.clone(), 0, 0x3000_0000).unwrap();
        assert_eq!(flash[1].bytes, ram);
        let differing: Vec<usize> =
            (0..ram.len()).filter(|&n| flash[1].bytes[n] != only[1].bytes[n]).collect();
        assert_eq!(differing, [s.selector]);
        assert_ne!(flash[0].bytes, only[0].bytes);
    }

    #[test]
    fn the_read_only_startup_ignores_the_recipes_ram_edits_and_keeps_the_stock_selector() {
        let (data, s, ram) = fixture();
        let mut setup = s.clone();
        setup.internal_flash_ram_edits = vec![image::Edit {
            address: 0x100,
            old: 0xffff_ffff,
            new: 0x1400_0068,
            what: "branch to the retained-metadata path".into(),
        }];
        let only = package_read_only(&data, &setup, ram.clone(), 0, 0x3000_0000).unwrap();
        let plain = package_read_only(&data, &s, ram.clone(), 0, 0x3000_0000).unwrap();
        assert_eq!(only[1].bytes, plain[1].bytes);
        assert_eq!(only[1].bytes, data[0x40000..0x41000].to_vec());
        assert_eq!(only[0].bytes, plain[0].bytes);
        let request = 0xc000 + read_only_template::SETUP_SD_REQUEST;
        assert_eq!(word(&only[0].bytes, request + 16).unwrap(), crate::xp4::crc32(&only[1].bytes));
        // An image that does not carry the preloaded selector (so is not the plan's) refuses, and
        // so does one whose reset prefix differs from the stock image anywhere else.
        let mut stock_selector = ram.clone();
        put64(&mut stock_selector, s.selector, 1).unwrap();
        assert!(package_read_only(&data, &s, stock_selector, 0, 0x3000_0000).is_err());
        let mut changed = ram.clone();
        changed[0x100] ^= 1;
        assert!(package_read_only(&data, &s, changed, 0, 0x3000_0000).is_err());
    }

    #[test]
    fn the_read_only_destination_has_to_lie_wholly_in_the_dram_the_loader_may_fill() {
        let (data, s, ram) = fixture();
        let size = ram.len() as u64;
        assert_eq!(size, 0x1000);
        assert_eq!((READ_ONLY_FLOOR, READ_ONLY_CEILING), (0x2445_9000, 0x3e40_8000));
        for good in [READ_ONLY_FLOOR, READ_ONLY_CEILING - size, 0x3000_0000] {
            assert!(package_read_only(&data, &s, ram.clone(), 0, good).is_ok(), "{good:#x}");
        }
        for bad in [
            0,
            0x1000,
            0x0690_f000,
            // the X-T5's address, where the X100VI keeps live-view buffers, and the first
            // megabytes above the stock RAM image
            0x2000_0000,
            0x1000_0000,
            READ_ONLY_FLOOR - 0x1000,
            // not 4 KiB aligned
            0x3000_0004,
            0x2000_0800,
            // the file would run past the ceiling, or start at or beyond it
            READ_ONLY_CEILING - size + 0x1000,
            READ_ONLY_CEILING,
            0xf000_0000,
            0xfa00_0000,
            u64::MAX,
        ] {
            assert!(package_read_only(&data, &s, ram.clone(), 0, bad).is_err(), "{bad:#x}");
        }
    }

    #[test]
    fn the_read_only_guard_needs_a_reset_image_that_holds_its_selector_word() {
        let (data, s, ram) = fixture();
        assert!(package_read_only(&data, &s, ram.clone(), 0, 0x3000_0000).is_ok());
        // A guard shorter than the 8-byte selector word cannot hold it (the loader would
        // underflow `guard_bytes - 8`), whatever else is true of the image.
        for size in [0usize, 4, 7] {
            let mut short = s.clone();
            short.reset_copy_size = size;
            assert!(package_read_only(&data, &short, ram.clone(), 0, 0x3000_0000).is_err(), "{size}");
        }
        // The selector word has to lie wholly inside the guarded extent and be word aligned.
        for (selector, size) in [(0x40usize, 0x44usize), (0x42, 0x1000), (0xff8, 0x1000)] {
            let mut odd = s.clone();
            odd.selector = selector;
            odd.reset_copy_size = size;
            assert!(package_read_only(&data, &odd, ram.clone(), 0, 0x3000_0000).is_err(), "{selector:#x} {size:#x}");
        }
    }

    #[test]
    fn the_read_only_loader_asset_keeps_its_binding_template_and_no_other_build_has_one() {
        use read_only_template as t;
        assert_eq!(t::SETUP_SD_READ_ONLY % 8, 0);
        assert_eq!(
            &t::IMAGE[t::SETUP_SD_READ_ONLY..t::SETUP_SD_READ_ONLY + 72],
            [1u64, 0, 0, 0, 0, 0, 0, 0, 0].map(u64::to_le_bytes).concat()
        );
        assert_eq!(READ_ONLY.read_only_binding, Some(t::SETUP_SD_READ_ONLY));
        assert_eq!((CARD.read_only_binding, INTERNAL_FLASH.read_only_binding), (None, None));
        assert_eq!(READ_ONLY.internal_flash_binding, None);
        let layout = include_str!("../assets/sd-boot-v1-xp5-read-only-loader.generated.json");
        assert!(layout.contains("setup_sd_read_only") && !layout.contains("setup_sd_internal_flash"));
        assert!(!include_str!("../assets/sd-boot-v1-xp5-loader.generated.json").contains("setup_sd_read_only"));
        assert!(!include_str!("../assets/sd-boot-v1-xp5-internal-flash-loader.generated.json")
            .contains("setup_sd_read_only"));
        assert_ne!(t::IMAGE, template::IMAGE);
        assert_ne!(t::IMAGE, flash_template::IMAGE);
        // Same prelude as the other two builds.
        assert_eq!(t::PRELUDE_STACK_MAX, template::PRELUDE_STACK_MAX);
        assert_eq!(t::XP5_SETUP_RETURN_BRANCH, template::XP5_SETUP_RETURN_BRANCH);
    }

    /// The synthetic update of `redirect::tests::stage` in the xp5 fixture's setup: the stock reset
    /// stage with the section load, both table copies, both tables and a raw section 0.
    fn redirect_fixture() -> (Vec<u8>, Setup, Vec<u8>) {
        let (fixture_data, mut s, _) = fixture();
        let (reset, mut update, _) = redirect::tests::stage(0);
        let ram_size = 0x8_1000 + redirect::tests::E2E_SECTION_BYTES;
        // The fixture's first 0x40000 bytes stay (its prefix proofs hash them); the stage sits where
        // the fixture's reset image does, with the selector word 1.
        update[..0x40000].copy_from_slice(&fixture_data[..0x40000]);
        let mut stock = reset.clone();
        put64(&mut stock, s.selector, 1).unwrap();
        update[0x40000..0x40000 + stock.len()].copy_from_slice(&stock);
        s.reset_copy_size = 0xb000;
        let mut ram = vec![0u8; ram_size];
        ram[..0xb000].copy_from_slice(&stock);
        put64(&mut ram, s.selector, 0).unwrap();
        ram[0x4_0000..0x4_0800].copy_from_slice(&update[0x64_0000..0x64_0800]);
        ram[0x8_1000..].copy_from_slice(&update[0xc_0000..0xc_0000 + redirect::tests::E2E_SECTION_BYTES]);
        (update, s, ram)
    }

    #[test]
    fn the_checked_redirect_packages_a_cut_image_and_binds_every_proof() {
        let (data, s, ram) = redirect_fixture();
        let at = 0xc000 + checked_redirect_template::SETUP_SD_REDIRECT;
        let read_only = 0xc000 + checked_redirect_template::SETUP_SD_READ_ONLY;
        for (mode, apply) in [(CheckedRedirect::Apply, 1u64), (CheckedRedirect::VerifyOnly, 0)] {
            let files = package_checked_redirect(&data, &s, ram.clone(), 0, 0x3000_0000, mode).unwrap();
            assert_eq!(
                files.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
                ["marble_init.bin", "ffboot.bin"]
            );
            // The image is cut after the section, and is the unedited image's prefix.
            let ffboot = &files[1].bytes;
            assert_eq!(ffboot.len(), 0x8_3000);
            let mut expected = ram.clone();
            put64(&mut expected, s.selector, 1).unwrap();
            assert_eq!(ffboot[..], expected[..0x8_3000]);
            let marble = &files[0].bytes;
            let word64 = |at: usize, n: usize| u64::from_le_bytes(marble[at + 8 * n..at + 8 * n + 8].try_into().unwrap());
            assert_eq!((word64(at, 0), word64(at, 1), word64(at, 2)), (1, apply, 0x2cf0));
            assert_eq!(word64(at, 4), 0xb264_0680);
            assert_eq!((word64(at, 7), word64(at, 8), word64(at, 9)), (0x8_1000, 0x2000, 0xf00c_0000));
            // The outputs stay zero, the request names the cut file, the read-only binding the destination.
            assert!((16..20).all(|n| word64(at, n) == 0));
            let request = 0xc000 + checked_redirect_template::SETUP_SD_REQUEST;
            assert_eq!(
                (word64(request, 1), word64(request, 2)),
                (ffboot.len() as u64, u64::from(crate::xp4::crc32(ffboot)))
            );
            assert_eq!(word64(read_only, 1), 0x3000_0000);
        }
        // The two modes differ in the apply word alone.
        let apply = package_checked_redirect(&data, &s, ram.clone(), 0, 0x3000_0000, CheckedRedirect::Apply).unwrap();
        let verify = package_checked_redirect(&data, &s, ram.clone(), 0, 0x3000_0000, CheckedRedirect::VerifyOnly).unwrap();
        let differing: Vec<usize> =
            (0..apply[0].bytes.len()).filter(|&n| apply[0].bytes[n] != verify[0].bytes[n]).collect();
        assert_eq!(differing, [at + 8]);
        assert_eq!(apply[1].bytes, verify[1].bytes);
    }

    #[test]
    fn the_checked_redirect_refuses_what_it_cannot_prove() {
        let (data, s, ram) = redirect_fixture();
        let go = |data: &[u8], s: &Setup, ram: Vec<u8>, destination: u64| {
            package_checked_redirect(data, s, ram, 0, destination, CheckedRedirect::Apply)
        };
        assert!(go(&data, &s, ram.clone(), 0x3000_0000).is_ok());
        // A destination outside the window, unaligned, not one run of ones, or one that overlaps the
        // section's own address.
        for bad in [0x2000_0000, 0x3000_0004, 0x3000_1000, READ_ONLY_CEILING, 0x2445_9000] {
            assert!(go(&data, &s, ram.clone(), bad).is_err(), "{bad:#x}");
        }
        // The card's section differs from the flash's, the cold table differs, the stage differs.
        let mut bad = ram.clone();
        bad[0x8_1000 + 5] ^= 1;
        assert!(go(&data, &s, bad, 0x3000_0000).is_err());
        let mut bad = ram.clone();
        bad[0x4_0058] ^= 1;
        assert!(go(&data, &s, bad, 0x3000_0000).is_err());
        let mut other = data.clone();
        other[0x40000 + 0x2cf0] ^= 0x20;
        let mut ram_other = ram.clone();
        ram_other[0x2cf0] ^= 0x20;
        assert!(go(&other, &s, ram_other, 0x3000_0000).is_err());
        // The plain fixture has no such stage at all.
        let (plain, plain_setup, plain_ram) = fixture();
        assert!(go(&plain, &plain_setup, plain_ram, 0x3000_0000).is_err());
    }

    #[test]
    fn the_checked_redirect_loader_asset_keeps_its_two_binding_templates_and_no_other_build_has_them() {
        use checked_redirect_template as t;
        assert_eq!((t::SETUP_SD_REDIRECT % 8, t::SETUP_SD_READ_ONLY % 8), (0, 0));
        let mut template = vec![1u64];
        template.resize(20, 0);
        assert_eq!(
            &t::IMAGE[t::SETUP_SD_REDIRECT..t::SETUP_SD_REDIRECT + 160],
            template.iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<_>>()
        );
        assert_eq!(CHECKED_REDIRECT.redirect_binding, Some(t::SETUP_SD_REDIRECT));
        assert_eq!(CHECKED_REDIRECT.read_only_binding, Some(t::SETUP_SD_READ_ONLY));
        assert_eq!(
            (CARD.redirect_binding, INTERNAL_FLASH.redirect_binding, READ_ONLY.redirect_binding),
            (None, None, None)
        );
        let layout = include_str!("../assets/sd-boot-v1-xp5-checked-redirect-loader.generated.json");
        assert!(layout.contains("setup_sd_redirect") && layout.contains("setup_sd_read_only"));
        for other in [
            include_str!("../assets/sd-boot-v1-xp5-loader.generated.json"),
            include_str!("../assets/sd-boot-v1-xp5-internal-flash-loader.generated.json"),
            include_str!("../assets/sd-boot-v1-xp5-read-only-loader.generated.json"),
        ] {
            assert!(!other.contains("setup_sd_redirect"));
        }
    }

    #[test]
    fn the_copy_stub_packages_the_same_cut_image_and_binds_the_stub_and_every_proof() {
        let (data, s, ram) = redirect_fixture();
        let at = 0xc000 + copy_stub_template::SETUP_SD_REDIRECT;
        let stub_at = 0xc000 + copy_stub_template::SETUP_SD_COPY_STUB;
        let read_only = 0xc000 + copy_stub_template::SETUP_SD_READ_ONLY;
        let checked = package_checked_redirect(&data, &s, ram.clone(), 0, 0x3000_0000, CheckedRedirect::Apply).unwrap();
        for (mode, apply) in [(CheckedRedirect::Apply, 1u64), (CheckedRedirect::VerifyOnly, 0)] {
            let files = package_copy_stub(&data, &s, ram.clone(), 0, 0x3000_0000, mode).unwrap();
            assert_eq!(
                files.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
                ["marble_init.bin", "ffboot.bin"]
            );
            // The file is the checked redirect's cut image, byte for byte.
            assert_eq!(files[1].bytes, checked[1].bytes);
            let marble = &files[0].bytes;
            let word64 = |at: usize, n: usize| u64::from_le_bytes(marble[at + 8 * n..at + 8 * n + 8].try_into().unwrap());
            // The call word is the copy call (0x2d04), its stock word and a `bl` to the stub at 0x200.
            assert_eq!((word64(at, 0), word64(at, 1), word64(at, 2)), (1, apply, 0x2d04));
            assert_eq!(word64(at, 3), 0x9400_0000 | (((0xa090i64 - 0x2d04) / 4) as u64 & 0x03ff_ffff));
            assert_eq!(word64(at, 4), 0x9400_0000 | (((0x200i64 - 0x2d04) / 4) as u64 & 0x03ff_ffff));
            assert_eq!((word64(at, 7), word64(at, 8), word64(at, 9)), (0x8_1000, 0x2000, 0xf00c_0000));
            assert!((16..20).all(|n| word64(at, n) == 0));
            // The stub binding: where, how long, the filler word, the CRC-32, the bytes, zero outputs.
            let len = stub_template::IMAGE.len() as u64;
            assert_eq!((word64(stub_at, 0), word64(stub_at, 1), word64(stub_at, 2), word64(stub_at, 3)), (1, 0x200, len, 0xd503_201f));
            let image = &marble[stub_at + 48..stub_at + 48 + len as usize];
            assert_eq!(word64(stub_at, 4), u64::from(crate::xp4::crc32(image)));
            assert_eq!(word64(stub_at, 5), stub_template::XP5_STUB_PARAMS as u64);
            assert_eq!(image[..stub_template::XP5_STUB_PARAMS], stub_template::IMAGE[..stub_template::XP5_STUB_PARAMS]);
            assert!(marble[stub_at + 48 + len as usize..stub_at + 48 + 1024 + 32].iter().all(|b| *b == 0));
            assert_eq!(word64(read_only, 1), 0x3000_0000);
            let request = 0xc000 + copy_stub_template::SETUP_SD_REQUEST;
            assert_eq!(
                (word64(request, 1), word64(request, 2)),
                (files[1].bytes.len() as u64, u64::from(crate::xp4::crc32(&files[1].bytes)))
            );
        }
        let apply = package_copy_stub(&data, &s, ram.clone(), 0, 0x3000_0000, CheckedRedirect::Apply).unwrap();
        let verify = package_copy_stub(&data, &s, ram.clone(), 0, 0x3000_0000, CheckedRedirect::VerifyOnly).unwrap();
        let differing: Vec<usize> =
            (0..apply[0].bytes.len()).filter(|&n| apply[0].bytes[n] != verify[0].bytes[n]).collect();
        assert_eq!(differing, [at + 8]);
        // The loader itself is a build of its own: the checked-redirect card's loader differs.
        assert_ne!(apply[0].bytes, checked[0].bytes);
    }

    #[test]
    fn the_copy_stub_refuses_what_it_cannot_prove_or_place() {
        let (data, s, ram) = redirect_fixture();
        let go = |data: &[u8], s: &Setup, ram: Vec<u8>, destination: u64| {
            package_copy_stub(data, s, ram, 0, destination, CheckedRedirect::Apply)
        };
        assert!(go(&data, &s, ram.clone(), 0x3000_0000).is_ok());
        for bad in [0x2000_0000, 0x3000_0004, READ_ONLY_CEILING, 0x2445_9000 - 0x1000] {
            assert!(go(&data, &s, ram.clone(), bad).is_err(), "{bad:#x}");
        }
        let mut bad = ram.clone();
        bad[0x8_1000 + 5] ^= 1;
        assert!(go(&data, &s, bad, 0x3000_0000).is_err());
        // A stage whose padding is not nops, or which refers to it.
        for (at, value) in [(0x3c0usize, 0xaa1f_03e1u32), (0x3000, 0x9400_0000 | (((0x400i64 - 0x3000) / 4) as u32 & 0x03ff_ffff))] {
            let mut other = data.clone();
            other[0x40000 + at..0x40000 + at + 4].copy_from_slice(&value.to_le_bytes());
            let mut ram_other = ram.clone();
            ram_other[at..at + 4].copy_from_slice(&value.to_le_bytes());
            assert!(go(&other, &s, ram_other, 0x3000_0000).is_err(), "{at:#x}");
        }
        // The plain fixture has no such stage at all.
        let (plain, plain_setup, plain_ram) = fixture();
        assert!(go(&plain, &plain_setup, plain_ram, 0x3000_0000).is_err());
    }

    #[test]
    fn the_copy_stub_loader_asset_keeps_its_three_binding_templates_and_no_other_build_has_the_stub_one() {
        use copy_stub_template as t;
        assert_eq!((t::SETUP_SD_REDIRECT % 8, t::SETUP_SD_READ_ONLY % 8, t::SETUP_SD_COPY_STUB % 8), (0, 0, 0));
        let mut template = vec![0u8; 48 + 1024 + 32];
        template[..8].copy_from_slice(&1u64.to_le_bytes());
        assert_eq!(&t::IMAGE[t::SETUP_SD_COPY_STUB..t::SETUP_SD_COPY_STUB + template.len()], template);
        assert_eq!(COPY_STUB.copy_stub_binding, Some(t::SETUP_SD_COPY_STUB));
        assert_eq!(COPY_STUB.redirect_binding, Some(t::SETUP_SD_REDIRECT));
        assert_eq!(
            (CARD.copy_stub_binding, INTERNAL_FLASH.copy_stub_binding, READ_ONLY.copy_stub_binding, CHECKED_REDIRECT.copy_stub_binding),
            (None, None, None, None)
        );
        for other in [
            include_str!("../assets/sd-boot-v1-xp5-loader.generated.json"),
            include_str!("../assets/sd-boot-v1-xp5-internal-flash-loader.generated.json"),
            include_str!("../assets/sd-boot-v1-xp5-read-only-loader.generated.json"),
            include_str!("../assets/sd-boot-v1-xp5-checked-redirect-loader.generated.json"),
        ] {
            assert!(!other.contains("setup_sd_copy_stub"));
        }
        // The stub image is position independent, a multiple of 8 bytes, with its parameter block last.
        let stub = stub_template::IMAGE;
        assert_eq!((stub.len() % 8, stub_template::XP5_STUB_PARAMS + 176, stub_template::XP5_STUB_ENTRY), (0, stub.len(), 0));
        assert!(stub[stub_template::XP5_STUB_PARAMS..].iter().all(|b| *b == 0));
        // None of the stub's parameter offsets overlaps another.
        let mut offsets = vec![
            stub_template::XP5_STUB_P_SELF, stub_template::XP5_STUB_P_DST, stub_template::XP5_STUB_P_FLASH,
            stub_template::XP5_STUB_P_BYTES, stub_template::XP5_STUB_P_DRAM, stub_template::XP5_STUB_P_CRC,
            stub_template::XP5_STUB_P_COPY, stub_template::XP5_STUB_P_GREEN, stub_template::XP5_STUB_P_RED,
            stub_template::XP5_STUB_P_PIN, stub_template::XP5_STUB_P_PULSE, stub_template::XP5_STUB_P_GAP,
            stub_template::XP5_STUB_P_ENABLE, stub_template::XP5_STUB_P_ENABLE_GREEN,
            stub_template::XP5_STUB_P_ENABLE_RED, stub_template::XP5_STUB_P_MUX, stub_template::XP5_STUB_P_RETURN,
            stub_template::XP5_STUB_P_RESTORE,
            stub_template::XP5_STUB_OUT_STATUS, stub_template::XP5_STUB_OUT_SRC_CRC,
            stub_template::XP5_STUB_OUT_DST_CRC, stub_template::XP5_STUB_OUT_RESULT,
        ];
        offsets.sort_unstable();
        assert!(offsets.windows(2).all(|w| w[1] == w[0] + 8));
        assert_eq!(offsets[0], stub_template::XP5_STUB_PARAMS);
    }

    #[test]
    fn the_read_only_startup_is_its_own_startup() {
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
        recipe.xp5 = Some(s);
        let options = StartupOptions {
            read_only_destination: Some(0x3000_0000),
            ..StartupOptions::default()
        };
        let names = |files: &[File]| files.iter().map(|f| f.name.clone()).collect::<Vec<_>>();
        let everything = recipe.build_startup_with_options(&update, options).unwrap();
        assert_eq!(names(&everything), ["marble_init.bin", "ffboot.bin", "FPUPDATE.DAT"]);
        // `build_boot_files` is an allowlist: the update is dropped, nothing else is returned.
        let boot = recipe.build_boot_files(&update, options).unwrap();
        assert_eq!(names(&boot), ["marble_init.bin", "ffboot.bin"]);
        assert_eq!(boot[0].bytes, everything[0].bytes);
        assert_eq!(boot[1].bytes, everything[1].bytes);
        // It excludes the internal-flash build, the fallback and a storage image, takes only XP5
        // and refuses a destination the loader would refuse.
        for other in [
            StartupOptions { internal_flash: true, ..options },
            StartupOptions { fallback_wait_seconds: Some(1), ..options },
            StartupOptions { read_only_destination: Some(0x1000), ..options },
            StartupOptions { read_only_destination: Some(0x3000_0004), ..options },
            StartupOptions { read_only_destination: Some(READ_ONLY_CEILING), ..options },
        ] {
            assert!(recipe.build_boot_files(&update, other).is_err(), "{other:?}");
        }
        assert!(recipe.build_startup_with_storage(&update, &[0; 8], options).is_err());
        // Its RAM image is read and never run, so a RAM overlay is refused.
        let refused = recipe
            .build_startup_with_overlay(&update, options, &image::Overlay::default())
            .unwrap_err();
        assert!(refused.to_string().contains("takes no RAM overlay"), "{refused}");
        let mut other_generation = recipe.clone();
        other_generation.generation = crate::Generation::Xp4;
        assert!(other_generation.build_boot_files(&update, options).is_err());
        // The default options still build the card-resident startup, not the read-only one.
        assert_eq!(StartupOptions::default().read_only_destination, None);
    }
}
