//! Raw XP5 entry, serial NOR preparation, and optional post-DDR FAT32 loading.
use crate::image::{self, File, Result, Source, extent, put, require, word};
use serde::{Deserialize, Serialize};
mod banks;
pub use banks::Banks;
mod card_boot;
pub use card_boot::CardBoot;
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
    boot_ready: usize,
    boot_zero_gate: usize,
    externals: &'static [(usize, u32, &'static str)],
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
    success_hold_seconds: u32,
) -> Result<Vec<File>> {
    package_with_storage(inverted, setup, ram, success_hold_seconds, None)
}

pub(crate) fn package_with_storage(
    inverted: &[u8], setup: &Setup, ram: Option<Vec<u8>>, success_hold_seconds: u32,
    storage: Option<&[u8]>,
) -> Result<Vec<File>> {
    package_inner(inverted, setup, ram, success_hold_seconds, storage, None)
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
    inverted: &[u8], setup: &Setup, ram: Vec<u8>, success_hold_seconds: u32,
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
        success_hold_seconds,
        None,
        Some(InternalFlash {
            fallback_wait_seconds,
            ram_edits,
            secondary_sgi,
        }),
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
) -> Result<()> {
    let at = flash_template::SETUP_SD_INTERNAL_FLASH;
    let empty = [1u64, 0, 0, 0, 0, 0, 0, 0].map(u64::to_le_bytes).concat();
    require(
        extent(program, at, 64)? == empty,
        "XP5 internal-flash binding template changed",
    )?;
    require(
        loader.image.len() == flash_template::IMAGE.len()
            && flash.secondary_sgi.iter().enumerate().all(|(n, &address)| {
                (0xf0000000..0xffff0000).contains(&address)
                    && address.is_multiple_of(0x10000)
                    && !flash.secondary_sgi[..n].contains(&address)
            }),
        "Invalid parked-core interrupt frames",
    )?;
    let (mut fallback, mut wait, mut source, mut bytes) = (0u64, 0u64, 0u64, 0u64);
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
    }
    for (n, value) in [1, fallback, wait, source, bytes]
        .into_iter()
        .chain(flash.secondary_sgi)
        .enumerate()
    {
        put64(program, at + n * 8, value)?;
    }
    Ok(())
}

fn package_inner(
    inverted: &[u8], setup: &Setup, ram: Option<Vec<u8>>, success_hold_seconds: u32,
    storage: Option<&[u8]>, internal_flash: Option<InternalFlash>,
) -> Result<Vec<File>> {
    let prefix = setup.verify(inverted)?;
    let loader = if internal_flash.is_some() { &INTERNAL_FLASH } else { &CARD };
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
        // Each unit executes 1,000,000 four-NOP iterations. Even allowing
        // two billion iterations per second, 2,048 units take >= 1.024 s.
        u64::from(success_hold_seconds) * 2_048,
    )?;
    put64(
        &mut program,
        cfg + 200,
        u64::from(ram.is_none() || internal_flash.is_some()),
    )?;
    if let Some(flash) = &internal_flash {
        bind_internal_flash(&mut program, loader, setup, flash, ram.as_deref())?;
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
    fn source(bytes: &[u8]) -> Source {
        Source {
            dat_offset: 0,
            size: bytes.len(),
            compression: image::Compression::Raw,
            decoded_size: bytes.len(),
            sha256: fujimod_firmware::sha256(bytes),
            initialized_prefix: None,
        }
    }
    fn fixture() -> (Vec<u8>, Setup, Vec<u8>) {
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
            flash_template::SETUP_SD_INTERNAL_FLASH..flash_template::SETUP_SD_INTERNAL_FLASH + 64,
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
            let expected = match wait {
                None => [0, 0, 0, 0],
                Some(wait) => [1, u64::from(wait), restore, s.reset_copy_size as u64],
            };
            assert_eq!((1..5).map(|n| field(raw, n)).collect::<Vec<_>>(), expected);
            assert_eq!(
                (5..8).map(|n| field(raw, n)).collect::<Vec<_>>(),
                [0xfa0b0000, 0xfa0d0000, 0xfa0f0000]
            );
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
                ..flash_template::SETUP_SD_INTERNAL_FLASH + 64],
            [1u64, 0, 0, 0, 0, 0, 0, 0].map(u64::to_le_bytes).concat()
        );
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
            for seconds in [1, 3, u32::MAX] {
                let held = package(&data, &s, payload.clone(), seconds).unwrap();
                let mut expected = normal[0].bytes.clone();
                put64(&mut expected, at, u64::from(seconds) * 2_048).unwrap();
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
}
