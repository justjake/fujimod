use crate::*;
use capstone::prelude::*;
use regex::Regex;
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap};
use std::sync::LazyLock;

struct Patterns {
    reg: Regex,
    fs: Regex,
    fd: Regex,
    sp: Regex,
    pc: Regex,
    pc_add: Regex,
    imm: Regex,
    a64reg: Regex,
}
static P: LazyLock<Patterns> = LazyLock::new(|| Patterns {
    reg: Regex::new(r"\b(?:r1[0-2]|r[0-9]|sb|sl|fp|ip)\b").unwrap(),
    fs: Regex::new(r"\bs(?:[12]?[0-9]|3[01])\b").unwrap(),
    fd: Regex::new(r"\bd(?:[12]?[0-9]|3[01])\b").unwrap(),
    sp: Regex::new(r"sp, #-?(?:0x[0-9a-f]+|\d+)").unwrap(),
    pc: Regex::new(r"\[pc, #(-?(?:0x[0-9a-f]+|\d+))\]").unwrap(),
    pc_add: Regex::new(r"pc, #-?(?:0x[0-9a-f]+|\d+)").unwrap(),
    imm: Regex::new(r"#(-?(?:0x[0-9a-f]+|\d+))").unwrap(),
    a64reg: Regex::new(r"\b([xwqvdsbh])(?:[12]?[0-9]|3[01])\b").unwrap(),
});
fn number(s: &str) -> Option<i64> {
    if let Some(s) = s.strip_prefix('-') {
        number(s).map(|v| -v)
    } else if let Some(s) = s.strip_prefix("0x") {
        i64::from_str_radix(s, 16).ok()
    } else {
        s.parse().ok()
    }
}
fn base(m: &str) -> &str {
    let m = m.split('.').next().unwrap_or(m);
    for c in [
        "eq", "ne", "hs", "lo", "mi", "pl", "vs", "vc", "hi", "ls", "ge", "lt", "gt", "le", "cs",
        "cc", "al",
    ] {
        if let Some(b) = m.strip_suffix(c)
            && matches!(b, "b" | "bl" | "blx" | "cbz" | "cbnz")
        {
            return b;
        }
    }
    m
}
fn word_ldr(m: &str) -> bool {
    m == "ldr"
        || [
            "eq", "ne", "hs", "lo", "mi", "pl", "vs", "vc", "hi", "ls", "ge", "lt", "gt", "le",
            "cs", "cc", "al",
        ]
        .iter()
        .any(|c| m.strip_prefix("ldr") == Some(*c))
}
fn literal(a: u64, op: &str) -> Option<u64> {
    let m = P.pc.captures(op)?;
    Some((a.wrapping_add(8).wrapping_add_signed(number(&m[1])?)) & !3)
}
/// Address-free A32 normalization, preserving the former fragment/pair token spelling.
pub(crate) fn normalize(a: u64, m: &str, op: &str, u: &Unit) -> String {
    let b = base(m);
    if matches!(b, "b" | "bl" | "blx" | "cbz" | "cbnz") && op.starts_with('#') {
        return format!("{m} T");
    }
    if matches!(b, "movw" | "movt") {
        return format!(
            "{m} {}, K",
            P.reg.replace_all(op.split(',').next().unwrap_or(""), "R")
        );
    }
    let mut op = op.to_string();
    if let Some(target) = literal(a, &op) {
        let class = if word_ldr(m) {
            u.read(target, 4)
                .map(|bytes| {
                    let v = u32::from_le_bytes(bytes.try_into().unwrap()) as u64;
                    if u.pointer(v) {
                        "A".into()
                    } else {
                        format!("{v:#x}")
                    }
                })
                .unwrap_or_else(|| "P".into())
        } else {
            "P".into()
        };
        op =
            P.pc.replace_all(&op, format!("[pc, ={class}]"))
                .into_owned();
    } else if matches!(b, "add" | "sub" | "adr") && op.contains("pc, #") {
        op = P.pc_add.replace_all(&op, "pc, P").into_owned();
    } else if b == "adr" {
        op = P.imm.replace_all(&op, "P").into_owned();
    }
    op = P.sp.replace_all(&op, "sp, S").into_owned();
    op = P.reg.replace_all(&op, "R").into_owned();
    op = P.fs.replace_all(&op, "Fs").into_owned();
    op = P.fd.replace_all(&op, "Fd").into_owned();
    format!("{m} {op}")
}
fn normalize64(m: &str, op: &str) -> String {
    let mut op = op.to_string();
    if m == "bl" || m == "b" || m.starts_with("b.") || matches!(m, "cbz" | "cbnz" | "tbz" | "tbnz")
    {
        if let Some(at) = op.rfind('#') {
            op.replace_range(at.., "T");
        }
    } else if matches!(m, "adr" | "adrp" | "movk" | "movz" | "movn") {
        op = P.imm.replace_all(&op, "K").into_owned();
    } else if m == "ldr" && !op.contains('[') {
        op = P.imm.replace_all(&op, "P").into_owned();
    }
    op = P.sp.replace_all(&op, "sp, S").into_owned();
    op = P.a64reg.replace_all(&op, "${1}R").into_owned();
    format!("{m} {op}")
}
fn hash64(bytes: &[u8]) -> u64 {
    u64::from_le_bytes(Sha256::digest(bytes)[..8].try_into().unwrap())
}
pub(crate) fn decode(u: &Unit, functions: &[Boundary]) -> Result<(Vec<Function>, Vec<String>)> {
    let cs = match u.layout.architecture {
        Architecture::Arm32 => Capstone::new().arm().mode(arch::arm::ArchMode::Arm).build(),
        Architecture::Aarch64 => Capstone::new()
            .arm64()
            .mode(arch::arm64::ArchMode::Arm)
            .build(),
    }
    .map_err(|e| Error::Invalid(e.to_string()))?;
    let mut dictionary = HashMap::<String, u32>::new();
    let mut words = Vec::new();
    let mut out = Vec::new();
    for f in functions {
        let mut texts = Vec::<String>::new();
        let mut calls = Vec::new();
        let mut constants = BTreeSet::new();
        let mut refs = f.references.clone();
        for &(lo, hi) in &f.ranges {
            let code = u
                .read(lo, (hi - lo) as usize)
                .ok_or_else(|| Error::Invalid("unmapped range".into()))?;
            let mut insns = std::collections::BTreeMap::new();
            let mut pos = 0;
            while pos < code.len() {
                let decoded = cs
                    .disasm_all(&code[pos..], lo + pos as u64)
                    .map_err(|e| Error::Invalid(e.to_string()))?;
                for i in decoded.iter() {
                    insns.insert(
                        i.address(),
                        (
                            i.mnemonic().unwrap_or("").to_string(),
                            i.op_str().unwrap_or("").to_string(),
                        ),
                    );
                    pos = (i.address() - lo) as usize + i.bytes().len();
                }
                if pos < code.len() {
                    pos += 4;
                }
            }
            let mut pool = BTreeSet::new();
            if u.layout.architecture == Architecture::Arm32 {
                for (&a, (m, op)) in &insns {
                    if (base(m).starts_with("ldr") || base(m).starts_with("vldr"))
                        && let Some(t) = literal(a, op)
                    {
                        pool.insert(t);
                        pool.insert(t + 4);
                    }
                    if word_ldr(m) && op.starts_with("pc, [pc, r") {
                        let mut p = a + 8;
                        while p < hi {
                            let Some(w) = u.read(p, 4) else {
                                break;
                            };
                            let v = u32::from_le_bytes(w.try_into().unwrap()) as u64;
                            if v < lo.saturating_sub(0x100000) || v >= hi + 0x100000 {
                                break;
                            }
                            pool.insert(p);
                            p += 4;
                        }
                    }
                }
            }
            for a in (lo..hi).step_by(4) {
                let Some((m, op)) = insns.get(&a).filter(|_| !pool.contains(&a)) else {
                    texts.push("|".into());
                    continue;
                };
                let b = base(m);
                if matches!(b, "bl" | "blx")
                    && op.starts_with('#')
                    && let Some(t) = number(&op[1..])
                {
                    calls.push((texts.len(), t as u64));
                }
                if !matches!(b, "movw" | "movt") {
                    for c in P.imm.captures_iter(op) {
                        if let Some(v) = number(&c[1])
                            && !u.pointer(v as u64)
                        {
                            constants.insert(v as u64);
                        }
                    }
                }
                if u.layout.architecture == Architecture::Arm32 {
                    if let (Some(low), Some(high)) = (u.read(a, 4), u.read(a + 4, 4)) {
                        let low = u32::from_le_bytes(low.try_into().unwrap());
                        let high = u32::from_le_bytes(high.try_into().unwrap());
                        if low & 0x0ff00000 == 0x03000000
                            && high & 0x0ff00000 == 0x03400000
                            && (low ^ high) & 0xf000f000 == 0
                            && low >> 28 != 15
                            && (low >> 12) & 15 != 15
                            && insns.contains_key(&(a + 4))
                        {
                            let imm = |w: u32| (w & 0xfff) | ((w >> 4) & 0xf000);
                            let value = (imm(low) | (imm(high) << 16)) as u64;
                            if u.pointer(value) {
                                refs.push(Reference {
                                    site: a,
                                    target: u.canonical_address(value),
                                    kind: "movw-movt".into(),
                                    text: read_string(u, value),
                                });
                            } else {
                                constants.insert(value);
                            }
                        }
                    }
                    if let Some(t) = literal(a, op) {
                        refs.push(Reference {
                            site: a,
                            target: t,
                            kind: "literal-pool".into(),
                            text: None,
                        });
                        if word_ldr(m)
                            && let Some(w) = u.read(t, 4)
                        {
                            let v = u32::from_le_bytes(w.try_into().unwrap()) as u64;
                            if u.pointer(v) {
                                refs.push(Reference {
                                    site: a,
                                    target: u.canonical_address(v),
                                    kind: "data".into(),
                                    text: read_string(u, v),
                                });
                            } else {
                                constants.insert(v);
                            }
                        }
                    }
                    texts.push(normalize(a, m, op, u));
                } else {
                    if matches!(m.as_str(), "adr" | "adrp" | "ldr")
                        && !op.contains('[')
                        && let Some(c) = P.imm.captures(op)
                        && let Some(v) = number(&c[1])
                    {
                        refs.push(Reference {
                            site: a,
                            target: v as u64,
                            kind: m.clone(),
                            text: read_string(u, v as u64),
                        });
                    }
                    if m == "adrp"
                        && let Some((next_m, next_op)) = insns.get(&(a + 4))
                        && next_m == "add"
                        && let Some(c) = P.imm.captures(op)
                        && let Some(page) = number(&c[1])
                        && let Some(bytes) = u.read(a + 4, 4)
                    {
                        let word = u32::from_le_bytes(bytes.try_into().unwrap());
                        let rd = op.split(',').next().unwrap_or("");
                        let args: Vec<_> = next_op.split(',').map(str::trim).collect();
                        if args.len() >= 3 && args[1] == rd && word & 0xff800000 == 0x91000000 {
                            let offset = (((word >> 10) & 0xfff) as u64)
                                << if word & (1 << 22) != 0 { 12 } else { 0 };
                            let value = (page as u64).wrapping_add(offset);
                            refs.push(Reference {
                                site: a,
                                target: u.canonical_address(value),
                                kind: "adrp-add".into(),
                                text: read_string(u, value),
                            });
                        }
                    }
                    texts.push(normalize64(m, op));
                }
            }
            texts.push("|".into());
        }
        let signature = Sha256::digest(texts.join("\n").as_bytes()).into();
        let instructions = texts.iter().filter(|t| t.as_str() != "|").count();
        let ngrams = texts
            .windows(3)
            .filter(|w| w.iter().all(|t| t != "|"))
            .map(|w| hash64(w.join("\n").as_bytes()))
            .collect();
        let strings = refs
            .iter()
            .filter_map(|r| r.text.clone())
            .filter(|s| s.len() >= 4)
            .collect();
        let tokens = texts
            .into_iter()
            .map(|s| {
                let n = words.len() as u32;
                *dictionary.entry(s.clone()).or_insert_with(|| {
                    words.push(s);
                    n
                })
            })
            .collect();
        out.push(Function {
            header: FunctionHeader {
                key: FunctionKey {
                    firmware: u.layout.firmware.clone(),
                    unit: u.layout.unit.clone(),
                    address: f.address,
                },
                architecture: u.layout.architecture,
                names: f.names.clone(),
                annotations: f.annotations.clone(),
                ranges: f.ranges.clone(),
                signature,
                instructions,
                calls,
            },
            tokens,
            references: refs,
            constants,
            strings,
            ngrams,
        });
    }
    Ok((out, words))
}
fn read_string(u: &Unit, a: u64) -> Option<String> {
    let mut bytes = Vec::new();
    for p in a..a.checked_add(512)? {
        let c = *u.read(p, 1)?.first()?;
        if c == 0 {
            return (bytes.len() >= 4).then(|| String::from_utf8_lossy(&bytes).into_owned());
        }
        if !(32..127).contains(&c) {
            return None;
        }
        bytes.push(c);
    }
    None
}
