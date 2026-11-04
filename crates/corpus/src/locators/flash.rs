use crate::{Architecture, FunctionHeader, Lookup};
use capstone::prelude::*;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

type Result<T> = std::result::Result<T, String>;
#[derive(Clone, Debug)]
pub(super) struct Instruction {
    pub(super) mnemonic: String,
    pub(super) operands: String,
}
pub(super) struct Routine {
    pub(super) header: FunctionHeader,
    pub(super) code: Vec<Instruction>,
}
impl Routine {
    pub(super) fn has(&self, mnemonic: &str, operands: &str) -> bool {
        self.code
            .iter()
            .any(|i| i.mnemonic == mnemonic && i.operands == operands)
    }
    pub(super) fn span(&self, name: &str) -> Value {
        let start = self.header.key.address;
        let end = self
            .header
            .ranges
            .iter()
            .map(|(_, end)| *end)
            .max()
            .unwrap();
        json!({"name": name, "address": start, "size": end-start})
    }
}
pub(super) struct Locator<'a, L> {
    pub(super) index: &'a L,
    pub(super) firmware: &'a str,
    pub(super) unit: String,
}
impl<L: Lookup> Locator<'_, L> {
    pub(super) fn routine(&self, address: u64) -> Result<Routine> {
        let entries = self
            .index
            .select(Some(self.firmware), Some(&self.unit), Some(address))
            .map_err(|e| e.to_string())?;
        if entries.len() != 1 {
            return Err(format!("{address:#x}: expected one indexed routine"));
        }
        let header = entries[0].function.as_ref().clone();
        let start = header.key.address;
        let end = header
            .ranges
            .iter()
            .map(|(_, end)| *end)
            .max()
            .ok_or("empty flash routine")?;
        if header.ranges.iter().any(|&(lo, hi)| lo < start || hi <= lo)
            || start != address
            || end - start > 0x8000
        {
            return Err("invalid flash routine extent".into());
        }
        let decoder = match header.architecture {
            Architecture::Aarch64 => Capstone::new()
                .arm64()
                .mode(arch::arm64::ArchMode::Arm)
                .build(),
            Architecture::Arm32 => Capstone::new().arm().mode(arch::arm::ArchMode::Arm).build(),
        }
        .map_err(|e| e.to_string())?;
        let mut code = vec![];
        for &(lo, hi) in &header.ranges {
            let bytes = self
                .index
                .read(self.firmware, &self.unit, lo, (hi - lo) as usize)
                .map_err(|e| e.to_string())?
                .ok_or("unmapped flash routine")?;
            let ins = decoder.disasm_all(&bytes, lo).map_err(|e| e.to_string())?;
            if ins.len() * 4 != bytes.len() {
                return Err("incomplete flash instruction decoding".into());
            }
            code.extend(ins.iter().map(|i| Instruction {
                mnemonic: i.mnemonic().unwrap_or("").into(),
                operands: i.op_str().unwrap_or("").into(),
            }));
        }
        Ok(Routine { header, code })
    }
    fn calls(&self, routine: &Routine) -> Result<Vec<Routine>> {
        let mut addresses = BTreeSet::new();
        for i in &routine.code {
            if i.mnemonic == "bl" {
                addresses.insert(number(i.operands.trim_start_matches('#'))?);
            }
        }
        addresses.into_iter().map(|a| self.routine(a)).collect()
    }
    pub(super) fn named(&self, name: &str, verify: impl Fn(&Routine) -> bool) -> Result<Routine> {
        let candidates = self
            .index
            .find(Some(name), None, Some(self.firmware), Some(&self.unit))
            .map_err(|e| e.to_string())?;
        let routines = candidates
            .iter()
            .filter_map(|f| self.routine(f.key.address).ok())
            .filter(verify)
            .collect();
        one(routines, name)
    }
}
pub(super) fn number(text: &str) -> Result<u64> {
    if let Some(hex) = text.strip_prefix("0x") {
        u64::from_str_radix(hex, 16)
    } else {
        text.parse()
    }
    .map_err(|e| format!("invalid immediate {text}: {e}"))
}
pub(super) fn one<T>(mut values: Vec<T>, name: &str) -> Result<T> {
    if values.len() != 1 {
        return Err(format!("{name}: {} verified candidates", values.len()));
    }
    Ok(values.remove(0))
}
/// Track straight-line ADRP/MOV/ADD values and absolute word loads. Values are
/// invalidated at branches and ABI calls; no runtime pointer is dereferenced.
fn canonical_register(register: &str) -> String {
    register
        .strip_prefix('w')
        .map_or_else(|| register.into(), |suffix| format!("x{suffix}"))
}
fn loads(routine: &Routine) -> Vec<(usize, u64)> {
    let mut regs = BTreeMap::<String, u64>::new();
    let mut found = vec![];
    for (p, i) in routine.code.iter().enumerate() {
        let ops: Vec<_> = i.operands.split(", ").collect();
        if matches!(i.mnemonic.as_str(), "b" | "ret" | "br") || i.mnemonic.starts_with("b.") {
            regs.clear();
        }
        if i.mnemonic == "bl" {
            regs.retain(|k, _| {
                k.trim_start_matches(['w', 'x'])
                    .parse::<u8>()
                    .is_ok_and(|n| n >= 19)
            });
            continue;
        }
        if i.mnemonic == "ldr" && ops.len() >= 2 && ops[0].starts_with('w') {
            let base = ops[1].trim_start_matches('[').trim_end_matches(']');
            let offset = ops
                .get(2)
                .and_then(|s| number(s.trim_start_matches('#').trim_end_matches(']')).ok())
                .unwrap_or(0);
            if let Some(value) = regs.get(&canonical_register(base)) {
                found.push((p, value + offset));
            }
        }
        let value = match i.mnemonic.as_str() {
            "adrp" | "mov" if ops.len() == 2 => number(ops[1].trim_start_matches('#')).ok(),
            "add" if ops.len() == 3 => regs
                .get(&canonical_register(ops[1]))
                .and_then(|v| number(ops[2].trim_start_matches('#')).ok().map(|n| v + n)),
            _ => None,
        };
        if let Some(reg) = ops.first() {
            if let Some(value) = value {
                regs.insert(canonical_register(reg), value);
            } else if !matches!(
                i.mnemonic.as_str(),
                "cmp" | "ccmp" | "str" | "strb" | "stp" | "tst" | "cbz" | "cbnz"
            ) {
                regs.remove(&canonical_register(reg));
            }
        }
    }
    found
}

/// Locate a stock flash reader for one processor family. `jedec_id` is the
/// installed hardware identity, verified against the driver's supported-device
/// table. It is not selected arbitrarily from the list of supported NOR chips.
/// `image_origin` and `control_offset` come from the decoded update packaging.
pub fn flash<L: Lookup>(
    index: &L,
    firmware: &str,
    processor: &str,
    jedec_id: Option<u32>,
    image_origin: u64,
    control_offset: u64,
) -> Result<Value> {
    if processor == "xp4" {
        return xp4_flash(index, firmware, image_origin);
    }
    if processor == "xpp" {
        return xpp_flash(index, firmware, image_origin);
    }
    if processor != "xp5" {
        return Err(format!(
            "{processor}: flash reader contract is not implemented"
        ));
    }
    let locator = Locator {
        index,
        firmware,
        unit: "main".into(),
    };
    let reader = locator.named("flash_read", |r| {
        [
            ("str", "x0, [sp, #0x10]"),
            ("str", "x1, [sp, #0x18]"),
            ("str", "w2, [sp, #0x20]"),
            ("str", "w3, [sp, #0x24]"),
            ("ret", ""),
        ]
        .into_iter()
        .all(|(m, o)| r.has(m, o))
    })?;
    let request = one(locator.calls(&reader)?, "read request")?;
    locator.named("flash_window_address", |r| {
        r.code.len() == 3
            && r.has("mov", "x1, #0xf0000000")
            && r.has("add", "x0, x0, x1")
            && r.has("ret", "")
    })?;
    if !request.has("mov", "x0, #0xf8000000")
        || !request.has("mov", "w0, #0x7ffffff")
        || !request.has("movk", "w0, #0x7766, lsl #16")
        || !request.has("mov", "w0, #0x6555")
    {
        return Err("read request lacks bounded flash window or initialized marker".into());
    }
    let marker = one(
        loads(&request)
            .into_iter()
            .filter(|(p, _)| {
                request
                    .code
                    .get(p + 1)
                    .is_some_and(|i| i.mnemonic == "mov" && i.operands == "w0, #0x6555")
            })
            .map(|(_, a)| a)
            .collect(),
        "initialized marker",
    )?;
    let dispatcher = one(
        locator
            .calls(&request)?
            .into_iter()
            .filter(|r| r.code.iter().any(|i| i.operands.ends_with(", #0x3f")))
            .collect(),
        "read dispatcher",
    )?;
    let dma = one(
        locator
            .calls(&dispatcher)?
            .into_iter()
            .filter(|r| r.has("mov", "w0, #0x11") && r.has("str", "w0, [sp, #0x78]"))
            .collect(),
        "DMA reader",
    )?;
    let cache = one(
        locator
            .calls(&dma)?
            .into_iter()
            .filter(|r| r.code.len() == 16 && r.has("orr", "x0, x0, x1") && r.has("mov", "w0, #0"))
            .collect(),
        "destination cache maintenance",
    )?;
    let nand = dispatcher.has("tst", "x0, #0x3f");
    let mut ranges = vec![
        reader.span("flash_read"),
        request.span("flash read request"),
        dispatcher.span("flash read dispatcher"),
        dma.span("flash DMA read"),
    ];
    let mut checks = vec![];
    let device = if nand {
        let mode = one(
            loads(&dispatcher)
                .into_iter()
                .filter(|(p, _)| {
                    dispatcher
                        .code
                        .get(p + 1)
                        .is_some_and(|i| i.mnemonic == "cbnz")
                })
                .map(|(_, a)| a)
                .collect(),
            "NAND mode",
        )?;
        let poll = one(
            locator
                .calls(&dma)?
                .into_iter()
                .filter(|r| {
                    r.code
                        .get(2)
                        .is_some_and(|i| i.mnemonic == "cbz" && i.operands.starts_with("w0,"))
                        && r.has("mov", "w0, #0x400")
                })
                .collect(),
            "DMA completion poll",
        )?;
        ranges.push(poll.span("flash DMA completion poll"));
        ranges.push(cache.span("destination cache maintenance"));
        // The NAND initializer requires all three ID bytes before publishing
        // this exact ID to its descriptor. Verify the complete constructor.
        let units = index
            .units(firmware, Some("main"))
            .map_err(|e| e.to_string())?;
        let sequence = [0x5280_0001u32 | (0xdc21 << 5), 0x72a0_0001 | (0xef << 5)];
        let pattern: Vec<u8> = sequence.iter().flat_map(|w| w.to_le_bytes()).collect();
        let decoder = Capstone::new()
            .arm64()
            .mode(arch::arm64::ArchMode::Arm)
            .build()
            .map_err(|e| e.to_string())?;
        if !units.iter().any(|u| {
            u.regions.iter().any(|region| {
                region
                    .bytes
                    .windows(pattern.len())
                    .enumerate()
                    .any(|(p, bytes)| {
                        if bytes != pattern || p < 48 || p % 4 != 0 {
                            return false;
                        }
                        let Some(bytes) = region.bytes.get(p - 48..p + 16) else {
                            return false;
                        };
                        let Ok(code) = decoder.disasm_all(bytes, region.address + p as u64 - 48)
                        else {
                            return false;
                        };
                        ["w0, #0xef", "w0, #0xdc", "w0, #0x21"]
                            .iter()
                            .all(|operand| {
                                code.iter().any(|i| {
                                    i.mnemonic() == Some("cmp") && i.op_str() == Some(operand)
                                })
                            })
                            && code.iter().any(|i| {
                                i.mnemonic() == Some("str") && i.op_str() == Some("w1, [x20]")
                            })
                    })
            })
        }) {
            return Err("NAND JEDEC validation and descriptor publication absent".into());
        }
        if jedec_id.is_some_and(|id| id != 0xefdc21) {
            return Err("installed NAND differs from driver".into());
        }
        checks.push(json!({"name":"serial NAND driver mode","address":mode,"size":4,"value":1}));
        json!({"kind":"serial-nand","jedec_id":"efdc21","part":"Winbond W35N01JW"})
    } else {
        let id = jedec_id.ok_or("NOR installed JEDEC identity is not established")?;
        let selector = locator.named("flash_select_supported_jedec_id", |r| {
            r.has("add", "x3, x3, #0x20") && r.has("cmp", "w2, #5")
        })?;
        let hits = index
            .select(
                Some(firmware),
                Some("main"),
                Some(selector.header.key.address),
            )
            .map_err(|e| e.to_string())?;
        let table = one(
            index
                .data_references(hits[0].id)
                .map_err(|e| e.to_string())?
                .into_iter()
                .filter(|r| r.kind == "adrp-add")
                .map(|r| r.target)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
            "JEDEC table",
        )?;
        let bytes = index
            .read(firmware, "main", table, 5 * 32)
            .map_err(|e| e.to_string())?
            .ok_or("unmapped JEDEC table")?;
        if !bytes
            .as_chunks::<32>()
            .0
            .iter()
            .any(|b| u32::from_le_bytes(b[..4].try_into().unwrap()) == id)
        {
            return Err("installed NOR absent from driver JEDEC table".into());
        }
        if id != 0x2c5b1b {
            return Err(format!("{id:06x}: part name is not established"));
        }
        ranges.push(cache.span("destination cache maintenance"));
        let calls = locator.calls(&request)?;
        let bank = one(
            calls
                .iter()
                .filter(|r| {
                    r.header.ranges[0].1 - r.header.ranges[0].0 == 60 && r.has("cmp", "w1, #1")
                })
                .collect::<Vec<_>>(),
            "flash bank selection",
        )?;
        let tail = one(
            calls
                .iter()
                .filter(|r| r.has("and", "x24, x23, #0xfffffffc"))
                .collect::<Vec<_>>(),
            "CPU tail copy",
        )?;
        ranges.push(bank.span("flash bank selection"));
        ranges.push(tail.span("flash CPU tail copy"));
        json!({"kind":"serial-nor","jedec_id":format!("{id:06x}"),"part":"Micron MT35XU01GBBA"})
    };
    // NAND compares the controller parameter page; NOR compares the mapped
    // loader. These are control-read selection policies, not device extents.
    let control_offset = if nand { 0x80000 } else { control_offset };
    checks.push(
        json!({"name":"flash driver initialized","address":marker,"size":4,"value":0x77666555u32}),
    );
    Ok(
        json!({"device":device,"window":{"address":0xf0000000u64,"size":0x8000000},"e1_window":{"address":0xf0000000u64,"size":0x8000000},"update_image":{"dat_offset":image_origin,"xor":255},"reader":{"symbol":"flash_read","alignment":64},"transport":"xp5-cmd-callback","code_ranges":ranges,"state_checks":checks,"control":{"offset":control_offset,"size":32768}}),
    )
}

/// Locate the X-Processor Pro stock reader and its large-transfer helper from
/// instruction behavior. The installed chip is a 64 MiB serial NOR; this
/// reader does not establish its manufacturer or JEDEC identity.
fn xpp_flash<L: Lookup>(index: &L, firmware: &str, image_origin: u64) -> Result<Value> {
    let locator = Locator {
        index,
        firmware,
        unit: "main".into(),
    };

    // XPP readers share this request-struct prologue, while clock setup and
    // aperture bounds vary. Match instruction behavior instead of carrying
    // camera addresses or hashes, then locate dependencies from call edges.
    const PREFIX: [u32; 8] = [
        0xe92d_4ff0,
        0xe28d_b020,
        0xe24d_d014,
        0xe590_5000,
        0xe590_4004,
        0xe590_6008,
        0xe5d0_700c,
        0xe5d0_a00d,
    ];
    let mut candidates = Vec::new();
    for unit in index
        .units(firmware, Some("main"))
        .map_err(|e| e.to_string())?
    {
        if unit.layout.architecture != Architecture::Arm32 {
            continue;
        }
        for region in &unit.regions {
            for (offset, bytes) in region
                .bytes
                .windows(PREFIX.len() * 4)
                .enumerate()
                .step_by(4)
            {
                if bytes
                    .chunks_exact(4)
                    .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
                    .ne(PREFIX)
                {
                    continue;
                }
                let address = region.address + offset as u64;
                let Ok(reader) = locator.routine(address) else {
                    continue;
                };
                let window_size = if reader.has("cmp", "r8, #0xf4000000") {
                    0x4000000
                } else if reader.has("cmn", "r8, #0x8000001") {
                    0x8000000
                } else {
                    continue;
                };
                if reader.header.architecture == Architecture::Arm32
                    && reader.code.len() >= 200
                    && (1..=2).contains(&reader.header.ranges.len())
                    && reader.has("add", "r3, r4, #0x10000000")
                    && reader.has("cmn", "r3, #0xf8000001")
                    && xpp_clock_vote(&reader)
                    && reader.has("movw", "r2, #0x6555")
                    && reader.has("movt", "r2, #0x7766")
                {
                    candidates.push((reader, window_size));
                }
            }
        }
    }
    let (reader, window_size) = one(candidates, "X-Processor Pro stock flash reader")?;
    let marker = xpp_initialized_marker(&reader)?;

    const WAIT_PREFIX: [u32; 10] = [
        0xe92d_4800,
        0xe28d_b004,
        0xe24d_d010,
        0xe50b_0014,
        0xe50b_1010,
        0xe50b_200c,
        0xe54b_3008,
        0xe3a0_3001,
        0xe54b_3007,
        0xe24b_0014,
    ];
    const WAIT_SUFFIX: [u32; 2] = [0xe24b_d004, 0xe8bd_8800];
    let mut wrapper_candidates = Vec::new();
    for unit in index
        .units(firmware, Some("main"))
        .map_err(|e| e.to_string())?
    {
        if unit.layout.architecture != Architecture::Arm32 {
            continue;
        }
        for region in &unit.regions {
            for (offset, bytes) in region
                .bytes
                .windows((WAIT_PREFIX.len() + 1 + WAIT_SUFFIX.len()) * 4)
                .enumerate()
                .step_by(4)
            {
                let words: Vec<_> = bytes
                    .chunks_exact(4)
                    .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
                    .collect();
                if words[..WAIT_PREFIX.len()] != WAIT_PREFIX
                    || words[WAIT_PREFIX.len()] & 0x0f00_0000 != 0x0b00_0000
                    || words[WAIT_PREFIX.len() + 1..] != WAIT_SUFFIX
                {
                    continue;
                }
                let address = region.address + offset as u64;
                let Ok(target) = arm_branch_target(
                    address + (WAIT_PREFIX.len() * 4) as u64,
                    words[WAIT_PREFIX.len()],
                ) else {
                    continue;
                };
                if target != reader.header.key.address {
                    continue;
                }
                let Ok(wrapper) = locator.routine(address) else {
                    continue;
                };
                if wrapper.code.len() == WAIT_PREFIX.len() + 1 + WAIT_SUFFIX.len()
                    && wrapper.header.ranges == [(address, address + 52)]
                {
                    wrapper_candidates.push(wrapper);
                }
            }
        }
    }
    if wrapper_candidates.is_empty() {
        for (id, reference) in index
            .referrers(firmware, Some("main"), reader.header.key.address)
            .map_err(|e| e.to_string())?
        {
            if !reference.kind.ends_with("CALL") {
                continue;
            }
            let Some(entry) = index.entry(id).map_err(|e| e.to_string())? else {
                continue;
            };
            let address = entry.function.key.address;
            let Ok(wrapper) = locator.routine(address) else {
                continue;
            };
            let calls_reader = wrapper.code.iter().any(|instruction| {
                instruction.mnemonic == "bl"
                    && number(instruction.operands.trim_start_matches('#')).ok()
                        == Some(reader.header.key.address)
            });
            if wrapper.header.architecture == Architecture::Arm32
                && wrapper.header.ranges == [(address, address + 52)]
                && wrapper.code.len() == 13
                && wrapper.code.first().is_some_and(|instruction| {
                    instruction.mnemonic == "stmdb" && instruction.operands.contains("lr")
                })
                && wrapper.code.iter().any(|instruction| {
                    instruction.mnemonic == "strb" && instruction.operands.starts_with("r3,")
                })
                && wrapper.code.iter().any(|instruction| {
                    instruction.mnemonic == "mov" && instruction.operands == "r3, #0x1"
                })
                && calls_reader
            {
                wrapper_candidates.push(wrapper);
            }
        }
    }
    let wait = one(
        wrapper_candidates,
        "X-Processor Pro flash-read wait wrapper",
    )?;

    let callees = locator.calls(&reader)?;
    let mode = one(
        callees.into_iter().filter(xpp_read_mode).collect(),
        "X-Processor Pro flash read-mode selector",
    )?;
    let mode_address = mode.header.key.address;
    let transfer = one(
        locator
            .calls(&reader)?
            .into_iter()
            .filter(|routine| {
                routine.code.len() > 50
                    && routine.code.iter().any(|instruction| {
                        arm_instruction_value(instruction, "ands", "r5", 0x3f)
                            && instruction.operands.contains("r7")
                    })
                    && routine.code.iter().any(|instruction| {
                        instruction.mnemonic == "bl"
                            && number(instruction.operands.trim_start_matches('#')).ok()
                                == Some(mode_address)
                    })
            })
            .collect(),
        "X-Processor Pro flash transfer helper",
    )?;
    let transfer_name = if locator.calls(&transfer)?.iter().any(|routine| {
        routine
            .header
            .names
            .iter()
            .any(|name| name == "sflash_dma_read")
    }) {
        "flash PL330 transfer"
    } else {
        "flash transfer helper"
    };
    let ranges = [
        reader.span("flash_read"),
        wait.span("flash_read_wait"),
        transfer.span(transfer_name),
        mode.span("flash read-mode selector"),
    ];
    let code_ranges: Vec<Value> = ranges.into_iter().collect();
    let checks = vec![json!({
        "name": "flash driver initialized",
        "address": marker,
        "size": 4,
        "value": 0x77666555u32
    })];
    Ok(json!({
        "device": {"kind":"serial-nor"},
        "window": {"address":0xf0000000u64,"size":window_size},
        "update_image": {"dat_offset":image_origin,"xor":255},
        "reader": {"symbol":"flash_read_wait","alignment":4},
        "transport": "xpp-cmd-callback",
        "code_ranges":code_ranges,
        "state_checks":checks,
        "control": {"offset":0,"size":32768}
    }))
}

fn xpp_clock_vote(routine: &Routine) -> bool {
    routine.has("mov", "r0, #0x15")
        || routine.code.windows(4).any(|instructions| {
            instructions[0].mnemonic == "mov"
                && instructions[0].operands == "r0, #0x0"
                && instructions[1].mnemonic == "mov"
                && instructions[1].operands == "r1, #0x3"
                && instructions[2].mnemonic == "movt"
                && instructions[2].operands == "r1, #0x15"
                && instructions[3].mnemonic == "bl"
        })
}

fn xpp_read_mode(routine: &Routine) -> bool {
    let code = &routine.code;
    (code.len() > 100
        && code
            .get(4)
            .is_some_and(|i| arm_instruction_value(i, "cmp", "r0", 1))
        && code
            .get(7)
            .is_some_and(|i| arm_instruction_value(i, "cmp", "r3", 5))
        && code.get(8).is_some_and(|i| i.mnemonic == "ldrls")
        && code
            .iter()
            .any(|i| i.mnemonic == "bic" && i.operands.contains("0x80000000")))
        || xpp_register_table_read_mode(routine)
}

fn xpp_register_table_read_mode(routine: &Routine) -> bool {
    let address_pair = |address: u64| {
        routine.code.windows(2).any(|instructions| {
            let immediate = |instruction: &Instruction| {
                let (register, value) = instruction.operands.split_once(", ")?;
                Some((register.to_string(), number(value.trim_start_matches('#')).ok()?))
            };
            let Some((low_register, low_value)) = immediate(&instructions[0]) else {
                return false;
            };
            let Some((high_register, high_value)) = immediate(&instructions[1]) else {
                return false;
            };
            matches!(instructions[0].mnemonic.as_str(), "mov" | "movw")
                && instructions[1].mnemonic == "movt"
                && low_register == high_register
                && low_value == address
                && high_value == 0xfff1
        })
    };
    routine.code.len() > 100
        && routine
            .code
            .iter()
            .any(|instruction| arm_instruction_value(instruction, "cmp", "r0", 1))
        && routine
            .code
            .iter()
            .any(|instruction| arm_instruction_value(instruction, "cmp", "r1", 2))
        && routine
            .code
            .iter()
            .any(|instruction| arm_instruction_value(instruction, "cmp", "r1", 3))
        && [0x4000, 0x4010, 0x4020].into_iter().all(address_pair)
}

fn arm_instruction_value(
    instruction: &Instruction,
    mnemonic: &str,
    register: &str,
    value: u64,
) -> bool {
    if instruction.mnemonic != mnemonic {
        return false;
    }
    let operands: Vec<_> = instruction.operands.split(", ").collect();
    operands.first() == Some(&register)
        && operands
            .last()
            .and_then(|operand| number(operand.trim_start_matches('#')).ok())
            == Some(value)
}

fn xpp_initialized_marker(reader: &Routine) -> Result<u64> {
    let pair = |at: usize| -> Option<(String, u32)> {
        let first = reader.code.get(at)?;
        let second = reader.code.get(at + 1)?;
        if first.mnemonic != "movw" || second.mnemonic != "movt" {
            return None;
        }
        let low: Vec<_> = first.operands.split(", ").collect();
        let high: Vec<_> = second.operands.split(", ").collect();
        if low.len() != 2 || high.len() != 2 || low[0] != high[0] {
            return None;
        }
        let low_value = u32::try_from(number(low[1].trim_start_matches('#')).ok()?).ok()?;
        let high_value = u32::try_from(number(high[1].trim_start_matches('#')).ok()?).ok()?;
        Some((low[0].to_string(), low_value | (high_value << 16)))
    };
    let mut markers = BTreeSet::new();
    for magic_at in 0..reader.code.len().saturating_sub(1) {
        let Some((magic_register, magic)) = pair(magic_at) else {
            continue;
        };
        if magic != 0x7766_6555 {
            continue;
        }
        for compare_at in magic_at + 2..reader.code.len().min(magic_at + 8) {
            let compare = &reader.code[compare_at];
            if compare.mnemonic != "cmp" {
                continue;
            }
            let operands: Vec<_> = compare.operands.split(", ").collect();
            if operands.len() != 2 || operands[1] != magic_register {
                continue;
            }
            for load_at in (magic_at.saturating_sub(8)..compare_at).rev() {
                let load = &reader.code[load_at];
                if load.mnemonic != "ldr" {
                    continue;
                }
                let Some((destination, memory)) = load.operands.split_once(", ") else {
                    continue;
                };
                let Some(memory) = memory
                    .strip_prefix('[')
                    .and_then(|memory| memory.strip_suffix(']'))
                else {
                    continue;
                };
                let fields: Vec<_> = memory.split(", ").collect();
                if fields.first() != Some(&destination)
                    || fields
                        .get(1)
                        .is_some_and(|offset| !matches!(*offset, "#0" | "#0x0"))
                    || fields.len() > 2
                {
                    continue;
                }
                for address_at in load_at.saturating_sub(8)..load_at {
                    if let Some((register, address)) = pair(address_at)
                        && register == destination
                    {
                        markers.insert(u64::from(address));
                    }
                }
            }
        }
    }
    one(
        markers.into_iter().collect(),
        "X-Processor Pro initialized marker",
    )
}

/// Locate the XP4 mapped-flash branch used by firmware readers and direct E1
/// capture. The first checked firmware establishes the instruction grammar;
/// each address and state input is recovered from its indexed routines.
fn xp4_flash<L: Lookup>(index: &L, firmware: &str, image_origin: u64) -> Result<Value> {
    if firmware == "xt3-511" {
        return xp4_xt3_flash(index, firmware, image_origin);
    }
    if firmware != "xs10-311" {
        return Err(format!(
            "{firmware}: XP4 mapped flash has not been located for this firmware"
        ));
    }
    const PREFIX: [u32; 4] = [0xe92d_4ff0, 0xe28d_b020, 0xe24d_d01c, 0xe1a0_7003];
    const CHECKED: [(usize, u32); 36] = [
        (6, 0xe5d3_3000),
        (7, 0xe353_0001),
        (9, 0xe1a0_4000),
        (10, 0xe1a0_8001),
        (11, 0xe1a0_5002),
        (12, 0xe300_317e),
        (13, 0xe150_0003),
        (16, 0xe1a0_0884),
        (18, 0xe1a0_6000),
        (19, 0xe1a0_8888),
        (20, 0xe358_0802),
        (22, 0xe3a0_4802),
        (23, 0xe1a0_a004),
        (24, 0xe3a0_9000),
        (25, 0xe50b_5034),
        (26, 0xe50b_6030),
        (27, 0xe50b_a02c),
        (28, 0xe3a0_3001),
        (29, 0xe54b_3028),
        (30, 0xe54b_9027),
        (31, 0xe24b_0034),
        (33, 0xe250_3000),
        (34, 0xe50b_3038),
        (36, 0xe285_5802),
        (37, 0xe286_6802),
        (38, 0xe357_0000),
        (40, 0xe1a0_0004),
        (41, 0xe12f_ff37),
        (42, 0xe284_4802),
        (43, 0xe158_0004),
        (45, 0xe51b_0038),
        (46, 0xe24b_d020),
        (47, 0xe8bd_8ff0),
        // These compare the requested block count with the checked boundary,
        // select the byte-offset translator, and invoke the stock reader.
        (15, 0xeb00_0000),
        (17, 0xeb00_0000),
        (32, 0xeb00_0000),
    ];
    let units = index
        .units(firmware, Some("main"))
        .map_err(|e| e.to_string())?;
    let mut matches = Vec::new();
    for unit in &units {
        if unit.layout.architecture != Architecture::Arm32 {
            continue;
        }
        for region in &unit.regions {
            for (offset, bytes) in region
                .bytes
                .windows(PREFIX.len() * 4)
                .enumerate()
                .step_by(4)
            {
                if bytes
                    .chunks_exact(4)
                    .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
                    .ne(PREFIX)
                {
                    continue;
                }
                let address = region.address + offset as u64;
                let Ok(routine) = (Locator {
                    index,
                    firmware,
                    unit: "main".into(),
                })
                .routine(address) else {
                    continue;
                };
                if routine.header.ranges.len() != 1
                    || routine.header.ranges[0] != (address, address + 0xe4)
                    || routine.code.len() != 57
                {
                    continue;
                }
                let Some(code) = index
                    .read(firmware, "main", address, 0xe4)
                    .map_err(|e| e.to_string())?
                else {
                    continue;
                };
                let words: Vec<_> = code
                    .chunks_exact(4)
                    .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
                    .collect();
                if !CHECKED.iter().all(|&(i, expected)| {
                    let actual = words[i];
                    if expected & 0x0fff_ffff == 0x0b00_0000 {
                        actual & 0x0f00_0000 == 0x0b00_0000
                    } else {
                        actual == expected
                    }
                }) {
                    continue;
                }
                let Ok(enabled) = arm_mov_pair(words[4], words[5]) else {
                    continue;
                };
                let (Ok(translator), Ok(stock_reader), Ok(cache)) = (
                    arm_branch_target(address + 68, words[17]),
                    arm_branch_target(address + 128, words[32]),
                    arm_branch_target(address + 60, words[15]),
                ) else {
                    continue;
                };
                let translator_routine = (Locator {
                    index,
                    firmware,
                    unit: "main".into(),
                })
                .routine(translator)?;
                let translated = index
                    .read(firmware, "main", translator, 24)
                    .map_err(|e| e.to_string())?
                    .ok_or("unmapped XP4 flash address translator")?;
                let translated: Vec<u32> = translated
                    .chunks_exact(4)
                    .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
                    .collect();
                if translated
                    != [
                        0xe52d_b004,
                        0xe28d_b000,
                        0xe280_020f,
                        0xe28b_d000,
                        0xe49d_b004,
                        0xe12f_ff1e,
                    ]
                    || translator_routine.header.ranges != [(translator, translator + 24)]
                {
                    continue;
                }
                let reader_routine = (Locator {
                    index,
                    firmware,
                    unit: "main".into(),
                })
                .routine(stock_reader)?;
                let reader_bytes = index
                    .read(
                        firmware,
                        "main",
                        stock_reader,
                        (reader_routine.header.ranges[0].1 - stock_reader) as usize,
                    )
                    .map_err(|e| e.to_string())?
                    .ok_or("unmapped XP4 stock flash reader")?;
                let reader_words: Vec<u32> = reader_bytes
                    .chunks_exact(4)
                    .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
                    .collect();
                let initialized = reader_words
                    .windows(4)
                    .any(|w| w == [0xe309_3940, 0xe340_3181, 0xe306_2555, 0xe347_2766]);
                let aperture_bound = reader_routine
                    .code
                    .iter()
                    .any(|i| i.mnemonic == "cmp" && i.operands == "r2, #0xf4000000");
                if !initialized || !aperture_bound {
                    continue;
                }
                let cache_routine = (Locator {
                    index,
                    firmware,
                    unit: "main".into(),
                })
                .routine(cache)?;
                let byte_reader = locate_xp4_byte_reader(index, firmware, translator)?;
                matches.push((
                    enabled,
                    routine,
                    translator_routine,
                    reader_routine,
                    cache_routine,
                    byte_reader,
                ));
            }
        }
    }
    if matches.len() != 1 {
        return Err(format!(
            "{firmware}: {} XP4 flash reader candidates",
            matches.len()
        ));
    }
    let (enabled, indexed_routine, translator_routine, reader_routine, cache_routine, byte_reader) =
        matches.pop().unwrap();
    let origin = u64::from(image_origin);
    if origin != 0x274 {
        return Err("XP4 update origin differs from its registered DAT bias".into());
    }
    let ranges = vec![
        indexed_routine.span("indexed firmware reader"),
        reader_routine.span("stock flash reader"),
        translator_routine.span("flash aperture translation"),
        cache_routine.span("flash destination cache maintenance"),
        byte_reader.span("byte-offset firmware reader"),
    ];
    Ok(json!({
        "device":{"kind":"serial-nor"},
        "window":{"address":0xf0000000u64,"size":0x4000000u64},
        "e1_window":{"address":0xf0000000u64,"size":0x4000000u64},
        "update_image":{"dat_offset":origin,"xor":255},
        "transport":null,
        "code_ranges":ranges,
        "state_checks":[
            {"name":"flash firmware reader enabled","address":enabled,"size":1,"value":1},
            {"name":"flash driver initialized","address":0x01819940u64,"size":4,"value":0x77666555u32}
        ],
        "control":{"offset":0,"size":32768}
    }))
}

/// Locate X-T3 5.11's mapped XP4 flash reader and its initialized-state guard.
fn xp4_xt3_flash<L: Lookup>(index: &L, firmware: &str, image_origin: u64) -> Result<Value> {
    const HIGH_LEVEL_READER: u64 = 0x01074fb8;
    const DMA_READER: u64 = 0x0107536c;
    const DMA_TRANSFER: u64 = 0x01074df8;
    const INITIALIZER: u64 = 0x01075808;
    const CACHE_SYNC: u64 = 0x010763cc;
    const HIGH_LEVEL_BYTES: u64 = 0x3b4;
    const DMA_READER_BYTES: u64 = 0x304;
    const DMA_TRANSFER_BYTES: u64 = 0x144;
    const INITIALIZER_BYTES: u64 = 0x128;

    let locator = Locator {
        index,
        firmware,
        unit: "main".into(),
    };
    let high_level = locator.routine(HIGH_LEVEL_READER)?;
    let reader = locator.routine(DMA_READER)?;
    let transfer = locator.routine(DMA_TRANSFER)?;
    let initializer = locator.routine(INITIALIZER)?;
    let cache_sync = locator.routine(CACHE_SYNC)?;
    let exact_span =
        |routine: &Routine, start: u64, size: u64| routine.header.ranges == [(start, start + size)];
    if !exact_span(&high_level, HIGH_LEVEL_READER, HIGH_LEVEL_BYTES)
        || !exact_span(&reader, DMA_READER, DMA_READER_BYTES)
        || !exact_span(&transfer, DMA_TRANSFER, DMA_TRANSFER_BYTES)
        || !exact_span(&initializer, INITIALIZER, INITIALIZER_BYTES)
        || !reader.has("cmp", "r2, #0xf4000000")
    {
        return Err("X-T3 flash reader boundaries or aperture check changed".into());
    }

    let reader_bytes = index
        .read(firmware, "main", DMA_READER, DMA_READER_BYTES as usize)
        .map_err(|error| error.to_string())?
        .ok_or("unmapped X-T3 flash reader")?;
    let words: Vec<u32> = reader_bytes
        .chunks_exact(4)
        .map(|bytes| u32::from_le_bytes(bytes.try_into().unwrap()))
        .collect();
    if words.len() != 193
        || words[0] != 0xe92d_4ff0
        || arm_mov_pair_for(words[26], words[27], 3)? != 0x0155_a69c
        || arm_mov_pair_for(words[28], words[29], 2)? != 0x7766_6555
        || arm_branch_target(DMA_READER + 0x170, words[92])? != DMA_TRANSFER
        || arm_branch_target(DMA_READER + 0x110, words[68])? != CACHE_SYNC
    {
        return Err("X-T3 flash reader state or transfer path changed".into());
    }

    let state_address = arm_mov_pair_for(words[26], words[27], 3)?;
    let references = index
        .referrers(firmware, Some("main"), state_address)
        .map_err(|error| error.to_string())?;
    let mut reader_checks_state = false;
    let mut initializer_sets_state = false;
    for (function_id, reference) in references {
        let function = index
            .entry(function_id)
            .map_err(|error| error.to_string())?
            .ok_or("missing X-T3 flash state reference")?;
        match (function.function.key.address, reference.kind.as_str()) {
            (DMA_READER, "READ") => reader_checks_state = true,
            (INITIALIZER, "WRITE") => initializer_sets_state = true,
            _ => {}
        }
    }
    if !reader_checks_state || !initializer_sets_state {
        return Err(
            "X-T3 flash initialized marker is not tied to its reader and initializer".into(),
        );
    }

    if image_origin != 0x274 {
        return Err("X-T3 update origin differs from its registered DAT bias".into());
    }
    Ok(json!({
        "device":{"kind":"serial-nor"},
        "window":{"address":0xf0000000u64,"size":0x4000000u64},
        "e1_window":{"address":0xf0000000u64,"size":0x4000000u64},
        "update_image":{"dat_offset":image_origin,"xor":255},
        "transport":null,
        "code_ranges":[
            high_level.span("firmware flash reader"),
            reader.span("stock flash reader"),
            transfer.span("flash DMA transfer"),
            initializer.span("flash controller initialization"),
            cache_sync.span("flash destination cache maintenance")
        ],
        "state_checks":[
            {"name":"flash driver initialized","address":state_address,"size":4,"value":0x77666555u32}
        ],
        "control":{"offset":0,"size":32768}
    }))
}

fn arm_mov_pair(low: u32, high: u32) -> Result<u64> {
    arm_mov_pair_for(low, high, 3)
}

fn arm_mov_pair_for(low: u32, high: u32, register: u32) -> Result<u64> {
    let immediate = |word: u32, opcode: u32| -> Result<u16> {
        if word & 0x0ff0_0000 != opcode {
            return Err("XP4 flash address is not built by MOVW/MOVT".into());
        }
        let rd = (word >> 12) & 0xf;
        let imm = (((word >> 16) & 0xf) << 12) | (word & 0xfff);
        if rd != register {
            return Err("XP4 flash address uses another register".into());
        }
        Ok(imm as u16)
    };
    let low = immediate(low, 0x0300_0000)?;
    let high = immediate(high, 0x0340_0000)?;
    Ok(u64::from(low) | (u64::from(high) << 16))
}

fn arm_branch_target(site: u64, word: u32) -> Result<u64> {
    if word & 0x0f00_0000 != 0x0b00_0000 {
        return Err("ARM flash reader call is not BL".into());
    }
    let raw = (word & 0x00ff_ffff) << 2;
    let displacement = ((raw << 6) as i32) >> 6;
    site.checked_add(8)
        .and_then(|pc| pc.checked_add_signed(i64::from(displacement)))
        .ok_or_else(|| "ARM flash call target overflow".into())
}

fn locate_xp4_byte_reader<L: Lookup>(
    index: &L,
    firmware: &str,
    translator: u64,
) -> Result<Routine> {
    let prefix = [0xe92d_4830u32, 0xe28d_b00c, 0xe1a0_5002, 0xe1a0_4001];
    let units = index
        .units(firmware, Some("main"))
        .map_err(|e| e.to_string())?;
    let mut matches = Vec::new();
    for unit in units {
        for region in &unit.regions {
            for (offset, bytes) in region.bytes.windows(16).enumerate().step_by(4) {
                if offset < 16 {
                    continue;
                }
                if bytes
                    .chunks_exact(4)
                    .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
                    .ne(prefix)
                {
                    continue;
                }
                let address = region.address + offset as u64 - 16;
                let Ok(routine) = (Locator {
                    index,
                    firmware,
                    unit: "main".into(),
                })
                .routine(address) else {
                    continue;
                };
                if routine.header.ranges.len() != 1
                    || routine.header.ranges[0] != (address, address + 68)
                    || routine.code.len() != 17
                {
                    continue;
                }
                let Some(code) = index
                    .read(firmware, "main", address, 68)
                    .map_err(|e| e.to_string())?
                else {
                    continue;
                };
                let words: Vec<_> = code
                    .chunks_exact(4)
                    .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
                    .collect();
                if words[2] != 0xe150_0003
                    || words[9..13] != [0xe3a0_3001, 0xe1a0_2005, 0xe1a0_1000, 0xe1a0_0004]
                    || words[14..] != [0xe8bd_8830, 0xe3e0_001d, 0xe12f_ff1e]
                    || arm_branch_target(address + 32, words[8]).ok() != Some(translator)
                {
                    continue;
                }
                let Ok(byte_reader) = arm_mov_pair(words[0], words[1]) else {
                    continue;
                };
                if byte_reader.checked_add(1) != Some(0x02fe_0000) {
                    continue;
                }
                let Ok(byte_stock_reader) = arm_branch_target(address + 52, words[13]) else {
                    continue;
                };
                let low_reader = (Locator {
                    index,
                    firmware,
                    unit: "main".into(),
                })
                .routine(byte_stock_reader)?;
                if low_reader.header.ranges.len() != 1
                    || low_reader.header.ranges[0].1 - low_reader.header.ranges[0].0 != 52
                {
                    continue;
                }
                matches.push(routine);
            }
        }
    }
    one(matches, "XP4 byte-offset flash reader")
}

/// Match the mapped loader against the decoded linear update image. The origin
/// is supplied by the update container decoder, rather than inferred from a
/// coincidental occurrence of setup bytes in another image.
pub fn image_offsets<L: Lookup>(
    index: &L,
    firmware: &str,
    inverted: &[u8],
    origin: u64,
) -> Result<(u64, u64)> {
    let loader = index
        .read(firmware, "main", 0x81000, 128)
        .map_err(|e| e.to_string())?
        .ok_or("mapped loader missing")?;
    let start = usize::try_from(origin).map_err(|_| "update origin overflow")?;
    let image = inverted.get(start..).ok_or("update origin outside file")?;
    if image.get(..16)
        != Some(&[
            255, 255, 255, 255, 0, 0, 0, 0, 170, 170, 170, 170, 85, 85, 85, 85,
        ])
    {
        return Err("signed setup envelope is not present at update origin".into());
    }
    let controls = image
        .windows(loader.len())
        .enumerate()
        .filter_map(|(p, b)| (b == loader && p.is_multiple_of(0x10000)).then_some(p))
        .collect();
    let control = one(controls, "mapped loader comparison control")?;
    if control + 32768 > image.len() {
        return Err("update comparison control is truncated".into());
    }
    Ok((origin, control as u64))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Boundary, Index, Region, UnitInput, UnitLayout};

    fn candidates(second: u32) -> Index {
        let mut index = Index::new();
        // Two self-authored MOV-immediate/RET routines with the same nominated
        // name. Only decoded semantics, not the name count, selects a result.
        let words = [
            0xd2800000u32 | (1 << 5),
            0xd65f03c0,
            0xd2800000 | (second << 5),
            0xd65f03c0,
        ];
        index
            .add_unit(
                UnitInput {
                    layout: UnitLayout {
                        firmware: "test".into(),
                        unit: "main".into(),
                        architecture: Architecture::Aarch64,
                        address_ranges: vec![(0x1000, 0x2000)],
                        address_aliases: vec![],
                    },
                    functions: [0x1000, 0x1008]
                        .into_iter()
                        .map(|address| Boundary {
                            address,
                            ranges: vec![(address, address + 8)],
                            names: vec!["candidate".into()],
                            annotations: vec![],
                            references: vec![],
                        })
                        .collect(),
                },
                vec![Region {
                    address: 0x1000,
                    bytes: words.iter().flat_map(|word| word.to_le_bytes()).collect(),
                }],
            )
            .unwrap();
        index
    }
    #[test]
    fn nominations_are_verified_before_ambiguity_is_decided() {
        let index = candidates(2);
        let locator = Locator {
            index: &index,
            firmware: "test",
            unit: "main".into(),
        };
        let routine = locator
            .named("candidate", |r| r.has("mov", "x0, #1"))
            .unwrap();
        assert_eq!(routine.header.key.address, 0x1000);
        assert!(
            locator
                .named("candidate", |r| r.has("mov", "x0, #3"))
                .is_err()
        );
        let index = candidates(1);
        let locator = Locator {
            index: &index,
            firmware: "test",
            unit: "main".into(),
        };
        assert!(
            locator
                .named("candidate", |r| r.has("mov", "x0, #1"))
                .is_err()
        );
    }
}
