//! Decoded shader instructions used by the GLSL translator.

use super::isa::{Instruction, OpCode, OperandDescriptor};

pub(super) const PROGRAM_SIZE: usize = 4096;
const DESCRIPTOR_SIZE: usize = 128;

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Operand {
    pub(super) register: u32,
    pub(super) swizzle: [u8; 4],
    pub(super) negate: bool,
    pub(super) index: u32,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Op {
    pub(super) instruction: Instruction,
    pub(super) opcode: OpCode,
    pub(super) destination: u32,
    pub(super) mask: u32,
    pub(super) sources: [Operand; 3],
}

impl Op {
    fn decode(instruction: Instruction, descriptors: &[u32; DESCRIPTOR_SIZE]) -> Op {
        let opcode = instruction.opcode();
        let operand = |descriptor: OperandDescriptor, source: u32, register: u32, index: u32| {
            let (swizzle, negate) = descriptor.source(source);
            Operand {
                register,
                swizzle,
                negate,
                index,
            }
        };
        match opcode {
            OpCode::Mad | OpCode::MadI => {
                let descriptor = OperandDescriptor(
                    descriptors[instruction.mad_descriptor_index() as usize % DESCRIPTOR_SIZE],
                );
                let (src1, src2, src3) = instruction.mad_sources();
                let index = instruction.mad_address_register_index();
                let (index2, index3) = if opcode == OpCode::MadI {
                    (0, index)
                } else {
                    (index, 0)
                };
                Op {
                    instruction,
                    opcode,
                    destination: instruction.mad_destination(),
                    mask: descriptor.destination_mask(),
                    sources: [
                        operand(descriptor, 1, src1, 0),
                        operand(descriptor, 2, src2, index2),
                        operand(descriptor, 3, src3, index3),
                    ],
                }
            }
            _ => {
                let descriptor = OperandDescriptor(
                    descriptors[instruction.descriptor_index() as usize % DESCRIPTOR_SIZE],
                );
                let (src1, src2) = instruction.sources();
                let index = instruction.address_register_index();
                let (index1, index2) = if opcode.is_inverted() {
                    (0, index)
                } else {
                    (index, 0)
                };
                Op {
                    instruction,
                    opcode,
                    destination: instruction.destination(),
                    mask: descriptor.destination_mask(),
                    sources: [
                        operand(descriptor, 1, src1, index1),
                        operand(descriptor, 2, src2, index2),
                        Operand::default(),
                    ],
                }
            }
        }
    }
}

pub(crate) struct Program {
    pub(crate) ops: Box<[Op]>,
    pub(crate) inputs: u16,
}

impl Program {
    pub(crate) fn decode(program: &[u32], descriptors: &[u32]) -> Result<Program, String> {
        if program.len() > PROGRAM_SIZE || descriptors.len() > DESCRIPTOR_SIZE {
            return Err("shader upload exceeds PICA program or descriptor memory".into());
        }
        let mut code = [0; PROGRAM_SIZE];
        code[..program.len()].copy_from_slice(program);
        let mut descriptor_bank = [0; DESCRIPTOR_SIZE];
        descriptor_bank[..descriptors.len()].copy_from_slice(descriptors);
        let ops: Box<[Op]> = code
            .iter()
            .map(|&word| Op::decode(Instruction(word), &descriptor_bank))
            .collect();
        let mut inputs = 0u16;
        for op in ops.iter() {
            let sources = match op.opcode {
                OpCode::Mad | OpCode::MadI => 3,
                OpCode::Mova | OpCode::Cmp => 2,
                opcode if opcode.writes() => 2,
                _ => continue,
            };
            for source in &op.sources[..sources] {
                if source.register < 0x10 {
                    inputs |= 1 << source.register;
                }
            }
        }
        Ok(Program { ops, inputs })
    }
}
