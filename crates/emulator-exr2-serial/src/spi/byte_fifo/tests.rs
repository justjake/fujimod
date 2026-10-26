use super::*;
use fujimod_emulator_devices::bus::InterruptLine;
use crate::dma::{peripheral, routing};
use fujimod_emulator_devices::{Host, Record};
use std::collections::BTreeMap;

#[derive(Default)]
struct TestWire {
    frames: Vec<(u64, u16)>,
}
impl Wire for TestWire {
    fn begin(&mut self, f: Frame) -> Result<u64, String> {
        if f.bits != 8 || f.mode != 3 || f.word > 255 {
            return Err("unexpected test frame".into());
        }
        Ok(f.word as u64)
    }
    fn finish(&mut self, token: u64, f: Frame, now: u64) -> Result<u16, String> {
        if token != self.begin(f)? {
            return Err("test frame token differs".into());
        }
        self.frames.push((now, f.word));
        Ok(f.word ^ 0xa5)
    }
    fn save(&self) -> Value {
        json!(self.frames)
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        self.frames = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        Ok(())
    }
}
struct Clock {
    now: u64,
    ram: BTreeMap<u64, u8>,
    irqs: Vec<(u64, usize, u32, bool)>,
}
impl Default for Clock {
    fn default() -> Self {
        Self {
            now: 0,
            ram: [0, 0, 0x3f, 0xc6]
                .into_iter()
                .enumerate()
                .map(|(i, v)| (0x1000 + i as u64, v))
                .chain((0x2000..0x2004).map(|a| (a, 0xee)))
                .collect(),
            irqs: vec![],
        }
    }
}
impl Host for Clock {
    fn now(&self) -> u64 {
        self.now
    }
    fn schedule(&mut self, _: usize, _: u64, _: u64) {
        panic!("serial clock uses its exact deadline")
    }
    fn set_irq(&mut self, _: u32, _: bool) {
        unreachable!()
    }
    fn raise_irq(&mut self, _: usize, _: u32) -> Result<(), String> {
        unreachable!()
    }
    fn set_core_irq(&mut self, cpu: usize, line: u32, high: bool) -> Result<(), String> {
        self.irqs.push((self.now, cpu, line, high));
        Ok(())
    }
    fn ram_read(&self, address: u64, out: &mut [u8]) -> Result<(), String> {
        for (i, b) in out.iter_mut().enumerate() {
            *b = *self
                .ram
                .get(&(address + i as u64))
                .ok_or("unmapped test source")?;
        }
        Ok(())
    }
    fn ram_write(&mut self, address: u64, bytes: &[u8]) -> Result<(), String> {
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
fn store(
    d: &mut dyn Device,
    h: &mut Clock,
    pc: u64,
    address: u64,
    value: u64,
) -> Result<(), String> {
    let mut cx = Cx::new(h, 0, pc, 0);
    d.write(&mut cx, address, 4, value);
    cx.take_refusal().map_or(Ok(()), Err)
}
fn advance(d: &mut dyn Device, h: &mut Clock, pc: u64, at: u64) {
    h.now = at;
    let mut cx = Cx::new(h, 1, pc, 0);
    d.advance(&mut cx);
    assert_eq!(cx.refusal(), None);
}
fn serial(h: &mut Clock, pc: u64) -> ByteFifo {
    let mut s = ByteFifo::new(0x5000, 64, Box::<TestWire>::default()).unwrap();
    for (offset, value) in [
        (0, 0x40),
        (4, 8),
        (8, FIFO_RESET),
        (8, 0),
        (0xc, 0x3311),
        (0x10, 1),
        (0x14, 1),
        (0x20, 0x400),
        (8, 0x01000000),
        (8, 0x09000000),
        (8, 0x09010000),
        (8, DMA_ENABLE),
    ] {
        store(&mut s, h, pc, 0x5000 + offset, value as u64).unwrap();
    }
    assert_eq!(s.next_deadline(0), None);
    s
}
fn transfer(
    pc: u64,
) -> (
    Value,
    Value,
    BTreeMap<u64, u8>,
    Vec<(u64, usize, u32, bool)>,
) {
    let mut h = Clock::default();
    let mut s = serial(&mut h, pc);
    let port = s.port();
    let shared_routing = routing::SharedRouting::default();
    let mut routes = routing::Block98500::new(0x8000, shared_routing.clone()).unwrap();
    let connections = BTreeMap::from([
        (
            9,
            peripheral::Connection {
                address: 0x501c,
                direction: Direction::Transmit,
                port: port.clone(),
            },
        ),
        (
            8,
            peripheral::Connection {
                address: 0x5018,
                direction: Direction::Receive,
                port: port.clone(),
            },
        ),
    ]);
    let mut irqs: [Vec<InterruptLine>; 8] = std::array::from_fn(|_| vec![]);
    irqs[2] = vec![InterruptLine::Core { core: 0, line: 93 }];
    irqs[3] = vec![InterruptLine::Core { core: 0, line: 94 }];
    let mut d =
        peripheral::PeripheralChannels::new(0x4000, shared_routing, connections, irqs).unwrap();
    store(&mut routes, &mut h, pc, 0x8000, 0x08090000).unwrap();
    for (channel, source, destination, mode) in [
        (2, 0x1000, 0x501c, 0x03200168),
        (3, 0x5018, 0x2000, 0x02100160),
    ] {
        for (offset, value) in [
            (0, source),
            (4, destination),
            (8, 4),
            (0x2c, mode | channel),
            (0x28, 8),
            (0x28, 1),
        ] {
            store(&mut d, &mut h, pc, 0x4000 + channel * 0x40 + offset, value).unwrap();
        }
    }
    assert_eq!(d.next_deadline(0), Some(0));
    for _ in 0..4 {
        advance(&mut d, &mut h, pc, 0);
    }
    assert_eq!(s.next_deadline(0), None);
    assert_eq!(d.next_deadline(0), None);
    assert_eq!(h.irqs, [(0, 0, 93, true)]); // TX FIFO acceptance is not RX completion.
    assert_eq!(h.ram[&0x2000], 0xee);
    store(&mut s, &mut h, pc, 0x5000, 0xc0).unwrap();
    assert_eq!(s.next_deadline(0), Some(64));
    let before = s.save();
    advance(&mut s, &mut h, pc, 63);
    assert_eq!(s.save(), before);
    for at in [64, 128, 192, 256] {
        advance(&mut s, &mut h, pc, at);
        assert_eq!(d.next_deadline(at), Some(at));
        advance(&mut d, &mut h, pc, at);
    }
    assert_eq!(h.irqs, [(0, 0, 93, true), (256, 0, 94, true)]);
    assert_eq!(
        (0..4).map(|i| h.ram[&(0x2000 + i)]).collect::<Vec<_>>(),
        [0xa5, 0xa5, 0x9a, 0x63]
    );
    assert_eq!(s.next_deadline(256), None);
    assert_eq!(d.next_deadline(256), None);
    assert_eq!(
        s.save()["wire"],
        json!([[64, 0], [128, 0], [192, 63], [256, 198]])
    );
    let saved = s.save();
    assert_eq!(s.save(), saved);
    assert_eq!(
        port.lock().unwrap().request(Direction::Receive, false),
        None
    );
    assert_eq!(s.save(), saved);
    (s.save(), d.save(), h.ram, h.irqs)
}
#[test]
fn dma_fifo_and_shift_clock_have_distinct_causal_completions() {
    assert_eq!(transfer(0x1234), transfer(0xdeadbeef));
}
fn queued(pc: u64) -> (ByteFifo, Clock) {
    let mut h = Clock::default();
    let mut s = serial(&mut h, pc);
    let port = s.port();
    for word in [0, 0, 0x3f, 0xc6] {
        port.lock().unwrap().transmit(0, 1, word).unwrap();
    }
    store(&mut s, &mut h, pc, 0x5000, 0xc0).unwrap();
    (s, h)
}
#[test]
fn late_ticks_and_checkpoint_restore_preserve_each_wire_edge() {
    let (mut a, mut ha) = queued(0x1234);
    let (mut b, mut hb) = queued(0xdead);
    for at in [64, 128, 192, 256] {
        advance(&mut a, &mut ha, 0x1234, at);
    }
    advance(&mut b, &mut hb, 0xdead, 256);
    assert_eq!(a.save(), b.save());
    let (mut source, mut host) = queued(0x4444);
    advance(&mut source, &mut host, 0x4444, 64);
    let snapshot = source.save();
    let mut resumed = ByteFifo::new(0x5000, 64, Box::<TestWire>::default()).unwrap();
    resumed.restore(&snapshot).unwrap();
    assert_eq!(resumed.save(), snapshot);
    for at in [127, 128, 192, 256] {
        advance(&mut source, &mut host, 0x4444, at);
        advance(&mut resumed, &mut host, 0x5555, at);
        assert_eq!(source.save(), resumed.save());
    }
    let port = resumed.port();
    let before = resumed.save();
    assert_eq!(
        port.lock().unwrap().request(Direction::Receive, false),
        Some(64)
    );
    assert_eq!(resumed.save(), before);
    let bytes = (0..4)
        .map(|_| port.lock().unwrap().receive(256, 1).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(bytes, [0xa5, 0xa5, 0x9a, 0x63]);
    assert_eq!(
        port.lock().unwrap().request(Direction::Receive, false),
        None
    );
}
#[test]
fn reset_cancels_a_shift_and_unknown_formats_or_corrupt_checkpoints_refuse() {
    let (mut s, mut h) = queued(0);
    store(&mut s, &mut h, 0, 0x5008, FIFO_RESET as u64).unwrap();
    assert_eq!(s.next_deadline(0), None);
    advance(&mut s, &mut h, 0, 300);
    assert_eq!(s.save()["wire"], json!([]));
    assert_eq!(s.save()["state"]["rx"], json!([]));
    for (address, value) in [
        (0x5004, 6),
        (0x5000, 0xe0),
        (0x5020, 0x401),
        (0x5018, 1),
        (0x5010, 2),
    ] {
        let before = s.save();
        assert!(store(&mut s, &mut h, 0, address, value).is_err());
        assert_eq!(s.save(), before);
    }
    let (mut s, _) = queued(0);
    let snapshot = s.save();
    for (field, value) in [
        ("format", json!(6)),
        ("control", json!(0x40)),
        ("rx", json!(vec![0; 16])),
    ] {
        let mut corrupt = snapshot.clone();
        corrupt["state"][field] = value;
        assert!(s.restore(&corrupt).is_err());
        assert_eq!(s.save(), snapshot);
    }
}
