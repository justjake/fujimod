//! Checked NAND removal from the rooted EXR loader's preloaded path.
use crate::image::{self, Edit, Result, require, word};
use crate::rom_sd::{address, call, edit, sequence};

/// Keep the empty translation-list initialization, but never scan the medium.
/// The neighboring cold-start calls identify initialization, the optional
/// installed-image probe, and shutdown. Runtime storage is a separate policy.
pub(super) fn edits(code: &[u8], table_call: usize) -> Result<Vec<Edit>> {
    let scan_call = table_call
        .checked_sub(0x2c)
        .ok_or_else(|| image::Error::Invalid("EXR table call has no cold-start prefix".into()))?;
    let setup = call(code, scan_call)?;
    require(
        sequence(code, setup, &[0xe92d4800, 0xe28db004, 0xe24dd008])
            && address(code, setup + 12, 3).is_ok()
            && address(code, setup + 20, 1).is_ok()
            && sequence(code, setup + 28, &[0xe5831000, 0xe3a03000, 0xe50b300c])
            && sequence(code, setup + 44, &[0xe50b2008, 0xe5813000, 0xe24b000c])
            && sequence(code, setup + 60, &[0xe24bd004, 0xe8bd8800]),
        "EXR empty bad-block list initialization differs",
    )?;
    let scan = call(code, setup + 56)?;
    require(
        sequence(
            code,
            scan,
            &[0xe92d4830, 0xe28db00c, 0xe24dd014, 0xe1a04000, 0xe1a05001],
        ) && call(code, scan + 20).is_ok()
            && sequence(code, scan + 24, &[0xe3500001, 0x13e00009])
            && sequence(
                code,
                scan + 0x50,
                &[0xe3a03000, 0xe50b3014, 0xe3a03f7d, 0xe50b3010],
            )
            && call(code, scan + 0x64).is_ok(),
        "EXR bad-block scanner differs",
    )?;
    let init_call = table_call - 0x58;
    let init = call(code, init_call)?;
    let shutdown_call = table_call + 0x70;
    let shutdown = call(code, shutdown_call)?;
    let probe_call = table_call + 0x18;
    let probe = call(code, probe_call)?;
    require(
        sequence(code, init, &[0xe92d4800, 0xe28db004])
            && call(code, init + 8)
                .is_ok_and(|target| word(code, target).is_ok_and(|w| w & 0xffff0000 == 0xe92d0000))
            && word(code, init + 12)? == 0xe8bd8800
            && sequence(code, shutdown, &[0xe92d4800, 0xe28db004])
            && call(code, shutdown + 8).is_ok()
            && sequence(code, shutdown + 12, &[0xe3a00000, 0xe8bd8800])
            && sequence(code, probe, &[0xe92d4810, 0xe28db008])
            && sequence(
                code,
                probe + 0x24,
                &[0xe3500000, 0x13a00001, 0x18bd8810, 0xe1d301be, 0xe30a3aaa],
            )
            && sequence(code, table_call + 0x20, &[0xe3500001]),
        "EXR NAND lifecycle or installed-image probe differs",
    )?;
    [
        (init_call, "skip NAND initialization on card startup"),
        (
            setup + 56,
            "retain empty bad-block list without NAND/OOB scan",
        ),
        (probe_call, "skip installed-image NAND header probe"),
        (shutdown_call, "skip unused NAND shutdown on card startup"),
    ]
    .into_iter()
    .map(|(at, what)| edit(code, at, 0xe3a00000, what))
    .collect()
}
