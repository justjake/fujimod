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

/// Locate the XP5 stock flash reader. `jedec_id` is the installed hardware
/// identity, verified against the driver's supported-device table. It is not
/// selected arbitrarily from the list of supported NOR chips. `image_origin`
/// and `control_offset` come from the decoded update's packaging records.
pub fn flash<L: Lookup>(
    index: &L,
    firmware: &str,
    processor: &str,
    jedec_id: Option<u32>,
    image_origin: u64,
    control_offset: u64,
) -> Result<Value> {
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
        json!({"device":device,"window":{"address":0xf0000000u64,"size":0x8000000},"update_image":{"dat_offset":image_origin,"xor":255},"reader":{"symbol":"flash_read","alignment":64},"transport":"xp5-cmd-callback","code_ranges":ranges,"state_checks":checks,"control":{"offset":control_offset,"size":32768}}),
    )
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
