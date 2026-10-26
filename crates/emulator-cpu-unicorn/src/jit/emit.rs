//! Wasm emission from admitted TCI. Host functions are imports, never JS objects.
use super::{BasicBlock, Binary, Compiler, Environment, PreparedBlock, Registers, ir::*};
use std::{collections::BTreeMap, sync::Arc};

#[derive(Debug)]
pub struct Module {
    pub bytes: Vec<u8>,
    pub exports: Vec<String>,
}
#[derive(Clone, Debug)]
pub struct PreparedFunction(pub(super) Arc<[u8]>);
impl PreparedFunction {
    pub fn bytes(&self) -> &[u8] {
        &self.0
    }
}
#[path = "helper.rs"]
mod helper;
#[path = "memory.rs"]
mod memory;

const CODE_BASE: u32 = 3;
const DISPATCH: u32 = 20;
const ADDRESS: u32 = 21;
const ENTRY: u32 = 22;
const DESCRIPTOR: u32 = 23;
const WIDE: u32 = 24;
const ARGUMENTS: [u8; 12] = [0, 1, 2, 3, 5, 6, 7, 8, 9, 10, 11, 12];

struct Body {
    binary: Binary,
    source: u32,
    locals: [u32; 26],
    forward_block: Option<u32>,
}
impl Body {
    fn emit(&mut self, bytes: &[u8]) {
        self.binary.extend(bytes);
    }
    fn get(&mut self, local: u32) {
        self.emit(&[0x20]);
        self.binary.unsigned(self.locals[local as usize]);
    }
    fn set(&mut self, local: u32) {
        self.emit(&[0x21]);
        self.binary.unsigned(self.locals[local as usize]);
    }
    fn forward_branch(&mut self, target: u32, conditional: bool) -> Result<(), String> {
        let current = self
            .forward_block
            .ok_or("forward branch outside a structured body")?;
        let depth = target
            .checked_sub(current + 1)
            .ok_or("backward structured branch")?;
        self.emit(&[if conditional { 0x0d } else { 0x0c }]);
        self.binary.unsigned(depth);
        Ok(())
    }
    fn constant(&mut self, value: u32) {
        self.emit(&[0x41]);
        self.binary.signed(i64::from(value as i32));
    }
    fn source_offset(&self, address: u32) -> Result<u32, String> {
        address
            .checked_sub(self.source)
            .ok_or_else(|| "TCI source address precedes body".into())
    }
    fn source_address(&mut self, address: u32) -> Result<(), String> {
        let offset = self.source_offset(address)?;
        self.get(CODE_BASE);
        if offset != 0 {
            self.constant(offset);
            self.emit(&[0x6a]);
        }
        Ok(())
    }
    fn wide(&mut self, value: u64) {
        self.emit(&[0x42]);
        self.binary.signed(value as i64);
    }
    fn reg(&mut self, r: Register) {
        self.get(u32::from(r.index()) + 4);
    }
    fn put(&mut self, r: Register) {
        self.set(u32::from(r.index()) + 4);
    }
    fn value(&mut self, value: Value) {
        match value {
            Value::Register(r) => self.reg(r),
            Value::Constant(n) => self.constant(n),
        }
    }
    fn mem(&mut self, opcode: u8, alignment: u32, offset: u32) {
        self.emit(&[opcode]);
        self.binary.unsigned(alignment);
        self.binary.unsigned(offset);
    }
    fn pair(&mut self, pair: [Value; 2]) {
        self.value(pair[0]);
        self.emit(&[0xad]);
        self.value(pair[1]);
        self.emit(&[0xad]);
        self.wide(32);
        self.emit(&[0x86, 0x84]);
    }
    fn split(&mut self, pair: [Register; 2]) {
        self.get(WIDE);
        self.emit(&[0xa7]);
        self.put(pair[0]);
        self.get(WIDE);
        self.wide(32);
        self.emit(&[0x88, 0xa7]);
        self.put(pair[1]);
    }
    fn address(&mut self, base: Register, offset: i32) {
        self.reg(base);
        self.constant(offset as u32);
        self.emit(&[0x6a]);
    }
    fn spill(&mut self, registers: impl IntoIterator<Item = Register>) {
        for r in registers {
            self.get(1);
            self.reg(r);
            self.mem(0x36, 2, u32::from(r.index()) * 4);
        }
    }
    fn operation(
        &mut self,
        instruction: &Instruction,
        targets: &BTreeMap<u32, u32>,
        modified: Registers,
        environment: &Environment,
    ) -> Result<bool, String> {
        let r = |n| Register::new(n).expect("fixed TCI register");
        let target = |address: &u32| {
            targets
                .get(address)
                .copied()
                .ok_or("invalid TCG branch target")
        };
        match &instruction.operation {
            Operation::Binary { op, dst, lhs, rhs } => {
                self.value(*lhs);
                self.value(*rhs);
                self.emit(&[match op {
                    BinaryOp::Add => 0x6a,
                    BinaryOp::Sub => 0x6b,
                    BinaryOp::Mul => 0x6c,
                    BinaryOp::And => 0x71,
                    BinaryOp::Or => 0x72,
                    BinaryOp::Xor => 0x73,
                    BinaryOp::Shl => 0x74,
                    BinaryOp::Shr => 0x76,
                    BinaryOp::Sar => 0x75,
                    BinaryOp::Rotl => 0x77,
                    BinaryOp::Rotr => 0x78,
                }]);
                self.put(*dst);
            }
            Operation::Unary { op, dst, src } => {
                if *op == UnaryOp::Neg {
                    self.constant(0);
                }
                if matches!(op, UnaryOp::Swap16 | UnaryOp::Swap32) {
                    let width = if *op == UnaryOp::Swap16 { 2 } else { 4 };
                    for byte in 0..width {
                        self.reg(*src);
                        self.constant(byte * 8);
                        self.emit(&[0x76]);
                        self.constant(255);
                        self.emit(&[0x71]);
                        self.constant((width - 1 - byte) * 8);
                        self.emit(&[0x74]);
                        if byte != 0 {
                            self.emit(&[0x72]);
                        }
                    }
                } else {
                    self.reg(*src);
                    match op {
                        UnaryOp::Move => {}
                        UnaryOp::Ext8Signed => self.emit(&[0xc0]),
                        UnaryOp::Ext16Signed => self.emit(&[0xc1]),
                        UnaryOp::Ext8Unsigned => {
                            self.constant(255);
                            self.emit(&[0x71]);
                        }
                        UnaryOp::Ext16Unsigned => {
                            self.constant(65535);
                            self.emit(&[0x71]);
                        }
                        UnaryOp::Not => {
                            self.constant(u32::MAX);
                            self.emit(&[0x73]);
                        }
                        UnaryOp::Neg => self.emit(&[0x6b]),
                        UnaryOp::Swap16 | UnaryOp::Swap32 => unreachable!(),
                    }
                }
                self.put(*dst);
            }
            Operation::Constant { dst, value } => {
                self.constant(*value);
                self.put(*dst);
            }
            Operation::HostMemory {
                op,
                data,
                base,
                offset,
            } => {
                self.address(*base, *offset);
                match op {
                    HostMemory::Load32 | HostMemory::Load8Unsigned => {
                        self.mem(
                            if *op == HostMemory::Load32 {
                                0x28
                            } else {
                                0x2d
                            },
                            0,
                            0,
                        );
                        self.put(*data);
                    }
                    HostMemory::Store32 | HostMemory::Store16 | HostMemory::Store8 => {
                        self.reg(*data);
                        self.mem(
                            match op {
                                HostMemory::Store32 => 0x36,
                                HostMemory::Store16 => 0x3b,
                                HostMemory::Store8 => 0x3a,
                                _ => unreachable!(),
                            },
                            0,
                            0,
                        );
                    }
                }
            }
            Operation::Compare {
                dst,
                lhs,
                rhs,
                condition,
            } => {
                self.reg(*lhs);
                self.value(*rhs);
                self.emit(&[comparison(*condition, false)]);
                self.put(*dst);
            }
            Operation::CompareDouble {
                dst,
                lhs,
                rhs,
                condition,
            } => {
                self.pair(lhs.map(Value::Register));
                self.pair(*rhs);
                self.emit(&[comparison(*condition, true)]);
                self.put(*dst);
            }
            Operation::Deposit {
                dst,
                lhs,
                rhs,
                position,
                length,
            } => {
                if *length == 0 || *length > 31 || u16::from(*position) + u16::from(*length) > 32 {
                    return Err("unsupported TCG deposit extent".into());
                }
                let mask = (((1u64 << length) - 1) << position) as u32;
                self.reg(*lhs);
                self.constant(!mask);
                self.emit(&[0x71]);
                self.reg(*rhs);
                self.constant(u32::from(*position));
                self.emit(&[0x74]);
                self.constant(mask);
                self.emit(&[0x71, 0x72]);
                self.put(*dst);
            }
            Operation::Pair { op, dst, lhs, rhs } => {
                self.pair(lhs.map(Value::Register));
                self.pair(rhs.map(Value::Register));
                self.emit(&[match op {
                    PairOp::Add => 0x7c,
                    PairOp::Sub => 0x7d,
                }]);
                self.set(WIDE);
                self.split(*dst);
            }
            Operation::MultiplyUnsigned { dst, lhs, rhs } => {
                self.reg(*lhs);
                self.emit(&[0xad]);
                self.reg(*rhs);
                self.emit(&[0xad, 0x7e]);
                self.set(WIDE);
                self.split(*dst);
            }
            Operation::GuestMemory {
                store,
                data,
                address,
                memop,
            } => {
                self.memory(
                    *store,
                    *data,
                    *address,
                    *memop,
                    instruction.pc,
                    environment.memory.as_ref(),
                )?;
                if !store {
                    self.set(WIDE);
                    match data {
                        Data::Word(r) => {
                            self.get(WIDE);
                            self.emit(&[0xa7]);
                            self.put(*r);
                        }
                        Data::Double(pair) => self.split(*pair),
                    }
                }
            }
            Operation::Call {
                function,
                words,
                mask,
            } => {
                if self.typed_helper(*function, *words, *mask, instruction.pc, environment)? {
                    return Ok(false);
                }
                let arguments = ARGUMENTS
                    .get(..*words as usize)
                    .ok_or("TCG helper argument count exceeds ABI")?;
                self.spill(arguments.iter().map(|n| r(*n)));
                self.get(0);
                self.value(*function);
                self.get(1);
                self.get(2);
                self.constant(*words);
                self.constant(*mask);
                self.source_address(instruction.pc)?;
                self.emit(&[0x10, 2, 0x04, 0x40]);
                self.wide(0);
                self.emit(&[0x0f, 0x0b]);
                for n in 0..2 {
                    self.get(1);
                    self.mem(0x28, 2, n * 4);
                    self.put(r(n as u8));
                }
            }
            Operation::Branch { target: address } => {
                if self.forward_block.is_some() {
                    self.forward_branch(target(address)?, false)?;
                } else {
                    self.constant(target(address)?);
                    self.set(DISPATCH);
                    self.emit(&[0x0c, 1]);
                }
                return Ok(true);
            }
            Operation::BranchIf {
                lhs,
                rhs,
                condition,
                target: address,
            } => {
                if self.forward_block.is_some() {
                    self.reg(*lhs);
                    self.value(*rhs);
                    self.emit(&[comparison(*condition, false)]);
                    self.forward_branch(target(address)?, true)?;
                } else {
                    self.constant(target(address)?);
                    self.constant(target(&instruction.next)?);
                    self.reg(*lhs);
                    self.value(*rhs);
                    self.emit(&[comparison(*condition, false), 0x1b]);
                    self.set(DISPATCH);
                    self.emit(&[0x0c, 1]);
                }
                return Ok(true);
            }
            Operation::BranchIfDouble {
                lhs,
                rhs,
                condition,
                target: address,
            } => {
                if self.forward_block.is_some() {
                    self.pair(lhs.map(Value::Register));
                    self.pair(*rhs);
                    self.emit(&[comparison(*condition, true)]);
                    self.forward_branch(target(address)?, true)?;
                } else {
                    self.constant(target(address)?);
                    self.constant(target(&instruction.next)?);
                    self.pair(lhs.map(Value::Register));
                    self.pair(*rhs);
                    self.emit(&[comparison(*condition, true), 0x1b]);
                    self.set(DISPATCH);
                    self.emit(&[0x0c, 1]);
                }
                return Ok(true);
            }
            Operation::Goto { link } => {
                self.spill(modified.iter());
                let offset = self.source_offset(*link)?;
                self.get(CODE_BASE);
                self.mem(0x28, 0, offset);
                self.source_address(link.checked_add(4).ok_or("TCI link address overflow")?)?;
                self.emit(&[0x6a, 0xad, 0x0f]);
                return Ok(true);
            }
            Operation::Exit { .. } => {
                // The immediate contains this generation's native TB token.
                // Read it from current bytecode, just like the patched goto link.
                self.get(CODE_BASE);
                self.mem(0x28, 0, self.source_offset(instruction.pc)?);
                self.emit(&[0xad]);
                self.wide(32);
                self.emit(&[0x86, 0x0f]);
                return Ok(true);
            }
            Operation::Barrier => {}
        }
        Ok(false)
    }
}
fn comparison(condition: Condition, wide: bool) -> u8 {
    let pair = match condition {
        Condition::Equal => [0x46, 0x51],
        Condition::NotEqual => [0x47, 0x52],
        Condition::LessSigned => [0x48, 0x53],
        Condition::GreaterEqualSigned => [0x4e, 0x59],
        Condition::LessEqualSigned => [0x4c, 0x57],
        Condition::GreaterSigned => [0x4a, 0x55],
        Condition::LessUnsigned => [0x49, 0x54],
        Condition::GreaterEqualUnsigned => [0x4f, 0x5a],
        Condition::LessEqualUnsigned => [0x4d, 0x58],
        Condition::GreaterUnsigned => [0x4b, 0x56],
    };
    pair[usize::from(wide)]
}
fn function_body(prepared: &PreparedBlock, environment: &Environment) -> Result<Binary, String> {
    // Bound nesting, and retain the general dispatcher for every backward edge.
    let forward = prepared.blocks.len() <= 128
        && prepared
            .blocks
            .iter()
            .enumerate()
            .all(|(index, block)| block.successors.iter().all(|target| *target > index))
        && matches!(
            prepared.instructions.last().map(|i| &i.operation),
            Some(Operation::Goto { .. } | Operation::Exit { .. })
        );
    // Parameters keep their ABI indices. Allocate only TCI registers this body
    // reads or modifies, followed by its control/memory scratch and wide pair.
    let mut locals = [u32::MAX; 26];
    for n in 0..4 {
        locals[n] = n as u32;
    }
    let mut next = 4;
    for n in 0..16 {
        let r = Register::new(n).expect("bounded TCI register");
        if prepared.incoming.contains(r) || prepared.modified.contains(r) {
            locals[usize::from(n) + 4] = next;
            next += 1;
        }
    }
    for logical in [DISPATCH, ADDRESS, ENTRY, DESCRIPTOR] {
        if logical != DISPATCH || !forward {
            locals[logical as usize] = next;
            next += 1;
        }
    }
    let integers = next - 4;
    locals[WIDE as usize] = next;
    locals[(WIDE + 1) as usize] = next + 1;
    let mut body = Body {
        binary: Binary::default(),
        source: prepared.instructions[0].address,
        locals,
        forward_block: None,
    };
    body.emit(&[2, integers as u8, 0x7f, 2, 0x7e]);
    for register in prepared.incoming.iter() {
        body.get(1);
        body.mem(0x28, 2, u32::from(register.index()) * 4);
        body.put(register);
    }
    let targets = prepared
        .blocks
        .iter()
        .enumerate()
        .map(|(n, block)| {
            (
                prepared.instructions[block.instructions.start].address,
                n as u32,
            )
        })
        .collect();
    if forward {
        // A forward target is the end of its nested block. At block n, depth 0
        // reaches n+1, depth 1 reaches n+2, and so on. Fallthrough closes one
        // block naturally; no dispatch local or repeated block selection runs.
        for _ in 1..prepared.blocks.len() {
            body.emit(&[0x02, 0x40]);
        }
        for (index, block) in prepared.blocks.iter().enumerate() {
            if index != 0 {
                body.emit(&[0x0b]);
            }
            body.forward_block = Some(index as u32);
            for instruction in &prepared.instructions[block.instructions.clone()] {
                if body.operation(instruction, &targets, prepared.modified, environment)? {
                    break;
                }
            }
        }
        body.emit(&[0x0b]);
        return Ok(body.binary);
    }
    body.constant(0);
    body.set(DISPATCH);
    body.emit(&[0x03, 0x40]);
    for (n, BasicBlock { instructions, .. }) in prepared.blocks.iter().enumerate() {
        body.get(DISPATCH);
        body.constant(n as u32);
        body.emit(&[0x46, 0x04, 0x40]);
        let mut terminated = false;
        for instruction in &prepared.instructions[instructions.clone()] {
            terminated = body.operation(instruction, &targets, prepared.modified, environment)?;
            if terminated {
                break;
            }
        }
        if !terminated {
            body.constant(n as u32 + 1);
            body.set(DISPATCH);
            body.emit(&[0x0c, 1]);
        }
        body.emit(&[0x0b]);
    }
    body.emit(&[0x00, 0x0b]);
    body.wide(0);
    body.emit(&[0x0b]);
    Ok(body.binary)
}
fn signature(body: &mut Binary, parameters: &[u8], results: &[u8]) {
    body.byte(0x60);
    body.unsigned(parameters.len() as u32);
    body.extend(parameters);
    body.unsigned(results.len() as u32);
    body.extend(results);
}
fn import(body: &mut Binary, name: &str, kind: u8) -> Result<(), String> {
    body.text("env")?;
    body.text(name)?;
    body.byte(kind);
    Ok(())
}
fn module_header() -> Binary {
    let mut b = Binary::default();
    b.extend(&[0, 97, 115, 109, 1, 0, 0, 0]);
    b
}
impl Compiler {
    pub fn lower(
        &self,
        block: &PreparedBlock,
        environment: &Environment,
    ) -> Result<PreparedFunction, String> {
        Ok(PreparedFunction(
            function_body(block, environment)?.into_bytes().into(),
        ))
    }
    /// Assemble bounded bodies; returned bytes have no host instance lifetime.
    pub fn compile(&self, blocks: &[PreparedFunction]) -> Result<Module, String> {
        if blocks.is_empty() || blocks.len() > 64 {
            return Err("TCG batch count exceeds bound".into());
        }
        let mut module = module_header();
        let mut types = Binary::default();
        types.unsigned(6);
        signature(&mut types, &[0x7f; 4], &[0x7e]);
        signature(&mut types, &[0x7f; 4], &[0x7e]);
        signature(&mut types, &[0x7f, 0x7f, 0x7e, 0x7f, 0x7f], &[]);
        signature(&mut types, &[0x7f; 7], &[0x7f]);
        signature(&mut types, &[0x7f], &[0x7e]);
        signature(&mut types, &[0x7f, 0x7f, 0x7f, 0x7e], &[]);
        module.section(1, types)?;
        let mut imports = Binary::default();
        imports.unsigned(5);
        for (name, index) in [("load", 1), ("store", 2), ("helper", 3)] {
            import(&mut imports, name, 0)?;
            imports.unsigned(index);
        }
        import(&mut imports, "memory", 2)?;
        if cfg!(all(target_arch = "wasm32", target_feature = "atomics")) {
            imports.extend(&[3, 1]);
            imports.unsigned(32768); // Shared wasm32 memory, maximum 2 GiB.
        } else {
            imports.extend(&[0, 1]);
        }
        import(&mut imports, "table", 1)?;
        imports.extend(&[0x70, 0, 0]);
        module.section(2, imports)?;
        let mut functions = Binary::default();
        functions.unsigned(blocks.len() as u32);
        for _ in blocks {
            functions.unsigned(0);
        }
        module.section(3, functions)?;
        let exports: Vec<_> = (0..blocks.len()).map(|n| format!("b{n}")).collect();
        let mut names = Binary::default();
        names.unsigned(exports.len() as u32);
        for (n, name) in exports.iter().enumerate() {
            names.text(name)?;
            names.byte(0);
            names.unsigned(n as u32 + 3);
        }
        module.section(7, names)?;
        let mut bodies = Binary::default();
        bodies.unsigned(blocks.len() as u32);
        for block in blocks {
            let body = block.bytes();
            bodies.unsigned(body.len().try_into().map_err(|_| "Wasm body exceeds u32")?);
            bodies.extend(body);
        }
        module.section(10, bodies)?;
        Ok(Module {
            bytes: module.into_bytes(),
            exports,
        })
    }
    /// The dispatch loop reads the live generation slot before every edge.
    pub fn dispatcher(layout: &[u32]) -> Result<Module, String> {
        let &[2, slot_offset, code_offset, counters] = layout else {
            return Err("invalid TCI dispatch layout".into());
        };
        let mut b = Body {
            binary: Binary::default(),
            source: 0,
            locals: std::array::from_fn(|n| n as u32),
            forward_block: None,
        };
        b.emit(&[2, 1, 0x7f, 1, 0x7e, 0x03, 0x40]);
        b.get(0);
        b.get(1);
        b.get(2);
        b.get(4);
        b.get(3);
        b.emit(&[0x11, 0, 0]);
        b.set(6);
        b.get(6);
        b.emit(&[0xa7, 0x22, 5, 0x45, 0x04, 0x40]);
        b.get(6);
        b.emit(&[0x0f, 0x0b]);
        // A single unaligned little-endian load checks both prefix bytes.
        b.get(5);
        b.mem(0x2f, 0, 0);
        b.constant(0x06ff);
        b.emit(&[0x47, 0x04, 0x40]);
        b.get(6);
        b.emit(&[0x0f, 0x0b]);
        b.get(5);
        b.mem(0x28, 0, 2);
        b.emit(&[0x22, 5]);
        b.mem(0x28, 0, slot_offset);
        b.emit(&[0x22, 3, 0x45, 0x04, 0x40]);
        b.get(6);
        b.emit(&[0x0f, 0x0b]);
        // Resolve the current allocation only after validating its live slot.
        b.get(5);
        b.mem(0x28, 0, code_offset);
        b.constant(6);
        b.emit(&[0x6a]);
        b.set(4);
        for offset in [0, 8, 16] {
            b.constant(counters);
            b.constant(counters);
            b.mem(0x29, 3, offset);
            b.wide(1);
            b.emit(&[0x7c]);
            b.mem(0x37, 3, offset);
        }
        b.emit(&[0x0c, 0, 0x0b, 0x00, 0x0b]);
        let mut module = module_header();
        let mut types = Binary::default();
        types.unsigned(2);
        signature(&mut types, &[0x7f; 4], &[0x7e]);
        signature(&mut types, &[0x7f; 5], &[0x7e]);
        module.section(1, types)?;
        let mut imports = Binary::default();
        imports.unsigned(2);
        import(&mut imports, "memory", 2)?;
        if cfg!(all(target_arch = "wasm32", target_feature = "atomics")) {
            imports.extend(&[3, 1]);
            imports.unsigned(32768); // Shared wasm32 memory, maximum 2 GiB.
        } else {
            imports.extend(&[0, 1]);
        }
        import(&mut imports, "table", 1)?;
        imports.extend(&[0x70, 0, 0]);
        module.section(2, imports)?;
        let mut functions = Binary::default();
        functions.extend(&[1, 1]);
        module.section(3, functions)?;
        let mut names = Binary::default();
        names.unsigned(1);
        names.text("dispatch")?;
        names.extend(&[0, 0]);
        module.section(7, names)?;
        let mut bodies = Binary::default();
        bodies.unsigned(1);
        bodies.unsigned(
            b.binary
                .bytes()
                .len()
                .try_into()
                .map_err(|_| "Wasm body exceeds u32")?,
        );
        bodies.extend(b.binary.bytes());
        module.section(10, bodies)?;
        let mut function_names = Binary::default();
        function_names.unsigned(1);
        function_names.unsigned(0);
        function_names.text("tcg_dispatch")?;
        let mut debug_names = Binary::default();
        debug_names.text("name")?;
        debug_names.section(1, function_names)?;
        module.section(0, debug_names)?;
        Ok(Module {
            bytes: module.into_bytes(),
            exports: vec!["dispatch".into()],
        })
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[path = "emit_tests.rs"]
mod tests;
