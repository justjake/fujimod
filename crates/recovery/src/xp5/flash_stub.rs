//! Internal-flash XP5 startup stub: the secondary-core release and the load-failure fallback.
//!
//! The compiled loader does two things an internal-flash startup cannot live with:
//!
//! * After the RAM image is complete it never wakes the cores the boot ROM parks in WFI (it does so
//!   only for a card-resident setup). The RAM image's reset stage with its preloaded selector
//!   expects them awake and waiting for its own SGI0, so without a wake the stage deadlocks. The
//!   stub sends that SGI0 where the loader publishes the image.
//! * On any failed load it halts (`fail()`: record the error, `wfi` forever). The installed flash
//!   still holds the stock firmware, so a stub can wait, put the stock reset image back if FFBOOT
//!   was already streaming over it, and let the stock bootstrap continue in flash.
//!
//! Both are one small hand-written stub (`assets/sd-boot-v1-xp5-flash-stub.S`) appended to the
//! loader and bound by redirecting a few loader words. The fallback unwinds `setup_sd_initialize`
//! by its compiled frame layout, so every word either binding depends on is checked against the
//! loader image first: a loader built differently refuses here instead of being patched blindly.
use super::{Setup, put64, template};
use crate::image::{Result, extent, put, require, word};

include!(concat!(env!("OUT_DIR"), "/xp5_flash_stub.rs"));

/// Setup-timer units per second: the loader's delay is `boot_setup_delay(milliseconds * 1000)`.
const UNITS_PER_SECOND: u64 = 1_000_000;
/// Bytes of parameters at `STUB_PARAMS`.
const PARAMETERS: usize = 0x48;

fn is_branch(instruction: u32, link: bool) -> bool {
    instruction & 0xfc00_0000 == if link { 0x9400_0000 } else { 0x1400_0000 }
}

fn branch_target(site: usize, instruction: u32) -> i64 {
    let mut immediate = i64::from(instruction & 0x03ff_ffff);
    if immediate & 0x0200_0000 != 0 {
        immediate -= 1 << 26;
    }
    site as i64 + immediate * 4
}

/// The compiled `fail()`: `adrp x1, progress; add x1, x1, #lo; sxtw x0, w0; str x0, [x1, #0x10];
/// dsb sy; wfi; wfi; b .-8`. Returns its offset in the loader image.
pub(super) fn locate_fail(loader: &[u8], address: u64) -> Result<usize> {
    let mut found = None;
    for site in (0..loader.len().saturating_sub(32)).step_by(4) {
        let w = |n: usize| word(loader, site + 4 * n);
        let (adrp, add) = (w(0)?, w(1)?);
        if adrp & 0x9f00_001f != 0x9000_0001
            || add & 0xffc0_03ff != 0x9100_0021
            || w(2)? != 0x9340_7c00
            || w(3)? != 0xf900_0820
            || w(4)? != 0xd503_3f9f
            || w(5)? != 0xd503_207f
            || w(6)? != 0xd503_207f
            || w(7)? != 0x17ff_fffe
        {
            continue;
        }
        // It records the error in the loader's own progress block.
        let pc = address + site as u64;
        let mut page = i64::from(((adrp >> 5) & 0x7_ffff) << 2 | ((adrp >> 29) & 3));
        if page & 0x10_0000 != 0 {
            page -= 1 << 21;
        }
        let target = ((pc & !0xfff) as i64 + (page << 12)) as u64 + u64::from((add >> 10) & 0xfff);
        require(
            target == address + template::SETUP_SD_PROGRESS as u64,
            "XP5 loader failure routine does not record in the progress block",
        )?;
        require(
            found.replace(site).is_none(),
            "XP5 loader has more than one failure routine",
        )?;
    }
    found.ok_or_else(|| {
        crate::image::Error::Invalid("XP5 loader failure routine not found".into())
    })
}

/// The `sev` after the publication of `a64_boot_ready = 1` on the path without a card-resident
/// setup: `stlr w1, [x0]; ldr x0, [x21, #8]; cbnz x0, <card-resident release>; sev; ldp x19, x20`.
/// Returns its offset in the loader image.
pub(super) fn locate_release(loader: &[u8]) -> Result<usize> {
    let (start, end) = (template::SETUP_SD_INITIALIZE, template::SETUP_SD_PHY_IMEM);
    require(start < end && end <= loader.len(), "XP5 loader function bounds differ")?;
    let mut found = None;
    for site in (start + 12..end).step_by(4) {
        if word(loader, site)? == 0xd503_209f
            && word(loader, site - 12)? == 0x889f_fc01
            && word(loader, site - 8)? == 0xf940_06a0
            && word(loader, site - 4)? & 0xff00_001f == 0xb500_0000
            && word(loader, site + 4)? == 0xa941_53f3
        {
            require(
                found.replace(site).is_none(),
                "XP5 loader publishes its readiness more than once",
            )?;
        }
    }
    found.ok_or_else(|| {
        crate::image::Error::Invalid("XP5 loader readiness publication not found".into())
    })
}

/// Check the assumptions the fallback makes about the compiled loader and return the offset of
/// `fail()`.
pub(super) fn verify_loader(loader: &[u8], address: u64) -> Result<usize> {
    let start = template::SETUP_SD_INITIALIZE;
    let end = template::SETUP_SD_PHY_IMEM;
    require(start < end && end <= loader.len(), "XP5 loader function bounds differ")?;
    // `setup_sd_initialize` keeps a 0x60-byte frame addressed by x29 and saves x19..x26 in it.
    for (offset, expected, what) in [
        (start, 0xa9ba_7bfd, "stp x29, x30, [sp, #-0x60]!"),
        (start + 0xc, 0x9100_03fd, "mov x29, sp"),
        (start + 0x10, 0xa901_53f3, "stp x19, x20, [sp, #0x10]"),
    ] {
        require(
            word(loader, offset)? == expected,
            &format!("XP5 loader prologue differs ({what})"),
        )?;
    }
    // x19..x26 are in the frame before any call of fail(): the saves come first in the function,
    // and the compiler repeats the late ones in the failure arms that skip them.
    let saves = [
        (0xa902_5bf5, "stp x21, x22, [sp, #0x20]"),
        (0xa903_63f7, "stp x23, x24, [sp, #0x30]"),
        (0xa904_6bf9, "stp x25, x26, [sp, #0x40]"),
    ];
    // The stack pointer stays at the frame: no add/sub-immediate writes SP in the function.
    for at in (start..end).step_by(4) {
        let w = word(loader, at)?;
        require(
            !(w & 0x1f00_0000 == 0x1100_0000 && w & 0x1f == 31 && w & 0x2000_0000 == 0),
            "XP5 loader moves the stack pointer inside its frame",
        )?;
    }
    let fail = locate_fail(loader, address)?;
    // Only `setup_sd_initialize` calls `fail()`, so x29 is always its frame there, and each call
    // follows the saves of x21..x26 in the function.
    for at in (0..loader.len()).step_by(4) {
        let w = word(loader, at)?;
        for link in [false, true] {
            if is_branch(w, link) && branch_target(at, w) == fail as i64 {
                require(
                    (start..end).contains(&at),
                    "XP5 loader failure routine is called outside setup_sd_initialize",
                )?;
                for (save, what) in saves {
                    require(
                        (start..at)
                            .step_by(4)
                            .any(|earlier| word(loader, earlier).is_ok_and(|w| w == save)),
                        &format!("XP5 loader frame differs ({what} after a failure call)"),
                    )?;
                }
            }
        }
    }
    Ok(fail)
}

/// Bind the stub at `at` in `program` (the loader image starts at `program[0]`, run at
/// `address`): its parameters filled and the readiness `sev` redirected into it. With
/// `fallback_wait_seconds`, `fail()` is redirected too. The bootstrap's gate stays the loader's
/// own: it releases for the fallback's `ready = 2` as for `1`, and the stock continuation it
/// replaces returns to RAM 0 like the gate does.
pub(super) fn install(
    program: &mut [u8],
    address: u64,
    at: usize,
    setup: &Setup,
    fallback_wait_seconds: Option<u32>,
) -> Result<()> {
    let loader = &program[..template::IMAGE.len()];
    let release = locate_release(loader)?;
    let fail = fallback_wait_seconds
        .map(|_| verify_loader(loader, address))
        .transpose()?;
    let params = at + STUB_PARAMS;
    let mut pristine = IMAGE.to_vec();
    pristine[STUB_PARAMS..STUB_PARAMS + PARAMETERS].fill(0);
    require(
        at >= template::IMAGE.len()
            && STUB_PARAMS + PARAMETERS <= IMAGE.len()
            && extent(program, at, IMAGE.len())? == pristine,
        "XP5 flash stub is not in place",
    )?;
    let jump = |from: usize, entry: usize| {
        super::branch(address + from as u64, address + (at + entry) as u64, false)
    };
    for (offset, value) in [
        (0x08, address + template::A64_BOOT_READY as u64),
        (0x40, address + release as u64 + 4),
    ] {
        put64(program, params + offset, value)?;
    }
    put(program, release, jump(release, RELEASE_ENTRY)?)?;
    if let (Some(fail), Some(wait_seconds)) = (fail, fallback_wait_seconds) {
        let restore_offset = setup
            .reset_dat
            .checked_sub(setup.prefix.dat_offset)
            .ok_or_else(|| {
                crate::image::Error::Invalid("XP5 reset image precedes the update".into())
            })?;
        let window = u64::from(!setup.flash.window_mask) + 1;
        require(
            setup.reset_copy_size > 0
                && setup.reset_copy_size % 8 == 0
                && restore_offset % 8 == 0
                && (restore_offset as u64)
                    .checked_add(setup.reset_copy_size as u64)
                    .is_some_and(|end| end <= window),
            "XP5 reset image is not a flash-aperture extent",
        )?;
        for (offset, value) in [
            (0x00, address + template::SETUP_SD_PROGRESS as u64),
            (0x10, setup.delay.address as u64),
            (0x20, setup.flash.aperture + restore_offset as u64),
            (0x28, setup.reset_copy_size as u64),
            (0x30, u64::from(wait_seconds)),
            (0x38, UNITS_PER_SECOND),
        ] {
            put64(program, params + offset, value)?;
        }
        put(program, fail, jump(fail, FAIL_ENTRY)?)?;
    }
    Ok(())
}
