use super::super::super::{Helper, HelperKind, HelperLayout, MemoryLayout};
use super::*;

fn memory_operations(store: bool, memop: u32) -> Vec<Operation> {
    vec![
        Operation::GuestMemory {
            store,
            data: Data::Double([r(0), r(1)]),
            address: r(2),
            memop,
        },
        self::store(0, 160),
        self::store(1, 164),
        Operation::Exit { value: 0x1234 },
    ]
}
#[test]
fn direct_memory_preserves_flags_hooks_alignment_and_live_tlb() {
    let environment = Environment {
        memory: Some(
            MemoryLayout::new(&[
                1, 10, 256, 8, 4, 5, 0, 4, 24, 128, 0, 4, 8, 12, 3, 4, 8, 4, 112, 0, 112, 2,
            ])
            .unwrap(),
        ),
        ..Default::default()
    };
    let mut operations = Vec::new();
    for shift in 0..4 {
        for signed in [false, true] {
            for mode in 0..2 {
                for swapped in [false, true] {
                    let memop = shift | if signed { 4 } else { 0 } | if swapped { 8 } else { 0 };
                    operations.push(memory_operations(false, (memop << 4) | mode));
                    operations.push(memory_operations(true, ((memop & !4) << 4) | mode));
                }
            }
        }
    }
    let alignment = compile_in(vec![memory_operations(true, (2 | 112) << 4)], &environment);
    let reload = compile_in(
        vec![vec![
            Operation::GuestMemory {
                store: false,
                data: Data::Double([r(0), r(1)]),
                address: r(2),
                memop: 2 << 4,
            },
            Operation::GuestMemory {
                store: false,
                data: Data::Double([r(0), r(1)]),
                address: r(3),
                memop: 2 << 4,
            },
            store(0, 160),
            store(1, 164),
            Operation::Exit { value: 0x1234 },
        ]],
        &environment,
    );
    run(
        "fast-memory",
        compile_in(operations, &environment),
        &[("alignment", alignment), ("reload", reload)],
    );
}
fn helper_environment() -> Environment {
    Environment {
        helper: Some(HelperLayout::new(&[1, 0x4100, 128, 0, 132, 4, 8, 0]).unwrap()),
        helpers: [
            (1, HelperKind::Other),
            (2, HelperKind::Other),
            (3, HelperKind::CheckExit),
            (4, HelperKind::Trace),
            (11, HelperKind::ShlCarry),
            (12, HelperKind::ShrCarry),
            (13, HelperKind::SarCarry),
            (14, HelperKind::RorCarry),
        ]
        .into_iter()
        .map(|(pointer, kind)| {
            (
                pointer,
                Helper {
                    kind,
                    target: pointer,
                },
            )
        })
        .collect(),
        ..Default::default()
    }
}
fn helper_operations(function: u32, words: u32, mask: u32) -> Vec<Operation> {
    vec![
        Operation::Call {
            function: Value::Constant(function),
            words,
            mask,
        },
        store(0, 0),
        store(1, 4),
        Operation::Exit { value: 0x1234 },
    ]
}
#[test]
fn typed_helpers_preserve_precise_pc_pending_exit_and_all_carry_counts() {
    let environment = helper_environment();
    let mut operations = vec![
        helper_operations(1, 4, 16),
        helper_operations(2, 0, 0),
        helper_operations(3, 2, 0),
    ];
    for pointer in 11..15 {
        operations.push(helper_operations(pointer, 3, 0));
    }
    operations.push(helper_operations(4, 5, 256));
    run("typed-helpers", compile_in(operations, &environment), &[]);
}
#[test]
fn refuses_malformed_typed_arguments_and_layouts() {
    let compiler = Compiler::new(&[] as &[&str]).unwrap();
    for (words, mask) in [(7, 0), (1, 4)] {
        assert!(
            compiler
                .lower(
                    &block(helper_operations(1, words, mask)),
                    &helper_environment()
                )
                .is_err()
        );
    }
    for (words, mask) in [(4, 256), (5, 0), (6, 256)] {
        assert!(
            compiler
                .lower(
                    &block(helper_operations(4, words, mask)),
                    &helper_environment()
                )
                .is_err()
        );
    }
    let mut missing = helper_environment();
    missing.helpers.get_mut(&4).unwrap().target = 0;
    assert!(
        compiler
            .lower(&block(helper_operations(4, 5, 256)), &missing)
            .is_err()
    );
    for raw in [&[][..], &[1, 0], &[0, 0, 0, 0, 0, 0, 0, 0]] {
        assert!(HelperLayout::new(raw).is_err());
    }
    assert!(MemoryLayout::new(&[]).is_err());
}
