use super::*;

struct Fake {
    next: u32,
    modules: Vec<Vec<u8>>,
    functions: BTreeSet<u32>,
    slots: BTreeMap<u32, u32>,
    live: BTreeSet<u32>,
    configured: Option<(u32, u32)>,
    fail_install: bool,
    metadata_calls: usize,
}
impl Default for Fake {
    fn default() -> Self {
        Self {
            next: 1,
            modules: Vec::new(),
            functions: BTreeSet::new(),
            slots: BTreeMap::new(),
            live: BTreeSet::new(),
            configured: None,
            fail_install: false,
            metadata_calls: 0,
        }
    }
}
impl Host for Fake {
    fn dispatch_layout(&mut self) -> Result<Vec<u32>, String> {
        Ok(vec![2, 16, 24, 0x9000])
    }
    fn metadata(&mut self, _engine: u32) -> Result<Metadata, String> {
        self.metadata_calls += 1;
        Ok(Metadata {
            names: vec!["movi_i32".into(), "exit_tb".into()],
            environment: Environment::default(),
        })
    }
    fn helper(&mut self, _pointer: u32) -> Result<Option<Helper>, String> {
        Ok(None)
    }
    fn grow_table(&mut self, count: u32) -> Result<u32, String> {
        let first = self.next;
        self.next += count;
        Ok(first)
    }
    fn install(&mut self, module: &Module, slots: &[u32]) -> Result<(), String> {
        assert_eq!(&module.bytes[..8], &[0, 97, 115, 109, 1, 0, 0, 0]);
        assert_eq!(slots.len(), module.exports.len());
        if self.fail_install {
            return Err("host installation refused".into());
        }
        self.modules.push(module.bytes.clone());
        for slot in slots {
            assert!(self.functions.insert(*slot));
        }
        Ok(())
    }
    fn clear_function(&mut self, slot: u32) {
        self.functions.remove(&slot);
    }
    fn publish(&mut self, address: u32, slot: u32) {
        assert!(self.live.contains(&address), "published a retired TB slot");
        assert!(slot == 0 || self.functions.contains(&slot));
        self.slots.insert(address, slot);
    }
    fn configure(&mut self, threshold: u32, dispatcher: u32) -> Result<(), String> {
        assert!(self.configured.is_none());
        self.configured = Some((threshold, dispatcher));
        Ok(())
    }
    fn now_millis(&self) -> f64 {
        0.0
    }
}
fn code(value: u32) -> Vec<u8> {
    let mut bytes = vec![0, 7, 9];
    bytes.extend(value.to_le_bytes());
    bytes.extend([1, 10, 0, 0, 0, 0, 0, 0, 0, 0]);
    bytes
}
fn address(engine: u32, id: u32) -> u32 {
    1000 + engine * 100 + id * 4
}
fn offer(tier: &mut State, host: &mut Fake, engine: u32, id: u32, value: u32) {
    let address = address(engine, id);
    host.live.insert(address);
    host.slots.insert(address, 0);
    tier.notice(host, 1, engine, id, address, 0x5000, &code(value))
        .unwrap();
}
fn invalidate(tier: &mut State, host: &mut Fake, engine: u32, id: u32) {
    let address = address(engine, id);
    host.live.remove(&address);
    host.slots.insert(address, 0);
    tier.notice(host, 2, engine, id, 0, 0, &[]).unwrap();
}
fn invariant(tier: &State) {
    assert_eq!(
        tier.installed,
        tier.records.values().filter(|r| r.slot.is_some()).count()
    );
    assert_eq!(
        tier.retained,
        tier.stats.dispatcher_bytes
            + tier.groups.values().map(|g| g.bytes).sum::<usize>()
            + tier
                .records
                .values()
                .map(|r| r.code.bytes().len())
                .sum::<usize>()
            + tier.cache.keys().map(|c| c.len()).sum::<usize>()
    );
    assert_eq!(tier.cache.len(), tier.lru.len());
    assert_eq!(tier.cache.len(), tier.cache_slots.len());
    assert_eq!(tier.free.len(), tier.cache.len() + tier.empty.len());
    assert!(tier.retained <= tier.options.max_bytes);
    assert!(tier.grown <= tier.options.max_functions);
    assert_eq!(tier.grown, tier.installed + tier.free.len());
    for (id, group) in &tier.groups {
        assert_eq!(
            group.live,
            tier.records
                .values()
                .filter(|r| r.group == Some(*id))
                .count()
                + tier.cache.values().filter(|r| r.group == *id).count()
        );
    }
}
#[test]
fn cancels_pending_generations_before_publication() {
    let mut host = Fake::default();
    let mut tier = State::new(
        JitOptions {
            batch_size: 2,
            ..Default::default()
        },
        &mut host,
    )
    .unwrap();
    offer(&mut tier, &mut host, 1, 1, 1);
    invalidate(&mut tier, &mut host, 1, 1);
    tier.flush(&mut host).unwrap();
    assert_eq!(tier.installed(), 0);
    assert_eq!(tier.stats().compiled, 0);
    assert_eq!(host.modules.len(), 1);
    invariant(&tier);
    offer(&mut tier, &mut host, 1, 2, 2);
    offer(&mut tier, &mut host, 2, 3, 3);
    assert_eq!(tier.installed(), 2);
    assert_eq!(tier.stats().batches, 1);
    assert_eq!(host.modules.len(), 2);
    invariant(&tier);
}
#[test]
fn reuses_only_exact_function_bytes_after_generation_retirement() {
    let mut host = Fake::default();
    let mut tier = State::new(
        JitOptions {
            batch_size: 1,
            max_functions: 1,
            ..Default::default()
        },
        &mut host,
    )
    .unwrap();
    offer(&mut tier, &mut host, 1, 1, 42);
    let first = host.slots[&address(1, 1)];
    let bytes = tier.retained_bytes();
    invalidate(&mut tier, &mut host, 1, 1);
    assert_eq!(host.slots[&address(1, 1)], 0);
    assert_eq!(tier.cached(), 1);
    invariant(&tier);
    offer(&mut tier, &mut host, 1, 2, 42);
    assert_eq!(host.slots[&address(1, 2)], first);
    assert_eq!(tier.stats().cache_hits, 1);
    assert_eq!(tier.stats().compiled, 1);
    assert_eq!(tier.retained_bytes(), bytes);
    assert_eq!(host.modules.len(), 2);
    invariant(&tier);
    invalidate(&mut tier, &mut host, 1, 2);
    offer(&mut tier, &mut host, 1, 3, 43);
    assert_eq!(tier.stats().compiled, 2);
    assert_eq!(tier.stats().cache_hits, 1);
    assert_ne!(host.modules[1], host.modules[2]);
    assert_eq!(host.next, 3);
    invariant(&tier);
}
#[test]
fn capacity_refusal_keeps_tci_then_reuses_retired_table_capacity() {
    let mut host = Fake::default();
    let mut tier = State::new(
        JitOptions {
            batch_size: 1,
            max_functions: 1,
            ..Default::default()
        },
        &mut host,
    )
    .unwrap();
    offer(&mut tier, &mut host, 1, 1, 1);
    offer(&mut tier, &mut host, 1, 2, 2);
    assert_eq!(host.slots[&address(1, 2)], 0);
    assert_eq!(tier.stats().capacity_refusals, 1);
    assert_eq!(tier.installed(), 1);
    invalidate(&mut tier, &mut host, 1, 1);
    offer(&mut tier, &mut host, 1, 3, 2);
    assert_eq!(tier.installed(), 1);
    assert_eq!(host.next, 3);
    assert_eq!(tier.cached(), 0);
    invariant(&tier);
}
#[test]
fn duplicate_cached_bodies_release_one_module_reference() {
    let mut host = Fake::default();
    let mut tier = State::new(
        JitOptions {
            batch_size: 2,
            ..Default::default()
        },
        &mut host,
    )
    .unwrap();
    offer(&mut tier, &mut host, 1, 1, 42);
    offer(&mut tier, &mut host, 1, 2, 42);
    invariant(&tier);
    invalidate(&mut tier, &mut host, 1, 1);
    invalidate(&mut tier, &mut host, 1, 2);
    assert_eq!(tier.cached(), 1);
    assert_eq!(tier.groups.len(), 1);
    invariant(&tier);
    offer(&mut tier, &mut host, 1, 3, 42);
    assert_eq!(tier.stats().cache_hits, 1);
    invariant(&tier);
}
#[test]
fn install_failure_releases_pending_bytes_and_never_publishes_a_tb() {
    let mut host = Fake::default();
    let mut tier = State::new(
        JitOptions {
            batch_size: 2,
            ..Default::default()
        },
        &mut host,
    )
    .unwrap();
    host.fail_install = true;
    offer(&mut tier, &mut host, 1, 1, 1);
    let a = address(1, 2);
    host.live.insert(a);
    host.slots.insert(a, 0);
    assert!(
        tier.notice(&mut host, 1, 1, 2, a, 0x5000, &code(2))
            .is_err()
    );
    assert_eq!(tier.stats().failures, 1);
    assert_eq!(host.slots[&address(1, 1)], 0);
    assert_eq!(host.slots[&a], 0);
    assert_eq!(tier.retained_bytes(), tier.stats().dispatcher_bytes);
    assert_eq!(host.functions.len(), 1);
    invariant(&tier);
    host.fail_install = false;
    offer(&mut tier, &mut host, 1, 3, 3);
    tier.flush(&mut host).unwrap();
    invariant(&tier);
}
#[test]
fn byte_budget_refusal_does_not_install_an_oversized_batch() {
    let mut host = Fake::default();
    let mut tier = State::new(
        JitOptions {
            batch_size: 1,
            max_bytes: 65536,
            ..Default::default()
        },
        &mut host,
    )
    .unwrap();
    let mut bytes = Vec::new();
    for value in 0u32..8000 {
        bytes.extend(&code(value)[..7]);
    }
    bytes.extend([1, 10, 0, 0, 0, 0, 0, 0, 0, 0]);
    host.live.insert(1000);
    tier.notice(&mut host, 1, 1, 1, 1000, 0x5000, &bytes)
        .unwrap();
    assert_eq!(tier.stats().capacity_refusals, 1);
    assert_eq!(tier.stats().failures, 0);
    assert_eq!(host.modules.len(), 1);
    invariant(&tier);
}
#[test]
fn engine_flush_retires_only_its_generations_and_rebuilds_metadata() {
    let mut host = Fake::default();
    let mut tier = State::new(
        JitOptions {
            batch_size: 1,
            ..Default::default()
        },
        &mut host,
    )
    .unwrap();
    offer(&mut tier, &mut host, 1, 1, 1);
    offer(&mut tier, &mut host, 2, 2, 2);
    host.live.remove(&address(1, 1));
    host.slots.insert(address(1, 1), 0);
    tier.notice(&mut host, 3, 1, 0, 0, 0, &[]).unwrap();
    assert_eq!(tier.installed(), 1);
    assert_ne!(host.slots[&address(2, 2)], 0);
    assert_eq!(tier.cached(), 1);
    offer(&mut tier, &mut host, 1, 3, 1);
    assert_eq!(host.metadata_calls, 3);
    assert_eq!(tier.stats().cache_hits, 1);
    invariant(&tier);
}
#[test]
fn unsupported_and_duplicate_offers_never_install_stale_code() {
    let mut host = Fake::default();
    let mut tier = State::new(
        JitOptions {
            batch_size: 1,
            ..Default::default()
        },
        &mut host,
    )
    .unwrap();
    host.live.insert(1000);
    tier.notice(&mut host, 1, 1, 1, 1000, 0, &[255, 2]).unwrap();
    assert_eq!(tier.stats().unsupported, 1);
    offer(&mut tier, &mut host, 1, 2, 2);
    let a = address(1, 2);
    assert!(
        tier.notice(&mut host, 1, 1, 2, a, 0x5000, &code(3))
            .is_err()
    );
    assert_eq!(host.slots[&a], 0);
    assert_eq!(tier.installed(), 0);
    assert_eq!(tier.stats().failures, 1);
    invariant(&tier);
}
#[test]
fn rejects_option_bounds_before_allocating_host_resources() {
    let mut host = Fake::default();
    assert!(
        State::new(
            JitOptions {
                max_functions: 0,
                ..Default::default()
            },
            &mut host
        )
        .is_err()
    );
    assert_eq!(host.next, 1);
    assert!(host.modules.is_empty());
    for options in [
        JitOptions {
            threshold: 0,
            ..Default::default()
        },
        JitOptions {
            batch_size: 65,
            ..Default::default()
        },
        JitOptions {
            max_bytes: 65535,
            ..Default::default()
        },
        JitOptions {
            max_bytes: (128 << 20) + 1,
            ..Default::default()
        },
    ] {
        assert!(options.validate().is_err());
    }
}

#[test]
fn public_tier_owns_one_import_environment() {
    let mut tier = Tier::new(
        JitOptions {
            batch_size: 1,
            ..Default::default()
        },
        Fake::default(),
    )
    .unwrap();
    tier.host.live.insert(1000);
    tier.notice(1, 1, 1, 1000, 0x5000, &code(42)).unwrap();
    assert_eq!(tier.installed(), 1);
    assert_ne!(tier.host.slots[&1000], 0);
    invariant(&tier.state);
}

#[test]
fn table_pressure_evicts_oldest_cached_function_instead_of_lowest_slot() {
    let mut host = Fake::default();
    let mut tier = State::new(
        JitOptions {
            batch_size: 1,
            max_functions: 3,
            ..Default::default()
        },
        &mut host,
    )
    .unwrap();
    for id in 1..=3 {
        offer(&mut tier, &mut host, 1, id, id);
    }
    for id in [3, 1, 2] {
        invalidate(&mut tier, &mut host, 1, id);
    }
    offer(&mut tier, &mut host, 1, 4, 4);
    invalidate(&mut tier, &mut host, 1, 4);
    offer(&mut tier, &mut host, 1, 5, 1);
    assert_eq!(tier.stats().compiled, 4);
    assert_eq!(tier.stats().cache_hits, 1);
    assert_eq!(host.next, 5);
    invariant(&tier);
}

#[test]
fn reuses_relocated_body_with_current_generation_exit_token() {
    let mut host = Fake::default();
    let mut tier = State::new(
        JitOptions {
            batch_size: 1,
            ..Default::default()
        },
        &mut host,
    )
    .unwrap();
    let exit = |token: u32| {
        let mut bytes = vec![1, 10];
        bytes.extend(token.to_le_bytes());
        bytes.extend([0; 4]);
        bytes
    };
    let a = address(1, 1);
    host.live.insert(a);
    tier.notice(&mut host, 1, 1, 1, a, 0x5000, &exit(0x12345678))
        .unwrap();
    let slot = host.slots[&a];
    invalidate(&mut tier, &mut host, 1, 1);
    let b = address(1, 2);
    host.live.insert(b);
    tier.notice(&mut host, 1, 1, 2, b, 0x7000, &exit(0x87654321))
        .unwrap();
    assert_eq!(host.slots[&b], slot);
    assert_eq!(tier.stats().compiled, 1);
    assert_eq!(tier.stats().cache_hits, 1);
    invariant(&tier);
}
