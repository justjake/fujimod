//! Control flow and incoming register requirements for generated block bodies.
use super::{Compiler, ir::*};
use std::{collections::VecDeque, ops::Range};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Registers(u16);
impl Registers {
    pub fn contains(self, register: Register) -> bool {
        self.0 & (1 << register.index()) != 0
    }
    pub fn iter(self) -> impl Iterator<Item = Register> {
        (0..16).filter_map(move |n| {
            let register = Register::new(n).expect("bounded register index");
            self.contains(register).then_some(register)
        })
    }
    fn insert(&mut self, register: Register) {
        self.0 |= 1 << register.index();
    }
    fn value(&mut self, value: Value) {
        if let Value::Register(register) = value {
            self.insert(register);
        }
    }
}
#[derive(Default)]
struct Effects {
    reads: Registers,
    writes: Registers,
}
fn effects(operation: &Operation) -> Result<Effects, String> {
    let mut e = Effects::default();
    match operation {
        Operation::Binary { dst, lhs, rhs, .. } => {
            e.reads.value(*lhs);
            e.reads.value(*rhs);
            e.writes.insert(*dst);
        }
        Operation::Unary { dst, src, .. } => {
            e.reads.insert(*src);
            e.writes.insert(*dst);
        }
        Operation::Constant { dst, .. } => e.writes.insert(*dst),
        Operation::HostMemory { op, data, base, .. } => {
            e.reads.insert(*base);
            match op {
                HostMemory::Load32 | HostMemory::Load8Unsigned => e.writes.insert(*data),
                HostMemory::Store32 | HostMemory::Store16 | HostMemory::Store8 => {
                    e.reads.insert(*data)
                }
            }
        }
        Operation::Compare { dst, lhs, rhs, .. } => {
            e.reads.insert(*lhs);
            e.reads.value(*rhs);
            e.writes.insert(*dst);
        }
        Operation::CompareDouble { dst, lhs, rhs, .. } => {
            for r in lhs {
                e.reads.insert(*r);
            }
            for v in rhs {
                e.reads.value(*v);
            }
            e.writes.insert(*dst);
        }
        Operation::BranchIf { lhs, rhs, .. } => {
            e.reads.insert(*lhs);
            e.reads.value(*rhs);
        }
        Operation::BranchIfDouble { lhs, rhs, .. } => {
            for r in lhs {
                e.reads.insert(*r);
            }
            for v in rhs {
                e.reads.value(*v);
            }
        }
        Operation::Deposit { dst, lhs, rhs, .. } => {
            e.reads.insert(*lhs);
            e.reads.insert(*rhs);
            e.writes.insert(*dst);
        }
        Operation::Pair { dst, lhs, rhs, .. } => {
            for r in lhs.iter().chain(rhs) {
                e.reads.insert(*r);
            }
            for r in dst {
                e.writes.insert(*r);
            }
        }
        Operation::MultiplyUnsigned { dst, lhs, rhs } => {
            e.reads.insert(*lhs);
            e.reads.insert(*rhs);
            for r in dst {
                e.writes.insert(*r);
            }
        }
        Operation::Call {
            function, words, ..
        } => {
            const ARGUMENTS: [u8; 12] = [0, 1, 2, 3, 5, 6, 7, 8, 9, 10, 11, 12];
            let arguments = ARGUMENTS
                .get(..*words as usize)
                .ok_or("TCG helper argument count exceeds ABI")?;
            e.reads.value(*function);
            for r in arguments {
                e.reads.insert(Register::new(*r)?);
            }
            e.writes.insert(Register::new(0)?);
            e.writes.insert(Register::new(1)?);
        }
        Operation::GuestMemory {
            store,
            data,
            address,
            ..
        } => {
            e.reads.insert(*address);
            let registers = match data {
                Data::Word(r) => &[*r][..],
                Data::Double(pair) => &pair[..],
            };
            for r in registers {
                if *store {
                    e.reads.insert(*r);
                } else {
                    e.writes.insert(*r);
                }
            }
        }
        Operation::Branch { .. }
        | Operation::Exit { .. }
        | Operation::Goto { .. }
        | Operation::Barrier => {}
    }
    Ok(e)
}

#[derive(Debug)]
pub struct BasicBlock {
    pub(super) instructions: Range<usize>,
    pub(super) successors: Vec<usize>,
    pub(super) live_in: Registers,
}
#[derive(Debug)]
pub struct PreparedBlock {
    pub(super) instructions: Vec<Instruction>,
    pub(super) blocks: Vec<BasicBlock>,
    pub(super) incoming: Registers,
    pub(super) modified: Registers,
}
impl BasicBlock {
    pub fn instructions(&self) -> Range<usize> {
        self.instructions.clone()
    }
    pub fn successors(&self) -> &[usize] {
        &self.successors
    }
    pub fn live_in(&self) -> Registers {
        self.live_in
    }
}
impl Compiler {
    pub fn prepare(&self, code: &[u8], start: u32) -> Result<PreparedBlock, String> {
        PreparedBlock::analyze(self.decode(code, start)?)
    }
}
impl PreparedBlock {
    pub fn instructions(&self) -> &[Instruction] {
        &self.instructions
    }
    pub fn blocks(&self) -> &[BasicBlock] {
        &self.blocks
    }
    pub fn incoming(&self) -> Registers {
        self.incoming
    }
    pub fn modified(&self) -> Registers {
        self.modified
    }
    pub(super) fn analyze(instructions: Vec<Instruction>) -> Result<Self, String> {
        if instructions.is_empty() {
            return Err("empty TCG operation stream".into());
        }
        let target = |address| {
            instructions
                .binary_search_by_key(&address, |op| op.address)
                .map_err(|_| "TCG branch is not an operation boundary".to_owned())
        };
        let mut leaders = vec![0];
        let mut modified = Registers::default();
        for (n, instruction) in instructions.iter().enumerate() {
            modified.0 |= effects(&instruction.operation)?.writes.0;
            match instruction.operation {
                Operation::Branch { target: address }
                | Operation::BranchIf {
                    target: address, ..
                }
                | Operation::BranchIfDouble {
                    target: address, ..
                } => leaders.push(target(address)?),
                _ => {}
            }
            if matches!(
                instruction.operation,
                Operation::Branch { .. }
                    | Operation::BranchIf { .. }
                    | Operation::BranchIfDouble { .. }
                    | Operation::Exit { .. }
                    | Operation::Goto { .. }
            ) && n + 1 < instructions.len()
            {
                leaders.push(n + 1);
            }
        }
        leaders.sort_unstable();
        leaders.dedup();
        let mut owner = vec![0; instructions.len()];
        let mut blocks = Vec::with_capacity(leaders.len());
        let mut uses = Vec::with_capacity(leaders.len());
        let mut definitions = Vec::with_capacity(leaders.len());
        for (index, first) in leaders.iter().copied().enumerate() {
            let end = leaders
                .get(index + 1)
                .copied()
                .unwrap_or(instructions.len());
            owner[first..end].fill(index);
            let (mut use_set, mut def_set) = (Registers::default(), Registers::default());
            for instruction in &instructions[first..end] {
                let e = effects(&instruction.operation)?;
                use_set.0 |= e.reads.0 & !def_set.0;
                def_set.0 |= e.writes.0;
            }
            uses.push(use_set);
            definitions.push(def_set);
            blocks.push(BasicBlock {
                instructions: first..end,
                successors: Vec::new(),
                live_in: Registers::default(),
            });
        }
        let mut predecessors = vec![Vec::new(); blocks.len()];
        for index in 0..blocks.len() {
            let end = blocks[index].instructions.end;
            let last = &instructions[end - 1].operation;
            let mut successors = Vec::new();
            match *last {
                Operation::Branch { target: address }
                | Operation::BranchIf {
                    target: address, ..
                }
                | Operation::BranchIfDouble {
                    target: address, ..
                } => successors.push(owner[target(address)?]),
                _ => {}
            }
            if !matches!(
                last,
                Operation::Branch { .. } | Operation::Exit { .. } | Operation::Goto { .. }
            ) {
                if end == instructions.len() {
                    return Err("TCG block falls outside its extent".into());
                }
                successors.push(owner[end]);
            }
            successors.sort_unstable();
            successors.dedup();
            for successor in &successors {
                predecessors[*successor].push(index);
            }
            blocks[index].successors = successors;
        }
        // Backward liveness includes the existing goto publication contract:
        // every possibly modified bank register must have a valid value there,
        // even on a path that skipped its write. Exit tokens do not publish it.
        let mut queue: VecDeque<_> = (0..blocks.len()).rev().collect();
        let mut queued = vec![true; blocks.len()];
        while let Some(index) = queue.pop_front() {
            queued[index] = false;
            let block = &blocks[index];
            let last = &instructions[block.instructions.end - 1].operation;
            let mut live_out = if matches!(last, Operation::Goto { .. }) {
                modified.0
            } else {
                0
            };
            for successor in &block.successors {
                live_out |= blocks[*successor].live_in.0;
            }
            let live_in = uses[index].0 | (live_out & !definitions[index].0);
            if live_in != blocks[index].live_in.0 {
                blocks[index].live_in.0 = live_in;
                for predecessor in &predecessors[index] {
                    if !queued[*predecessor] {
                        queued[*predecessor] = true;
                        queue.push_back(*predecessor);
                    }
                }
            }
        }
        let incoming = blocks[0].live_in;
        Ok(Self {
            instructions,
            blocks,
            incoming,
            modified,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn r(n: u8) -> Register {
        Register::new(n).unwrap()
    }
    fn op(n: u32, operation: Operation) -> Instruction {
        Instruction {
            address: n * 16,
            pc: n * 16 + 2,
            next: (n + 1) * 16,
            operation,
        }
    }
    fn regs(set: Registers) -> Vec<u8> {
        set.iter().map(Register::index).collect()
    }
    #[test]
    fn overwritten_destinations_do_not_require_bank_loads() {
        let prepared = PreparedBlock::analyze(vec![
            op(
                0,
                Operation::Constant {
                    dst: r(9),
                    value: 42,
                },
            ),
            op(1, Operation::Goto { link: 0x1800 }),
        ])
        .unwrap();
        assert!(regs(prepared.incoming).is_empty());
        assert_eq!(regs(prepared.modified), [9]);
    }
    #[test]
    fn skipped_write_keeps_incoming_bank_value() {
        let prepared = PreparedBlock::analyze(vec![
            op(
                0,
                Operation::BranchIf {
                    lhs: r(0),
                    rhs: Value::Constant(0),
                    condition: Condition::Equal,
                    target: 48,
                },
            ),
            op(
                1,
                Operation::Constant {
                    dst: r(9),
                    value: 42,
                },
            ),
            op(2, Operation::Branch { target: 64 }),
            op(3, Operation::Barrier),
            op(4, Operation::Goto { link: 0x1800 }),
        ])
        .unwrap();
        assert_eq!(regs(prepared.incoming), [0, 9]);
        assert_eq!(prepared.blocks[0].successors, [1, 2]);
    }
    #[test]
    fn all_branch_paths_define_destination_before_publication() {
        let prepared = PreparedBlock::analyze(vec![
            op(
                0,
                Operation::BranchIf {
                    lhs: r(0),
                    rhs: Value::Constant(0),
                    condition: Condition::Equal,
                    target: 48,
                },
            ),
            op(
                1,
                Operation::Constant {
                    dst: r(9),
                    value: 42,
                },
            ),
            op(2, Operation::Branch { target: 64 }),
            op(
                3,
                Operation::Constant {
                    dst: r(9),
                    value: 43,
                },
            ),
            op(4, Operation::Goto { link: 0x1800 }),
        ])
        .unwrap();
        assert_eq!(regs(prepared.incoming), [0]);
    }
    #[test]
    fn loop_liveness_reaches_a_fixed_point() {
        let prepared = PreparedBlock::analyze(vec![
            op(
                0,
                Operation::Constant {
                    dst: r(1),
                    value: 1,
                },
            ),
            op(
                1,
                Operation::Binary {
                    op: BinaryOp::Add,
                    dst: r(0),
                    lhs: Value::Register(r(0)),
                    rhs: Value::Register(r(1)),
                },
            ),
            op(
                2,
                Operation::BranchIf {
                    lhs: r(0),
                    rhs: Value::Constant(10),
                    condition: Condition::LessUnsigned,
                    target: 16,
                },
            ),
            op(3, Operation::Goto { link: 0x1800 }),
        ])
        .unwrap();
        assert_eq!(regs(prepared.incoming), [0]);
        assert_eq!(regs(prepared.modified), [0, 1]);
    }
    #[test]
    fn helper_arguments_skip_r4_and_keep_dynamic_function() {
        let prepared = PreparedBlock::analyze(vec![
            op(
                0,
                Operation::Call {
                    function: Value::Register(r(13)),
                    words: 12,
                    mask: 0x1555,
                },
            ),
            op(1, Operation::Exit { value: 0 }),
        ])
        .unwrap();
        assert_eq!(
            regs(prepared.incoming),
            [0, 1, 2, 3, 5, 6, 7, 8, 9, 10, 11, 12, 13]
        );
        assert_eq!(regs(prepared.modified), [0, 1]);
    }
    #[test]
    fn refuses_mid_operation_targets_and_fallthrough_outside_extent() {
        assert!(PreparedBlock::analyze(vec![op(0, Operation::Branch { target: 1 })]).is_err());
        assert!(PreparedBlock::analyze(vec![op(0, Operation::Barrier)]).is_err());
    }
}
