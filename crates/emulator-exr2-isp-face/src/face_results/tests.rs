use super::*;
use fujimod_emulator_devices::{Host, Record};

#[derive(Default)]
struct Clock {
    now: u64,
    interrupts: Vec<(usize, u32, u64)>,
}
impl Host for Clock {
    fn now(&self) -> u64 {
        self.now
    }
    fn schedule(&mut self, _: usize, _: u64, _: u64) {
        panic!("unexpected closure")
    }
    fn set_irq(&mut self, _: u32, _: bool) {
        panic!("unexpected shared IRQ")
    }
    fn raise_irq(&mut self, cpu: usize, line: u32) -> Result<(), String> {
        self.interrupts.push((cpu, line, self.now));
        Ok(())
    }
    fn ram_read(&self, _: u64, _: &mut [u8]) -> Result<(), String> {
        panic!("no software structure reads")
    }
    fn ram_write(&mut self, _: u64, _: &[u8]) -> Result<(), String> {
        panic!("results are MMIO only")
    }
    fn record(&mut self, _: Record) {}
}
fn write(
    model: &mut FaceResults,
    host: &mut Clock,
    core: usize,
    offset: u64,
    value: u64,
) -> Option<String> {
    let mut cx = Cx::new(host, core, 0x12345678, 0);
    model.write(&mut cx, BASE + offset, 4, value);
    cx.take_refusal()
}
fn read(model: &mut FaceResults, host: &mut Clock, offset: u64) -> u64 {
    let mut cx = Cx::new(host, 0, 0xfedcba98, 0);
    let value = model.read(&mut cx, BASE + offset, 4);
    assert_eq!(cx.take_refusal(), None);
    value
}
fn tick(model: &mut FaceResults, host: &mut Clock) {
    let mut cx = Cx::new(host, usize::MAX, 0, 0);
    model.advance(&mut cx);
    assert_eq!(cx.take_refusal(), None);
}
fn ready(profile: Profile) -> (FaceResults, IrqRoutes, Clock) {
    let routes = IrqRoutes::default();
    routes.set(47, Some(0)).unwrap();
    let mut model = FaceResults::new(profile, routes.clone());
    let mut host = Clock::default();
    assert_eq!(write(&mut model, &mut host, 0, 0x48, 0x40001000), None);
    (model, routes, host)
}

#[test]
fn completion_latches_start_endpoint_and_ignores_later_masking() {
    let (mut model, routes, mut host) = ready(Profile::Box64x20);
    assert_eq!(write(&mut model, &mut host, 0, 4, 1), None);
    assert_eq!(model.next_deadline(0), Some(LATENCY));
    routes.set(47, None).unwrap();
    for now in [0, 1, LATENCY - 1] {
        host.now = now;
        tick(&mut model, &mut host);
        assert!(host.interrupts.is_empty());
        assert_eq!(read(&mut model, &mut host, 0x600), 0);
    }
    host.now = LATENCY;
    tick(&mut model, &mut host);
    assert_eq!(host.interrupts, [(0, 47, LATENCY)]);
    assert_eq!(read(&mut model, &mut host, 0x600), 1);
    assert_eq!(read(&mut model, &mut host, 0x614), 64 * 128);
    assert_eq!(model.next_deadline(host.now), None);
    tick(&mut model, &mut host);
    assert_eq!(host.interrupts.len(), 1);
    // Rearming while both controllers mask the line keeps the bank its last
    // unique enable established.
    assert_eq!(write(&mut model, &mut host, 0, 4, 1), None);
    host.now += LATENCY;
    tick(&mut model, &mut host);
    assert_eq!(host.interrupts[1], (0, 47, 2 * LATENCY));
    routes.set(47, Some(1)).unwrap();
    assert_eq!(write(&mut model, &mut host, 0, 4, 1), None);
    host.now += LATENCY;
    tick(&mut model, &mut host);
    assert_eq!(host.interrupts[2], (1, 47, 3 * LATENCY));
}

#[test]
fn repeated_empty_requests_outlive_diagnostics_and_restore_exactly() {
    let (mut model, routes, mut host) = ready(Profile::Empty);
    for _ in 0..4097 {
        assert_eq!(write(&mut model, &mut host, 0, 4, 1), None);
        host.now += LATENCY;
        tick(&mut model, &mut host);
        for offset in [0x600, 0x610, 0x614, 0x618] {
            assert_eq!(read(&mut model, &mut host, offset), 0);
        }
    }
    assert_eq!(model.state.requested, 4097);
    assert_eq!(model.state.produced, 4097);
    assert_eq!(model.state.recent.len(), HISTORY);
    assert_eq!(host.interrupts.len(), 4097);
    let before = model.save();
    for _ in 0..5 {
        assert_eq!(model.fingerprint().unwrap(), before);
        model.checkpoint().unwrap();
        assert_eq!(model.save(), before);
    }
    let mut restored = FaceResults::new(Profile::Empty, routes);
    restored.restore(&before).unwrap();
    assert_eq!(restored.save(), before);
}

#[test]
fn invalid_start_refuses_without_mutation_and_reset_cancels_pending_job() {
    let (mut model, _, mut host) = ready(Profile::Empty);
    let before = model.save();
    assert!(write(&mut model, &mut host, 1, 4, 1).is_some());
    assert_eq!(model.save(), before);
    assert_eq!(write(&mut model, &mut host, 0, 0x48, 4), None);
    let before = model.save();
    assert!(write(&mut model, &mut host, 0, 4, 1).is_some());
    assert_eq!(model.save(), before);
    assert_eq!(write(&mut model, &mut host, 0, 0x48, 0x40001000), None);
    assert_eq!(write(&mut model, &mut host, 0, 4, 1), None);
    let before = model.save();
    assert!(write(&mut model, &mut host, 0, 4, 1).is_some());
    assert_eq!(model.save(), before);
    let mut restored = FaceResults::new(Profile::Empty, IrqRoutes::default());
    restored.restore(&before).unwrap();
    host.now = LATENCY;
    tick(&mut restored, &mut host);
    assert_eq!(host.interrupts, [(0, 47, LATENCY)]);
    assert_eq!(write(&mut model, &mut host, 0, 0, 0), None);
    tick(&mut model, &mut host);
    assert_eq!(host.interrupts.len(), 1);
    let mut invalid = before;
    invalid["due"] = serde_json::json!(1);
    assert!(model.restore(&invalid).is_err());
}

#[test]
fn image_source_addresses_latch_at_start_and_survive_restore() {
    let (mut model, routes, mut host) = ready(Profile::Empty);
    for address in [0x40000000, 0x4abc0000, 0x5ffffffc] {
        assert_eq!(write(&mut model, &mut host, 0, 0x48, address), None);
        assert_eq!(write(&mut model, &mut host, 0, 4, 1), None);
        assert_eq!(
            model.state.recent.back().unwrap().source_address,
            address as u32
        );
        assert_eq!(write(&mut model, &mut host, 0, 0x48, 0x40002000), None);
        assert_eq!(
            model.state.recent.back().unwrap().source_address,
            address as u32
        );
        let saved = model.save();
        let mut restored = FaceResults::new(Profile::Empty, routes.clone());
        restored.restore(&saved).unwrap();
        assert_eq!(restored.save(), saved);
        host.now += LATENCY;
        tick(&mut model, &mut host);
    }
}

#[test]
fn invalid_source_addresses_refuse_start_and_saved_requests() {
    for address in [0, 3, 0x3ffffffc, 0x40000001, 0x60000000, 0xfffffff8] {
        let (mut model, _, mut host) = ready(Profile::Empty);
        assert_eq!(write(&mut model, &mut host, 0, 0x48, address), None);
        let before = model.save();
        assert!(write(&mut model, &mut host, 0, 4, 1).is_some());
        assert_eq!(model.save(), before);

        assert_eq!(write(&mut model, &mut host, 0, 0x48, 0x40001000), None);
        assert_eq!(write(&mut model, &mut host, 0, 4, 1), None);
        let before = model.save();
        let mut invalid = before.clone();
        invalid["recent"][0]["source_address"] = serde_json::json!(address);
        assert!(model.restore(&invalid).is_err());
        assert_eq!(model.save(), before);
    }
}
