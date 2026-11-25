//! Fail-closed, flat-address AArch64 routine policy. All memory transfers are
//! checked before execution; write callbacks are observations, never vetoes.
use super::{Options, initialize};
use crate::board::{Board, scalar};
use fujimod_emulator_cpu::{Access, Cpu, Hook, HookRange, Model, Register, arm64};
use serde::{Deserialize, Serialize};
use std::{
    cell::{Cell, RefCell},
    ffi::c_void,
    rc::{Rc, Weak},
};
type Span = (u64, u64);

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Stub {
    pub address: u64,
    pub returns: u64,
    pub label: String,
    #[serde(default)]
    pub dynamic: bool,
    #[serde(default)]
    pub reads: Vec<(u8, ReadSize)>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum ReadSize {
    Fixed(u64),
    Argument { argument: u8 },
    Indirect { indirect: u64, size: u64 },
}
fn default_observer_limit() -> usize {
    4096
}
fn default_transaction_limit() -> usize {
    4096
}
#[derive(Serialize)]
pub struct RegisterObservation {
    pub pc: u64,
    pub registers: std::collections::BTreeMap<String, u64>,
}
/// Declared AArch64 exception level for the checked routine entry PSTATE.
///
/// EL1 and EL3 are admitted; EL3 entries may additionally read and write the
/// EL3 system registers and `eret` to a declared, AArch64 target. EL2 remains
/// reserved and refuses before any instruction runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EntryLevel {
    El1,
    El2,
    El3,
}
impl EntryLevel {
    /// PSTATE `M[3:0]` encoding for this level in its high (SP_ELx) stack mode.
    pub fn m4(self) -> u64 {
        match self {
            EntryLevel::El1 => 0b0101,
            EntryLevel::El2 => 0b1001,
            EntryLevel::El3 => 0b1101,
        }
    }
    /// Numeric exception level (1, 2, or 3).
    pub fn level(self) -> u64 {
        match self {
            EntryLevel::El1 => 1,
            EntryLevel::El2 => 2,
            EntryLevel::El3 => 3,
        }
    }
    /// Complete initial PSTATE value: `0x3c0 | m4()`, the level's high-stack mode.
    pub fn pstate(self) -> u64 {
        0x3c0 | self.m4()
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckedOptions {
    #[serde(default)]
    pub observe_addresses: Vec<u64>,
    #[serde(default = "default_observer_limit")]
    pub observer_limit: usize,
    /// Named read-only model registers and their expected values.
    #[serde(default)]
    pub system_reads: Vec<(String, u64)>,
    /// Current values of registers the CPU core does not implement, such as
    /// the A53-only `S3_1_C15_C2_1`. A declaration is the model: the guard
    /// retires each read and write without executing it, returns the declared
    /// value for reads, and updates it on writes. Every observation records
    /// whether the value came from the model or from this declaration.
    #[serde(default)]
    pub system_values: Vec<(String, u64)>,
    /// Named system registers the routine may read or write. Writes require
    /// membership here; reads may also be declared through `system_reads`.
    #[serde(default)]
    pub system_registers: Vec<String>,
    #[serde(default)]
    pub stop_addresses: Vec<u64>,
    pub entry: u64,
    /// Declared entry exception level. Required; an absent declaration is
    /// refused with an explicit message rather than serde's missing-field error.
    #[serde(default)]
    pub entry_level: Option<EntryLevel>,
    /// Explicit initial PSTATE. When absent, the declared level's high-stack
    /// PSTATE is used; when present, its M[3:0] must match the declared level.
    #[serde(default)]
    pub pstate: Option<u64>,
    pub arguments: Vec<u64>,
    pub return_address: u64,
    pub stack_pointer: u64,
    pub stack: Span,
    pub max_instructions: u64,
    pub quantum: usize,
    pub executable: Vec<Span>,
    pub readable: Vec<Span>,
    pub writable: Vec<Span>,
    /// Identity-mapped device windows whose loads and stores dispatch through
    /// the board's device bus. Empty for a device-free board; every other
    /// MMIO address stays refused before execution.
    #[serde(default)]
    pub devices: Vec<Span>,
    /// Bound on recorded bus transactions inside `devices`.
    #[serde(default = "default_transaction_limit")]
    pub transaction_limit: usize,
    pub store_limit: usize,
    pub stubs: Vec<Stub>,
    /// Explicit initial values for x0..x30; omitted registers use zero.
    pub registers: Vec<(u8, u64)>,
    /// Explicit expected exits for a partial-frame fragment, overriding only
    /// named callee-save expectations. Ordinary calls preserve all of them.
    #[serde(default)]
    pub fragment_exit: std::collections::BTreeMap<String, u64>,
}
#[derive(Serialize)]
pub struct Store {
    pub pc: u64,
    pub address: u64,
    pub size: u32,
    pub value: u64,
}
#[derive(Serialize)]
pub struct StubHit {
    pub address: u64,
    pub arguments: Vec<u64>,
    pub returns: u64,
    pub writes: Vec<WriteReceipt>,
    pub reads: Vec<WriteReceipt>,
}
#[derive(Serialize)]
pub struct WriteReceipt {
    pub address: u64,
    pub size: usize,
}
pub type StubResponse = (u64, Vec<(u64, Vec<u8>)>);
pub type StubSnapshot = Vec<(u64, Vec<u8>)>;
#[derive(Serialize)]
pub struct SystemReadObservation {
    pub pc: u64,
    pub name: String,
    pub encoding: u16,
    pub destination: u8,
    pub value: u64,
    /// Whether the value came from the CPU model or from a caller declaration.
    pub source: SystemValueSource,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemValueSource {
    Model,
    Declared,
}
#[derive(Serialize)]
pub struct SystemWriteObservation {
    pub pc: u64,
    pub name: String,
    pub value: u64,
}
#[derive(Default)]
struct Observed {
    failure: Option<String>,
    returned: bool,
    stopped_at: Option<u64>,
    stub: Option<usize>,
    stores: Vec<Store>,
    hits: Vec<StubHit>,
    observations: Vec<RegisterObservation>,
    system_reads: Vec<SystemReadObservation>,
    system_writes: Vec<SystemWriteObservation>,
    pending: Option<SystemReadObservation>,
    /// System-register writes the guest made that must reach the backend from
    /// outside a hook: the backend ignores table-register writes issued while
    /// its own translation is being set up.
    deferred: Vec<(u16, u64)>,
    /// Single-threaded exclusive monitor: the most recent exclusive load's
    /// (address, byte width). Only one core runs under this harness, so a
    /// single reservation is the entire architectural monitor.
    exclusive: Option<(u64, u8)>,
}
#[derive(Serialize)]
pub struct CheckedResult {
    pub returned: bool,
    pub stopped_at: Option<u64>,
    pub failure: Option<String>,
    pub return_value: Option<u64>,
    pub registers: std::collections::BTreeMap<String, u64>,
    pub initial_registers: std::collections::BTreeMap<String, u64>,
    pub instructions: u64,
    pub virtual_ticks: u64,
    pub stores: Vec<Store>,
    pub stub_hits: Vec<StubHit>,
    pub observations: Vec<RegisterObservation>,
    pub system_read_model_values: Vec<(String, u64)>,
    pub system_read_observations: Vec<SystemReadObservation>,
    pub system_writes: Vec<SystemWriteObservation>,
    /// Bus transactions inside the declared device windows, in order.
    pub device_transactions: Vec<crate::board::bus::Transaction>,
    pub device_transactions_omitted: u64,
}
struct Guard {
    cpu: Weak<dyn Cpu>,
    pause: Rc<Cell<bool>>,
    options: CheckedOptions,
    /// Identity-mapped MMIO windows served by the board's interrupt
    /// controller. Transfers into these windows are admitted even though
    /// they are not declared RAM; the board serves them on the MMIO path.
    interrupt_windows: Vec<Span>,
    /// Declared values for registers the CPU core does not implement, keyed by
    /// encoding. Reads return the stored value; writes replace it.
    system_state: RefCell<std::collections::BTreeMap<u16, u64>>,
    observed: RefCell<Observed>,
}
fn contains(spans: &[Span], address: u64, size: u64) -> bool {
    address
        .checked_add(size)
        .is_some_and(|end| spans.iter().any(|&(a, b)| a <= address && end <= b))
}
fn overlaps(a: Span, b: Span) -> bool {
    a.0 < b.1 && b.0 < a.1
}
fn validate_ranges(ranges: &[Span], backing: &[Span]) -> Result<(), String> {
    if ranges.len() > 4096
        || ranges
            .iter()
            .any(|&(a, b)| a >= b || !contains(backing, a, b - a))
    {
        return Err("Routine ranges must be nonempty spans in identity-mapped RAM".into());
    }
    Ok(())
}
fn admitted_system_register(name: &str) -> Option<u16> {
    Some(match name {
        "MIDR_EL1" => 0xc000,
        "MPIDR_EL1" => 0xc005,
        "CurrentEL" => 0xc212,
        "SCTLR_EL1" => 0xc080,
        "TCR_EL1" => 0xc102,
        "TTBR0_EL1" => 0xc100,
        "TTBR1_EL1" => 0xc101,
        "MAIR_EL1" => 0xc510,
        "VBAR_EL1" => 0xc600,
        "VBAR_EL2" => 0xe600,
        "VBAR_EL3" => 0xf600,
        "SPSR_EL3" => 0xf200,
        "ELR_EL3" => 0xf201,
        "CTR_EL0" => 0xd801,
        "CLIDR_EL1" => 0xc801,
        "CSSELR_EL1" => 0xd000,
        "CCSIDR_EL1" => 0xc800,
        "SPSel" => 0xc210,
        "DAIF" => 0xda11,
        _ => return parse_implementation_defined_name(name),
    })
}
/// Encoding for a systematic implementation-defined name such as
/// `S3_1_C15_C2_1`, which is how the camera's Cortex-A53 register
/// `CPUACTLR_EL1` appears in disassembly. These encodings carry no
/// architectural meaning, so a caller must declare the value it models.
fn parse_implementation_defined_name(name: &str) -> Option<u16> {
    let mut parts = name.strip_prefix('S')?.split('_');
    let op0 = parts.next()?.parse::<u16>().ok()?;
    let op1 = parts.next()?.parse::<u16>().ok()?;
    let crn = parts.next()?.strip_prefix('C')?.parse::<u16>().ok()?;
    let crm = parts.next()?.strip_prefix('C')?.parse::<u16>().ok()?;
    let op2 = parts.next()?.parse::<u16>().ok()?;
    if parts.next().is_some() {
        return None;
    }
    let encoding = (op0 << 14) | (op1 << 11) | (crn << 7) | (crm << 3) | op2;
    implementation_defined_name(encoding)?;
    Some(encoding)
}
/// Refuse the write that switches address translation on.
///
/// Every fetch and access after `SCTLR_EL1.M` is set has to walk page tables,
/// and this harness has no translation model: continuing would keep fetching
/// at flat addresses while claiming a checked execution. The condition is a
/// guest register value, not a PC or a firmware global, so any routine that
/// enables the MMU meets the same refusal at its own `MSR`.
/// Canonical name for an admitted system-register encoding, used by the MSR
/// and MRS paths to resolve the architectural register a word names.
fn system_register_name(encoding: u16) -> Option<String> {
    let name = match encoding {
        0xc000 => "MIDR_EL1",
        0xc005 => "MPIDR_EL1",
        0xc212 => "CurrentEL",
        0xc080 => "SCTLR_EL1",
        0xc102 => "TCR_EL1",
        0xc100 => "TTBR0_EL1",
        0xc101 => "TTBR1_EL1",
        0xc510 => "MAIR_EL1",
        0xc600 => "VBAR_EL1",
        0xe600 => "VBAR_EL2",
        0xf600 => "VBAR_EL3",
        0xf200 => "SPSR_EL3",
        0xf201 => "ELR_EL3",
        0xd801 => "CTR_EL0",
        0xc801 => "CLIDR_EL1",
        0xd000 => "CSSELR_EL1",
        0xc800 => "CCSIDR_EL1",
        0xc210 => "SPSel",
        0xda11 => "DAIF",
        _ => return implementation_defined_name(encoding),
    };
    Some(name.to_string())
}
/// Systematic name for the implementation-defined system-register space this
/// firmware uses: `S3_<op1>_C<crn>_C<crm>_<op2>`. A53-only registers such as
/// `CPUACTLR_EL1` (`S3_1_C15_C2_1`) live there and have no architectural value,
/// so naming them only makes them declarable; nothing reads without a
/// declaration. `S3_6_*` and `S3_7_*` are reserved for implementations, and
/// `CRn=15` with `op1 != 0` is implementation defined at every exception level.
fn implementation_defined_name(encoding: u16) -> Option<String> {
    let op0 = encoding >> 14;
    let op1 = (encoding >> 11) & 7;
    let crn = (encoding >> 7) & 15;
    let crm = (encoding >> 3) & 15;
    let op2 = encoding & 7;
    if op0 != 3 || op1 == 0 || (op1 < 6 && crn != 15) {
        return None;
    }
    Some(format!("S{op0}_{op1}_C{crn}_C{crm}_{op2}"))
}
/// True for an architectural cache or TLB maintenance operation.
/// The reserved encodings must not be admitted as TLB invalidations.
fn tlbi_selects_a_valid_operand(op1: u32, crm: u32, op2: u32) -> bool {
    let mask = match (op1, crm) {
        // EL1: one, inner-shareable or outer-shareable scope and ranges.
        (0, 1 | 3 | 7) => 0b1010_1111,
        (0, 2 | 5 | 6) => 0b1010_1010,
        // EL2: stage-two, range and EL1/EL2 selections.
        (4, 0) => 0b0110_0110,
        (4, 1 | 3 | 7) => 0b0111_0011,
        (4, 2 | 5 | 6) => 0b0010_0010,
        (4, 4) => 0b1111_1111,
        // EL3: EL3 selections and physical-address ranges.
        (6, 1 | 7) => 0b0011_0011,
        (6, 3) => 0b0010_0011,
        (6, 2 | 5 | 6) => 0b0010_0010,
        (6, 4) => 0b1000_1000,
        _ => return false,
    };
    mask & (1 << op2) != 0
}

/// Barriers, event hints and `NOP` that need no model in this harness.
///
/// A barrier only orders accesses against other observers. This harness runs
/// one core, keeps no caches and no store buffer, and retires each store before
/// decoding the next instruction, so admitted barriers need no pipeline model.
/// `DSB` and `DMB` accept the defined shareability/load/store options. `SSBB`
/// and `PSSBB` occupy `DSB` options 0 and 4; `ISB` accepts only option `0xf`.
/// The supplied images use `dsb sy`, `dsb ish`, `dsb ishst`, `dmb sy` and
/// `dmb ish`. `SEV`
/// signals an event to other cores, and no second core here can be waiting.
/// `NOP` names no state. Hints that would suspend the core (`WFI`, `WFE`) and
/// hint-space words with no such justification stay refused.
fn is_barrier_or_hint(word: u32) -> bool {
    if word == 0xd503_201f || word == 0xd503_209f {
        return true;
    }
    // Mask only the option field: the low byte distinguishes DSB, DMB and ISB.
    let option = (word >> 8) & 0xf;
    let standard = matches!(option, 1..=3 | 5..=7 | 9..=11 | 13..=15);
    if word & 0xffff_f0ff == 0xd503_309f {
        return standard || matches!(option, 0 | 4); // SSBB / PSSBB.
    }
    if word & 0xffff_f0ff == 0xd503_30bf {
        return standard;
    }
    word == 0xd503_3fdf
}

/// True for an architectural cache or TLB maintenance operation.
///
/// Reserved SYS encodings are refused, as are AT (writes PAR_EL1) and DC
/// ZVA (stores zeros to a cache line). Both operandless (Rt=31) and
/// register-addressed forms use this decoder.
fn is_cache_or_tlbi(word: u32) -> bool {
    // SYS form: 1101 0101 00 L op0 op1 CRn CRm op2 Rt.
    if word & 0xfff8_0000 != 0xd508_0000 {
        return false;
    }
    let op1 = (word >> 16) & 7;
    let crn = (word >> 12) & 0xf;
    let crm = (word >> 8) & 0xf;
    let op2 = (word >> 5) & 7;
    if crn == 8 {
        return tlbi_selects_a_valid_operand(op1, crm, op2);
    }
    if crn != 7 {
        return false;
    }
    // IC and DC SYS selectors. The op2 field is part of the operation:
    // accepting a CRm family without it also accepts unrelated SYS words.
    // CRm=4 contains DC ZVA, which stores to memory.
    match (op1, crm, op2) {
        // IC IALLUIS / IALLU; DC invalidate by VA or set/way.
        (0, 1 | 5, 0) | (0, 6, 1..=6) => true,
        // DC clean / clean and invalidate by set/way; CIVAPS / CIGDVAPS.
        (0, 10 | 14, 2 | 4 | 6) | (0, 15, 1 | 5) => true,
        // IC IVAU.
        (3, 5, 1) => true,
        // DC maintenance by VA.
        (3, 10 | 12 | 13 | 14, 1 | 3 | 5) => true,
        // DC CVAOC / CVAU / CGDVAOC; CIVAOC / CIGDVAOC.
        (3, 11, 0 | 1 | 7) | (3, 15, 0 | 7) => true,
        _ => false,
    }
}
fn model_value(cpu: &dyn Cpu, name: &str, encoding: u16) -> Result<u64, String> {
    if name == "CurrentEL" {
        Ok(scalar(cpu, Register::Pstate)? & 0xc)
    } else {
        scalar(
            cpu,
            Register::Arm64System(fujimod_emulator_cpu::arm::SystemRegister::from_encoding(
                encoding,
            )),
        )
    }
}
impl Guard {
    fn pause(&self, cpu: &dyn Cpu) -> Result<(), String> {
        self.pause.set(true);
        cpu.stop_before_instruction()
    }
    fn finish_system_read(&self, cpu: &dyn Cpu) -> Result<(), String> {
        let pending = self.observed.borrow_mut().pending.take();
        if let Some(read) = pending {
            if scalar(cpu, Register::Pc)? != read.pc.wrapping_add(4) {
                return Err(format!(
                    "MRS did not complete at {:#x}; PC {:#x}",
                    read.pc,
                    scalar(cpu, Register::Pc)?
                ));
            }
            let value = model_value(cpu, &read.name, read.encoding)?;
            if value != read.value
                || (read.destination != 31 && scalar(cpu, Register::X(read.destination))? != value)
            {
                return Err(format!(
                    "System register model value changed during MRS at {:#x}",
                    read.pc
                ));
            }
            self.observed.borrow_mut().system_reads.push(read);
        }
        Ok(())
    }
    /// Emulate one base A64 exclusive access (LDXR/LDAXR/STXR/STLXR) with a
    /// single-threaded monitor. This harness runs exactly one core, so one
    /// (address, width) reservation is the whole architectural monitor; there
    /// is no cross-core or multi-threaded interleaving to model. Register-pair
    /// exclusives are refused because their two-register width does not fit
    /// the simple reservation this guard guarantees.
    fn exclusive_access(
        &self,
        cpu: &dyn Cpu,
        pc: u64,
        exclusive: arm64::Exclusive,
    ) -> Result<(), String> {
        if exclusive.pair {
            return Err(format!(
                "Unsupported memory instruction at {pc:#x}: register-pair exclusive"
            ));
        }
        let address = scalar(
            cpu,
            if exclusive.base == 31 {
                Register::Sp
            } else {
                Register::X(exclusive.base)
            },
        )?;
        let width = u64::from(exclusive.bytes);
        let address = self.physical(
            cpu,
            address,
            if exclusive.load {
                Access::Read
            } else {
                Access::Write
            },
        )?;
        if address % width != 0 {
            return Err(format!(
                "unaligned exclusive access at {address:#x} width {width} (pc {pc:#x})"
            ));
        }
        if exclusive.load {
            if !contains(&self.options.readable, address, width) {
                return Err(format!(
                    "exclusive read outside declared RAM at {address:#x} width {width} (pc {pc:#x})"
                ));
            }
            let mut bytes = vec![0u8; width as usize];
            cpu.read_memory(address, &mut bytes)?;
            let mut value = [0u8; 8];
            value[..width as usize].copy_from_slice(&bytes);
            // A load into the zero register discards the value.
            if exclusive.first != 31 {
                cpu.write_register(Register::X(exclusive.first), &value)?;
            }
            self.observed.borrow_mut().exclusive = Some((address, exclusive.bytes));
        } else {
            if !contains(&self.options.writable, address, width) {
                return Err(format!(
                    "exclusive store outside declared RAM at {address:#x} width {width} (pc {pc:#x})"
                ));
            }
            // The stored value register is read as zero when it is the zero
            // register, which is the ordinary release-to-zero idiom.
            let value = if exclusive.first == 31 {
                0
            } else {
                scalar(cpu, Register::X(exclusive.first))?
            };
            let matches = self.observed.borrow().exclusive == Some((address, exclusive.bytes));
            let status: u64 = if matches {
                if self.observed.borrow().stores.len() + 1 > self.options.store_limit {
                    return Err("Store limit reached before instruction".into());
                }
                let bytes = value.to_le_bytes();
                cpu.write_memory(address, &bytes[..width as usize])?;
                let mask = u64::MAX >> (64 - u32::from(exclusive.bytes) * 8);
                self.observed.borrow_mut().stores.push(Store {
                    pc,
                    address,
                    size: width as u32,
                    value: value & mask,
                });
                0
            } else {
                1
            };
            // A matching store clears the monitor after writing; a failed
            // store performs no write and clears it too.
            self.observed.borrow_mut().exclusive = None;
            if exclusive.status != 31 {
                cpu.write_register(Register::X(exclusive.status), &status.to_le_bytes())?;
            }
        }
        cpu.write_register(Register::Pc, &pc.wrapping_add(4).to_le_bytes())?;
        self.pause(cpu)
    }
    fn instruction(&self, pc: u64, size: u32) -> Result<(), String> {
        let cpu = self.cpu.upgrade().ok_or("Routine CPU disappeared")?;
        self.finish_system_read(&*cpu)?;
        if self.options.stop_addresses.contains(&pc) {
            self.observed.borrow_mut().stopped_at = Some(pc);
            return self.pause(&*cpu);
        }
        if pc == self.options.return_address {
            self.observed.borrow_mut().returned = true;
            return self.pause(&*cpu);
        }
        if pc & 3 != 0
            || size != 4
            || !contains(
                &self.options.executable,
                self.physical(&*cpu, pc, Access::Fetch)?,
                4,
            )
        {
            return Err(format!(
                "Instruction fetch outside declared code range at {pc:#x}"
            ));
        }
        if self.options.observe_addresses.contains(&pc) {
            if self.observed.borrow().observations.len() >= self.options.observer_limit {
                return Err("Observer limit reached before instruction".into());
            }
            let registers = cpu_registers(&*cpu)?;
            self.observed
                .borrow_mut()
                .observations
                .push(RegisterObservation { pc, registers });
        }
        if let Some(index) = self.options.stubs.iter().position(|s| s.address == pc) {
            self.observed.borrow_mut().stub = Some(index);
            return self.pause(&*cpu);
        }
        let mut bytes = [0; 4];
        cpu.read_memory(pc, &mut bytes)?;
        let word = u32::from_le_bytes(bytes);
        // ERET switches the exception level; only an EL3 entry may execute it.
        if word == 0xd69f_03e0 {
            if self.options.entry_level != Some(EntryLevel::El3) {
                return Err("EL3 eret requires an EL3 entry level".into());
            }
            let spsr = scalar(
                &*cpu,
                Register::Arm64System(fujimod_emulator_cpu::arm::SystemRegister::from_encoding(
                    0xf200,
                )),
            )?;
            if spsr & 0x10 != 0 || !matches!(spsr & 0xf, 0b0000 | 0b0100 | 0b0101) {
                return Err("eret refuses a non-AArch64 SPSR_EL3 mode".into());
            }
            let elr = scalar(
                &*cpu,
                Register::Arm64System(fujimod_emulator_cpu::arm::SystemRegister::from_encoding(
                    0xf201,
                )),
            )?;
            if !contains(
                &self.options.executable,
                self.physical(&*cpu, elr, Access::Fetch)?,
                4,
            ) {
                return Err(format!(
                    "eret target outside declared code range at {elr:#x}"
                ));
            }
            return Ok(());
        }
        // CLREX clears the guard's one-core exclusive monitor. The CPU also
        // executes it to clear its own reservation; it makes no RAM access.
        if word & 0xffff_f0ff == 0xd503_305f {
            self.observed.borrow_mut().exclusive = None;
            return Ok(());
        }
        // Barrier, event-hint and NOP forms are ordering points in this
        // single-core, cache-free routine: admitted RAM stores complete before
        // the next instruction, and an instruction barrier needs no pipeline
        // model. They share the MSR system-instruction top bits, so they must
        // be admitted before the MSR/MRS class checks below.
        if is_barrier_or_hint(word) {
            return Ok(());
        }
        // MSR (immediate) with Rt=31 writes one PSTATE field: SPSel, DAIFSet,
        // DAIFClr, UAO, PAN, SSBS, DIT and TCO all share this encoding and
        // name no architectural system register, so the CPU model owns the
        // whole effect. CRn=4 with Rt=31 separates the class from the hint,
        // barrier and system-operation forms sharing the same top bits, and
        // the register-form MRS/MSR classes differ in bit 20 (0xd52/0xd51),
        // so this admission cannot reach them.
        if word & 0xfff0_f01f == 0xd500_401f {
            return Ok(());
        }
        // IC, DC and TLBI maintenance. The model keeps no caches and no TLB,
        // so a valid operation changes no routine-visible state. Reserved
        // encodings, address translation and DC ZVA remain refused.
        if is_cache_or_tlbi(word) {
            return Ok(());
        }
        // Exact MRS class: all other system instructions retain the default refusal.
        if word & 0xffe0_0000 == 0xd520_0000 {
            let encoding = ((word >> 5) & 0xffff) as u16;
            let name = system_register_name(encoding).ok_or_else(|| {
                format!("Unsupported instruction {word:#010x} at {pc:#x}: undeclared MRS")
            })?;
            if self.options.entry_level == Some(EntryLevel::El1)
                && matches!(encoding, 0xf200 | 0xf201 | 0xf600 | 0xe600)
            {
                return Err(format!(
                    "EL3 system register {name} requires an EL3 entry level"
                ));
            }
            // A caller-declared register is the model: the core implements no
            // such register, so the guard writes the declared value and retires
            // the instruction without executing it.
            if let Some(declared_value) = self.system_state.borrow().get(&encoding).copied() {
                let destination = (word & 31) as u8;
                if destination != 31 {
                    cpu.write_register(Register::X(destination), &declared_value.to_le_bytes())?;
                }
                self.observed
                    .borrow_mut()
                    .system_reads
                    .push(SystemReadObservation {
                        pc,
                        name,
                        encoding,
                        destination,
                        value: declared_value,
                        source: SystemValueSource::Declared,
                    });
                cpu.write_register(Register::Pc, &pc.wrapping_add(4).to_le_bytes())?;
                return self.pause(&*cpu);
            }
            let declared = self
                .options
                .system_reads
                .iter()
                .find(|(name, _)| admitted_system_register(name) == Some(encoding));
            if declared.is_none()
                && !self
                    .options
                    .system_registers
                    .iter()
                    .any(|n| n.eq_ignore_ascii_case(&name))
            {
                return Err(format!(
                    "Unsupported instruction {word:#010x} at {pc:#x}: undeclared MRS"
                ));
            }
            let value = model_value(&*cpu, &name, encoding)?;
            if let Some((_, expected)) = declared
                && value != *expected
            {
                return Err(format!(
                    "System register model value changed before MRS at {pc:#x}"
                ));
            }
            self.observed.borrow_mut().pending = Some(SystemReadObservation {
                pc,
                name: name.to_string(),
                encoding,
                destination: (word & 31) as u8,
                value,
                source: SystemValueSource::Model,
            });
            return Ok(());
        }
        // Exact MSR class: a declared system-register write is serviced by the
        // guard, so the CPU never executes the instruction. Bit 20 (the load
        // bit) is part of the class mask: it distinguishes MSR register from
        // the barrier/hint family that shares the top system-instruction bits.
        if word & 0xfff0_0000 == 0xd510_0000 {
            let encoding = ((word >> 5) & 0xffff) as u16;
            let name = system_register_name(encoding).ok_or_else(|| {
                format!("Unsupported instruction {word:#010x} at {pc:#x}: undeclared MSR")
            })?;
            if self.options.entry_level == Some(EntryLevel::El1)
                && matches!(encoding, 0xf200 | 0xf201 | 0xf600 | 0xe600)
            {
                return Err(format!(
                    "EL3 system register {name} requires an EL3 entry level"
                ));
            }
            if self.system_state.borrow().contains_key(&encoding) {
                let value = if word & 31 == 31 {
                    0
                } else {
                    scalar(&*cpu, Register::X((word & 31) as u8))?
                };
                self.defer_backend_write(encoding, &name, value);
                self.system_state.borrow_mut().insert(encoding, value);
                self.observed
                    .borrow_mut()
                    .system_writes
                    .push(SystemWriteObservation { pc, name, value });
                cpu.write_register(Register::Pc, &pc.wrapping_add(4).to_le_bytes())?;
                return self.pause(&*cpu);
            }
            if !self
                .options
                .system_registers
                .iter()
                .any(|n| n.eq_ignore_ascii_case(&name))
            {
                return Err(format!(
                    "Unsupported instruction {word:#010x} at {pc:#x}: undeclared MSR"
                ));
            }
            let value = if word & 31 == 31 {
                0
            } else {
                scalar(&*cpu, Register::X((word & 31) as u8))?
            };
            self.defer_backend_write(encoding, &name, value);
            cpu.write_register(
                Register::Arm64System(fujimod_emulator_cpu::arm::SystemRegister::from_encoding(
                    encoding,
                )),
                &value.to_le_bytes(),
            )?;
            self.observed
                .borrow_mut()
                .system_writes
                .push(SystemWriteObservation {
                    pc,
                    name: name.to_string(),
                    value,
                });
            cpu.write_register(Register::Pc, &pc.wrapping_add(4).to_le_bytes())?;
            return self.pause(&*cpu);
        }
        if word & 0x0a00_0000 == 0x0800_0000 {
            // Exclusives are emulated by this guard with a single-threaded
            // monitor; the CPU never executes them. Pair forms and exclusive
            // access to device windows stay refused.
            if let Some(exclusive) = arm64::decode_exclusive(word)
                .map_err(|error| format!("Unsupported memory instruction at {pc:#x}: {error}"))?
            {
                return self.exclusive_access(&*cpu, pc, exclusive);
            }
            let decoded = arm64::decode_load_store(word, pc, |r| {
                scalar(
                    &*cpu,
                    if r == 31 {
                        Register::Sp
                    } else {
                        Register::X(r)
                    },
                )
            })
            .map_err(|e| format!("Unsupported memory instruction at {pc:#x}: {e}"))?;
            // Validate every member of a pair and the whole SIMD width before
            // admitting the instruction. Padding in mapped pages is not RAM input.
            let mut records = 0;
            for transfer in &decoded.transfers {
                let address = self.physical(
                    &*cpu,
                    transfer.address,
                    if decoded.load {
                        Access::Read
                    } else {
                        Access::Write
                    },
                )?;
                let width = u64::from(transfer.size);
                let allowed = if decoded.load {
                    &self.options.readable
                } else {
                    &self.options.writable
                };
                if !contains(allowed, address, width)
                    && !contains(&self.options.devices, address, width)
                    && !contains(&self.interrupt_windows, address, width)
                {
                    return Err(format!(
                        "{} outside declared range at {address:#x} width {width} (pc {pc:#x})",
                        if decoded.load { "read" } else { "store" }
                    ));
                }
                if !decoded.load {
                    records += width.div_ceil(8) as usize;
                }
            }
            if self.observed.borrow().stores.len() + records > self.options.store_limit {
                return Err("Store limit reached before instruction".into());
            }
            return Ok(());
        }
        // Admit integer arithmetic, ordinary branches and selected register-only
        // scalar FP operations. Memory, system, trap, wait, authenticated branch
        // and other FP/SIMD families remain refused.
        let integer = word & 0x1f00_0000 == 0x1000_0000 // ADR/ADRP
            || matches!(word & 0x1f80_0000,
                0x1100_0000 | 0x1200_0000 | 0x1280_0000 | 0x1300_0000 | 0x1380_0000)
            || matches!(word & 0x1f00_0000,0x0a00_0000 | 0x0b00_0000 | 0x1b00_0000)
            || word & 0x1fe0_fc00 == 0x1a00_0000 // ADC/SBC
            || matches!(word & 0x1fe0_0000,0x1a40_0000 | 0x1a80_0000) // CCMP/CSEL
            || (word & 0x7fff_0000 == 0x5ac0_0000 && (word>>10)&63 <= 5) // RBIT/REV/CLZ/CLS
            || (word & 0x7fe0_0000 == 0x1ac0_0000
                && matches!((word>>10)&63,2|3|8..=11|16..=23)); // DIV/shifts/CRC
        // Pointer authentication and newer system-dependent operations that
        // share data-processing major groups are not generic integer ALU ops.
        let branch = word & 0x7c00_0000 == 0x1400_0000
            || word & 0xff00_0010 == 0x5400_0000
            || word & 0x7e00_0000 == 0x3400_0000
            || word & 0x7e00_0000 == 0x3600_0000
            || matches!(word & 0xffff_fc1f, 0xd61f_0000 | 0xd63f_0000 | 0xd65f_0000);
        let scalar_fmov_immediate = word & 0xff20_1fe0 == 0x1e20_1000 && (word >> 22) & 3 <= 1; // S/D forms; half and reserved forms refuse.
        let scalar_fmov_register_transfer = matches!(
            word & 0xffff_fc00,
            0x1e26_0000 | 0x1e27_0000 | 0x9e66_0000 | 0x9e67_0000
        ); // FMOV W/S and X/D bit transfers; other SIMD register moves refuse.
        let scalar_ucvtf = matches!(word & 0xffff_fc00, 0x1e23_0000 | 0x7e21_d800); // UCVTF S,W and S,S; other conversion widths refuse.
        let scalar_fp_math = ((word & 0xff20_fc00 == 0x1e20_0800 // FMUL S/D.
            || word & 0xff20_0c00 == 0x1e20_0c00 // FCSEL S/D.
            || word & 0xff20_fc1f == 0x1e20_2000) // FCMP S/D.
            && (word >> 22) & 3 <= 1)
            || word & 0xdfff_fc00 == 0x5ea1_b800; // FCVTZS/U scalar S to S.
        // DSB SY, DMB ISH and ISB are admitted above as ordering points.
        if integer
            || branch
            || scalar_fmov_immediate
            || scalar_fmov_register_transfer
            || scalar_ucvtf
            || scalar_fp_math
        {
            return Ok(());
        }
        Err(format!("Unsupported instruction {word:#010x} at {pc:#x}"))
    }
    /// Queue a system-register write that must be applied outside this hook.
    /// A backend ignores table-register writes issued while it is stepping, so
    /// the outer loop applies them between runs and before the next fetch.
    fn defer_backend_write(&self, encoding: u16, name: &str, value: u64) {
        if matches!(
            name,
            "TTBR0_EL1" | "TTBR1_EL1" | "TCR_EL1" | "MAIR_EL1" | "SCTLR_EL1"
        ) {
            self.observed.borrow_mut().deferred.push((encoding, value));
        }
    }
    /// The EL1 stage-1 inputs the guest has published, read from the CPU.
    fn translation_state(&self, cpu: &dyn Cpu) -> Result<arm64::TranslationState, String> {
        let read = |name: &str| -> Result<u64, String> {
            let encoding = arm64::system_register(name).expect("named register");
            if let Some(value) = self
                .system_state
                .borrow()
                .get(&encoding.encoding().expect("architecture encoding"))
            {
                return Ok(*value);
            }
            scalar(cpu, Register::Arm64System(encoding))
        };
        Ok(arm64::TranslationState {
            sctlr: read("SCTLR_EL1")?,
            tcr: read("TCR_EL1")?,
            ttbr0: read("TTBR0_EL1")?,
            ttbr1: read("TTBR1_EL1").unwrap_or(0),
            pstate: scalar(cpu, Register::Pstate).unwrap_or(0),
        })
    }
    /// Resolve a guest address to the physical address this guard validates and
    /// accesses. With translation off the address is unchanged.
    fn physical(&self, cpu: &dyn Cpu, address: u64, access: Access) -> Result<u64, String> {
        let state = self.translation_state(cpu)?;
        if state.sctlr & 1 == 0 {
            return Ok(address);
        }
        let mut table = [0u8; 8];
        let (physical, attribute) = state.walk(address, access, |entry| {
            cpu.read_memory(entry, &mut table)?;
            Ok(u64::from_le_bytes(table))
        })?;
        let Some(index) = attribute else {
            return Ok(physical);
        };
        let mair_encoding = arm64::system_register("MAIR_EL1").expect("named register");
        let mair = match self
            .system_state
            .borrow()
            .get(&mair_encoding.encoding().expect("architecture encoding"))
        {
            Some(value) => *value,
            None => scalar(cpu, Register::Arm64System(mair_encoding))?,
        };
        let attribute = (mair >> (u32::from(index) * 8)) & 0xff;
        // The running core performs the access itself, so the classification is
        // only recorded: Device memory is reached by the core's own MMIO path,
        // and the cacheability variants of Normal memory behave alike without
        // modeled caches. The declared executable/readable/writable spans still
        // bound every resolved physical address.
        let _ = attribute;
        Ok(physical)
    }
    fn fail(&self, message: String) {
        self.observed.borrow_mut().failure.get_or_insert(message);
        if let Some(cpu) = self.cpu.upgrade() {
            let _ = self.pause(&*cpu);
        }
    }
}
unsafe fn instruction(pc: u64, size: u32, data: *mut c_void) {
    let guard = unsafe { &*data.cast::<Guard>() };
    if let Err(error) = guard.instruction(pc, size) {
        guard.fail(error)
    }
}
unsafe fn store(_: Access, address: u64, size: i32, value: i64, data: *mut c_void) {
    let guard = unsafe { &*data.cast::<Guard>() };
    if let Some(cpu) = guard.cpu.upgrade() {
        let width = size as u32;
        // Native scalar callbacks report wide SIMD stores in 8-byte pieces.
        if !(1..=8).contains(&width) {
            guard.fail("Unsupported native store observation width".into());
            return;
        }
        let pc = scalar(&*cpu, Register::Pc).unwrap_or(0);
        let mut observed = guard.observed.borrow_mut();
        // Any non-exclusive store to the reserved range clears the monitor.
        if let Some((reserved, bytes)) = observed.exclusive {
            let reserved_end = reserved.saturating_add(u64::from(bytes));
            let store_end = address.saturating_add(u64::from(width));
            if address < reserved_end && reserved < store_end {
                observed.exclusive = None;
            }
        }
        observed.stores.push(Store {
            pc,
            address,
            size: width,
            value: value as u64 & (u64::MAX >> (64 - width * 8)),
        });
    }
}
fn registers(board: &Board) -> Result<std::collections::BTreeMap<String, u64>, String> {
    cpu_registers(board.cpu(0)?)
}
fn cpu_registers(cpu: &dyn Cpu) -> Result<std::collections::BTreeMap<String, u64>, String> {
    (0..31)
        .map(|r| (format!("x{r}"), Register::X(r)))
        .chain([
            ("sp".into(), Register::Sp),
            ("pc".into(), Register::Pc),
            ("pstate".into(), Register::Pstate),
        ])
        .map(|(name, r)| Ok((name, scalar(cpu, r)?)))
        .collect()
}
/// Run an identity-mapped, MMU-off caller with strict guards. Device access
/// is limited to the caller's declared windows on the board's strict bus.
/// This API models no architectural handoff, cache or hardware behavior.
pub fn checked_call(board: &mut Board, options: CheckedOptions) -> Result<CheckedResult, String> {
    checked_call_with(board, options, |_, _| {
        Err("Dynamic stub requires an explicit callback".into())
    })
}
pub fn checked_call_with(
    board: &mut Board,
    options: CheckedOptions,
    mut callback: impl FnMut(&StubHit, &StubSnapshot) -> Result<StubResponse, String>,
) -> Result<CheckedResult, String> {
    let entry_level = match options.entry_level {
        Some(level) => level,
        None => return Err("Missing entry exception level".into()),
    };
    if entry_level == EntryLevel::El2 {
        return Err(format!(
            "Unsupported entry exception level {} (reserved)",
            entry_level.level()
        ));
    }
    let pstate = match options.pstate {
        None => entry_level.pstate(),
        Some(value) if value & 0xf == entry_level.m4() => value,
        Some(_) => return Err("PSTATE exception level contradicts declared entry level".into()),
    };
    if (options.devices.is_empty() && !board.state.borrow().bus.is_empty())
        || board.cpus.len() != 1
        || board.cpu(0)?.model() != Model::ArmV8A
        || board.execution_policy() != fujimod_emulator_cpu::ExecutionPolicy::Exact
    {
        return Err("Checked AArch64 routines require one exact ArmV8A core".into());
    }
    let backing = board
        .mappings()
        .iter()
        .filter(|m| m.core == 0 && m.address == m.physical && m.permissions == 7)
        .map(|m| (m.address, m.address + m.size as u64))
        .collect::<Vec<_>>();
    for ranges in [&options.executable, &options.readable, &options.writable] {
        validate_ranges(ranges, &backing)?
    }
    // Device windows must be identity-mapped bus dispatch on the routine's
    // core. Holes inside them are refused by the strict bus, not here.
    let dispatched = board
        .mmio_mappings()
        .iter()
        .filter(|&&(core, address, _, physical)| core == 0 && address == physical)
        .map(|&(_, address, bytes, _)| (address, address + bytes as u64))
        .collect::<Vec<_>>();
    if options.devices.len() > 4096
        || options
            .devices
            .iter()
            .any(|&(a, b)| a >= b || !contains(&dispatched, a, b - a))
        || options.transaction_limit == 0
        || options.transaction_limit > 1 << 20
    {
        return Err("Device windows must be nonempty identity-mapped MMIO spans".into());
    }
    if !contains(&options.executable, options.entry, 4) {
        return Err("Initial entry must lie within declared executable code".into());
    }
    let sentinel = (
        options.return_address,
        options
            .return_address
            .checked_add(4)
            .ok_or("Return address overflow")?,
    );
    if options.stack_pointer & 15 != 0
        || options.stack.0 >= options.stack.1
        || !contains(&[options.stack], options.stack_pointer, 1)
        || !contains(
            &options.writable,
            options.stack.0,
            options.stack.1 - options.stack.0,
        )
        || !contains(&backing, options.return_address, 4)
        || options.executable.is_empty()
        || options.executable.iter().any(|&s| overlaps(s, sentinel))
        || options
            .writable
            .iter()
            .any(|&w| overlaps(w, sentinel) || options.executable.iter().any(|&x| overlaps(w, x)))
        || options.store_limit == 0
        || options.store_limit > 1 << 20
        || options.stubs.len() > 4096
        || options.observe_addresses.len() > 4096
        || options
            .observe_addresses
            .iter()
            .any(|a| a & 3 != 0 || !contains(&backing, *a, 4))
        || options.observer_limit == 0
        || options.observer_limit > 1 << 20
        || options
            .stubs
            .iter()
            .any(|s| s.address & 3 != 0 || !contains(&options.executable, s.address, 4))
        || options.registers.iter().any(|&(r, _)| r > 30)
    {
        return Err(
            "Invalid checked AArch64 stack (16-byte alignment), sentinel or guard ranges".into(),
        );
    }
    let call = Options {
        core: 0,
        entry: options.entry,
        arguments: options.arguments.clone(),
        return_address: options.return_address,
        stack_pointer: Some(options.stack_pointer),
        pstate: Some(pstate),
        max_instructions: options.max_instructions,
        quantum: options.quantum,
    };
    super::validate(&call)?;
    for (name, value) in &options.fragment_exit {
        if !(name == "sp" || (19..30).any(|n| *name == format!("x{n}")))
            || (name == "sp" && (value & 15 != 0 || !contains(&[options.stack], *value, 1)))
        {
            return Err("Fragment exit must name x19..x29 or an aligned SP in the stack".into());
        }
    }
    if options.stubs.iter().any(|s| {
        s.reads.len() > 32
            || s.reads.iter().any(|(arg, n)| {
                *arg > 7
                    || match n {
                        ReadSize::Fixed(n) => *n == 0 || *n > 1 << 20,
                        ReadSize::Argument { argument } => *argument > 7,
                        ReadSize::Indirect { indirect, size } => {
                            *indirect > 1 << 20 || *size == 0 || *size > 1 << 20
                        }
                    }
            })
    }) {
        return Err("Invalid stub read snapshot bounds".into());
    }
    // Translation changes are never allowed through the routine instruction policy.
    let sctlr = board.read_register(
        0,
        Register::Arm64System(arm64::system_register("SCTLR_EL1")?),
    )?;
    if sctlr & 1 != 0 {
        return Err("Checked AArch64 routine requires MMU disabled".into());
    }
    if options.stop_addresses.len() > 4096
        || options
            .stop_addresses
            .iter()
            .any(|address| address & 3 != 0 || !contains(&backing, *address, 4))
    {
        return Err("Invalid explicit routine stop addresses".into());
    }
    if options.system_reads.len() > 9 || options.system_values.len() > 9 {
        return Err("Too many declared system reads".into());
    }
    let mut names = std::collections::BTreeSet::new();
    for (name, _) in options.system_reads.iter().chain(&options.system_values) {
        if admitted_system_register(name).is_none() || !names.insert(name) {
            return Err(format!("Unsupported or duplicate system register {name}"));
        }
    }
    for (name, _) in &options.system_values {
        if options.system_registers.iter().any(|n| n == name) {
            return Err(format!(
                "System register {name} is both declared to the model and given a value"
            ));
        }
    }
    initialize(board, &call)?;
    for (name, expected) in &options.system_reads {
        let actual = model_value(board.cpu(0)?, name, admitted_system_register(name).unwrap())?;
        if actual != *expected {
            return Err(format!(
                "System register {name} model value {actual:#x} differs from expected {expected:#x}"
            ));
        }
    }
    for index in 0..32 {
        board.cpu(0)?.write_register(Register::V(index), &[0; 16])?
    }
    board.write_register(0, Register::Fpcr, 0)?;
    board.write_register(0, Register::Fpsr, 0)?;
    for &(r, value) in &options.registers {
        board.write_register(0, Register::X(r), value)?
    }
    let initial = registers(board)?;
    let start = board.instruction_counts()[0];
    let tick = board.now();
    let system_state = options
        .system_values
        .iter()
        .filter_map(|(name, value)| Some((admitted_system_register(name)?, *value)))
        .collect::<std::collections::BTreeMap<u16, u64>>();
    // Hand the declared translation state to the executing core as well, so the
    // backend and the guard resolve the same physical addresses. Table bases and
    // attributes land before the enable, so nothing runs against half-installed
    // tables.
    for name in ["TTBR0_EL1", "TTBR1_EL1", "TCR_EL1", "MAIR_EL1", "SCTLR_EL1"] {
        let Some(encoding) = admitted_system_register(name) else {
            continue;
        };
        let Some(value) = system_state.get(&encoding) else {
            continue;
        };
        board.write_register(
            0,
            Register::Arm64System(fujimod_emulator_cpu::arm::SystemRegister::from_encoding(
                encoding,
            )),
            *value,
        )?;
    }
    let mut guard = Box::new(Guard {
        cpu: Rc::downgrade(&board.cpus[0]),
        pause: board.host_pause.clone(),
        options,
        system_state: RefCell::new(system_state),
        interrupt_windows: board.interrupt_windows(),
        observed: RefCell::new(Observed::default()),
    });
    let data = (&*guard as *const Guard).cast_mut().cast();
    let cpu = board.cpus[0].clone();
    let code_hook = unsafe {
        cpu.install_hook(Hook::Instruction {
            callback: instruction,
            data,
            range: HookRange::All,
        })?
    };
    let write_hook = match unsafe {
        cpu.install_hook(Hook::Memory {
            callback: store,
            data,
            read: false,
            write: true,
            range: HookRange::All,
        })
    } {
        Ok(hook) => hook,
        Err(error) => {
            cpu.remove_hook(code_hook)?;
            return Err(error);
        }
    };
    if !guard.options.devices.is_empty()
        && let Err(error) = board.transaction_watch_begin(
            guard.options.devices.clone(),
            guard.options.transaction_limit,
        )
    {
        cpu.remove_hook(code_hook)?;
        cpu.remove_hook(write_hook)?;
        return Err(error);
    }
    board.harness_interrupt_delivery(0, false);
    let execution = (|| -> Result<(), String> {
        loop {
            if guard.observed.borrow().failure.is_some()
                || guard.observed.borrow().returned
                || guard.observed.borrow().stopped_at.is_some()
            {
                break;
            }
            let stub = guard.observed.borrow_mut().stub.take();
            if let Some(index) = stub {
                let stub = &guard.options.stubs[index];
                if guard.observed.borrow().hits.len() >= guard.options.max_instructions as usize {
                    return Err("Callee substitution limit reached".into());
                }
                let arguments = (0..8)
                    .map(|r| board.read_register(0, Register::X(r)))
                    .collect::<Result<_, _>>()?;
                let mut hit = StubHit {
                    address: stub.address,
                    returns: stub.returns,
                    arguments,
                    writes: vec![],
                    reads: vec![],
                };
                if stub.dynamic {
                    let mut snapshots = vec![];
                    for (argument, length) in &stub.reads {
                        let mut address = hit.arguments[*argument as usize];
                        let size = match length {
                            ReadSize::Fixed(n) => *n,
                            ReadSize::Argument { argument } => hit.arguments[*argument as usize],
                            ReadSize::Indirect { indirect, size } => {
                                let location = address
                                    .checked_add(*indirect)
                                    .ok_or("Stub indirect pointer overflow")?;
                                if !contains(&guard.options.readable, location, 8) {
                                    return Err(
                                        "Stub indirect pointer outside readable range".into()
                                    );
                                }
                                let mut pointer = [0; 8];
                                board.read_memory(0, location, &mut pointer)?;
                                address = u64::from_le_bytes(pointer);
                                snapshots.push((location, pointer.to_vec()));
                                *size
                            }
                        };
                        if size == 0 || size > 1 << 20 {
                            return Err("Stub snapshot size exceeds its bound".into());
                        }
                        if !contains(&guard.options.readable, address, size) {
                            return Err("Stub read outside declared readable range".into());
                        }
                        let mut bytes = vec![0; size as usize];
                        board.read_memory(0, address, &mut bytes)?;
                        snapshots.push((address, bytes));
                    }
                    hit.reads = snapshots
                        .iter()
                        .map(|(address, bytes)| WriteReceipt {
                            address: *address,
                            size: bytes.len(),
                        })
                        .collect();
                    let (returns, writes) = callback(&hit, &snapshots)?;
                    if writes.len() > 4096
                        || writes.iter().any(|(a, b)| {
                            b.is_empty()
                                || b.len() > 1 << 20
                                || !contains(&guard.options.writable, *a, b.len() as u64)
                        })
                    {
                        return Err(
                            "Stub write outside declared writable ranges or intervention bound"
                                .into(),
                        );
                    }
                    // Validate the entire effect set before applying any write.
                    for (address, bytes) in writes {
                        board.write_memory(0, address, &bytes)?;
                        hit.writes.push(WriteReceipt {
                            address,
                            size: bytes.len(),
                        });
                    }
                    hit.returns = returns;
                }
                board.write_register(0, Register::X(0), hit.returns)?;
                guard.observed.borrow_mut().hits.push(hit);
                board.write_register(0, Register::Pc, board.read_register(0, Register::X(30))?)?;
            }
            if board.read_register(0, Register::Pc)? == guard.options.return_address {
                guard.observed.borrow_mut().returned = true;
                break;
            }
            let deferred = std::mem::take(&mut guard.observed.borrow_mut().deferred);
            for (encoding, value) in deferred {
                board.write_register(
                    0,
                    Register::Arm64System(
                        fujimod_emulator_cpu::arm::SystemRegister::from_encoding(encoding),
                    ),
                    value,
                )?;
            }
            let used = board.instruction_counts()[0] - start;
            if used >= guard.options.max_instructions {
                return Err("Instruction limit reached".into());
            }
            board.run_for_instructions(
                guard.options.max_instructions - used,
                guard.options.quantum,
            )?;
        }
        Ok(())
    })();
    if guard.observed.borrow().pending.is_some()
        && let Err(error) = guard.finish_system_read(&*cpu)
    {
        guard.observed.borrow_mut().failure.get_or_insert(error);
    }
    let cleanup_code = cpu.remove_hook(code_hook);
    let cleanup_write = cpu.remove_hook(write_hook);
    board.harness_interrupt_delivery(0, true);
    let watched = if guard.options.devices.is_empty() {
        Ok(crate::board::bus::TransactionBatch {
            transactions: vec![],
            omitted: 0,
        })
    } else {
        board.transaction_watch_end()
    };
    cleanup_code?;
    cleanup_write?;
    let watched = watched?;
    if let Err(error) = execution {
        guard.observed.borrow_mut().failure.get_or_insert(error);
    }
    let final_registers = registers(board)?;
    let observed = guard.observed.get_mut();
    if observed.returned
        || (observed.stopped_at.is_some() && !guard.options.fragment_exit.is_empty())
    {
        for name in (19..30).map(|r| format!("x{r}")).chain(["sp".into()]) {
            if final_registers[&name]
                != guard
                    .options
                    .fragment_exit
                    .get(&name)
                    .copied()
                    .unwrap_or(initial[&name])
            {
                observed
                    .failure
                    .get_or_insert(format!("Callee-save violation: {name}"));
            }
        }
    }
    let returned = observed.returned && observed.failure.is_none();
    Ok(CheckedResult {
        returned,
        stopped_at: observed.stopped_at,
        failure: observed.failure.take(),
        return_value: returned.then_some(final_registers["x0"]),
        registers: final_registers,
        initial_registers: initial,
        instructions: board.instruction_counts()[0] - start,
        virtual_ticks: board.now() - tick,
        stores: std::mem::take(&mut observed.stores),
        stub_hits: std::mem::take(&mut observed.hits),
        observations: std::mem::take(&mut observed.observations),
        system_read_model_values: guard.options.system_reads.clone(),
        system_read_observations: std::mem::take(&mut observed.system_reads),
        system_writes: std::mem::take(&mut observed.system_writes),
        device_transactions: watched.transactions,
        device_transactions_omitted: watched.omitted,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::{
        InterruptMap,
        bus::{Bus, Mode},
        factory,
    };
    use fujimod_catalog_configuration::RunEngine;
    use serde_json::json;
    fn board(words: &[u32]) -> Board {
        let graph:factory::Graph=serde_json::from_value(json!({
            "processors":[{"model":"armv8a","registers":[],"released":true,"mprofile":null,"rprofile":null}],
            "ram":[{"base":0,"size":65536}],
            "mappings":[{"core":0,"address":0,"physical":0,"size":65536,"permissions":7}],
            "dma_mappings":[],"decoders":[],"peripherals":[],"instructions_per_tick":[1,1]
        })).unwrap();
        let b = graph
            .build(
                factory::Options::new(RunEngine::Unicorn)
                    .with_translation_cache_size_bytes(1 << 20),
                Bus::new(Mode::Strict),
                None,
            )
            .unwrap();
        b.write_memory(
            0,
            0x1000,
            &words
                .iter()
                .flat_map(|w| w.to_le_bytes())
                .collect::<Vec<_>>(),
        )
        .unwrap();
        b
    }
    fn options() -> CheckedOptions {
        serde_json::from_value(
            json!({"entry":4096,"entry_level":"el1","arguments":[],"return_address":45056,
            "stack_pointer":36864,"stack":[32768,40960],"max_instructions":20,"quantum":10,
            "executable":[[4096,4112]],"readable":[[4096,4112],[16384,16400],[32768,40960]],
            "writable":[[16384,16392],[32768,40960]],"store_limit":20,"stubs":[],"registers":[]}),
        )
        .unwrap()
    }
    fn sysreg(encoding: u16) -> Register {
        Register::Arm64System(fujimod_emulator_cpu::arm::SystemRegister::from_encoding(
            encoding,
        ))
    }
    #[test]
    fn gic_distributor_transfer_is_admitted_and_served() {
        // MOVZ X1,#0xfa00,LSL#16; MOV W0,#1; STR W0,[X1]; LDR W2,[X1];
        // MOV X0,X2; RET. The distributor's control register (offset 0)
        // is outside every declared RAM span, so the routine only completes
        // when the interrupt windows are admitted and the board serves them;
        // GICD_CTLR reads back 0x51 after writing 1 (group enable).
        let graph: factory::Graph = serde_json::from_value(json!({
            "processors":[{"model":"armv8a","registers":[],"released":true,"mprofile":null,"rprofile":null}],
            "ram":[{"base":0,"size":65536}],
            "mappings":[{"core":0,"address":0,"physical":0,"size":65536,"permissions":7}],
            "dma_mappings":[],"decoders":[],"peripherals":[],"instructions_per_tick":[1,1]
        })).unwrap();
        let interrupts = InterruptMap::gic_v3(
            0xfa00_0000,
            0xfa08_0000,
            0x20000,
            1,
            512,
            fujimod_emulator_devices::gic::SecurityStates::One,
        )
        .unwrap();
        let mut b = graph
            .build(
                factory::Options::new(RunEngine::Unicorn)
                    .with_translation_cache_size_bytes(1 << 20),
                Bus::new(Mode::Strict),
                Some(interrupts),
            )
            .unwrap();
        b.write_memory(
            0,
            0x1000,
            &[
                0xd2bf4001u32,
                0x52800020,
                0xb9000020,
                0xb9400022,
                0xaa0203e0,
                0xd65f03c0,
            ]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect::<Vec<_>>(),
        )
        .unwrap();
        let o: CheckedOptions = serde_json::from_value(json!({
            "entry":4096,"entry_level":"el1","arguments":[],"return_address":45056,
            "stack_pointer":36864,"stack":[32768,40960],"max_instructions":20,"quantum":10,
            "executable":[[4096,4120]],"readable":[[4096,4120],[16384,16400],[32768,40960]],
            "writable":[[16384,16392],[32768,40960]],"store_limit":20,"stubs":[],"registers":[]
        }))
        .unwrap();
        let r = checked_call(&mut b, o).unwrap();
        assert!(r.returned, "{:?}", r.failure);
        assert_eq!(r.return_value, Some(0x51));
    }
    #[test]
    fn el1_declaration_without_pstate_runs_to_completion() {
        let mut b = board(&[0xd65f03c0]);
        let o = options();
        assert_eq!(o.entry_level, Some(EntryLevel::El1));
        assert!(o.pstate.is_none());
        let r = checked_call(&mut b, o).unwrap();
        assert!(r.returned, "{:?}", r.failure);
    }
    #[test]
    fn explicit_el1_pstate_matching_declared_level_is_accepted() {
        let mut b = board(&[0xd65f03c0]);
        let mut o = options();
        o.pstate = Some(0x3c5);
        let r = checked_call(&mut b, o).unwrap();
        assert!(r.returned, "{:?}", r.failure);
    }
    #[test]
    fn el1_pstate_contradicting_declared_level_refuses_before_execution() {
        let mut b = board(&[0xd65f03c0]);
        let mut o = options();
        o.pstate = Some(0x3cd);
        let error = checked_call(&mut b, o)
            .err()
            .expect("contradicting pstate accepted");
        assert_eq!(
            error,
            "PSTATE exception level contradicts declared entry level"
        );
        assert_eq!(b.instruction_counts()[0], 0);
    }
    #[test]
    fn reserved_entry_levels_refuse_before_execution() {
        let mut b = board(&[0xd65f03c0]);
        let mut o = options();
        o.entry_level = Some(EntryLevel::El2);
        let error = checked_call(&mut b, o)
            .err()
            .expect("reserved level accepted");
        assert_eq!(error, "Unsupported entry exception level 2 (reserved)");
        assert_eq!(b.instruction_counts()[0], 0);
    }
    #[test]
    fn missing_entry_level_refuses_before_execution() {
        let mut b = board(&[0xd65f03c0]);
        let mut o = options();
        o.entry_level = None;
        let error = checked_call(&mut b, o)
            .err()
            .expect("missing level accepted");
        assert_eq!(error, "Missing entry exception level");
        assert_eq!(b.instruction_counts()[0], 0);
    }
    #[test]
    fn initial_entry_requires_four_admitted_executable_bytes() {
        for entry in [0xb000, 0x1010, 0x100c] {
            let mut b = board(&[0xd65f03c0]);
            let mut o = options();
            o.entry = entry;
            o.executable = vec![(0x1000, 0x100e)];
            let error = checked_call(&mut b, o)
                .err()
                .expect("invalid initial entry accepted");
            assert!(error.contains("Initial entry") && error.contains("executable"));
            assert_eq!(b.instruction_counts()[0], 0);
        }
    }
    #[test]
    fn return_preserves_full_width_arguments() {
        let mut b = board(&[0x8b010000, 0xd65f03c0]);
        let mut o = options();
        o.arguments = vec![0x1_0000_0001, 0x2_0000_0002];
        let r = checked_call(&mut b, o).unwrap();
        assert!(r.returned);
        assert_eq!(r.return_value, Some(0x3_0000_0003));
        assert_eq!(r.instructions, 2);
    }
    #[test]
    fn dsb_sy_retires_between_checked_store_and_load() {
        // STR X0,[X1]; DSB SY; LDR X2,[X1]; RET.
        let mut b = board(&[0xf9000020, 0xd5033f9f, 0xf9400022, 0xd65f03c0]);
        let mut o = options();
        o.arguments = vec![0x1234_5678_9abc_def0, 0x4000];
        let r = checked_call(&mut b, o).unwrap();
        assert!(r.returned, "{:?}", r.failure);
        assert_eq!(r.instructions, 4);
        assert_eq!(r.registers["x2"], 0x1234_5678_9abc_def0);
        assert_eq!(r.stores.len(), 1);
        assert_eq!((r.stores[0].pc, r.stores[0].address), (0x1000, 0x4000));
        let mut bytes = [0; 8];
        b.read_memory(0, 0x4000, &mut bytes).unwrap();
        assert_eq!(bytes, 0x1234_5678_9abc_def0u64.to_le_bytes());
    }
    #[test]
    fn dmb_ish_retires_between_checked_store_and_load() {
        // STR X0,[X1]; DMB ISH; LDR X2,[X1]; RET.
        let mut b = board(&[0xf9000020, 0xd5033bbf, 0xf9400022, 0xd65f03c0]);
        let mut o = options();
        o.arguments = vec![0x1234_5678_9abc_def0, 0x4000];
        let r = checked_call(&mut b, o).unwrap();
        assert!(r.returned, "{:?}", r.failure);
        assert_eq!(r.instructions, 4);
        assert_eq!(r.registers["x2"], 0x1234_5678_9abc_def0);
        assert_eq!(r.stores.len(), 1);
        assert_eq!((r.stores[0].pc, r.stores[0].address), (0x1000, 0x4000));
    }
    #[test]
    fn scalar_fmov_immediate_retires_without_admitting_fadd() {
        for (word, expected) in [
            (0x1e2e_1008, 0x3f80_0000u64),        // FMOV S8,#1.0.
            (0x1e6e_1008, 0x3ff0_0000_0000_0000), // FMOV D8,#1.0.
        ] {
            let mut b = board(&[word, 0xd65f_03c0]);
            let r = checked_call(&mut b, options()).unwrap();
            assert!(r.returned, "{word:#x}: {:?}", r.failure);
            let mut vector = [0; 16];
            b.cpu(0)
                .unwrap()
                .read_register(Register::V(8), &mut vector)
                .unwrap();
            assert_eq!(
                u64::from_le_bytes(vector[..8].try_into().unwrap()),
                expected
            );
        }
        let mut b = board(&[0x1e20_2800, 0xd65f_03c0]); // FADD S0,S0,S0.
        let r = checked_call(&mut b, options()).unwrap();
        assert_eq!(r.instructions, 0);
        assert!(r.failure.unwrap().contains("Unsupported instruction"));
    }
    #[test]
    fn scalar_fmov_register_transfer_round_trips_integer_bits() {
        for (to_fp, from_fp, input, expected) in [
            (
                0x1e27_0008, // FMOV S8,W0.
                0x1e26_0101, // FMOV W1,S8.
                0x1234_5678_90ab_cdefu64,
                0x90ab_cdefu64,
            ),
            (
                0x9e67_0008, // FMOV D8,X0.
                0x9e66_0101, // FMOV X1,D8.
                0x1234_5678_90ab_cdefu64,
                0x1234_5678_90ab_cdefu64,
            ),
        ] {
            let mut b = board(&[to_fp, from_fp, 0xd65f_03c0]);
            let mut o = options();
            o.arguments = vec![input];
            let r = checked_call(&mut b, o).unwrap();
            assert!(r.returned, "{to_fp:#x}: {:?}", r.failure);
            assert_eq!(r.instructions, 3);
            assert_eq!(r.registers["x1"], expected);
        }
    }
    #[test]
    fn scalar_ucvtf_from_integer_register_and_scalar_bits_retires() {
        for (words, expected_instructions) in [
            (
                vec![0x1e23_0000, 0x1e26_0001, 0xd65f_03c0], // UCVTF S0,W0; FMOV W1,S0.
                3,
            ),
            (
                vec![
                    0x1e27_0008, // FMOV S8,W0.
                    0x7e21_d902, // UCVTF S2,S8.
                    0x1e26_0041, // FMOV W1,S2.
                    0xd65f_03c0,
                ],
                4,
            ),
        ] {
            let mut b = board(&words);
            let mut o = options();
            o.arguments = vec![640];
            let r = checked_call(&mut b, o).unwrap();
            assert!(r.returned, "{:?}", r.failure);
            assert_eq!(r.instructions, expected_instructions);
            assert_eq!(r.registers["x1"], 640.0f32.to_bits() as u64);
        }
    }
    #[test]
    fn scalar_fp_register_math_and_round_toward_zero_are_checked() {
        for conversion in [0x5ea1_b800, 0x7ea1_b800] {
            // FMOV S8,#1; FMOV S0,#4; FMUL S0,S8,S0; FCVTZS/U S0,S0; RET.
            let mut b = board(&[
                0x1e2e_1008,
                0x1e22_1000,
                0x1e20_0900,
                conversion,
                0xd65f_03c0,
            ]);
            let mut o = options();
            o.executable = vec![(0x1000, 0x1014)];
            o.readable = vec![(0x1000, 0x1014), (0x4000, 0x4010), (0x8000, 0xa000)];
            let r = checked_call(&mut b, o).unwrap();
            assert!(r.returned, "{conversion:#x}: {:?}", r.failure);
            let mut vector = [0; 16];
            b.cpu(0)
                .unwrap()
                .read_register(Register::V(0), &mut vector)
                .unwrap();
            assert_eq!(u32::from_le_bytes(vector[..4].try_into().unwrap()), 4);
        }
        // FMOV S1,#1; FMOV S0,#4; FMOV S8,#2; FCMP S1,S0;
        // FCSEL S8,S8,S0,HI; RET. HI is false, so S8 becomes 4.0.
        let mut b = board(&[
            0x1e2e_1001,
            0x1e22_1000,
            0x1e20_1008,
            0x1e20_2020,
            0x1e20_8d08,
            0xd65f_03c0,
        ]);
        let mut o = options();
        o.executable = vec![(0x1000, 0x1018)];
        o.readable = vec![(0x1000, 0x1018), (0x4000, 0x4010), (0x8000, 0xa000)];
        let r = checked_call(&mut b, o).unwrap();
        assert!(r.returned, "{:?}", r.failure);
        let mut vector = [0; 16];
        b.cpu(0)
            .unwrap()
            .read_register(Register::V(8), &mut vector)
            .unwrap();
        assert_eq!(
            u32::from_le_bytes(vector[..4].try_into().unwrap()),
            0x4080_0000
        );
    }
    #[test]
    fn denied_pair_has_no_partial_store_or_retired_instruction() {
        let mut b = board(&[0xa9000820, 0xd65f03c0]);
        let mut o = options();
        o.arguments = vec![7, 0x4000, 9];
        let r = checked_call(&mut b, o).unwrap();
        assert!(!r.returned);
        assert_eq!(r.instructions, 0);
        assert!(r.stores.is_empty());
        let mut bytes = [0; 16];
        b.read_memory(0, 0x4000, &mut bytes).unwrap();
        assert_eq!(bytes, [0; 16]);
    }
    #[test]
    fn declared_midr_read_executes_and_records_model_value() {
        let mut b = board(&[0xd5380000, 0xd65f03c0]);
        let actual = b
            .read_register(
                0,
                Register::Arm64System(arm64::system_register("MIDR_EL1").unwrap()),
            )
            .unwrap();
        let mut value = json!({"entry":4096,"entry_level":"el1","arguments":[],"return_address":45056,
            "stack_pointer":36864,"stack":[32768,40960],"max_instructions":20,"quantum":10,
            "executable":[[4096,4112]],"readable":[[4096,4112],[16384,16400],[32768,40960]],
            "writable":[[16384,16392],[32768,40960]],"store_limit":20,"stubs":[],"registers":[]});
        value["system_reads"] = json!([["MIDR_EL1", actual]]);
        let options = serde_json::from_value(value).unwrap();
        let result = serde_json::to_value(checked_call(&mut b, options).unwrap()).unwrap();
        assert_eq!(result["returned"], true);
        assert_eq!(result["registers"]["x0"], actual);
        assert_eq!(result["system_read_observations"][0]["value"], actual);
    }
    #[test]
    fn explicit_model_reads_preserve_affinity_and_write_bounded_store() {
        // MRS x0, MPIDR_EL1; STR x0,[x1]; MRS x2,TCR_EL1; RET.
        let mut b = board(&[0xd53800a0, 0xf9000020, 0xd5382042, 0xd65f03c0]);
        let mpidr = 0x12_8000_0001;
        b.write_register(
            0,
            Register::Arm64System(arm64::system_register("MPIDR_EL1").unwrap()),
            mpidr,
        )
        .unwrap();
        let tcr = b
            .read_register(
                0,
                Register::Arm64System(arm64::system_register("TCR_EL1").unwrap()),
            )
            .unwrap();
        let mut o = options();
        o.arguments = vec![0, 0x4000];
        o.system_reads = vec![("MPIDR_EL1".into(), mpidr), ("TCR_EL1".into(), tcr)];
        let r = checked_call(&mut b, o).unwrap();
        assert!(r.returned, "{:?}", r.failure);
        assert_eq!(r.registers["x0"], mpidr);
        assert_eq!(r.registers["x2"], tcr);
        assert_eq!(r.stores[0].value, mpidr);
        assert_eq!(r.system_read_observations.len(), 2);
        assert_eq!(
            (
                r.system_read_observations[0].pc,
                r.system_read_observations[0].encoding,
                r.system_read_observations[0].destination,
                r.system_read_observations[0].value
            ),
            (0x1000, 0xc005, 0, mpidr)
        );
    }
    #[test]
    fn cache_identity_reads_name_their_own_encodings() {
        // MRS x0, CLIDR_EL1. CLIDR sits one op2 above CCSIDR at the same op1,
        // and CTR_EL0 sits at op1=3, so the three encodings never alias. An
        // alias would serve one register's model value for another's read,
        // which the declared-value check cannot detect.
        let mut b = board(&[0xd5390020, 0xd65f03c0]);
        let clidr = b
            .read_register(
                0,
                Register::Arm64System(arm64::system_register("c801").unwrap()),
            )
            .unwrap();
        let mut o = options();
        o.system_reads = vec![("CLIDR_EL1".into(), clidr)];
        let r = checked_call(&mut b, o).unwrap();
        assert!(r.returned, "{:?}", r.failure);
        assert_eq!(r.registers["x0"], clidr);
        assert_eq!(r.system_read_observations[0].name, "CLIDR_EL1");
        assert_eq!(r.system_read_observations[0].encoding, 0xc801);
    }
    #[test]
    fn ctr_read_uses_its_op1_three_encoding() {
        // MRS x0, CTR_EL0, the same shape at op1=3.
        let mut b = board(&[0xd53b0020, 0xd65f03c0]);
        let ctr = b
            .read_register(
                0,
                Register::Arm64System(arm64::system_register("d801").unwrap()),
            )
            .unwrap();
        let mut o = options();
        o.system_reads = vec![("CTR_EL0".into(), ctr)];
        let r = checked_call(&mut b, o).unwrap();
        assert!(r.returned, "{:?}", r.failure);
        assert_eq!(r.registers["x0"], ctr);
        assert_eq!(r.system_read_observations[0].name, "CTR_EL0");
        assert_eq!(r.system_read_observations[0].encoding, 0xd801);
    }
    #[test]
    fn invalid_declarations_fail_before_retirement() {
        for declarations in [
            vec![("unknown".to_string(), 0)],
            vec![("CNTFRQ_EL0".to_string(), 0)],
            vec![("TCR_EL1".to_string(), 0), ("TCR_EL1".to_string(), 0)],
            vec![("MIDR_EL1".to_string(), u64::MAX)],
        ] {
            let mut b = board(&[0xd5380000, 0xd65f03c0]);
            let mut o = options();
            o.system_reads = declarations;
            assert!(checked_call(&mut b, o).is_err());
            assert_eq!(b.instruction_counts()[0], 0);
        }
    }
    #[test]
    fn barrier_admission_covers_the_forms_the_images_use() {
        for word in [
            0xd503_201f, // nop
            0xd503_209f, // sev
            0xd503_3f9f, // dsb sy
            0xd503_3b9f, // dsb ish
            0xd503_3a9f, // dsb ishst
            0xd503_3fbf, // dmb sy
            0xd503_3bbf, // dmb ish
            0xd503_3fdf, // isb
            0xd503_309f, // ssbb, which occupies the reserved dsb option-0 form
        ] {
            assert!(is_barrier_or_hint(word), "{word:#010x}");
        }
        for word in [
            0xd503_3f5f, // clrex clears an exclusive monitor instead
            0xd503_229f, // csdb: a hint this harness does not model
            0xd503_389f, // reserved DSB option 0x8
            0xd503_38bf, // reserved DMB option 0x8
            0xd503_3edf, // reserved ISB option 0xe
            0xd503_3f7f, // generic MSR, not a barrier
            0xd503_3fef, // generic MSR, not an ISB encoding
            0xd400_41bf, // svc enters an exception
        ] {
            assert!(!is_barrier_or_hint(word), "{word:#010x}");
        }
    }
    #[test]
    fn barrier_forms_retire_and_leave_state_alone() {
        // DSB ISHST; ISB; ADD X1,SP,#0; RET reaches the return with the stack
        // pointer the two barriers leave untouched.
        let mut b = board(&[0xd503_3a9f, 0xd503_3fdf, 0x9100_03e1, 0xd65f_03c0]);
        let r = checked_call(&mut b, options()).unwrap();
        assert!(r.returned, "{:?}", r.failure);
        assert_eq!(r.instructions, 4);
        assert_eq!(r.registers["x1"], 0x9000);
        assert_eq!(r.registers["sp"], 0x9000);
    }
    #[test]
    fn declared_system_values_service_reads_the_core_does_not_implement() {
        // MRS X1, S3_1_C15_C2_1; RET. The A53 CPUACTLR_EL1 encoding has no
        // value in the CPU model, so a caller declaration is the model.
        let mut b = board(&[0xd539_f221, 0xd65f_03c0]);
        let mut o = options();
        o.system_values = vec![("S3_1_C15_C2_1".into(), 0x1234_5678)];
        let r = checked_call(&mut b, o).unwrap();
        assert!(r.returned);
        assert_eq!(r.system_read_observations.len(), 1);
        let read = &r.system_read_observations[0];
        assert_eq!(read.name, "S3_1_C15_C2_1");
        assert_eq!(read.encoding, 0xcf91);
        assert_eq!(read.destination, 1);
        assert_eq!(read.value, 0x1234_5678);
        assert_eq!(read.source, SystemValueSource::Declared);
        assert_eq!(r.registers["x1"], 0x1234_5678);
        // The read never reaches the CPU: the core implements no such
        // register, so the guard retires it and the routine continues.
        assert_eq!(r.failure, None);
    }
    /// Three-level tables at physical 0x1000/0x2000/0x3000, the program's own
    /// page identity-mapped, and a data virtual page that maps to a different
    /// physical frame than its own address.
    fn translated_board(program: &[u32]) -> Board {
        translated_board_with(program, DATA_PA | 0x407)
    }
    fn translated_board_with(program: &[u32], data_leaf: u64) -> Board {
        let graph: factory::Graph = serde_json::from_value(json!({
            "processors":[{"model":"armv8a","registers":[],"released":true,"mprofile":null,"rprofile":null}],
            "ram":[{"base":0,"size":262144}],
            "mappings":[{"core":0,"address":0,"physical":0,"size":262144,"permissions":7}],
            "dma_mappings":[{"address":0,"physical":0,"size":262144,"permissions":3}],
            "decoders":[],"peripherals":[],"instructions_per_tick":[1,1]
        }))
        .unwrap();
        let b = graph
            .build(
                factory::Options::new(RunEngine::Unicorn)
                    .with_translation_cache_size_bytes(1 << 20),
                Bus::new(Mode::Strict),
                None,
            )
            .unwrap();
        for (address, values) in [(0x1000u64, vec![0x2003u64]), (0x2000, vec![0x3003u64])] {
            b.write_memory(
                0,
                address,
                &values
                    .iter()
                    .flat_map(|v| v.to_le_bytes())
                    .collect::<Vec<_>>(),
            )
            .unwrap();
        }
        let mut level3 = vec![0u64; 512];
        level3[(CODE >> 12) as usize] = CODE | 0x407;
        level3[(DATA_VA >> 12) as usize] = data_leaf;
        b.write_memory(
            0,
            0x3000,
            &level3
                .iter()
                .flat_map(|v| v.to_le_bytes())
                .collect::<Vec<_>>(),
        )
        .unwrap();
        b.write_memory(
            0,
            CODE,
            &program
                .iter()
                .flat_map(|w| w.to_le_bytes())
                .collect::<Vec<_>>(),
        )
        .unwrap();
        b
    }
    const CODE: u64 = 0x6000;
    const DATA_VA: u64 = 0x2_0000;
    const DATA_PA: u64 = 0x7000;
    fn translated_options() -> CheckedOptions {
        serde_json::from_value(json!({
            "entry":CODE,"entry_level":"el1","pstate":0x3c5,"arguments":[],"return_address":CODE + 0x100,
            "stack_pointer":36864,"stack":[32768,40960],"max_instructions":20,"quantum":10,
            "executable":[[CODE,CODE + 0x20]],"readable":[[CODE,CODE + 0x20],[32768,40960]],
            "writable":[[DATA_PA,DATA_PA + 0x10],[32768,40960]],"store_limit":20,
            "stubs":[],"registers":[],
            "system_values":[["SCTLR_EL1",1],["TTBR0_EL1",0x1000],["TCR_EL1",0x20],["MAIR_EL1",0x4400]]
        }))
        .unwrap()
    }
    #[test]
    fn a_translated_checked_call_stores_through_a_nonidentity_page() {
        // MOVZ X1,#0x2,LSL#16; MOVZ X2,#0xbeef; STR X2,[X1]; RET.
        let mut b = translated_board(&[0xd2a0_0041, 0xd297_dde2, 0xf900_0022, 0xd65f_03c0]);
        let result = checked_call(&mut b, translated_options()).unwrap();
        assert_eq!(result.failure, None);
        assert_eq!(result.instructions, 4);
        let mut bytes = [0u8; 8];
        b.read_memory(0, DATA_PA, &mut bytes).unwrap();
        assert_eq!(u64::from_le_bytes(bytes), 0xbeef);
        b.read_memory(0, DATA_VA, &mut bytes).unwrap();
        assert_eq!(
            u64::from_le_bytes(bytes),
            0,
            "the virtual address is a decoy"
        );
        assert_eq!(result.stores[0].address, DATA_PA);
    }
    #[test]
    fn translation_enabled_inside_the_call_reaches_the_mapped_code_page() {
        // The entry page is identity-mapped, but the page the program jumps to
        // after enabling translation maps elsewhere: a stale flat fetch would
        // run the decoy instructions at the virtual address instead.
        let mut b = translated_board(&[
            0xd280_0020, // MOVZ X0,#1
            0xd518_1000, // MSR SCTLR_EL1,X0
            0xd503_3fdf, // ISB
            0xd28e_0005, // MOVZ X5,#0x7000
            0xd61f_00a0, // BR X5
        ]);
        let mut level3 = [0u64; 512];
        level3[(CODE >> 12) as usize] = CODE | 0x407;
        level3[(JUMP_VA >> 12) as usize] = JUMP_PA | 0x407;
        b.write_memory(
            0,
            0x3000,
            &level3
                .iter()
                .flat_map(|v| v.to_le_bytes())
                .collect::<Vec<_>>(),
        )
        .unwrap();
        // A decoy at the virtual address and the real continuation elsewhere.
        b.write_memory(
            0,
            JUMP_VA,
            &[0xd503_201fu32; 8]
                .iter()
                .flat_map(|w| w.to_le_bytes())
                .collect::<Vec<_>>(),
        )
        .unwrap();
        b.write_memory(
            0,
            JUMP_PA,
            &[0xd282_4680u32, 0xd65f_03c0]
                .iter()
                .flat_map(|w| w.to_le_bytes())
                .collect::<Vec<_>>(),
        )
        .unwrap();
        let mut options = translated_options();
        options.system_values = vec![
            ("SCTLR_EL1".into(), 0),
            ("TTBR0_EL1".into(), 0x1000),
            ("TCR_EL1".into(), 0x20),
            ("MAIR_EL1".into(), 0x4400),
        ];
        options.executable = vec![(CODE, CODE + 0x20), (JUMP_PA, JUMP_PA + 0x10)];
        options.readable = vec![
            (CODE, CODE + 0x20),
            (JUMP_PA, JUMP_PA + 0x10),
            (32768, 40960),
        ];
        let result = checked_call(&mut b, options).unwrap();
        assert_eq!(result.failure, None);
        assert_eq!(result.registers["x0"], 0x1234);
        assert!(result.returned);
    }
    const JUMP_VA: u64 = 0x7000;
    const JUMP_PA: u64 = 0xB000;
    #[test]
    fn a_device_page_is_reached_by_the_running_core() {
        // Attribute index 0 is Device in the declared MAIR: the walker still
        // resolves the page, and the running core performs the access itself
        // under the declared spans, which is how firmware reaches a GIC.
        let program = [0xd2a0_0041u32, 0xd297_dde2, 0xf900_0022, 0xd65f_03c0];
        let mut b = translated_board_with(&program, DATA_PA | 0x403);
        let options = translated_options();
        let result = checked_call(&mut b, options).unwrap();
        assert_eq!(result.failure, None);
        assert!(result.returned);
        assert_eq!(result.stores[0].address, DATA_PA);
        assert_eq!(result.stores[0].value, 0xbeef);
        let mut bytes = [0u8; 8];
        b.read_memory(0, DATA_PA, &mut bytes).unwrap();
        assert_eq!(u64::from_le_bytes(bytes), 0xbeef);
    }
    #[test]
    fn writing_sctlr_with_translation_off_retires() {
        // MOVZ x0,#0; MSR SCTLR_EL1,x0; RET. The same write with M clear only
        // replaces the declared register value, so an MMU-off routine still
        // runs to its return.
        let mut b = board(&[0xd280_0000, 0xd518_1000, 0xd65f_03c0]);
        let mut o = options();
        o.system_values = vec![("SCTLR_EL1".into(), 0x30d0_0800)];
        let r = checked_call(&mut b, o).unwrap();
        assert!(r.returned, "{:?}", r.failure);
        // The guard retires the serviced MSR without counting it, so the two
        // executed words are the MOVZ and the RET.
        assert_eq!(r.instructions, 2);
        assert_eq!(r.system_writes.len(), 1);
        assert_eq!(r.system_writes[0].name, "SCTLR_EL1");
        assert_eq!(r.system_writes[0].value, 0);
    }
    #[test]
    fn declared_system_writes_update_the_declared_value() {
        // MRS x1,S3_1_C15_C2_1; MOVZ x1,#7; MSR S3_1_C15_C2_1,x1;
        // MRS x2,S3_1_C15_C2_1; RET. The declaration is the register state.
        let mut b = board(&[
            0xd539_f221,
            0xd280_00e1,
            0xd519_f221,
            0xd539_f222,
            0xd65f_03c0,
        ]);
        let mut o = options();
        o.executable = vec![(4096, 4116)];
        o.system_values = vec![("S3_1_C15_C2_1".into(), 0x11)];
        let r = checked_call(&mut b, o).unwrap();
        assert!(r.returned);
        assert_eq!(r.registers["x1"], 7);
        assert_eq!(r.registers["x2"], 7);
        assert_eq!(r.system_read_observations.len(), 2);
        assert_eq!(r.system_read_observations[0].value, 0x11);
        assert_eq!(r.system_read_observations[1].value, 7);
        assert_eq!(r.system_writes.len(), 1);
        assert_eq!(r.system_writes[0].value, 7);
    }
    #[test]
    fn contradictory_system_register_declarations_are_refused() {
        let mut b = board(&[0xd539_f221, 0xd65f_03c0]);
        let mut o = options();
        o.system_values = vec![("S3_1_C15_C2_1".into(), 0)];
        o.system_registers = vec!["S3_1_C15_C2_1".into()];
        let error = match checked_call(&mut b, o) {
            Ok(_) => panic!("contradictory declarations were accepted"),
            Err(error) => error,
        };
        assert!(error.contains("both declared to the model and given a value"));
        let mut b = board(&[0xd539_f221, 0xd65f_03c0]);
        let mut o = options();
        o.system_values = vec![("S3_1_C15_C2_1".into(), 0)];
        o.system_reads = vec![("S3_1_C15_C2_1".into(), 0)];
        let error = match checked_call(&mut b, o) {
            Ok(_) => panic!("duplicate declaration was accepted"),
            Err(error) => error,
        };
        assert!(error.contains("Unsupported or duplicate system register"));
    }
    #[test]
    fn implementation_defined_names_are_systematic_and_declarable() {
        // MRS X1, S3_1_C15_C2_1 is the A53 CPUACTLR_EL1 read at 0x5b580.
        assert_eq!(
            system_register_name(0xcf91).as_deref(),
            Some("S3_1_C15_C2_1")
        );
        assert_eq!(
            parse_implementation_defined_name("S3_1_C15_C2_1"),
            Some(0xcf91)
        );
        assert_eq!(
            system_register_name(0xdf80).as_deref(),
            Some("S3_3_C15_C0_0")
        );
        assert_eq!(
            parse_implementation_defined_name("S3_3_C15_C0_0"),
            Some(0xdf80)
        );
        for (name, encoding) in [
            ("MIDR_EL1", 0xc000u16),
            ("CTR_EL0", 0xd801),
            ("ELR_EL3", 0xf201),
            ("CLIDR_EL1", 0xc801),
        ] {
            assert_eq!(system_register_name(encoding).as_deref(), Some(name));
            assert_eq!(admitted_system_register(name), Some(encoding));
        }
        // Only the implementation-defined space becomes a systematic name, so
        // an architectural register still needs its own table entry.
        assert_eq!(system_register_name(0xdb02), None);
        assert_eq!(system_register_name(0xda11).as_deref(), Some("DAIF"));
        assert_eq!(system_register_name(0xc210).as_deref(), Some("SPSel"));
        assert_eq!(admitted_system_register("DAIF"), Some(0xda11));
        assert_eq!(admitted_system_register("SPSel"), Some(0xc210));
        assert_eq!(parse_implementation_defined_name("S3_0_C15_C2_1"), None);
        assert_eq!(parse_implementation_defined_name("S4_1_C15_C2_1"), None);
        assert_eq!(parse_implementation_defined_name("S3_1_C15_C2"), None);
        assert_eq!(parse_implementation_defined_name("S3_x_C15_C2_1"), None);
    }
    #[test]
    fn implementation_defined_reads_require_a_declaration() {
        // MRS X1, S3_1_C15_C2_1; RET names no architectural value, so the call
        // refuses before retirement until the caller declares the register.
        let mut b = board(&[0xd539_f221, 0xd65f_03c0]);
        let r = checked_call(&mut b, options()).unwrap();
        assert_eq!(r.instructions, 0);
        assert!(r.failure.unwrap().contains("undeclared MRS"));
    }
    #[test]
    fn cache_and_tlb_maintenance_without_an_operand_register_retire() {
        // IC IALLU; TLBI VMALLE1; ADD X1,SP,#0; RET records the stack pointer
        // the two maintenance operations leave untouched.
        let mut b = board(&[0xd508_751f, 0xd508_871f, 0x9100_03e1, 0xd65f_03c0]);
        let r = checked_call(&mut b, options()).unwrap();
        assert!(r.returned, "{:?}", r.failure);
        assert_eq!(r.instructions, 4);
        assert_eq!(r.registers["x1"], 0x9000);
        assert_eq!(r.registers["sp"], 0x9000);
    }
    #[test]
    fn cache_and_tlbi_with_an_operand_register_retire() {
        // DC ISW requires an actual set/way operand (x11 here); TLBI VAE1
        // names an address in x0. Both leave no register or memory state
        // observable to this bounded non-MMU fragment.
        let mut b = board(&[0xd508_764b, 0xd508_871f, 0x9100_03e1, 0xd65f_03c0]);
        let r = checked_call(&mut b, options()).unwrap();
        assert!(r.returned, "{:?}", r.failure);
        assert_eq!(r.instructions, 4);
        assert_eq!(r.registers["x1"], 0x9000);
        assert_eq!(r.registers["sp"], 0x9000);
    }
    #[test]
    fn cache_and_tlbi_admission_accepts_documented_operations_only() {
        for word in [
            0xd508_751f, // ic iallu
            0xd508_711f, // ic ialluis
            0xd508_763f, // dc ivac, xzr
            0xd50b_753f, // ic ivau, xzr
            0xd50b_7c3f, // dc cvap, xzr
            0xd50b_7e3f, // dc civac, xzr
            0xd508_871f, // tlbi vmalle1
            0xd50c_879f, // tlbi alle1
            0xd508_8700, // tlbi vmalle1 (Rt is reserved)
            0xd508_8720, // tlbi vae1, x0
            0xd50e_871f, // tlbi alle3
            0xd50b_7520, // ic ivau, x0
        ] {
            assert!(is_cache_or_tlbi(word), "{word:#010x}");
        }
        for word in [
            0xd508_801f, // Reserved SYS #0,c8,c0,#0: never a TLBI operand
            0xd508_713f, // SYS #0,c7,c1,#1: not IC IALLUIS
            0xd50b_7540, // SYS #3,c7,c5,#2,x0: not IC IVAU
            0xd50a_871f, // Reserved op1=2 CRn=8 encoding
            0xd50e_839f, // SYS #6,c8,c3,#4: not a TLBI selector
            0xd508_841f, // Reserved CRm=4 EL1 encoding
            0xd508_883f, // Reserved CRm=8 EL1 encoding
            0xd50c_783f, // AT S1E2R, xzr: writes PAR_EL1
            0xd50b_743f, // DC ZVA, xzr: stores zeros to a cache line
            0xd50a_6a5f, // AT S12E1RW, xzr: writes PAR_EL1
            0xd508_7b9f, // SYS #0, c7, c11, #4: reserved cache operand
            0xd503_201f, // nop
            0xd400_41bf, // svc #0x20d
            0xd440_0000, // hlt #0
            0xd518_1000, // msr tcr_el1, x0
            0xd538_0000, // mrs x0, midr_el1
        ] {
            assert!(!is_cache_or_tlbi(word), "{word:#010x}");
        }
    }
    #[test]
    fn default_undeclared_and_other_system_instructions_remain_refused() {
        for (word, declared) in [
            (0xd5380000, false),
            (0xd538001f, true),
            (0xd5181000, true),
            (0xd503229f, true), // CSDB is a hint this harness does not model.
        ] {
            let mut b = board(&[word, 0xd65f03c0]);
            let mut o = options();
            if declared {
                o.system_reads = vec![("TCR_EL1".into(), 0)];
            }
            let r = checked_call(&mut b, o).unwrap();
            assert_eq!(r.instructions, 0, "{word:#x}");
            assert!(r.failure.is_some());
        }
    }
    #[test]
    fn declared_read_does_not_admit_out_of_bounds_store() {
        let mut b = board(&[0xd5380000, 0xf9000020, 0xd65f03c0]);
        let midr = b
            .read_register(
                0,
                Register::Arm64System(arm64::system_register("MIDR_EL1").unwrap()),
            )
            .unwrap();
        let mut o = options();
        o.arguments = vec![0, 0x4008];
        o.system_reads = vec![("MIDR_EL1".into(), midr)];
        let r = checked_call(&mut b, o).unwrap();
        assert_eq!(r.instructions, 1);
        assert!(r.failure.is_some());
        assert!(r.stores.is_empty());
    }
    #[test]
    fn each_declared_register_read_matches_the_cpu_model() {
        for name in [
            "MIDR_EL1",
            "MPIDR_EL1",
            "CurrentEL",
            "SCTLR_EL1",
            "TCR_EL1",
            "TTBR0_EL1",
            "TTBR1_EL1",
            "MAIR_EL1",
        ] {
            let encoding = admitted_system_register(name).unwrap();
            let mut b = board(&[0xd5200000 | ((encoding as u32) << 5), 0xd65f03c0]);
            let expected = if name == "CurrentEL" {
                4
            } else {
                model_value(b.cpu(0).unwrap(), name, encoding).unwrap()
            };
            let mut o = options();
            o.system_reads = vec![(name.into(), expected)];
            let r = checked_call(&mut b, o).unwrap();
            assert!(r.returned, "{name}: {:?}", r.failure);
            assert_eq!(r.registers["x0"], expected, "{name}");
            assert_eq!(r.system_read_observations[0].value, expected, "{name}");
        }
    }
    #[test]
    fn declared_xzr_read_and_budget_are_checked() {
        let mut b = board(&[0xd538001f, 0xd65f03c0]);
        let midr = model_value(b.cpu(0).unwrap(), "MIDR_EL1", 0xc000).unwrap();
        let mut o = options();
        o.max_instructions = 1;
        o.system_reads = vec![("MIDR_EL1".into(), midr)];
        let r = checked_call(&mut b, o).unwrap();
        assert_eq!(r.instructions, 1);
        assert!(r.failure.unwrap().contains("Instruction limit"));
        assert_eq!(r.system_read_observations[0].destination, 31);
    }
    #[test]
    fn system_write_is_rejected_before_retirement() {
        let mut b = board(&[0xd5181000, 0xd65f03c0]);
        let r = checked_call(&mut b, options()).unwrap();
        assert_eq!(r.instructions, 0);
        assert!(r.failure.unwrap().contains("Unsupported instruction"));
    }
    #[test]
    fn exclusive_store_after_matching_load_stores_and_reports_zero() {
        // LDXR W1,[X0]; STXR W2,W1,[X0]; RET.
        let mut b = board(&[0x885f7c01, 0x88027c01, 0xd65f03c0]);
        b.write_memory(0, 0x4000, &0x1122_3344u32.to_le_bytes())
            .unwrap();
        let mut o = options();
        o.arguments = vec![0x4000];
        let r = checked_call(&mut b, o).unwrap();
        assert!(r.returned, "{:?}", r.failure);
        assert_eq!(r.registers["x1"], 0x1122_3344);
        assert_eq!(r.registers["x2"], 0);
        assert_eq!(r.stores.len(), 1);
        assert_eq!((r.stores[0].address, r.stores[0].size), (0x4000, 4));
        assert_eq!(r.stores[0].value, 0x1122_3344);
        let mut bytes = [0; 4];
        b.read_memory(0, 0x4000, &mut bytes).unwrap();
        assert_eq!(bytes, 0x1122_3344u32.to_le_bytes());
    }
    #[test]
    fn exclusive_store_without_monitor_reports_one_and_does_not_store() {
        // STXR W2,W1,[X0]; RET with no preceding exclusive load.
        let mut b = board(&[0x88027c01, 0xd65f03c0]);
        let mut o = options();
        o.arguments = vec![0x4000, 0xdead_beef];
        let r = checked_call(&mut b, o).unwrap();
        assert!(r.returned, "{:?}", r.failure);
        assert_eq!(r.registers["x2"], 1);
        assert!(r.stores.is_empty());
        let mut bytes = [0; 4];
        b.read_memory(0, 0x4000, &mut bytes).unwrap();
        assert_eq!(bytes, [0; 4]);
    }
    #[test]
    fn clrex_clears_the_exclusive_monitor() {
        // LDXR W1,[X0]; CLREX; STXR W2,W1,[X0]; RET.
        let mut b = board(&[0x885f7c01, 0xd5033f5f, 0x88027c01, 0xd65f03c0]);
        b.write_memory(0, 0x4000, &0x1122_3344u32.to_le_bytes())
            .unwrap();
        let mut o = options();
        o.arguments = vec![0x4000];
        let r = checked_call(&mut b, o).unwrap();
        assert!(r.returned, "{:?}", r.failure);
        assert_eq!(r.registers["x2"], 1);
        assert!(r.stores.is_empty());
        let mut bytes = [0; 4];
        b.read_memory(0, 0x4000, &mut bytes).unwrap();
        assert_eq!(bytes, 0x1122_3344u32.to_le_bytes());
    }
    #[test]
    fn intervening_store_to_monitored_address_fails_exclusive_store() {
        // LDXR W1,[X0]; STR W3,[X0]; STXR W2,W1,[X0]; RET.
        let mut b = board(&[0x885f7c01, 0xb9000003, 0x88027c01, 0xd65f03c0]);
        let mut o = options();
        o.arguments = vec![0x4000, 0, 0, 0x55];
        let r = checked_call(&mut b, o).unwrap();
        assert!(r.returned, "{:?}", r.failure);
        assert_eq!(r.registers["x2"], 1);
        assert_eq!(r.stores.len(), 1);
        assert_eq!((r.stores[0].address, r.stores[0].value), (0x4000, 0x55));
        let mut bytes = [0; 4];
        b.read_memory(0, 0x4000, &mut bytes).unwrap();
        assert_eq!(bytes, 0x55u32.to_le_bytes());
    }
    #[test]
    fn mismatched_exclusive_width_fails() {
        // LDXR W1,[X0]; STXRH W2,W1,[X0]; RET.
        let mut b = board(&[0x885f7c01, 0x48027c01, 0xd65f03c0]);
        let mut o = options();
        o.arguments = vec![0x4000, 0x1234_5678];
        let r = checked_call(&mut b, o).unwrap();
        assert!(r.returned, "{:?}", r.failure);
        assert_eq!(r.registers["x2"], 1);
        assert!(r.stores.is_empty());
        let mut bytes = [0; 4];
        b.read_memory(0, 0x4000, &mut bytes).unwrap();
        assert_eq!(bytes, [0; 4]);
    }
    #[test]
    fn exclusive_access_obeys_declared_ranges() {
        // LDXR W1,[X0] where X0 points outside readable ranges: refused.
        let mut b = board(&[0x885f7c01, 0xd65f03c0]);
        let mut o = options();
        o.arguments = vec![0x5000];
        let r = checked_call(&mut b, o).unwrap();
        assert!(!r.returned);
        assert!(
            r.failure
                .unwrap()
                .contains("exclusive read outside declared RAM")
        );
        assert!(r.stores.is_empty());
    }
    #[test]
    fn misaligned_exclusive_load_is_refused_before_retirement() {
        // LDXR W1,[X0]; RET with a four-byte access at an unaligned address.
        let mut b = board(&[0x885f7c01, 0xd65f03c0]);
        let mut o = options();
        o.arguments = vec![0x4001];
        let r = checked_call(&mut b, o).unwrap();
        assert!(!r.returned);
        assert_eq!(r.instructions, 0);
        assert!(r.failure.unwrap().contains("unaligned exclusive access"));
        assert!(r.stores.is_empty());
    }
    #[test]
    fn el3_entry_writes_elr_reads_spsr_and_erets_to_el1() {
        // MSR ELR_EL3,x0; MRS x1,SPSR_EL3; ISB; ERET; RET (at x0).
        let mut b = board(&[
            0xd51e_4020,
            0xd53e_4001,
            0xd503_3fdf,
            0xd69f_03e0,
            0xd65f_03c0,
        ]);
        b.write_register(0, sysreg(0xf200), 0x3c5).unwrap(); // SPSR_EL3: EL1h.
        // Set the active SP at the board's EL1h reset level; the entry's EL3
        // PSTATE switch saves it into SP_EL1 so `eret` lands on the same SP.
        b.write_register(0, Register::Sp, 0x9000).unwrap();
        b.write_register(0, sysreg(0xf088), 0x401).unwrap(); // SCR_EL3: AArch64 lower EL.
        b.write_register(0, sysreg(0xe088), 0x80000000).unwrap(); // HCR_EL2: AArch64 EL1.
        let mut o = options();
        o.entry_level = Some(EntryLevel::El3);
        o.pstate = Some(0x3cd);
        o.arguments = vec![0x1010];
        o.system_registers = vec!["spsr_el3".into(), "elr_el3".into()];
        o.system_reads = vec![("SPSR_EL3".into(), 0x3c5)];
        o.executable = vec![(0x1000, 0x1014)];
        let r = checked_call(&mut b, o).unwrap();
        assert!(r.returned, "{:?}", r.failure);
        assert_eq!(r.registers["x1"], 0x3c5);
        assert_eq!(r.registers["pstate"] & 0xf, 0b0101);
        assert_eq!(r.system_writes.len(), 1);
        assert_eq!(r.system_writes[0].pc, 0x1000);
        assert_eq!(r.system_writes[0].name, "ELR_EL3");
        assert_eq!(r.system_writes[0].value, 0x1010);
    }
    #[test]
    fn el1_entry_eret_refuses_with_el3_message() {
        let mut b = board(&[0xd69f_03e0, 0xd65f_03c0]);
        let r = checked_call(&mut b, options()).unwrap();
        assert_eq!(r.instructions, 0);
        assert!(
            r.failure
                .unwrap()
                .contains("EL3 eret requires an EL3 entry level")
        );
    }
    #[test]
    fn el3_entry_undeclared_msr_refuses() {
        // MSR SCTLR_EL1,x0 without any system_registers declaration.
        let mut b = board(&[0xd5181000, 0xd65f_03c0]);
        let mut o = options();
        o.entry_level = Some(EntryLevel::El3);
        o.pstate = Some(0x3cd);
        let r = checked_call(&mut b, o).unwrap();
        assert_eq!(r.instructions, 0);
        assert!(r.failure.unwrap().contains("undeclared MSR"));
    }
    #[test]
    fn el1_entry_el3_register_read_refuses() {
        // MRS x1,SPSR_EL3; RET.
        let mut b = board(&[0xd53e_4001, 0xd65f_03c0]);
        b.write_register(0, sysreg(0xf200), 0x3c5).unwrap();
        let mut o = options();
        o.system_reads = vec![("SPSR_EL3".into(), 0x3c5)];
        let r = checked_call(&mut b, o).unwrap();
        assert_eq!(r.instructions, 0);
        assert!(
            r.failure
                .unwrap()
                .contains("EL3 system register SPSR_EL3 requires an EL3 entry level")
        );
    }
    #[test]
    fn isb_alone_runs_to_completion() {
        let mut b = board(&[0xd503_3fdf, 0xd65f_03c0]);
        let r = checked_call(&mut b, options()).unwrap();
        assert!(r.returned, "{:?}", r.failure);
        assert_eq!(r.instructions, 2);
    }
    #[test]
    fn msr_immediate_spsel_selects_the_stack_bank() {
        let sp_el0 = sysreg(0xc208);
        let sp_el1 = sysreg(0xe208);
        let _ = (sp_el0, sp_el1);
        // EL1h fragment: SPSel=#0 picks SP_EL0 (=0xaa80), ADD captures it,
        // SPSel=#1 picks SP_EL1 (lee default harness stack = 0x9000).
        // Declaring the post-swap SP as the fragment's exit stack lets the
        // callee-save validator accept the bank without warning.
        let mut b = board(&[0xd500_40bf, 0x9100_03e1, 0xd500_41bf, 0xd65f_03c0]);
        b.write_register(0, sp_el0, 0xaa80).unwrap();
        b.write_register(0, sp_el1, 0xbbc0).unwrap();
        let mut o = options();
        o.executable = vec![(0x1000, 0x1010)];
        o.readable = vec![(0x1000, 0x1010), (0x4000, 0x4010), (0x8000, 0xa000)];
        o.fragment_exit.insert("sp".into(), 0x9000);
        let r = checked_call(&mut b, o).unwrap();
        assert!(r.returned, "{:?}", r.failure);
        assert_eq!(r.instructions, 4);
        assert_eq!(r.registers["x1"], 0xaa80);
        assert_eq!(r.registers["pstate"] & 1, 1);
        assert_eq!(r.registers["sp"], 0x9000);
    }
    #[test]
    fn msr_immediate_daif_clear_and_set_age_pstate() {
        // MSR DAIFClr,#3 unmasks F and I; RET runs with them clear.
        let mut b = board(&[0xd503_43ff, 0xd65f_03c0]);
        let r = checked_call(&mut b, options()).unwrap();
        assert!(r.returned, "{:?}", r.failure);
        assert_eq!(r.instructions, 2);
        assert_eq!(r.registers["pstate"] & 0x3c0, 0x300);
        // MSR DAIFSet,#3 masks them again.
        let mut b = board(&[0xd503_43df, 0xd65f_03c0]);
        let r = checked_call(&mut b, options()).unwrap();
        assert!(r.returned, "{:?}", r.failure);
        assert_eq!(r.instructions, 2);
        assert_eq!(r.registers["pstate"] & 0x3c0, 0x3c0);
    }
    #[test]
    fn trap_instruction_sharing_the_low_bits_stays_refused() {
        // SVC #0x20d: Rt=31 low bits from the immediate class, other top bits.
        let mut b = board(&[0xd400_41bf, 0xd65f_03c0]);
        let r = checked_call(&mut b, options()).unwrap();
        assert_eq!(r.instructions, 0);
        assert!(r.failure.unwrap().contains("Unsupported instruction"));
    }
    #[test]
    fn el3_eret_non_aarch64_spsr_mode_refuses() {
        // ERET with SPSR_EL3 M[3:0] = 0b0001; RET is never reached.
        let mut b = board(&[0xd69f_03e0, 0xd65f_03c0]);
        b.write_register(0, sysreg(0xf200), 0x3c1).unwrap();
        let mut o = options();
        o.entry_level = Some(EntryLevel::El3);
        o.pstate = Some(0x3cd);
        let r = checked_call(&mut b, o).unwrap();
        assert_eq!(r.instructions, 0);
        assert!(
            r.failure
                .unwrap()
                .contains("eret refuses a non-AArch64 SPSR_EL3 mode")
        );
    }
}
