use super::*;
use fujimod_emulator_devices::{Host, Record};
use std::cell::RefCell;

struct Clock {
    now: u64,
    ram: BTreeMap<u64, u8>,
    reads: RefCell<Vec<(u64, usize)>>,
    writes: Vec<(u64, Vec<u8>)>,
    irqs: Vec<(u64, usize, u32, bool)>,
}
impl Default for Clock {
    fn default() -> Self {
        Self {
            now: 0,
            ram: (0x1000..0x1004)
                .map(|a| (a, (a - 0x1000) as u8))
                .chain((0x2000..0x2004).map(|a| (a, 0xee)))
                .collect(),
            reads: RefCell::new(vec![]),
            writes: vec![],
            irqs: vec![],
        }
    }
}
impl Host for Clock {
    fn now(&self) -> u64 {
        self.now
    }
    fn schedule(&mut self, _: usize, _: u64, _: u64) {
        panic!("serial DMA must use exact deadlines");
    }
    fn set_irq(&mut self, _: u32, _: bool) {
        unreachable!()
    }
    fn raise_irq(&mut self, _: usize, _: u32) -> Result<(), String> {
        unreachable!()
    }
    fn set_core_irq(&mut self, core: usize, line: u32, high: bool) -> Result<(), String> {
        self.irqs.push((self.now, core, line, high));
        Ok(())
    }
    fn ram_read(&self, address: u64, out: &mut [u8]) -> Result<(), String> {
        self.reads.borrow_mut().push((address, out.len()));
        for (i, b) in out.iter_mut().enumerate() {
            *b = *self
                .ram
                .get(&(address + i as u64))
                .ok_or("unmapped test source")?;
        }
        Ok(())
    }
    fn ram_write(&mut self, address: u64, bytes: &[u8]) -> Result<(), String> {
        self.writes.push((address, bytes.to_vec()));
        for (i, b) in bytes.iter().enumerate() {
            *self
                .ram
                .get_mut(&(address + i as u64))
                .ok_or("unmapped test destination")? = *b;
        }
        Ok(())
    }
    fn record(&mut self, _: Record) {}
}
fn options() -> Options {
    Options {
        serial_base: 0x5000,
        dma_base: 0x4000,
        routing_base: 0x8000,
        channels: [2, 3],
        transmit_request: 9,
        receive_request: 8,
        frame_ticks: 64,
        irq_targets: vec![(2, 0, 93), (3, 0, 94)],
    }
}

fn advance(d: &mut Transport, h: &mut Clock, at: u64) {
    h.now = at;
    let mut cx = Cx::new(h, 0, 0, 0);
    d.advance(&mut cx);
    assert_eq!(cx.take_refusal(), None);
}
fn write(d: &mut Transport, h: &mut Clock, address: u64, value: u64) {
    let mut cx = Cx::new(h, 0, 0, 0);
    d.write(&mut cx, address, 4, value);
    assert_eq!(cx.take_refusal(), None);
    let now = h.now;
    advance(d, h, now);
}
fn start_count(h: &mut Clock, count: u64) -> Transport {
    let mut d = Transport::new(options(), Input::Fixed { high: true }).unwrap();
    for (a, v) in [
        (0x5000, 0x40),
        (0x5004, 8),
        (0x5008, 0x09090000),
        (0x5020, 0x400),
        (0x8000, 0x08090000),
    ] {
        write(&mut d, h, a, v);
    }
    for (channel, source, destination, mode) in [
        (2, 0x1000, 0x501c, 0x03200168),
        (3, 0x5018, 0x2000, 0x02100160),
    ] {
        for (offset, value) in [
            (0, source),
            (4, destination),
            (8, count),
            (0x2c, mode | channel),
            (0x28, 8),
            (0x28, 1),
        ] {
            write(&mut d, h, 0x4000 + channel * 0x40 + offset, value);
        }
    }
    write(&mut d, h, 0x5000, 0xc0);
    d
}
fn start(h: &mut Clock) -> Transport {
    start_count(h, 4)
}

#[test]
fn fixed_input_pair_moves_only_programmed_buffers_at_serial_edges() {
    let mut h = Clock::default();
    let mut d = start(&mut h);
    assert_eq!(
        *h.reads.borrow(),
        (0..4).map(|i| (0x1000 + i, 1)).collect::<Vec<_>>()
    );
    assert_eq!(h.irqs, [(0, 0, 93, true)]);
    assert!(h.writes.is_empty());
    let before = d.fingerprint().unwrap();
    for _ in 0..3 {
        assert_eq!(d.next_deadline(0), Some(64));
        assert_eq!(d.fingerprint().unwrap(), before);
        d.checkpoint().unwrap();
        d.save();
    }
    advance(&mut d, &mut h, 63);
    assert_eq!(d.fingerprint().unwrap(), before);
    for at in [64, 128, 192, 256] {
        advance(&mut d, &mut h, at);
    }
    assert_eq!(
        h.writes,
        (0..4).map(|i| (0x2000 + i, vec![255])).collect::<Vec<_>>()
    );
    assert_eq!(h.irqs, [(0, 0, 93, true), (256, 0, 94, true)]);
    assert_eq!(d.next_deadline(256), None);
}
#[test]
fn pending_pair_restores_typed_children_without_replaying_dma() {
    let mut h = Clock::default();
    let mut d = start(&mut h);
    let saved = d.checkpoint().unwrap();
    let before = d.fingerprint().unwrap();
    let invalid = saved.clone().with_children(vec![]);
    assert!(d.restore_checkpoint(&invalid).is_err());
    assert_eq!(d.fingerprint().unwrap(), before);
    let mut h2 = Clock::default();
    let mut restored = Transport::new(options(), Input::Fixed { high: true }).unwrap();
    restored.restore_checkpoint(&saved).unwrap();
    assert!(h2.reads.borrow().is_empty());
    assert!(h2.writes.is_empty());
    assert!(h2.irqs.is_empty());
    for at in [64, 128, 192, 256] {
        advance(&mut d, &mut h, at);
        advance(&mut restored, &mut h2, at);
        assert_eq!(d.fingerprint().unwrap(), restored.fingerprint().unwrap());
    }
    assert_eq!(h.writes, h2.writes);
    assert!(h2.reads.borrow().is_empty());
    let changed = options();
    assert!(
        Transport::new(changed, Input::Fixed { high: false })
            .unwrap()
            .validate_checkpoint(&saved)
            .is_err()
    );
    let mut overlap = options();
    overlap.routing_base = overlap.serial_base;
    assert!(Transport::new(overlap, Input::Fixed { high: true }).is_err());
}

#[test]
fn coarse_service_preserves_the_complete_long_transfer_state() {
    let mut fine = Clock::default();
    fine.ram = (0x1000..0x1083)
        .map(|a| (a, (a - 0x1000) as u8))
        .chain((0x2000..0x2083).map(|a| (a, 0xee)))
        .collect();
    let mut coarse = Clock::default();
    coarse.ram = fine.ram.clone();
    let mut a = start_count(&mut fine, 131);
    let mut b = start_count(&mut coarse, 131);
    for tick in (64..=131 * 64).step_by(64) {
        advance(&mut a, &mut fine, tick);
    }
    advance(&mut b, &mut coarse, 131 * 64);
    assert_eq!(a.fingerprint().unwrap(), b.fingerprint().unwrap());
    assert_eq!(fine.ram, coarse.ram);
    assert_eq!(fine.writes, coarse.writes);
    assert_eq!(a.next_deadline(fine.now), None);
    assert_eq!(b.next_deadline(coarse.now), None);
}
