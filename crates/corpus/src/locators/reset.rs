//! Reset-vector and handoff diagnostics. A reset allocation or an SRAM setup
//! size is not promoted to a processor ROM extent.
use crate::Lookup;
use capstone::prelude::*;
use serde::Serialize;
use sha2::{Digest, Sha256};

#[derive(Debug, Serialize)]
pub struct ResetAudit {
    pub reason: String,
    pub vectors: Vec<ResetVector>,
}
#[derive(Debug, Serialize)]
pub struct ResetVector {
    pub update_offset: usize,
    pub address: u64,
    pub entry: u64,
    pub prefix_size: usize,
    pub prefix_sha256: String,
    pub calls: Vec<ResetCall>,
}
#[derive(Debug, Serialize)]
pub struct ResetCall {
    pub site: u64,
    pub target: u64,
    /// Names are diagnostic nominations, not proof of a ROM-copy contract.
    pub indexed_names: Vec<String>,
}
fn word(bytes: &[u8], p: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(p..p + 4)?.try_into().ok()?))
}
/// Inspect every complete low-RAM reset-vector table in the update prefix and
/// follow its reset literal to a fresh 512-byte instruction window. Each direct
/// call is decoded and cross-referenced with the index. These wrappers hand off
/// to mutable RAM/flash code; none is a processor ROM boundary declaration.
pub fn boot_rom_audit<L: Lookup>(
    index: &L,
    firmware: &str,
    processor: &str,
    inverted: &[u8],
) -> Result<ResetAudit, String> {
    let decoder = Capstone::new()
        .arm()
        .mode(arch::arm::ArchMode::Arm)
        .build()
        .map_err(|e| e.to_string())?;
    let pattern: Vec<u8> = (0..8).flat_map(|_| 0xe59ff018u32.to_le_bytes()).collect();
    let prefix = inverted;
    let mut vectors = vec![];
    for (word_index, word_bytes) in prefix.as_chunks::<4>().0.iter().enumerate() {
        if word_bytes != &0xe59ff018u32.to_le_bytes() {
            continue;
        }
        let offset = word_index * 4;
        if prefix.get(offset..offset + 32) != Some(pattern.as_slice()) {
            continue;
        }
        let Some(targets) = (0..8)
            .map(|p| word(inverted, offset + 32 + p * 4))
            .collect::<Option<Vec<_>>>()
        else {
            continue;
        };
        let address = u64::from(*targets.iter().min().unwrap() & !0xffff);
        let entry = u64::from(targets[0]);
        if address >= 0x40000 || entry < address || entry >= address + 0x10000 {
            continue;
        }
        let body_offset = offset + (entry - address) as usize;
        let Some(bytes) = inverted.get(body_offset..body_offset + 512) else {
            continue;
        };
        let mut calls = vec![];
        // Decode one word at a time: a literal pool ends a linear disassembly,
        // but it must not hide the rest of the reset/handoff diagnostic window.
        for (p, bytes) in bytes.as_chunks::<4>().0.iter().enumerate() {
            let code = decoder
                .disasm_all(bytes, entry + p as u64 * 4)
                .map_err(|e| e.to_string())?;
            let Some(ins) = code.iter().next() else {
                continue;
            };
            if ins.mnemonic() != Some("bl") {
                continue;
            }
            let operand = ins
                .op_str()
                .unwrap_or("")
                .trim_start_matches('#')
                .trim_start_matches("0x");
            let Ok(target) = u64::from_str_radix(operand, 16) else {
                continue;
            };
            let names = index
                .find(None, Some(target), Some(firmware), Some("main"))
                .map_err(|e| e.to_string())?
                .into_iter()
                .flat_map(|f| f.names)
                .collect();
            calls.push(ResetCall {
                site: ins.address(),
                target,
                indexed_names: names,
            });
        }
        vectors.push(ResetVector {
            update_offset: offset,
            address,
            entry,
            prefix_size: bytes.len(),
            prefix_sha256: format!("{:x}", Sha256::digest(bytes)),
            calls,
        });
    }
    let reason = match processor {
        "xp4" => {
            "Reset and SRAM setup hand off to the flash-backed RAM loader. Setup-size and loader-copy fields bound mutable code, not boot ROM; no complete ROM source/length contract is established."
        }
        "xpp" => {
            "Reset and kernel handoff bound low-RAM loader images. High peripheral register constants and incoming ROM state do not establish a complete boot ROM extent."
        }
        "exr" => {
            "The duplicated low-RAM loader vectors and kernel handoff declare loader allocations; controller command values with high bits set are not ROM addresses. No ROM base/length contract is established."
        }
        "exrpro" => {
            "The duplicated low-RAM reset and kernel handoff code do not declare a complete ROM copy, signature boundary or ROM length. The EXR II ROM extent is not inherited by this processor."
        }
        _ => "Reset-vector inspection does not establish a complete readable processor ROM extent.",
    };
    Ok(ResetAudit {
        reason: reason.into(),
        vectors,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Index;
    #[test]
    fn reset_allocations_do_not_become_rom_extents() {
        let mut bytes = vec![0; 0x1000];
        for n in 0..8 {
            bytes[n * 4..n * 4 + 4].copy_from_slice(&0xe59ff018u32.to_le_bytes());
            bytes[32 + n * 4..36 + n * 4].copy_from_slice(&(0x100u32).to_le_bytes());
        }
        let audit = boot_rom_audit(&Index::new(), "sample", "xp4", &bytes).unwrap();
        assert_eq!(audit.vectors.len(), 1);
        assert_eq!(audit.vectors[0].entry, 0x100);
        assert!(audit.reason.contains("not boot ROM"));
        assert!(super::super::boot_rom("xp4").is_err());
    }
}
