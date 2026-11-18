//! Emulator-only FujiMod loader for the shared-session boot.
//!
//! A mod is handed over as a built `manifest.json`, placed into guest RAM before the run, and its hook words are attached once
//! the stock UILIB-ready flag the manifest names reads the declared value, with every core
//! stopped between two run slices. Nothing here writes a firmware update or a card image: the
//! bytes exist only in the guest RAM of one run.
//!
//! The plan and checks run against a `Machine` so the preflight can be tested without an engine.
use crate::host::Memory;
use fujimod_emulator::board::Board;
use fujimod_emulator_cpu::{Access, Model, Register, arm, arm64};
use sha2::{Digest, Sha256};
use serde_json::{Value, json};
use std::path::Path;

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// The firmware identity and the supported full-boot hook ABI.
pub struct Target<'a> {
    pub id: &'a str,
    pub sha256: &'a str,
    pub model: Model,
}

/// A manifest number: an integer, or a string with a `0x` prefix.
pub(crate) fn number(v: &Value) -> Result<u64, String> {
    if let Some(n) = v.as_u64() {
        return Ok(n);
    }
    let s = v.as_str().ok_or("mod value must be a number or 0x hex string")?;
    let hex = s.strip_prefix("0x").ok_or_else(|| format!("{s:?} lacks a 0x prefix"))?;
    u64::from_str_radix(hex, 16).map_err(|e| format!("{s:?}: {e}"))
}
pub(crate) fn array<'a>(v: &'a Value, name: &str) -> Result<&'a [Value], String> {
    v[name]
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| format!("mod manifest lacks {name}"))
}
pub(crate) fn extent(v: &Value) -> Result<(u64, usize), String> {
    let address = number(&v["address"])?;
    let size = usize::try_from(number(&v["size"])?).map_err(|e| e.to_string())?;
    address
        .checked_add(size as u64)
        .ok_or("mod extent overflow")?;
    Ok((address, size))
}


/// Guest memory and translation inputs, as the loader needs them.
pub trait Machine: Memory {
    fn arm32_core_count(&self) -> usize { 0 }

    fn arm32_translate(&self, _: usize, _: u64, _: Access) -> Result<u64, String> {
        Err("ARM32 mod translation is unavailable".into())
    }

    fn arm32_mmu_enabled(&self, _: usize) -> Result<bool, String> {
        Err("ARM32 mod MMU status is unavailable".into())
    }

    /// Stage-1 translation inputs of the AArch64 application cores.
    fn translations(&self) -> Result<Vec<(usize, arm64::TranslationState)>, String> { Ok(Vec::new()) }
}

/// The shared board; `write_physical` tells every core's translator which code pages changed.
pub struct BoardMachine<'a>(pub &'a Board);

impl Memory for BoardMachine<'_> {
    fn read(&self, address: u64, bytes: &mut [u8]) -> Result<(), String> {
        self.0.inspect_physical(address, bytes)
    }
    fn write(&self, address: u64, bytes: &[u8]) -> Result<(), String> {
        self.0.write_physical(address, bytes)
    }
}

impl Machine for BoardMachine<'_> {
    fn arm32_core_count(&self) -> usize {
        self.0.instruction_counts().iter().enumerate()
            .filter(|(core, _)| self.0.cpu(*core).is_ok_and(|cpu| matches!(cpu.model(), Model::CortexA7 | Model::CortexR5F)))
            .count()
    }

    fn arm32_translate(&self, core: usize, address: u64, access: Access) -> Result<u64, String> {
        self.0.cpu(core)?.translate(address, access)
    }

    fn arm32_mmu_enabled(&self, core: usize) -> Result<bool, String> {
        let sctlr = arm::Coprocessor { cp: 15, opc1: 0, crn: 1, crm: 0, opc2: 0, secure: 0, wide: false };
        Ok(self.0.cpu(core)?.coprocessor(sctlr, None)? & 1 != 0)
    }

    fn translations(&self) -> Result<Vec<(usize, arm64::TranslationState)>, String> {
        let register = |core: usize, name: &str| {
            self.0
                .read_register(core, Register::Arm64System(arm64::system_register(name)?))
                .map_err(|error| format!("core {core} {name}: {error}"))
        };
        (0..self.0.instruction_counts().len())
            .filter(|core| self.0.cpu(*core).is_ok_and(|cpu| cpu.model() == Model::ArmV8A))
            .map(|core| {
                Ok((
                    core,
                    arm64::TranslationState {
                        sctlr: register(core, "SCTLR_EL1")?,
                        tcr: register(core, "TCR_EL1")?,
                        ttbr0: register(core, "TTBR0_EL1")?,
                        ttbr1: register(core, "TTBR1_EL1")?,
                        // EL1h: the firmware's tasks run at EL1 with PAN clear.
                        pstate: 0x5,
                    },
                ))
            })
            .collect()
    }
}

fn overlaps(a: (u64, u64), b: (u64, u64)) -> bool {
    a.0 < b.0.saturating_add(b.1) && b.0 < a.0.saturating_add(a.1)
}

fn read_word(machine: &dyn Machine, address: u64) -> Result<u32, String> {
    let mut word = [0u8; 4];
    machine.read(address, &mut word)?;
    Ok(u32::from_le_bytes(word))
}

#[derive(Debug)]
struct Word {
    address: u64,
    old: u32,
    new: u32,
    name: String,
}

/// One built mod: its manifest, the checked plan taken from it and the payload bytes.
#[derive(Debug)]
pub struct Loaded {
    manifest: Value,
    manifest_sha256: String,
    payloads: Vec<(u64, Vec<u8>)>,
    reservations: Vec<(u64, usize)>,
    immutable: Vec<(u64, usize, String)>,
    words: Vec<Word>,
    ready: (u64, u32),
    placed: bool,
    pending: bool,
    /// (core, SCTLR_EL1.M) as read when the words attached.
    mmu: Vec<(usize, bool)>,
    aarch64: bool,
}

/// Reads one built manifest: the single argument is a `manifest.json` or a directory holding one.
pub fn prepare(
    target: Target<'_>, specs: &[String],
    mut validate_original: impl FnMut(u64, u32) -> Result<(), String>,
) -> Result<Loaded, String> {
    if let [one] = specs {
        let path = Path::new(one);
        let manifest = if path.is_file() && path.file_name().is_some_and(|name| name == "manifest.json") {
            Some(path.to_path_buf())
        } else if path.join("manifest.json").is_file() {
            Some(path.join("manifest.json"))
        } else {
            None
        };
        if let Some(manifest) = manifest {
            let directory = manifest.parent().ok_or("manifest has no directory")?;
            return read_built(directory, target, &mut validate_original);
        }
    }
    Err("--mod takes one built manifest.json or a directory holding one".into())
}

fn read_built(
    directory: &Path, target: Target<'_>,
    validate_original: &mut impl FnMut(u64, u32) -> Result<(), String>,
) -> Result<Loaded, String> {
    let directory = directory.canonicalize().map_err(|error| error.to_string())?;
    let text = crate::read(&directory.join("manifest.json"), 16 << 20)?;
    let manifest: Value = serde_json::from_slice(&text).map_err(|error| error.to_string())?;
    let mut payloads = Vec::new();
    for region in array(&manifest, "regions")? {
        let file = region["file"].as_str().ok_or("mod region lacks a file name")?;
        let path = directory.join(file).canonicalize().map_err(|error| format!("{file}: {error}"))?;
        if Path::new(file).components().count() != 1 || path.parent() != Some(directory.as_path()) {
            return Err("mod region file name must be local".into());
        }
        let (address, size) = extent(region)?;
        let bytes = crate::read(&path, (size as u64).min(64 << 20))?;
        if bytes.len() != size || region["sha256"].as_str() != Some(sha256_hex(&bytes).as_str()) {
            return Err(format!("mod payload {file} differs from its manifest size or hash"));
        }
        payloads.push((address, bytes));
    }
    Loaded::from_manifest(manifest, sha256_hex(&text), payloads, target, validate_original)
}

impl Loaded {
    /// Checks identity, ABI and the geometry of the plan; no machine is involved.
    fn from_manifest(
        manifest: Value,
        manifest_sha256: String,
        payloads: Vec<(u64, Vec<u8>)>,
        target: Target<'_>,
        mut validate_original: impl FnMut(u64, u32) -> Result<(), String>,
    ) -> Result<Self, String> {
        let aarch64 = target.model == Model::ArmV8A;
        let (arch, abi, version) = match target.model {
            Model::ArmV8A => ("aarch64", "aapcs64-lp64-le", 2),
            Model::CortexR5F | Model::CortexA7 => ("arm32", "aapcs32-le", 1),
            Model::CortexM4 => return Err("mods require an application CPU".into()),
        };
        let identity_ok = manifest["mode"] == "emulator"
            && manifest["target"] == target.id
            && manifest["arch"] == arch
            && manifest["abi"] == abi
            && manifest["hook_record_version"] == version
            && manifest["firmware_sha256"] == target.sha256
            && manifest["live_safe"] == false;
        if !identity_ok {
            return Err("mod manifest is not an emulator build of this firmware (mode, target, ABI or hash differ)".into());
        }
        if !array(&manifest, "inits")?.is_empty() {
            return Err("mod declares an init; full-boot mod init boundaries must be provided by stock hook words".into());
        }
        let ready = &manifest["load_ready"];
        if !ready.is_object() {
            return Err("mod manifest lacks load_ready; hook words cannot be attached safely".into());
        }
        let ready = (
            number(&ready["address"])?,
            u32::try_from(number(&ready["value"])?).map_err(|_| "load_ready value exceeds u32")?,
        );
        let reservations = array(&manifest, "reservations")?
            .iter()
            .map(extent)
            .collect::<Result<Vec<_>, _>>()?;
        for (index, a) in reservations.iter().enumerate() {
            if reservations[..index]
                .iter()
                .any(|b| overlaps((a.0, a.1 as u64), (b.0, b.1 as u64)))
            {
                return Err("mod reservations overlap each other".into());
            }
        }
        for (address, bytes) in &payloads {
            let end = address + bytes.len() as u64;
            if !reservations
                .iter()
                .any(|(base, size)| *base <= *address && end <= base + *size as u64)
            {
                return Err(format!("mod payload at {address:#x} is outside its reservations"));
            }
        }
        let mut words: Vec<Word> = Vec::new();
        for word in array(&manifest, "words")? {
            let address = number(&word["address"])?;
            let old = u32::try_from(number(&word["old"])?).map_err(|_| "mod old word exceeds u32")?;
            let new = u32::try_from(number(&word["new"])?).map_err(|_| "mod new word exceeds u32")?;
            if address % 4 != 0 || words.iter().any(|known| known.address == address) {
                return Err(format!("mod word {address:#x} is misaligned or repeated"));
            }
            if reservations
                .iter()
                .any(|(base, size)| overlaps((address, 4), (*base, *size as u64)))
            {
                return Err(format!("mod word {address:#x} lies inside a mod reservation"));
            }
            validate_original(address, old)?;
            words.push(Word {
                address,
                old,
                new,
                name: word["name"].as_str().unwrap_or("").to_owned(),
            });
        }
        if words.is_empty() {
            return Err("mod has no hook words".into());
        }
        let immutable = array(&manifest, "immutable")?
            .iter()
            .map(|range| {
                let (address, size) = extent(range)?;
                let digest = range["sha256"].as_str().ok_or("immutable range lacks sha256")?;
                Ok((address, size, digest.to_owned()))
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(Self {
            manifest,
            manifest_sha256,
            payloads,
            reservations,
            immutable,
            words,
            ready,
            placed: false,
            pending: true,
            mmu: Vec::new(),
            aarch64,
        })
    }

    /// Before the run: the reservations must be free, then the payload goes in and the words wait.
    pub fn place(
        &mut self,
        machine: &dyn Machine,
        excluded: &[(u64, u64)],
        protected: &[u64],
    ) -> Result<(), String> {
        if self.placed {
            return Err("mod is already placed".into());
        }
        for &(base, size) in &self.reservations {
            let region = (base, size as u64);
            if excluded.iter().any(|other| overlaps(region, *other)) {
                return Err(format!("mod reservation {base:#x}+{size:#x} intersects startup memory"));
            }
            if !is_zero(machine, base, size)? {
                return Err(format!("mod reservation {base:#x}+{size:#x} is occupied"));
            }
        }
        for word in &self.words {
            if protected.contains(&word.address) {
                return Err(format!("mod site {:#x} is a discovery callee stub", word.address));
            }
        }
        for (address, bytes) in &self.payloads {
            machine.write(*address, bytes)?;
        }
        self.placed = true;
        eprintln!(
            "FUJIMOD mod placed: build={} reservations={:x?} words={} attach when {:#x}=={}",
            self.manifest["build_id"], self.reservations, self.words.len(), self.ready.0, self.ready.1
        );
        Ok(())
    }

    /// Delay placement until an explicitly declared guest readiness word is set.
    pub fn advance(&mut self, machine: &dyn Machine, now: u64, protected: &[u64]) -> Result<(), String> {
        if !self.placed {
            if read_word(machine, self.ready.0)? != self.ready.1 {
                return Ok(());
            }
            self.permissions(machine)?;
            for word in &self.words {
                if read_word(machine, word.address)? != word.old {
                    return Err(format!("mod site {:#x} differs from its original instruction", word.address));
                }
            }
            self.place(machine, &[], protected)?;
        }
        self.attach_if_ready(machine, now)
    }

    /// Between run slices: once the ready flag holds, check and attach the hook words once. A
    /// failed check or word write refuses the whole attach and leaves every word at its stock
    /// value (words already written are restored). The payload placed earlier stays in guest RAM
    /// on refusal: the caller ends the run on the error.
    pub fn attach_if_ready(&mut self, machine: &dyn Machine, now: u64) -> Result<(), String> {
        if !self.placed {
            return Err("mod is not placed".into());
        }
        if !self.pending || read_word(machine, self.ready.0)? != self.ready.1 {
            return Ok(());
        }
        for (address, size, digest) in &self.immutable {
            let mut bytes = vec![0u8; *size];
            machine.read(*address, &mut bytes)?;
            if sha256_hex(&bytes) != *digest {
                return Err(format!("mod immutable code differs at {address:#x}"));
            }
        }
        for word in &self.words {
            let held = read_word(machine, word.address)?;
            if held != word.old {
                return Err(format!(
                    "mod site {:#x} holds {held:#010x}, expected stock {:#010x}",
                    word.address, word.old
                ));
            }
        }
        for &(base, size) in &self.reservations {
            self.unused_is_zero(machine, base, size)?;
        }
        let mmu = self.permissions(machine)?;
        for (index, word) in self.words.iter().enumerate() {
            if let Err(error) = machine.write(word.address, &word.new.to_le_bytes()) {
                let unrestored = self.words[..index]
                    .iter()
                    .filter(|done| machine.write(done.address, &done.old.to_le_bytes()).is_err())
                    .count();
                return Err(format!(
                    "mod word {:#x}: {error} ({unrestored} earlier words could not be restored)",
                    word.address
                ));
            }
        }
        let mmu_text: Vec<String> = mmu
            .iter()
            .map(|(core, on)| format!("{core}:{}", if *on { "on" } else { "off" }))
            .collect();
        for word in &self.words {
            eprintln!(
                "FUJIMOD mod attached at {now}: {:#x} {:#010x} -> {:#010x} {} mmu={}",
                word.address,
                word.old,
                word.new,
                word.name,
                mmu_text.join(",")
            );
        }
        self.mmu = mmu;
        self.pending = false;
        Ok(())
    }

    // Bytes of a reservation outside the payload regions must still be zero: the stock code has
    // not taken the arena since placement.
    fn unused_is_zero(&self, machine: &dyn Machine, base: u64, size: usize) -> Result<(), String> {
        let mut bytes = vec![0u8; size];
        machine.read(base, &mut bytes)?;
        for (index, byte) in bytes.iter().enumerate() {
            let at = base + index as u64;
            if *byte != 0
                && !self
                    .payloads
                    .iter()
                    .any(|(start, data)| *start <= at && at < start + data.len() as u64)
            {
                return Err(format!("mod reservation occupied at {at:#x}"));
            }
        }
        Ok(())
    }

    // Every core must map each reservation page to itself for fetch and for write, in the stage-1
    // tables as they stand when the words attach. A core whose MMU is off (SCTLR_EL1.M = 0, as
    // before it starts) passes trivially, so the result records each core's M bit.
    fn permissions(&self, machine: &dyn Machine) -> Result<Vec<(usize, bool)>, String> {
        if !self.aarch64 {
            let cores = machine.arm32_core_count();
            if cores == 0 {
                return Err("no ARM32 application core can validate mod placement".into());
            }
            let mut mmu = Vec::with_capacity(cores);
            for core in 0..cores {
                let enabled = machine.arm32_mmu_enabled(core)?;
                mmu.push((core, enabled));
                for &(base, size) in &self.reservations {
                    let end = base + size as u64;
                    for page in (base & !4095..end).step_by(4096) {
                        let address = page.max(base);
                        for access in [Access::Fetch, Access::Write] {
                            if machine.arm32_translate(core, address, access)? != address {
                                return Err(format!("mod reservation at {address:#x} is not identity mapped on core {core}"));
                            }
                        }
                    }
                }
            }
            return Ok(mmu);
        }
        let mut mmu = Vec::new();
        for (core, state) in machine.translations()? {
            mmu.push((core, state.sctlr & 1 == 1));
            for &(base, size) in &self.reservations {
                for page in (0..size as u64).step_by(4096) {
                    let address = base + page;
                    for access in [Access::Fetch, Access::Write] {
                        let physical = state
                            .translate(address, access, |table| {
                                let mut entry = [0u8; 8];
                                machine.read(table, &mut entry)?;
                                Ok(u64::from_le_bytes(entry))
                            })
                            .map_err(|error| format!("mod reservation {address:#x} on core {core}: {error}"))?;
                        if physical != address {
                            return Err(format!(
                                "mod reservation {address:#x} on core {core} maps to {physical:#x}, not itself"
                            ));
                        }
                    }
                }
            }
        }
        Ok(mmu)
    }

    pub(crate) fn attached(&self) -> bool {
        self.placed && !self.pending
    }

    /// Mod status for the run record: whether the words attached and every 4- or 8-byte data
    /// symbol of the manifest (the counters) as the guest holds it now.
    pub fn report(&self, machine: &dyn Machine) -> Value {
        let mut counters = serde_json::Map::new();
        for symbol in self.manifest["symbols"].as_array().into_iter().flatten() {
            let size = symbol["size"].as_u64().unwrap_or(0);
            if symbol["type"] != "STT_OBJECT" || !matches!(size, 4 | 8) {
                continue;
            }
            if let (Some(name), Ok(address)) = (symbol["name"].as_str(), number(&symbol["address"])) {
                let mut bytes = [0u8; 8];
                if machine.read(address, &mut bytes[..size as usize]).is_ok() {
                    counters.insert(name.to_owned(), json!(u64::from_le_bytes(bytes)));
                }
            }
        }
        let flag = read_word(machine, self.ready.0).ok();
        json!({
            "build_id": self.manifest["build_id"],
            "manifest_sha256": self.manifest_sha256,
            "placed": self.placed,
            "attached": self.attached(),
            "mmu": self.mmu.iter().map(|(core, on)| (format!("core{core}"), json!(on))).collect::<serde_json::Map<_, _>>(),
            "ready_flag": flag,
            "counters": counters,
        })
    }
}

fn is_zero(machine: &dyn Machine, base: u64, size: usize) -> Result<bool, String> {
    // Read in 1 MiB pieces so a 2 MiB arena does not need one large buffer per call.
    let mut buffer = vec![0u8; size.min(1 << 20)];
    let mut done = 0usize;
    while done < size {
        let length = buffer.len().min(size - done);
        machine.read(base + done as u64, &mut buffer[..length])?;
        if buffer[..length].iter().any(|byte| *byte != 0) {
            return Ok(false);
        }
        done += length;
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    /// Sparse little-endian guest memory; the MMU is off on the one core. Writes to `fail` error.
    #[derive(Default)]
    struct Mock {
        memory: RefCell<BTreeMap<u64, u8>>,
        fail: Option<u64>,
    }

    impl Mock {
        fn set(&self, address: u64, bytes: &[u8]) {
            for (index, byte) in bytes.iter().enumerate() {
                self.memory.borrow_mut().insert(address + index as u64, *byte);
            }
        }
        fn get(&self, address: u64, length: usize) -> Vec<u8> {
            (0..length as u64)
                .map(|offset| self.memory.borrow().get(&(address + offset)).copied().unwrap_or(0))
                .collect()
        }
    }

    impl Memory for Mock {
        fn read(&self, address: u64, bytes: &mut [u8]) -> Result<(), String> {
            bytes.copy_from_slice(&self.get(address, bytes.len()));
            Ok(())
        }
        fn write(&self, address: u64, bytes: &[u8]) -> Result<(), String> {
            if self.fail == Some(address) {
                return Err("injected".into());
            }
            self.set(address, bytes);
            Ok(())
        }
    }

    impl Machine for Mock {
        fn translations(&self) -> Result<Vec<(usize, arm64::TranslationState)>, String> {
            Ok(vec![(0, arm64::TranslationState { sctlr: 0, tcr: 0, ttbr0: 0, ttbr1: 0, pstate: 5 })])
        }
    }

    const SITE: u64 = 0x0326_d8f4;
    const SECOND: u64 = SITE + 4;
    const READY: u64 = 0x0599_daf8;
    const OLD: u32 = 0x97ff_f605;
    const NEW: u32 = 0x973e_49c3;

    fn payload() -> Vec<u8> {
        (0..40u8).collect()
    }

    fn firmware(sha: &str) -> crate::xp5::firmware::Firmware {
        let stock = |address| (address, OLD.to_le_bytes().to_vec());
        crate::xp5::firmware::Firmware::for_tests("x100vi-131", sha, vec![stock(SITE), stock(SECOND)])
    }

    fn manifest() -> Value {
        json!({
            "mode": "emulator", "target": "x100vi-131", "arch": "aarch64", "abi": "aapcs64-lp64-le",
            "hook_record_version": 2, "firmware_sha256": "ab", "live_safe": false,
            "reservations": [{"address": "0x200000", "size": 0x200000}, {"address": "0x74000", "size": 0xc000}],
            "regions": [{"address": "0x200000", "size": 40, "file": "region.bin", "sha256": sha256_hex(&payload())}],
            "words": [
                {"address": "0x326d8f4", "old": "0x97fff605", "new": "0x973e49c3", "name": "hook"},
                {"address": "0x326d8f8", "old": "0x97fff605", "new": "0x973e49c3", "name": "second"},
            ],
            "symbols": [{"name": "counter", "address": "0x200020", "size": 8, "type": "STT_OBJECT"}],
            "immutable": [{"address": "0x200000", "size": 28, "sha256": sha256_hex(&payload()[..28])}],
            "inits": [],
            "load_ready": {"address": "0x599daf8", "value": 1},
            "build_id": "test",
        })
    }

    fn build(value: Value) -> Result<Loaded, String> {
        let firmware = firmware("ab");
        Loaded::from_manifest(value, "digest".into(), vec![(0x20_0000, payload())],
            Target { id: &firmware.id, sha256: &firmware.source_sha256, model: Model::ArmV8A },
            |address, old| firmware.expect_word(address, old, "mod patch site"))
    }

    fn loaded() -> Loaded {
        build(manifest()).unwrap()
    }

    /// A placed mod and a machine whose stock words are in place and whose ready flag holds.
    fn placed(fail: Option<u64>) -> (Loaded, Mock) {
        let machine = Mock { fail, ..Mock::default() };
        for site in [SITE, SECOND] {
            machine.set(site, &OLD.to_le_bytes());
        }
        let mut mod_ = loaded();
        mod_.place(&machine, &[], &[]).unwrap();
        machine.set(READY, &1u32.to_le_bytes());
        (mod_, machine)
    }

    #[test]
    fn identity_is_refused_when_it_differs() {
        assert!(build(manifest()).is_ok());
        for (field, value) in [
            ("mode", json!("runtime")),
            ("target", json!("xm5-130")),
            ("arch", json!("arm")),
            ("abi", json!("aapcs32")),
            ("hook_record_version", json!(1)),
            ("firmware_sha256", json!("cd")),
            ("live_safe", json!(true)),
        ] {
            let mut m = manifest();
            m[field] = value;
            assert!(build(m).is_err(), "{field} should be refused");
        }
        let mut stale = manifest();
        stale["words"][0]["old"] = json!("0x97fff606");
        assert!(build(stale).unwrap_err().contains("expected"));
        let mut bare = manifest();
        bare["load_ready"]["address"] = json!("599daf8");
        assert!(build(bare).unwrap_err().contains("0x prefix"));
    }

    #[test]
    fn a_refused_attach_writes_nothing() {
        // A reservation byte taken since placement: every word check passes, the arena check fails.
        let (mut m, machine) = placed(None);
        machine.set(0x20_0100, &[1]);
        assert!(m.attach_if_ready(&machine, 1).unwrap_err().contains("occupied"));
        assert_eq!(machine.get(SITE, 4), OLD.to_le_bytes());
        // The second word's write fails: the first goes back to stock.
        let (mut m, machine) = placed(Some(SECOND));
        assert!(m.attach_if_ready(&machine, 1).is_err());
        assert_eq!(machine.get(SITE, 4), OLD.to_le_bytes());
        assert_eq!(machine.get(SECOND, 4), OLD.to_le_bytes());
        assert!(!m.attached());
        assert_eq!(m.report(&machine)["attached"], false);
    }

    #[test]
    fn words_wait_for_the_ready_flag_and_attach_once() {
        let (mut m, machine) = placed(None);
        machine.set(READY, &0u32.to_le_bytes());
        m.attach_if_ready(&machine, 1).unwrap();
        assert!(!m.attached());
        assert_eq!(machine.get(SITE, 4), OLD.to_le_bytes());
        machine.set(READY, &1u32.to_le_bytes());
        m.attach_if_ready(&machine, 2).unwrap();
        assert!(m.attached());
        assert_eq!(machine.get(SITE, 4), NEW.to_le_bytes());
        assert_eq!(machine.get(SECOND, 4), NEW.to_le_bytes());
        // A second call neither checks nor writes again.
        machine.set(SITE, &0u32.to_le_bytes());
        m.attach_if_ready(&machine, 3).unwrap();
        assert_eq!(machine.get(SITE, 4), 0u32.to_le_bytes());
        let report = m.report(&machine);
        assert_eq!(report["attached"], true);
        assert_eq!(report["mmu"]["core0"], false);
    }
}
