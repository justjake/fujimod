//! Typed operations for the pinned 32-bit TCI byte stream.

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Register(u8);
impl Register {
    pub fn new(index: u8) -> Result<Self, String> {
        if index < 16 {
            Ok(Self(index))
        } else {
            Err(format!("invalid TCG register {index}"))
        }
    }
    pub fn index(self) -> u8 {
        self.0
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Value {
    Register(Register),
    Constant(u32),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    And,
    Or,
    Xor,
    Shl,
    Shr,
    Sar,
    Rotl,
    Rotr,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnaryOp {
    Move,
    Ext8Signed,
    Ext16Signed,
    Ext8Unsigned,
    Ext16Unsigned,
    Not,
    Neg,
    Swap16,
    Swap32,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostMemory {
    Load32,
    Load8Unsigned,
    Store32,
    Store16,
    Store8,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Condition {
    Equal,
    NotEqual,
    LessSigned,
    GreaterEqualSigned,
    LessEqualSigned,
    GreaterSigned,
    LessUnsigned,
    GreaterEqualUnsigned,
    LessEqualUnsigned,
    GreaterUnsigned,
}
impl Condition {
    pub fn from_tcg(value: u8) -> Result<Self, String> {
        Ok(match value {
            8 => Self::Equal,
            9 => Self::NotEqual,
            2 => Self::LessSigned,
            3 => Self::GreaterEqualSigned,
            10 => Self::LessEqualSigned,
            11 => Self::GreaterSigned,
            4 => Self::LessUnsigned,
            5 => Self::GreaterEqualUnsigned,
            12 => Self::LessEqualUnsigned,
            13 => Self::GreaterUnsigned,
            _ => return Err(format!("unsupported TCG condition {value}")),
        })
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PairOp {
    Add,
    Sub,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Data {
    Word(Register),
    Double([Register; 2]),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Operation {
    Binary {
        op: BinaryOp,
        dst: Register,
        lhs: Value,
        rhs: Value,
    },
    Unary {
        op: UnaryOp,
        dst: Register,
        src: Register,
    },
    Constant {
        dst: Register,
        value: u32,
    },
    HostMemory {
        op: HostMemory,
        data: Register,
        base: Register,
        offset: i32,
    },
    Compare {
        dst: Register,
        lhs: Register,
        rhs: Value,
        condition: Condition,
    },
    CompareDouble {
        dst: Register,
        lhs: [Register; 2],
        rhs: [Value; 2],
        condition: Condition,
    },
    Branch {
        target: u32,
    },
    BranchIf {
        lhs: Register,
        rhs: Value,
        condition: Condition,
        target: u32,
    },
    BranchIfDouble {
        lhs: [Register; 2],
        rhs: [Value; 2],
        condition: Condition,
        target: u32,
    },
    Deposit {
        dst: Register,
        lhs: Register,
        rhs: Register,
        position: u8,
        length: u8,
    },
    Pair {
        op: PairOp,
        dst: [Register; 2],
        lhs: [Register; 2],
        rhs: [Register; 2],
    },
    MultiplyUnsigned {
        dst: [Register; 2],
        lhs: Register,
        rhs: Register,
    },
    Call {
        function: Value,
        words: u32,
        mask: u32,
    },
    GuestMemory {
        store: bool,
        data: Data,
        address: Register,
        memop: u32,
    },
    Exit {
        value: u32,
    },
    Goto {
        link: u32,
    },
    Barrier,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Instruction {
    pub address: u32,
    pub pc: u32,
    pub next: u32,
    pub operation: Operation,
}
