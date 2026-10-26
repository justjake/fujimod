use super::*;

unsafe fn instruction(_: u64, _: u32, _: *mut c_void) {}
fn hook(begin: u64, end: u64) -> Hook {
    Hook::Instruction {
        callback: instruction,
        data: std::ptr::null_mut(),
        range: HookRange::Inclusive { begin, end },
    }
}
fn keys(ranges: &Ranges, address: u64) -> Vec<usize> {
    ranges.at(address).iter().map(|(key, _)| *key).collect()
}

#[test]
fn overlapping_ranges_preserve_order_and_inclusive_edges_after_removal() {
    let mut index = Index::default();
    let all = Hook::Instruction {
        callback: instruction,
        data: std::ptr::null_mut(),
        range: HookRange::All,
    };
    let hooks = [
        all,
        hook(10, 20),
        hook(15, 25),
        hook(20, 20),
        hook(u64::MAX, u64::MAX),
    ];
    for (key, &hook) in hooks.iter().enumerate() {
        index.insert(key, hook);
    }
    assert_eq!(keys(&index.instructions, 9), [0]);
    assert_eq!(keys(&index.instructions, 10), [0, 1]);
    assert_eq!(keys(&index.instructions, 19), [0, 1, 2]);
    assert_eq!(keys(&index.instructions, 20), [0, 1, 2, 3]);
    assert_eq!(keys(&index.instructions, 21), [0, 2]);
    assert_eq!(keys(&index.instructions, 25), [0, 2]);
    assert_eq!(keys(&index.instructions, 26), [0]);
    assert_eq!(keys(&index.instructions, u64::MAX), [0, 4]);
    for key in [1, 0, 3, 2, 4] {
        index.remove(key, hooks[key]);
        for address in [0, 9, 10, 15, 19, 20, 21, 25, 26, u64::MAX] {
            assert!(!keys(&index.instructions, address).contains(&key));
        }
    }
    assert!(index.instructions.0.is_empty());
}

#[test]
fn sparse_instruction_points_do_not_populate_other_callback_kinds() {
    let mut index = Index::default();
    for key in 0..4096 {
        index.insert(key, hook(key as u64 * 4, key as u64 * 4));
    }
    for address in 0..16384 {
        let expected = if address % 4 == 0 {
            vec![address as usize / 4]
        } else {
            vec![]
        };
        assert_eq!(keys(&index.instructions, address), expected);
        assert!(index.stores.at(address).is_empty());
        assert!(index.blocks.at(address).is_empty());
    }
    for key in 0..4096 {
        index.remove(key, hook(key as u64 * 4, key as u64 * 4));
    }
    assert!(index.instructions.0.is_empty());
}

#[test]
fn native_callbacks_are_indexed_only_for_selected_accesses() {
    unsafe fn memory(_: Access, _: u64, _: i32, _: i64, _: *mut c_void) {}
    let mut index = Index::default();
    let range = HookRange::Inclusive { begin: 10, end: 20 };
    let hooks = [
        Hook::Memory {
            callback: memory,
            data: std::ptr::null_mut(),
            read: true,
            write: false,
            range,
        },
        Hook::Memory {
            callback: memory,
            data: std::ptr::null_mut(),
            read: false,
            write: true,
            range,
        },
        Hook::CompletedStore {
            callback: memory,
            data: std::ptr::null_mut(),
            range,
        },
    ];
    for (key, &hook) in hooks.iter().enumerate() {
        index.insert(key, hook);
    }
    assert_eq!(keys(&index.reads, 15), [0]);
    assert_eq!(keys(&index.writes, 15), [1]);
    assert_eq!(keys(&index.stores, 15), [2]);
    for (key, &hook) in hooks.iter().enumerate() {
        index.remove(key, hook);
    }
    assert!(index.reads.0.is_empty());
    assert!(index.writes.0.is_empty());
    assert!(index.stores.0.is_empty());
}

#[test]
fn observer_points_use_backend_hooks_without_installing_native_hooks() {
    let cpu = super::super::tests::processor_with_options(&Memory::default(), Options::default());
    cpu.map_zeroed(0, 0x4000, 7).unwrap();
    let native_hooks = cpu.execution.cpu.hooks.borrow().len();
    assert!(matches!(
        cpu.execution
            .classify(0x1000, 0xe3a00001)
            .unwrap()
            .instruction,
        InstructionDisposition::Execute
    ));
    let id = unsafe { cpu.install_hook(hook(0x1000, 0x1000)) }.unwrap();
    assert_eq!(cpu.execution.cpu.hooks.borrow().len(), native_hooks);
    assert!(matches!(
        cpu.execution
            .classify(0x1000, 0xe3a00001)
            .unwrap()
            .instruction,
        InstructionDisposition::ObserveAndEndBlock
    ));
    assert!(matches!(
        cpu.execution
            .classify(0x1004, 0xe3a00001)
            .unwrap()
            .instruction,
        InstructionDisposition::Execute
    ));
    cpu.remove_hook(id).unwrap();
    assert!(matches!(
        cpu.execution
            .classify(0x1000, 0xe3a00001)
            .unwrap()
            .instruction,
        InstructionDisposition::Execute
    ));
    assert_eq!(cpu.execution.cpu.hooks.borrow().len(), native_hooks);
}

#[test]
fn range_index_matches_direct_filter_through_interleaved_mutations() {
    let mut index = Index::default();
    let mut active = BTreeMap::new();
    let mut seed = 0x739u64;
    for key in 0..256 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let begin = seed % 64;
        let end = begin + (seed >> 32) % 32;
        let observer = hook(begin, end);
        index.insert(key, observer);
        active.insert(key, (begin, end, observer));
        if key % 3 == 0 {
            let remove = *active.keys().next().unwrap();
            let (_, _, observer) = active.remove(&remove).unwrap();
            index.remove(remove, observer);
        }
        for address in 0..96 {
            let expected: Vec<_> = active
                .iter()
                .filter(|(_, (begin, end, _))| *begin <= address && address <= *end)
                .map(|(key, _)| *key)
                .collect();
            assert_eq!(keys(&index.instructions, address), expected);
        }
    }
}

#[test]
fn removal_leaves_the_spans_a_whole_index_coalescing_would() {
    // Deterministic pseudo-random installs and removals over overlapping ranges.
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    let mut next = |bound: u64| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed % bound
    };
    let mut ranges = Ranges::default();
    let mut live = vec![];
    for key in 0..600 {
        if live.len() > 8 && next(3) == 0 {
            let (key, hook): (usize, Hook) = live.swap_remove(next(live.len() as u64) as usize);
            let Hook::Instruction { range, .. } = hook else {
                unreachable!()
            };
            ranges.remove(key, range);
        } else {
            let begin = next(256);
            let hook = hook(begin, begin + next(64));
            let Hook::Instruction { range, .. } = hook else {
                unreachable!()
            };
            ranges.insert(key, hook, range);
            live.push((key, hook));
        }
        // Coalesced: no empty first span and no span repeating the one before.
        let spans: Vec<Vec<usize>> = ranges
            .0
            .values()
            .map(|hooks| hooks.iter().map(|(key, _)| *key).collect())
            .collect();
        assert!(spans.first().is_none_or(|first| !first.is_empty()));
        assert!(spans.windows(2).all(|pair| pair[0] != pair[1]));
        for address in 0..330 {
            let mut expected: Vec<usize> = live
                .iter()
                .filter(|(_, hook)| matches!(hook, Hook::Instruction { range: HookRange::Inclusive { begin, end }, .. } if (*begin..=*end).contains(&address)))
                .map(|(key, _)| *key)
                .collect();
            expected.sort();
            let mut found = keys(&ranges, address);
            found.sort();
            assert_eq!(found, expected, "address {address}");
        }
    }
}
