use super::ir::*;

#[derive(Clone, Copy)]
enum Opcode {
    Binary(BinaryOp),
    Unary(UnaryOp),
    Constant,
    HostMemory(HostMemory),
    Compare,
    CompareDouble,
    Branch,
    BranchIf,
    BranchIfDouble,
    Deposit,
    Pair(PairOp),
    MultiplyUnsigned,
    Call,
    GuestMemory { store: bool, double: bool },
    Exit,
    Goto,
    Barrier,
}
impl Opcode {
    fn named(name: &str) -> Option<Self> {
        Some(match name {
            "add_i32" => Self::Binary(BinaryOp::Add),
            "sub_i32" => Self::Binary(BinaryOp::Sub),
            "mul_i32" => Self::Binary(BinaryOp::Mul),
            "and_i32" => Self::Binary(BinaryOp::And),
            "or_i32" => Self::Binary(BinaryOp::Or),
            "xor_i32" => Self::Binary(BinaryOp::Xor),
            "shl_i32" => Self::Binary(BinaryOp::Shl),
            "shr_i32" => Self::Binary(BinaryOp::Shr),
            "sar_i32" => Self::Binary(BinaryOp::Sar),
            "rotl_i32" => Self::Binary(BinaryOp::Rotl),
            "rotr_i32" => Self::Binary(BinaryOp::Rotr),
            "mov_i32" => Self::Unary(UnaryOp::Move),
            "ext8s_i32" => Self::Unary(UnaryOp::Ext8Signed),
            "ext16s_i32" => Self::Unary(UnaryOp::Ext16Signed),
            "ext8u_i32" => Self::Unary(UnaryOp::Ext8Unsigned),
            "ext16u_i32" => Self::Unary(UnaryOp::Ext16Unsigned),
            "not_i32" => Self::Unary(UnaryOp::Not),
            "neg_i32" => Self::Unary(UnaryOp::Neg),
            "bswap16_i32" => Self::Unary(UnaryOp::Swap16),
            "bswap32_i32" => Self::Unary(UnaryOp::Swap32),
            "movi_i32" => Self::Constant,
            "ld_i32" => Self::HostMemory(HostMemory::Load32),
            "ld8u_i32" => Self::HostMemory(HostMemory::Load8Unsigned),
            "st_i32" => Self::HostMemory(HostMemory::Store32),
            "st16_i32" => Self::HostMemory(HostMemory::Store16),
            "st8_i32" => Self::HostMemory(HostMemory::Store8),
            "setcond_i32" => Self::Compare,
            "setcond2_i32" => Self::CompareDouble,
            "br" => Self::Branch,
            "brcond_i32" => Self::BranchIf,
            "brcond2_i32" => Self::BranchIfDouble,
            "deposit_i32" => Self::Deposit,
            "add2_i32" => Self::Pair(PairOp::Add),
            "sub2_i32" => Self::Pair(PairOp::Sub),
            "mulu2_i32" => Self::MultiplyUnsigned,
            "call" => Self::Call,
            "qemu_ld_i32" => Self::GuestMemory {
                store: false,
                double: false,
            },
            "qemu_ld_i64" => Self::GuestMemory {
                store: false,
                double: true,
            },
            "qemu_st_i32" => Self::GuestMemory {
                store: true,
                double: false,
            },
            "qemu_st_i64" => Self::GuestMemory {
                store: true,
                double: true,
            },
            "exit_tb" => Self::Exit,
            "goto_tb" => Self::Goto,
            "mb" => Self::Barrier,
            _ => return None,
        })
    }
}

/// The opcode-name table belongs to one target engine. Resolve it once rather
/// than matching strings for each operation offered for compilation.
pub struct Compiler {
    opcodes: Vec<(String, Option<Opcode>)>,
}
impl Compiler {
    pub fn new(names: &[impl AsRef<str>]) -> Result<Self, String> {
        if names.len() > 256 {
            return Err("TCG opcode table exceeds byte index".into());
        }
        Ok(Self {
            opcodes: names
                .iter()
                .map(|n| (n.as_ref().to_owned(), Opcode::named(n.as_ref())))
                .collect(),
        })
    }
    pub fn decode(&self, code: &[u8], start: u32) -> Result<Vec<Instruction>, String> {
        if code.is_empty() || code.len() > 65536 {
            return Err("TCG block extent exceeds bound".into());
        }
        start
            .checked_add(code.len() as u32)
            .ok_or("TCG block address overflow")?;
        let mut at = 0;
        let mut instructions = Vec::new();
        while at < code.len() {
            let header = code.get(at..at + 2).ok_or("truncated TCG header")?;
            let end = at + usize::from(header[1]);
            if header[1] < 2 || end > code.len() {
                return Err("invalid TCG operation extent".into());
            }
            let (name, opcode) = self
                .opcodes
                .get(usize::from(header[0]))
                .ok_or_else(|| format!("unknown TCG opcode {}", header[0]))?;
            let opcode = opcode.ok_or_else(|| format!("unsupported TCG operation {name}"))?;
            let mut cursor = Cursor {
                code,
                at: at + 2,
                end,
                start,
            };
            let operation = cursor.operation(opcode)?;
            if cursor.at != end {
                return Err(format!("TCG operation size differs: {name}"));
            }
            instructions.push(Instruction {
                address: start + at as u32,
                pc: start + at as u32 + 2,
                next: start + end as u32,
                operation,
            });
            at = end;
        }
        Ok(instructions)
    }
}

struct Cursor<'a> {
    code: &'a [u8],
    at: usize,
    end: usize,
    start: u32,
}
impl Cursor<'_> {
    fn byte(&mut self) -> Result<u8, String> {
        if self.at >= self.end {
            return Err("truncated TCG operand".into());
        }
        let value = self.code[self.at];
        self.at += 1;
        Ok(value)
    }
    fn word(&mut self) -> Result<u32, String> {
        if self.at + 4 > self.end {
            return Err("truncated TCG word".into());
        }
        let value = u32::from_le_bytes(self.code[self.at..self.at + 4].try_into().unwrap());
        self.at += 4;
        Ok(value)
    }
    fn register(&mut self) -> Result<Register, String> {
        Register::new(self.byte()?)
    }
    fn pair(&mut self) -> Result<[Register; 2], String> {
        Ok([self.register()?, self.register()?])
    }
    fn value(&mut self) -> Result<Value, String> {
        Ok(match self.byte()? {
            255 => Value::Constant(self.word()?),
            n => Value::Register(Register::new(n)?),
        })
    }
    fn values(&mut self) -> Result<[Value; 2], String> {
        Ok([self.value()?, self.value()?])
    }
    fn condition(&mut self) -> Result<Condition, String> {
        Condition::from_tcg(self.byte()?)
    }
    fn operation(&mut self, opcode: Opcode) -> Result<Operation, String> {
        Ok(match opcode {
            Opcode::Binary(op) => Operation::Binary {
                op,
                dst: self.register()?,
                lhs: self.value()?,
                rhs: self.value()?,
            },
            Opcode::Unary(op) => Operation::Unary {
                op,
                dst: self.register()?,
                src: self.register()?,
            },
            Opcode::Constant => Operation::Constant {
                dst: self.register()?,
                value: self.word()?,
            },
            Opcode::HostMemory(op) => Operation::HostMemory {
                op,
                data: self.register()?,
                base: self.register()?,
                offset: self.word()? as i32,
            },
            Opcode::Compare => Operation::Compare {
                dst: self.register()?,
                lhs: self.register()?,
                rhs: self.value()?,
                condition: self.condition()?,
            },
            Opcode::CompareDouble => Operation::CompareDouble {
                dst: self.register()?,
                lhs: self.pair()?,
                rhs: self.values()?,
                condition: self.condition()?,
            },
            Opcode::Branch => Operation::Branch {
                target: self.word()?,
            },
            Opcode::BranchIf => Operation::BranchIf {
                lhs: self.register()?,
                rhs: self.value()?,
                condition: self.condition()?,
                target: self.word()?,
            },
            Opcode::BranchIfDouble => Operation::BranchIfDouble {
                lhs: self.pair()?,
                rhs: self.values()?,
                condition: self.condition()?,
                target: self.word()?,
            },
            Opcode::Deposit => {
                let (dst, lhs, rhs) = (self.register()?, self.register()?, self.register()?);
                let (position, length) = (self.byte()?, self.byte()?);
                if length == 0 || length > 31 || u16::from(position) + u16::from(length) > 32 {
                    return Err("unsupported TCG deposit extent".into());
                }
                Operation::Deposit {
                    dst,
                    lhs,
                    rhs,
                    position,
                    length,
                }
            }
            Opcode::Pair(op) => Operation::Pair {
                op,
                dst: self.pair()?,
                lhs: self.pair()?,
                rhs: self.pair()?,
            },
            Opcode::MultiplyUnsigned => Operation::MultiplyUnsigned {
                dst: self.pair()?,
                lhs: self.register()?,
                rhs: self.register()?,
            },
            Opcode::Call => {
                let (function, words, mask) = (self.value()?, self.word()?, self.word()?);
                if words > 12 {
                    return Err("TCG helper argument count exceeds ABI".into());
                }
                Operation::Call {
                    function,
                    words,
                    mask,
                }
            }
            Opcode::GuestMemory { store, double } => Operation::GuestMemory {
                store,
                data: if double {
                    Data::Double(self.pair()?)
                } else {
                    Data::Word(self.register()?)
                },
                address: self.register()?,
                memop: self.word()?,
            },
            Opcode::Exit => {
                let value = self.word()?;
                // The pinned wasm32 TCI ABI encodes this immediate in two
                // words. Its exit token uses the low pointer word.
                self.word()?;
                Operation::Exit { value }
            }
            Opcode::Goto => {
                while (self.start + self.at as u32) & 3 != 0 {
                    self.byte()?;
                }
                let link = self.start + self.at as u32;
                self.word()?;
                Operation::Goto { link }
            }
            Opcode::Barrier => Operation::Barrier,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn r(index: u8) -> Register {
        Register::new(index).unwrap()
    }
    fn instruction(name: &str, payload: &[u8], start: u32) -> Result<Instruction, String> {
        let mut code = vec![0, (payload.len() + 2) as u8];
        code.extend(payload);
        let mut decoded = Compiler::new(&[name])?.decode(&code, start)?;
        Ok(decoded.remove(0))
    }
    #[test]
    fn byte_stream_keeps_immediate_tags_and_exact_extents() {
        let code = [
            0, 7, 0, 0x78, 0x56, 0x34, 0x12, 1, 9, 1, 0, 255, 4, 3, 2, 1, 2, 10, 0, 0, 0, 0, 0, 0,
            0, 0,
        ];
        let compiler = Compiler::new(&["movi_i32", "add_i32", "exit_tb"]).unwrap();
        let ops = compiler.decode(&code, 0x2000).unwrap();
        assert_eq!(
            ops[0].operation,
            Operation::Constant {
                dst: r(0),
                value: 0x12345678
            }
        );
        assert_eq!(
            ops[1].operation,
            Operation::Binary {
                op: BinaryOp::Add,
                dst: r(1),
                lhs: Value::Register(r(0)),
                rhs: Value::Constant(0x01020304)
            }
        );
        assert_eq!(
            (ops[1].address, ops[1].pc, ops[1].next),
            (0x2007, 0x2009, 0x2010)
        );
        assert_eq!(ops[2].operation, Operation::Exit { value: 0 });
        for end in 1..code.len() {
            if ![7, 16].contains(&end) {
                assert!(
                    compiler.decode(&code[..end], 0x2000).is_err(),
                    "prefix {end}"
                );
            }
        }
    }
    #[test]
    fn register_immediate_helper_and_memory_boundaries() {
        assert!(instruction("mov_i32", &[16, 0], 0).is_err());
        assert!(instruction("add_i32", &[0, 0, 254], 0).is_err());
        assert!(instruction("call", &[0, 13, 0, 0, 0, 0, 0, 0, 0], 0).is_err());
        let op = instruction("qemu_ld_i64", &[15, 14, 13, 0x78, 0x56, 0x34, 0x12], 0).unwrap();
        assert_eq!(
            op.operation,
            Operation::GuestMemory {
                store: false,
                data: Data::Double([r(15), r(14)]),
                address: r(13),
                memop: 0x12345678
            }
        );
        assert_eq!(
            instruction("ld_i32", &[0, 1, 255, 255, 255, 255], 0)
                .unwrap()
                .operation,
            Operation::HostMemory {
                op: HostMemory::Load32,
                data: r(0),
                base: r(1),
                offset: -1
            }
        );
    }
    #[test]
    fn goto_alignment_is_absolute_and_target_remains_a_live_link() {
        for start in 0..4 {
            let padding = (4 - ((start + 2) & 3)) & 3;
            let mut payload = vec![0xaa; padding as usize];
            payload.extend(0x12345678u32.to_le_bytes());
            let op = instruction("goto_tb", &payload, start).unwrap();
            assert_eq!(
                op.operation,
                Operation::Goto {
                    link: start + 2 + padding
                }
            );
        }
    }
    #[test]
    fn scalar_opcode_table_preserves_operation_identity() {
        for (name, op) in [
            ("add_i32", BinaryOp::Add),
            ("sub_i32", BinaryOp::Sub),
            ("mul_i32", BinaryOp::Mul),
            ("and_i32", BinaryOp::And),
            ("or_i32", BinaryOp::Or),
            ("xor_i32", BinaryOp::Xor),
            ("shl_i32", BinaryOp::Shl),
            ("shr_i32", BinaryOp::Shr),
            ("sar_i32", BinaryOp::Sar),
            ("rotl_i32", BinaryOp::Rotl),
            ("rotr_i32", BinaryOp::Rotr),
        ] {
            assert_eq!(
                instruction(name, &[15, 14, 255, 255, 255, 255, 255], 0)
                    .unwrap()
                    .operation,
                Operation::Binary {
                    op,
                    dst: r(15),
                    lhs: Value::Register(r(14)),
                    rhs: Value::Constant(u32::MAX)
                }
            );
        }
        for (name, op) in [
            ("mov_i32", UnaryOp::Move),
            ("ext8s_i32", UnaryOp::Ext8Signed),
            ("ext16s_i32", UnaryOp::Ext16Signed),
            ("ext8u_i32", UnaryOp::Ext8Unsigned),
            ("ext16u_i32", UnaryOp::Ext16Unsigned),
            ("not_i32", UnaryOp::Not),
            ("neg_i32", UnaryOp::Neg),
            ("bswap16_i32", UnaryOp::Swap16),
            ("bswap32_i32", UnaryOp::Swap32),
        ] {
            assert_eq!(
                instruction(name, &[15, 14], 0).unwrap().operation,
                Operation::Unary {
                    op,
                    dst: r(15),
                    src: r(14)
                }
            );
        }
        for (name, op) in [
            ("ld_i32", HostMemory::Load32),
            ("ld8u_i32", HostMemory::Load8Unsigned),
            ("st_i32", HostMemory::Store32),
            ("st16_i32", HostMemory::Store16),
            ("st8_i32", HostMemory::Store8),
        ] {
            assert_eq!(
                instruction(name, &[15, 14, 0, 0, 0, 128], 0)
                    .unwrap()
                    .operation,
                Operation::HostMemory {
                    op,
                    data: r(15),
                    base: r(14),
                    offset: i32::MIN
                }
            );
        }
    }
    #[test]
    fn branch_pair_and_call_operand_order() {
        assert_eq!(
            instruction(
                "brcond2_i32",
                &[1, 2, 3, 255, 255, 255, 255, 255, 8, 0x78, 0x56, 0x34, 0x12],
                0
            )
            .unwrap()
            .operation,
            Operation::BranchIfDouble {
                lhs: [r(1), r(2)],
                rhs: [Value::Register(r(3)), Value::Constant(u32::MAX)],
                condition: Condition::Equal,
                target: 0x12345678
            }
        );
        assert_eq!(
            instruction("setcond2_i32", &[0, 1, 2, 3, 4, 13], 0)
                .unwrap()
                .operation,
            Operation::CompareDouble {
                dst: r(0),
                lhs: [r(1), r(2)],
                rhs: [Value::Register(r(3)), Value::Register(r(4))],
                condition: Condition::GreaterUnsigned
            }
        );
        for (name, op) in [("add2_i32", PairOp::Add), ("sub2_i32", PairOp::Sub)] {
            assert_eq!(
                instruction(name, &[0, 1, 2, 3, 4, 5], 0).unwrap().operation,
                Operation::Pair {
                    op,
                    dst: [r(0), r(1)],
                    lhs: [r(2), r(3)],
                    rhs: [r(4), r(5)]
                }
            );
        }
        assert_eq!(
            instruction("mulu2_i32", &[0, 1, 2, 3], 0)
                .unwrap()
                .operation,
            Operation::MultiplyUnsigned {
                dst: [r(0), r(1)],
                lhs: r(2),
                rhs: r(3)
            }
        );
        assert_eq!(
            instruction(
                "call",
                &[255, 0x78, 0x56, 0x34, 0x12, 12, 0, 0, 0, 0x55, 0x15, 0, 0],
                0
            )
            .unwrap()
            .operation,
            Operation::Call {
                function: Value::Constant(0x12345678),
                words: 12,
                mask: 0x1555
            }
        );
        assert_eq!(
            instruction("mb", &[], 0).unwrap().operation,
            Operation::Barrier
        );
        assert!(Compiler::new(&vec!["mb"; 257]).is_err());
    }
    #[test]
    fn refuses_unknown_extents_conditions_and_deposits() {
        let c = Compiler::new(&["mb"]).unwrap();
        for code in [
            &[][..],
            &[0],
            &[0, 0],
            &[0, 1],
            &[0, 3],
            &[0, 3, 0],
            &[1, 2],
        ] {
            assert!(c.decode(code, 0).is_err());
        }
        assert!(c.decode(&vec![0; 65537], 0).is_err());
        assert!(c.decode(&[0, 2], u32::MAX).is_err());
        assert!(
            Compiler::new(&["unsupported"])
                .unwrap()
                .decode(&[0, 2], 0)
                .unwrap_err()
                .contains("unsupported")
        );
        assert!(instruction("brcond_i32", &[0, 1, 0, 0, 0, 0, 0], 0).is_err());
        for (position, length) in [(0, 0), (0, 32), (31, 2), (255, 1)] {
            assert!(instruction("deposit_i32", &[0, 1, 2, position, length], 0).is_err());
        }
    }
}
