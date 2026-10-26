use super::*;
use std::{path::PathBuf, process::Command};

fn r(n: u8) -> Register {
    Register::new(n).unwrap()
}
fn block(operations: Vec<Operation>) -> PreparedBlock {
    PreparedBlock::analyze(
        operations
            .into_iter()
            .enumerate()
            .map(|(n, operation)| Instruction {
                address: 0x5000 + n as u32 * 16,
                pc: 0x5002 + n as u32 * 16,
                next: 0x5010 + n as u32 * 16,
                operation,
            })
            .collect(),
    )
    .unwrap()
}
fn store(data: u8, offset: i32) -> Operation {
    Operation::HostMemory {
        op: HostMemory::Store32,
        data: r(data),
        base: r(14),
        offset,
    }
}
fn compile(operations: Vec<Vec<Operation>>) -> Module {
    compile_in(operations, &Environment::default())
}
fn compile_in(operations: Vec<Vec<Operation>>, environment: &Environment) -> Module {
    let compiler = Compiler::new(&[] as &[&str]).unwrap();
    let blocks: Vec<_> = operations.into_iter().map(block).collect();
    let bodies = blocks
        .iter()
        .map(|block| compiler.lower(block, environment).unwrap())
        .collect::<Vec<_>>();
    let mut module = compiler.compile(&bodies).unwrap();
    // Supply real immediate bytes at the live source address in Node fixtures.
    let mut source = Binary::default();
    source.text("tci_test_source").unwrap();
    for (index, block) in blocks.iter().enumerate() {
        for instruction in &block.instructions {
            if let Operation::Exit { value } = instruction.operation {
                for word in [
                    index as u32,
                    instruction.pc - block.instructions[0].address,
                    value,
                ] {
                    source.extend(&word.to_le_bytes());
                }
            }
        }
    }
    let mut trailer = Binary::default();
    trailer.section(0, source).unwrap();
    module.bytes.extend(trailer.bytes());
    module
}

fn run(name: &str, module: Module, extras: &[(&str, Module)]) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let directory = root
        .join("target/jit-tests")
        .join(format!("{name}-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let filename = directory.join("program.wasm");
    std::fs::write(&filename, module.bytes).unwrap();
    for (name, module) in extras {
        std::fs::write(directory.join(format!("{name}.wasm")), &module.bytes).unwrap();
    }
    let driver = match name {
        "fast-memory" => "memory.test.mts",
        "typed-helpers" => "helper.test.mts",
        _ => "emit.test.mts",
    };
    let result = Command::new(std::env::var_os("JIT_NODE").unwrap_or_else(|| "node".into()))
        .arg(root.join("src/jit").join(driver))
        .arg(filename)
        .arg(name)
        .output()
        .expect("Node is required for the JIT execution gate");
    assert!(
        result.status.success(),
        "{name}: {}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}

#[path = "fast_tests.rs"]
mod fast;
#[test]
fn executes_scalar_arithmetic_and_unary_operations() {
    let mut operations = Vec::new();
    for op in [
        BinaryOp::Add,
        BinaryOp::Sub,
        BinaryOp::Mul,
        BinaryOp::And,
        BinaryOp::Or,
        BinaryOp::Xor,
        BinaryOp::Shl,
        BinaryOp::Shr,
        BinaryOp::Sar,
        BinaryOp::Rotl,
        BinaryOp::Rotr,
    ] {
        operations.push(vec![
            Operation::Binary {
                op,
                dst: r(2),
                lhs: Value::Register(r(0)),
                rhs: Value::Register(r(1)),
            },
            store(2, 0),
            Operation::Exit { value: 0x1234 },
        ]);
    }
    for op in [
        UnaryOp::Move,
        UnaryOp::Ext8Signed,
        UnaryOp::Ext16Signed,
        UnaryOp::Ext8Unsigned,
        UnaryOp::Ext16Unsigned,
        UnaryOp::Not,
        UnaryOp::Neg,
        UnaryOp::Swap16,
        UnaryOp::Swap32,
    ] {
        operations.push(vec![
            Operation::Unary {
                op,
                dst: r(2),
                src: r(0),
            },
            store(2, 0),
            Operation::Exit { value: 0x1234 },
        ]);
    }
    run("scalar", compile(operations), &[]);
}
#[test]
fn executes_wide_arithmetic_comparisons_and_deposit() {
    let mut operations = Vec::new();
    for condition in [
        Condition::Equal,
        Condition::NotEqual,
        Condition::LessSigned,
        Condition::GreaterEqualSigned,
        Condition::LessEqualSigned,
        Condition::GreaterSigned,
        Condition::LessUnsigned,
        Condition::GreaterEqualUnsigned,
        Condition::LessEqualUnsigned,
        Condition::GreaterUnsigned,
    ] {
        operations.push(vec![
            Operation::Compare {
                dst: r(4),
                lhs: r(0),
                rhs: Value::Register(r(2)),
                condition,
            },
            store(4, 0),
            Operation::Exit { value: 0x1234 },
        ]);
        operations.push(vec![
            Operation::CompareDouble {
                dst: r(4),
                lhs: [r(0), r(1)],
                rhs: [Value::Register(r(2)), Value::Register(r(3))],
                condition,
            },
            store(4, 0),
            Operation::Exit { value: 0x1234 },
        ]);
    }
    for op in [PairOp::Add, PairOp::Sub] {
        operations.push(vec![
            Operation::Pair {
                op,
                dst: [r(4), r(5)],
                lhs: [r(0), r(1)],
                rhs: [r(2), r(3)],
            },
            store(4, 0),
            store(5, 4),
            Operation::Exit { value: 0x1234 },
        ]);
    }
    operations.push(vec![
        Operation::MultiplyUnsigned {
            dst: [r(4), r(5)],
            lhs: r(0),
            rhs: r(2),
        },
        store(4, 0),
        store(5, 4),
        Operation::Exit { value: 0x1234 },
    ]);
    operations.push(vec![
        Operation::Deposit {
            dst: r(2),
            lhs: r(0),
            rhs: r(1),
            position: 8,
            length: 16,
        },
        store(2, 0),
        Operation::Exit { value: 0x1234 },
    ]);
    operations.push(vec![Operation::Exit { value: u32::MAX }]);
    run("wide", compile(operations), &[]);
}
#[test]
fn preserves_skipped_destination_on_both_branch_paths() {
    run(
        "branch",
        compile(vec![
            vec![
                Operation::BranchIf {
                    lhs: r(0),
                    rhs: Value::Constant(0),
                    condition: Condition::Equal,
                    target: 0x5030,
                },
                Operation::Constant {
                    dst: r(9),
                    value: 42,
                },
                Operation::Branch { target: 0x5040 },
                Operation::Barrier,
                Operation::Goto { link: 0x5044 },
            ],
            vec![
                Operation::BranchIfDouble {
                    lhs: [r(0), r(1)],
                    rhs: [Value::Register(r(2)), Value::Register(r(3))],
                    condition: Condition::Equal,
                    target: 0x5030,
                },
                Operation::Constant {
                    dst: r(9),
                    value: 42,
                },
                Operation::Branch { target: 0x5040 },
                Operation::Barrier,
                Operation::Goto { link: 0x5044 },
            ],
        ]),
        &[],
    );
}
#[test]
fn emits_generic_helper_argument_and_exit_contract() {
    run(
        "helper",
        compile(vec![vec![
            Operation::Call {
                function: Value::Constant(9),
                words: 12,
                mask: 0x1555,
            },
            store(0, 0),
            store(1, 4),
            Operation::Exit { value: 0x1234 },
        ]]),
        &[],
    );
}
#[test]
fn emits_memory_thunks_and_unaligned_host_access() {
    run(
        "memory",
        compile(vec![
            vec![
                Operation::GuestMemory {
                    store: false,
                    data: Data::Word(r(0)),
                    address: r(1),
                    memop: 0x21,
                },
                store(0, 0),
                Operation::GuestMemory {
                    store: false,
                    data: Data::Double([r(2), r(3)]),
                    address: r(1),
                    memop: 0x33,
                },
                store(2, 4),
                store(3, 8),
                Operation::GuestMemory {
                    store: true,
                    data: Data::Double([r(2), r(3)]),
                    address: r(4),
                    memop: 0x44,
                },
                Operation::Exit { value: 0x1234 },
            ],
            vec![
                Operation::HostMemory {
                    op: HostMemory::Store16,
                    data: r(0),
                    base: r(14),
                    offset: 1,
                },
                Operation::HostMemory {
                    op: HostMemory::Load8Unsigned,
                    data: r(2),
                    base: r(14),
                    offset: 1,
                },
                Operation::HostMemory {
                    op: HostMemory::Store8,
                    data: r(1),
                    base: r(14),
                    offset: 3,
                },
                Operation::HostMemory {
                    op: HostMemory::Load32,
                    data: r(3),
                    base: r(14),
                    offset: 0,
                },
                store(2, 8),
                store(3, 12),
                Operation::Exit { value: 0x1234 },
            ],
        ]),
        &[],
    );
}
#[test]
fn dispatches_long_chains_with_live_retirement_and_fallback() {
    let program = compile(vec![
        vec![
            Operation::Call {
                function: Value::Constant(9),
                words: 0,
                mask: 0,
            },
            Operation::Goto { link: 0x5014 },
        ],
        vec![Operation::Exit { value: 0x12345678 }],
    ]);
    let dispatcher = Compiler::dispatcher(&[2, 16, 24, 0x9000]).unwrap();
    run("dispatch", program, &[("dispatch", dispatcher)]);
}
#[test]
fn refuses_invalid_batches_and_dispatch_metadata() {
    let compiler = Compiler::new(&[] as &[&str]).unwrap();
    assert!(compiler.compile(&[]).is_err());
    let blocks = (0..65)
        .map(|_| {
            compiler
                .lower(
                    &block(vec![Operation::Exit { value: 0 }]),
                    &Environment::default(),
                )
                .unwrap()
        })
        .collect::<Vec<_>>();
    assert!(compiler.compile(&blocks).is_err());
    for layout in [&[][..], &[1, 0], &[0, 0, 0], &[1, 0, 0, 0]] {
        assert!(Compiler::dispatcher(layout).is_err());
    }
}

#[test]
fn relocation_keeps_function_bytes_equal_and_rejects_backward_source_refs() {
    let compiler = Compiler::new(&[] as &[&str]).unwrap();
    let operations = vec![
        Operation::Call {
            function: Value::Constant(9),
            words: 0,
            mask: 0,
        },
        Operation::Goto { link: 0x5014 },
        Operation::Exit { value: 0x12345678 },
    ];
    let original = block(operations);
    let expected = compiler.lower(&original, &Environment::default()).unwrap();
    let mut instructions = original.instructions.clone();
    for instruction in &mut instructions {
        instruction.address += 0x2000;
        instruction.pc += 0x2000;
        instruction.next += 0x2000;
        match &mut instruction.operation {
            Operation::Goto { link } => *link += 0x2000,
            Operation::Exit { value } => *value = 0x87654321,
            _ => {}
        }
    }
    let relocated = PreparedBlock::analyze(instructions).unwrap();
    assert_eq!(
        expected.bytes(),
        compiler
            .lower(&relocated, &Environment::default())
            .unwrap()
            .bytes()
    );
    let invalid = block(vec![Operation::Goto { link: 0x4000 }]);
    assert!(compiler.lower(&invalid, &Environment::default()).is_err());
}

#[test]
fn structured_forward_paths_and_backward_loops_preserve_the_bank() {
    let mut operations = vec![
        vec![
            Operation::BranchIf {
                lhs: r(0),
                rhs: Value::Constant(0),
                condition: Condition::Equal,
                target: 0x5050,
            },
            Operation::Constant {
                dst: r(9),
                value: 7,
            },
            Operation::BranchIf {
                lhs: r(0),
                rhs: Value::Constant(1),
                condition: Condition::Equal,
                target: 0x5050,
            },
            Operation::Constant {
                dst: r(9),
                value: 9,
            },
            Operation::Branch { target: 0x5060 },
            Operation::Constant {
                dst: r(9),
                value: 11,
            },
            Operation::Goto { link: 0x5064 },
        ],
        vec![
            Operation::Constant {
                dst: r(9),
                value: 0,
            },
            Operation::Binary {
                op: BinaryOp::Add,
                dst: r(9),
                lhs: Value::Register(r(9)),
                rhs: Value::Constant(1),
            },
            Operation::BranchIf {
                lhs: r(9),
                rhs: Value::Constant(5),
                condition: Condition::LessUnsigned,
                target: 0x5010,
            },
            Operation::Goto { link: 0x5034 },
        ],
    ];
    // Stay below and cross the structured nesting bound with the same program.
    for count in [63, 64] {
        let mut body = vec![Operation::Constant {
            dst: r(9),
            value: 0,
        }];
        for _ in 0..count {
            body.push(Operation::BranchIf {
                lhs: r(0),
                rhs: Value::Constant(0),
                condition: Condition::Equal,
                target: 0x5000 + (body.len() as u32 + 2) * 16,
            });
            body.push(Operation::Binary {
                op: BinaryOp::Add,
                dst: r(9),
                lhs: Value::Register(r(9)),
                rhs: Value::Constant(1),
            });
        }
        body.push(Operation::Goto {
            link: 0x5004 + body.len() as u32 * 16,
        });
        operations.push(body);
    }
    run("control", compile(operations), &[]);
}
