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
#[derive(Serialize)]
pub struct RegisterObservation {
    pub pc: u64,
    pub registers: std::collections::BTreeMap<String, u64>,
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
    #[serde(default)]
    pub stop_addresses: Vec<u64>,
    pub entry: u64,
    pub arguments: Vec<u64>,
    pub return_address: u64,
    pub stack_pointer: u64,
    pub stack: Span,
    pub max_instructions: u64,
    pub quantum: usize,
    pub executable: Vec<Span>,
    pub readable: Vec<Span>,
    pub writable: Vec<Span>,
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
    pending: Option<SystemReadObservation>,
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
}
struct Guard {
    cpu: Weak<dyn Cpu>,
    pause: Rc<Cell<bool>>,
    options: CheckedOptions,
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

        _ => return None,
    })
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
        if pc & 3 != 0 || size != 4 || !contains(&self.options.executable, pc, 4) {
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
        // Exact MRS class: all other system instructions retain the default refusal.
        if word & 0xffe0_0000 == 0xd520_0000 {
            let encoding = ((word >> 5) & 0xffff) as u16;
            let (name, expected) = self
                .options
                .system_reads
                .iter()
                .find(|(name, _)| admitted_system_register(name) == Some(encoding))
                .ok_or_else(|| {
                    format!("Unsupported instruction {word:#010x} at {pc:#x}: undeclared MRS")
                })?;
            let value = model_value(&*cpu, name, encoding)?;
            if value != *expected {
                return Err(format!(
                    "System register model value changed before MRS at {pc:#x}"
                ));
            }
            self.observed.borrow_mut().pending = Some(SystemReadObservation {
                pc,
                name: name.clone(),
                encoding,
                destination: (word & 31) as u8,
                value,
            });
            return Ok(());
        }
        if word & 0x0a00_0000 == 0x0800_0000 {
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
                let address = transfer.address;
                let width = u64::from(transfer.size);
                let allowed = if decoded.load {
                    &self.options.readable
                } else {
                    &self.options.writable
                };
                if !contains(allowed, address, width) {
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
        // DSB SY and DMB ISH are ordering points in this single-core, bus-free
        // routine: admitted RAM stores complete before the next instruction.
        if matches!(word, 0xd503_201f | 0xd503_3f9f | 0xd503_3bbf)
            || integer
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
        guard.observed.borrow_mut().stores.push(Store {
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
/// Run a device-free, identity-mapped, MMU-off caller with strict guards.
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
    if !board.state.borrow().bus.is_empty()
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
        pstate: Some(0x3c5),
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
    if options.system_reads.len() > 9 {
        return Err("Too many declared system reads".into());
    }
    let mut names = std::collections::BTreeSet::new();
    for (name, _) in &options.system_reads {
        if admitted_system_register(name).is_none() || !names.insert(name) {
            return Err(format!("Unsupported or duplicate system register {name}"));
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
    let mut guard = Box::new(Guard {
        cpu: Rc::downgrade(&board.cpus[0]),
        pause: board.host_pause.clone(),
        options,
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
    cleanup_code?;
    cleanup_write?;
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::{
        bus::{Bus, Mode},
        factory,
    };
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
                factory::Options::new(factory::Engine::Unicorn)
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
        serde_json::from_value(json!({"entry":4096,"arguments":[],"return_address":45056,
            "stack_pointer":36864,"stack":[32768,40960],"max_instructions":20,"quantum":10,
            "executable":[[4096,4112]],"readable":[[4096,4112],[16384,16400],[32768,40960]],
            "writable":[[16384,16392],[32768,40960]],"store_limit":20,"stubs":[],"registers":[]}))
        .unwrap()
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
        let mut value = json!({"entry":4096,"arguments":[],"return_address":45056,
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
    fn default_undeclared_and_other_system_instructions_remain_refused() {
        for (word, declared) in [
            (0xd5380000, false),
            (0xd538001f, true),
            (0xd5181000, true),
            (0xd5033b9f, true), // DSB ISH remains outside this policy.
            (0xd5033fbf, true), // DMB SY remains outside this policy.
            (0xd5033fdf, true), // ISB remains outside this policy.
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
}
