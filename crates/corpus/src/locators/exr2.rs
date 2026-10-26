//! Parallel NAND main-area reader, geometry and correction contract.
use super::flash::{Locator, Routine, number, one};
use crate::Lookup;
use serde_json::{Value, json};
use std::collections::BTreeMap;
type Result<T> = std::result::Result<T, String>;

pub(super) fn wide_constants(r: &Routine) -> Vec<u64> {
    r.code
        .windows(2)
        .filter_map(|p| {
            let (a, b) = (&p[0], &p[1]);
            let suffix = a.mnemonic.strip_prefix("movw")?;
            if b.mnemonic != format!("movt{suffix}") {
                return None;
            }
            let (reg, low) = a.operands.split_once(", #")?;
            let (other, high) = b.operands.split_once(", #")?;
            (reg == other).then(|| Some(number(low).ok()? | number(high).ok()? << 16))?
        })
        .collect()
}
fn stores(r: &Routine) -> BTreeMap<u64, u64> {
    let mut regs = BTreeMap::<String, u64>::new();
    let mut out = BTreeMap::new();
    for i in &r.code {
        let ops: Vec<_> = i.operands.split(", ").collect();
        if i.mnemonic == "bl" {
            for reg in ["r0", "r1", "r2", "r3", "ip"] {
                regs.remove(reg);
            }
            continue;
        }
        if i.mnemonic == "str" && ops.len() == 2 {
            if let (Some(value), Some(address)) = (
                regs.get(ops[0]),
                regs.get(ops[1].trim_start_matches('[').trim_end_matches(']')),
            ) {
                out.insert(*address, *value);
            }
        }
        let value = match i.mnemonic.as_str() {
            "mov" | "movw" if ops.len() == 2 => number(ops[1].trim_start_matches('#')).ok(),
            "movt" if ops.len() == 2 => regs.get(ops[0]).and_then(|low| {
                number(ops[1].trim_start_matches('#'))
                    .ok()
                    .map(|high| (low & 0xffff) | (high << 16))
            }),
            _ => None,
        };
        if let Some(reg) = ops.first() {
            if let Some(value) = value {
                regs.insert(reg.to_string(), value);
            } else if !matches!(i.mnemonic.as_str(), "str" | "strb" | "cmp" | "tst") {
                regs.remove(*reg);
            }
        }
    }
    out
}
fn calls(r: &Routine, target: u64) -> bool {
    r.code.iter().any(|i| {
        i.mnemonic == "bl" && number(i.operands.trim_start_matches('#')).ok() == Some(target)
    })
}
fn read_contract(r: &Routine, page: u32, pages: u32) -> bool {
    let common = r.has("tst", "r1, #0x1f")
        && r.has("movw", "r3, #0x8003")
        && r.has("movne", "r6, #0xa")
        && r.has("moveq", "r3, #0x70")
        && r.has("movne", "r3, #0x40")
        && r.has("orr", "r3, r3, #0x2080");
    let Some(timeout) = r
        .code
        .iter()
        .position(|i| i.mnemonic == "movw" && i.operands == "r3, #0x271")
    else {
        return false;
    };
    let count_writes: Vec<_> = r.code[..timeout]
        .iter()
        .filter(|i| {
            i.operands.starts_with("r8,") && !matches!(i.mnemonic.as_str(), "str" | "cmp" | "tst")
        })
        .collect();
    let count_is_pages = count_writes.len() == 1
        && count_writes[0].mnemonic == "mov"
        && count_writes[0].operands == "r8, r1";
    let count_is_quarter_pages = count_writes.len() == 2
        && count_writes[0].mnemonic == "mov"
        && count_writes[0].operands == "r8, r1"
        && ((count_writes[1].mnemonic == "lsl" && count_writes[1].operands == "r8, r8, #2")
            || (count_writes[1].mnemonic == "mov" && count_writes[1].operands == "r8, r8, lsl #2"));
    common
        && match (page, pages) {
            (512, 32) => {
                count_is_pages
                    && r.has("and", "sl, r2, #0x1f")
                    && r.has("rsb", "sl, sl, #0x20")
                    && r.has("lsl", "sb, sl, #9")
            }
            (2048, 64) => count_is_quarter_pages && r.has("mov", "r1, #0x30"),
            _ => false,
        }
}
fn read_commands(r: &Routine, command: u64, geometry: u64, page: u32) -> bool {
    let (mode, second) = match page {
        512 => ("r3, #4", "r1, r0"),
        2048 => ("r3, #1", "r1, #0x30"),
        _ => return false,
    };
    let geometry_call = r.code.windows(3).any(|w| {
        w[0].mnemonic == "mov"
            && w[0].operands == mode
            && w[1].mnemonic == "str"
            && w[1].operands == "r3, [r0, #-4]!"
            && w[2].mnemonic == "bl"
            && number(w[2].operands.trim_start_matches('#')).ok() == Some(geometry)
    });
    let command_call = r.code.windows(5).any(|w| {
        w[0].mnemonic == "mov"
            && w[0].operands == "r0, #0"
            && w[1].mnemonic == "mov"
            && w[1].operands == second
            && w[2].mnemonic == "mov"
            && w[2].operands == "r2, r0"
            && w[3].mnemonic == "uxtb"
            && w[3].operands.starts_with("r3,")
            && w[4].mnemonic == "bl"
            && number(w[4].operands.trim_start_matches('#')).ok() == Some(command)
    });
    geometry_call && command_call
}

fn instruction_address(r: &Routine, position: usize) -> Option<u64> {
    let mut remaining = position as u64 * 4;
    r.header.ranges.iter().find_map(|&(lo, hi)| {
        if remaining < hi - lo {
            Some(lo + remaining)
        } else {
            remaining -= hi - lo;
            None
        }
    })
}

fn free_handle_required(r: &Routine) -> bool {
    let optimized = r.code.windows(3).any(|w| {
        w[0].mnemonic == "ldr"
            && w[0].operands == "r3, [r2, r0, lsl #2]"
            && w[1].mnemonic == "cmp"
            && w[1].operands == "r3, #0"
            && w[2].mnemonic == "mvnne"
            && w[2].operands == "r0, #0x13"
    }) && r.has("str", "r3, [r2, r0, lsl #2]");
    let stack_arguments = r.has("str", "r0, [fp, #-0x10]") && r.has("str", "r2, [fp, #-0x18]");
    let unoptimized = stack_arguments
        && r.code.windows(7).any(|w| {
            if !w[..6]
                .iter()
                .zip([
                    ("ldr", "r3, [fp, #-0x10]"),
                    ("lsl", "r3, r3, #2"),
                    ("ldr", "r2, [fp, #-0x18]"),
                    ("add", "r3, r2, r3"),
                    ("ldr", "r3, [r3]"),
                    ("cmp", "r3, #0"),
                ])
                .all(|(i, (mnemonic, operands))| i.mnemonic == mnemonic && i.operands == operands)
                || w[6].mnemonic != "bne"
            {
                return false;
            }
            let target = number(w[6].operands.trim_start_matches('#')).ok();
            r.code.iter().enumerate().any(|(p, i)| {
                instruction_address(r, p) == target
                    && i.mnemonic == "mvn"
                    && i.operands == "r3, #0x13"
            })
        })
        && r.has("str", "r3, [r2]");
    optimized || unoptimized
}

fn literal_word<L: Lookup>(
    locator: &Locator<'_, L>,
    r: &Routine,
    position: usize,
) -> Result<(u64, u64)> {
    let offset = r.code[position]
        .operands
        .strip_prefix("r2, [pc, #")
        .and_then(|s| s.strip_suffix(']'))
        .ok_or("DMA handle argument is not a PC-relative literal")?;
    let address = instruction_address(r, position)
        .ok_or("DMA handle literal instruction is outside its ranges")?
        + 8;
    let address = if let Some(offset) = offset.strip_prefix('-') {
        address.checked_sub(number(offset)?)
    } else {
        address.checked_add(number(offset)?)
    }
    .ok_or("DMA handle literal address overflows")?;
    let bytes = locator
        .index
        .read(locator.firmware, &locator.unit, address, 4)
        .map_err(|e| e.to_string())?
        .ok_or("unmapped DMA handle literal")?;
    Ok((
        address,
        u64::from(u32::from_le_bytes(
            bytes.try_into().map_err(|_| "short DMA handle literal")?,
        )),
    ))
}

/// The aligned read path leases receive channel 1. Its allocator requires a
/// zero handle; the high-level block driver's lifecycle flag is not consulted.
fn receive_dma<L: Lookup>(
    locator: &Locator<'_, L>,
    reader: &Routine,
) -> Result<(Routine, Routine, u64, u64)> {
    let acquire = locator.named("nand_acquire_dma_channel", |r| {
        r.has("cmp", "r0, #1")
            && r.has("add", "r2, r5, #4")
            && r.has("ldr", "r2, [r3, r2, lsl #2]")
            && r.has("str", "r2, [r6]")
    })?;
    if !reader.code.windows(3).any(|w| {
        w[0].mnemonic == "mov"
            && w[0].operands == "r0, #1"
            && w[1].mnemonic == "sub"
            && w[1].operands.starts_with("r1, fp, #")
            && w[2].mnemonic == "bl"
            && number(w[2].operands.trim_start_matches('#')).ok()
                == Some(acquire.header.key.address)
    }) {
        return Err("NAND reader does not acquire receive DMA channel 1".into());
    }
    let bases = acquire
        .code
        .windows(4)
        .filter_map(|w| {
            if w[0].mnemonic != "movw"
                || w[1].mnemonic != "movt"
                || w[2].mnemonic != "add"
                || w[2].operands != "r2, r5, #4"
                || w[3].mnemonic != "ldr"
                || w[3].operands != "r2, [r3, r2, lsl #2]"
            {
                return None;
            }
            Some(
                number(w[0].operands.strip_prefix("r3, #")?).ok()?
                    | number(w[1].operands.strip_prefix("r3, #")?).ok()? << 16,
            )
        })
        .collect();
    let base = one(bases, "NAND DMA state base")?;
    let (position, target) = one(
        acquire
            .code
            .windows(3)
            .enumerate()
            .filter_map(|(p, w)| {
                (w[0].mnemonic == "mov"
                    && w[0].operands == "r1, #2"
                    && w[1].mnemonic == "ldr"
                    && w[1].operands.starts_with("r2, [pc, #")
                    && w[2].mnemonic == "bl")
                    .then(|| Some((p + 1, number(w[2].operands.trim_start_matches('#')).ok()?)))?
            })
            .collect(),
        "NAND DMA handle allocator",
    )?;
    let (literal_address, handle_array) = literal_word(locator, &acquire, position)?;
    if handle_array != base + 16 {
        return Err("NAND DMA allocation and returned handle arrays differ".into());
    }
    let allocator = locator.routine(target)?;
    if !free_handle_required(&allocator) {
        return Err("NAND DMA allocator does not require an unleased handle".into());
    }
    Ok((acquire, allocator, base + 20, literal_address))
}

/// Locate a corrected 512-byte or 2 KiB physical-page read ABI. The update origin is decoded
/// by the container reader. The transport uses an owned heap allocation.
/// Call under the located driver lock, with a zero-initialized progress word.
pub fn exr2_flash<L: Lookup>(
    index: &L,
    firmware: &str,
    image_origin: u64,
    control_offset: u64,
) -> Result<Value> {
    let locator = Locator {
        index,
        firmware,
        unit: "main".into(),
    };
    let initialized = locator.named("nand_driver_initialized", |r| {
        r.has("ldr", "r0, [r3]") && r.code.len() == 8
    })?;
    let flag = one(wide_constants(&initialized), "NAND initialized flag")?;
    let initializer = locator.named("nand_initialize_driver", |r| {
        stores(r).get(&flag) == Some(&1)
    })?;
    let assignments = stores(&initializer);
    let getter = locator.named("nand_get_page_size", |r| {
        r.has("ldrheq", "r0, [r3, #8]")
            && r.has("addeq", "r2, r2, r2, lsl #1")
            && r.has("addeq", "r3, r3, r2, lsl #3")
    })?;
    let constants = wide_constants(&getter);
    let mut geometry = vec![];
    for &address in &constants {
        let Some(&selected) = assignments.get(&address) else {
            continue;
        };
        if address == flag || selected > 3 {
            continue;
        }
        for &table in &constants {
            let Some(record) = index
                .read(firmware, "main", table + selected * 24, 24)
                .map_err(|e| e.to_string())?
            else {
                continue;
            };
            let blocks = u32::from_le_bytes(record[..4].try_into().unwrap());
            let page = u16::from_le_bytes(record[8..10].try_into().unwrap()) as u32;
            let pages = u32::from(record[10]);
            if (512..=8192).contains(&blocks) && matches!((page, pages), (512, 32) | (2048, 64)) {
                geometry.push((address, selected, table, blocks, page, pages));
            }
        }
    }
    let (geometry_address, selected, table, blocks, page, pages) =
        one(geometry, "selected NAND geometry")?;
    let reader = locator.named("nand_read_transfer", |r| read_contract(r, page, pages))?;
    let (dma, handle_allocator, receive_handle, handle_literal) = receive_dma(&locator, &reader)?;
    let transfer_geometry = locator.named("nand_set_geometry", |r| {
        r.has("ldr", "r2, [r0]") && r.has("str", "r2, [r3, #-0xfff]")
    })?;
    let gate = locator.named("nand_set_controller_gate", |r| {
        r.has("cmp", "r0, #0")
            && r.has("cmp", "r0, #1")
            && r.has("mov", "r0, #1")
            && r.has("mov", "r0, #0")
    })?;
    let correction = locator.named("nand_set_location_bank_enable", |r| {
        r.has("movne", "r2, #0x80000000") && r.has("str", "r2, [r3, #-0xf67]")
    })?;
    let poll = locator.named("nand_poll_controller_busy", |r| {
        r.has("and", "r3, r3, #4") && r.has("movw", "r4, #0x271")
    })?;
    let command = locator.named("nand_pack_command_bytes", |r| {
        r.has("bfi", "ip, r1, #0x10, #8") && r.has("str", "ip, [r3, #-0xfe3]")
    })?;
    if !read_commands(
        &reader,
        command.header.key.address,
        transfer_geometry.header.key.address,
        page,
    ) {
        return Err("NAND page geometry and READ0 command sequence differ".into());
    }
    let trigger = locator.named("nand_set_trigger", |r| r.has("str", "r2, [r3, #-0xff7]"))?;
    let timing = locator.named("nand_get_timing", |r| r.has("ldr", "r3, [r3, #-0xffb]"))?;
    let wrapper = locator.named("nand_read_pages_wrapper", |r| {
        calls(r, reader.header.key.address)
            && r.has("str", "ip, [sp, #0xc]")
            && r.has("str", "ip, [sp, #8]")
    })?;
    let physical = locator.named("nand_read_physical_blocks", |r| {
        calls(r, wrapper.header.key.address) && wide_constants(r).contains(&table)
    })?;
    let lock = locator.routine(
        physical
            .header
            .calls
            .first()
            .ok_or("physical NAND reader has no lock")?
            .1,
    )?;
    let unlock = locator.routine(
        physical
            .header
            .calls
            .last()
            .ok_or("physical NAND reader has no unlock")?
            .1,
    )?;
    let semaphore = one(
        lock.code
            .iter()
            .filter_map(|i| {
                (i.mnemonic == "mov" && i.operands.starts_with("r0, #")).then(|| i.operands.clone())
            })
            .collect(),
        "NAND lock semaphore",
    )?;
    if lock.code.len() != 6
        || !lock.has("mvn", "r1, #0")
        || unlock.code.len() != 5
        || !unlock.has("mov", &semaphore)
        || !calls(&physical, lock.header.key.address)
        || !calls(&physical, unlock.header.key.address)
    {
        return Err("physical NAND reader lock/unlock contract differs".into());
    }
    for routine in [&gate, &poll, &command, &trigger, &timing] {
        if !calls(&reader, routine.header.key.address) {
            return Err("NAND read family does not call the verified controller helpers".into());
        }
    }
    if !calls(&gate, correction.header.key.address) || !calls(&poll, timing.header.key.address) {
        return Err("NAND correction gate or polling call graph differs".into());
    }
    let mut ranges = vec![reader.span("nand_read_transfer")];
    for (r, name) in [
        (&gate, "NAND correction mode"),
        (&correction, "NAND correction gate"),
        (&poll, "NAND completion polling"),
        (&command, "NAND read command bytes"),
        (&trigger, "NAND read trigger"),
        (&timing, "NAND corrected status"),
        (&lock, "NAND driver lock"),
        (&unlock, "NAND driver unlock"),
        (&initializer, "NAND geometry initialization"),
        (&getter, "NAND page geometry query"),
        (&dma, "NAND receive DMA acquisition"),
        (&handle_allocator, "NAND DMA handle allocation"),
    ] {
        ranges.push(r.span(name));
    }
    if page == 512 {
        ranges.push(transfer_geometry.span("NAND page transfer geometry"));
    }
    ranges
        .push(json!({"name":"NAND DMA handle table reference","address":handle_literal,"size":4}));
    ranges.push(
        json!({"name":"selected NAND geometry record","address":table+selected*24,"size":24}),
    );
    Ok(json!({
        "device":{"kind":"raw-nand"},
        "window":{"address":0,"size":u64::from(blocks)*u64::from(page)*u64::from(pages)},
        "update_image":{"dat_offset":image_origin,"xor":255},
        "reader":{"symbol":"nand_read_transfer","alignment":page,"nand":{"page_bytes":page,"pages_per_block":pages,"destination_alignment":32,"controller_mode":0,"event_mode":0,"timeout_ms":2000,"max_pages_per_call":1,"lock":lock.header.key.address,"unlock":unlock.header.key.address}},
        "transport":"exr2-query-heap","code_ranges":ranges,
        "state_checks":[{"name":"NAND receive DMA channel available","address":receive_handle,"size":4,"value":0},{"name":"selected NAND geometry","address":geometry_address,"size":4,"value":selected}],
        "control":{"offset":control_offset,"size":32768}
    }))
}

/// Choose an update-backed control from the raw reset-vector allocation. The
/// first-stage header grammar and an aligned complete ARM vector table must
/// agree; this does not depend on the function index containing reset code.
pub fn exr2_control(image: &[u8]) -> Result<u64> {
    let signature = [0x00ffaa55u32, 0x01234567, 0x89abcdef];
    if !signature
        .iter()
        .enumerate()
        .all(|(i, word)| image.get(i * 4..i * 4 + 4) == Some(word.to_le_bytes().as_slice()))
    {
        return Err("EXR II NAND first-stage header differs".into());
    }
    let vectors: Vec<u8> = (0..8).flat_map(|_| 0xe59ff018u32.to_le_bytes()).collect();
    let prefix = &image[..image.len().min(0x100000)];
    let candidates: Vec<u64> = prefix
        .windows(vectors.len())
        .enumerate()
        .filter_map(|(p, bytes)| {
            if bytes != vectors || !p.is_multiple_of(0x4000) || image.get(p..p + 32768).is_none() {
                return None;
            }
            Some(p as u64)
        })
        .collect();
    candidates
        .into_iter()
        .min()
        .ok_or_else(|| "no update-backed NAND reset-vector control".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn synthetic_reader() -> Routine {
        let instructions = [
            ("mov", "r8, r1"),
            ("movw", "r3, #0x271"),
            ("tst", "r1, #0x1f"),
            ("movw", "r3, #0x8003"),
            ("movne", "r6, #0xa"),
            ("moveq", "r3, #0x70"),
            ("movne", "r3, #0x40"),
            ("orr", "r3, r3, #0x2080"),
            ("and", "sl, r2, #0x1f"),
            ("rsb", "sl, sl, #0x20"),
            ("lsl", "sb, sl, #9"),
            ("mov", "r3, #4"),
            ("str", "r3, [r0, #-4]!"),
            ("bl", "#0x200"),
            ("mov", "r0, #0"),
            ("mov", "r1, r0"),
            ("mov", "r2, r0"),
            ("uxtb", "r3, sl"),
            ("bl", "#0x300"),
        ];
        synthetic_routine(&instructions)
    }
    fn synthetic_routine(instructions: &[(&str, &str)]) -> Routine {
        Routine {
            header: crate::FunctionHeader {
                key: crate::FunctionKey {
                    firmware: "sample".into(),
                    unit: "main".into(),
                    address: 0x100,
                },
                architecture: crate::Architecture::Arm32,
                names: vec![],
                annotations: vec![],
                ranges: vec![(0x100, 0x100 + instructions.len() as u64 * 4)],
                signature: [0; 32],
                instructions: instructions.len(),
                calls: vec![],
            },
            code: instructions
                .iter()
                .copied()
                .map(|(mnemonic, operands)| super::super::flash::Instruction {
                    mnemonic: mnemonic.into(),
                    operands: operands.into(),
                })
                .collect(),
        }
    }
    #[test]
    fn dma_precondition_requires_rejection_of_a_leased_handle() {
        let optimized = [
            ("ldr", "r3, [r2, r0, lsl #2]"),
            ("cmp", "r3, #0"),
            ("mvnne", "r0, #0x13"),
            ("str", "r3, [r2, r0, lsl #2]"),
        ];
        let mut r = synthetic_routine(&optimized);
        assert!(free_handle_required(&r));
        r.code[2].mnemonic = "mvneq".into();
        assert!(!free_handle_required(&r));
        let unoptimized = [
            ("str", "r0, [fp, #-0x10]"),
            ("str", "r2, [fp, #-0x18]"),
            ("ldr", "r3, [fp, #-0x10]"),
            ("lsl", "r3, r3, #2"),
            ("ldr", "r2, [fp, #-0x18]"),
            ("add", "r3, r2, r3"),
            ("ldr", "r3, [r3]"),
            ("cmp", "r3, #0"),
            ("bne", "#0x128"),
            ("str", "r3, [r2]"),
            ("mvn", "r3, #0x13"),
        ];
        let mut r = synthetic_routine(&unoptimized);
        assert!(free_handle_required(&r));
        r.code[8].operands = "#0x124".into();
        assert!(!free_handle_required(&r));
    }
    #[test]
    fn small_page_reader_rejects_scaled_counts_and_missing_correction() {
        let mut reader = synthetic_reader();
        assert!(read_contract(&reader, 512, 32));
        assert!(!read_contract(&reader, 512, 64));
        assert!(!read_contract(&reader, 2048, 64));
        reader.code.insert(
            1,
            super::super::flash::Instruction {
                mnemonic: "lsl".into(),
                operands: "r8, r8, #2".into(),
            },
        );
        assert!(!read_contract(&reader, 512, 32));
        reader.code.remove(1);
        reader
            .code
            .retain(|i| i.mnemonic != "movne" || i.operands != "r6, #0xa");
        assert!(!read_contract(&reader, 512, 32));
    }
    #[test]
    fn small_page_commands_require_the_matching_geometry_and_helpers() {
        let mut reader = synthetic_reader();
        assert!(read_commands(&reader, 0x300, 0x200, 512));
        assert!(!read_commands(&reader, 0x304, 0x200, 512));
        assert!(!read_commands(&reader, 0x300, 0x204, 512));
        assert!(!read_commands(&reader, 0x300, 0x200, 2048));
        let geometry = reader
            .code
            .iter_mut()
            .find(|i| i.operands == "r3, #4")
            .unwrap();
        geometry.operands = "r3, #1".into();
        assert!(!read_commands(&reader, 0x300, 0x200, 512));
        let geometry = reader
            .code
            .iter_mut()
            .find(|i| i.operands == "r3, #1")
            .unwrap();
        geometry.operands = "r3, #4".into();
        let command = reader
            .code
            .iter_mut()
            .find(|i| i.operands == "r1, r0")
            .unwrap();
        command.operands = "r1, #0x30".into();
        assert!(!read_commands(&reader, 0x300, 0x200, 512));
    }
    #[test]
    fn control_uses_the_first_aligned_complete_reset_allocation() {
        let mut image = vec![0; 0x18000];
        for (p, word) in [0x00ffaa55u32, 0x01234567, 0x89abcdef].iter().enumerate() {
            image[p * 4..p * 4 + 4].copy_from_slice(&word.to_le_bytes());
        }
        for start in [0x4000, 0x8000] {
            for p in 0..8 {
                image[start + p * 4..start + p * 4 + 4]
                    .copy_from_slice(&0xe59ff018u32.to_le_bytes());
            }
        }
        assert_eq!(exr2_control(&image).unwrap(), 0x4000);
        image[0] ^= 1;
        assert!(exr2_control(&image).is_err());
        image[0] ^= 1;
        assert!(exr2_control(&image[..0x9000]).is_err());
    }
}
