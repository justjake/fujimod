use super::*;
use fujimod_emulator_devices::dma::Port;
use fujimod_emulator_devices::{Host, Record};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
struct WirePort {
    transmit_due: Option<u64>,
    transmitted: Vec<u8>,
    received: VecDeque<(u64, u8)>,
}
impl Port for WirePort {
    fn request(&self, direction: Direction, _: bool) -> Option<u64> {
        match direction {
            Direction::Transmit => self.transmit_due,
            Direction::Receive => self.received.front().map(|r| r.0),
        }
    }
    fn transmit(&mut self, now: u64, bytes: u8, value: u64) -> Result<(), String> {
        if bytes != 1 || value > 255 || !self.transmit_due.is_some_and(|at| at <= now) {
            return Err("test transmit without a byte request".into());
        }
        self.transmitted.push(value as u8);
        self.transmit_due = None;
        Ok(())
    }
    fn receive(&mut self, now: u64, bytes: u8) -> Result<u64, String> {
        if bytes != 1 || !self.received.front().is_some_and(|r| r.0 <= now) {
            return Err("test receive without a byte request".into());
        }
        Ok(self.received.pop_front().unwrap().1 as u64)
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
            ram: (0x1000..0x1010)
                .map(|a| (a, (a - 0x1000) as u8))
                .chain((0x2000..0x2010).map(|a| (a, 0xee)))
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
        panic!("DMA must wait on request wires, not polling timers")
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
        for (index, byte) in out.iter_mut().enumerate() {
            *byte = *self
                .ram
                .get(&(address + index as u64))
                .ok_or("unmapped test DMA source")?;
        }
        Ok(())
    }
    fn ram_write(&mut self, address: u64, bytes: &[u8]) -> Result<(), String> {
        for (index, byte) in bytes.iter().enumerate() {
            *self
                .ram
                .get_mut(&(address + index as u64))
                .ok_or("unmapped test DMA destination")? = *byte;
        }
        Ok(())
    }
    fn record(&mut self, _: Record) {}
}
fn devices() -> (
    PeripheralChannels,
    super::super::routing::Block98500,
    Arc<Mutex<WirePort>>,
) {
    let port = Arc::new(Mutex::new(WirePort::default()));
    let routing = SharedRouting::default();
    let connections = BTreeMap::from([
        (
            9,
            Connection {
                address: 0x501c,
                direction: Direction::Transmit,
                port: port.clone(),
            },
        ),
        (
            8,
            Connection {
                address: 0x5018,
                direction: Direction::Receive,
                port: port.clone(),
            },
        ),
    ]);
    let mut irqs: [Vec<InterruptLine>; 8] = std::array::from_fn(|_| vec![]);
    irqs[2] = vec![InterruptLine::Core { core: 0, line: 93 }];
    irqs[3] = vec![InterruptLine::Core { core: 1, line: 94 }];
    (
        PeripheralChannels::new(0x4000, routing.clone(), connections, irqs).unwrap(),
        super::super::routing::Block98500::new(0x8000, routing).unwrap(),
        port,
    )
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
fn prepare(
    d: &mut PeripheralChannels,
    r: &mut super::super::routing::Block98500,
    h: &mut Clock,
    pc: u64,
) {
    store(r, h, pc, 0x8000, 0x08090000).unwrap();
    for (channel, source, destination, mode) in [(2, 0x1000, 0x501c, TX), (3, 0x5018, 0x2000, RX)] {
        let base = 0x4000 + channel * 0x40;
        for (offset, value) in [
            (0, source),
            (4, destination),
            (8, 3),
            (0x2c, mode as u64 | channel),
            (0x28, 8),
            (0x28, 1),
        ] {
            store(d, h, pc, base + offset, value).unwrap();
        }
    }
}
fn advance(d: &mut PeripheralChannels, h: &mut Clock, pc: u64, at: u64) {
    h.now = at;
    let mut cx = Cx::new(h, 1, pc, 0);
    d.advance(&mut cx);
    assert_eq!(cx.refusal(), None);
}
fn trace(
    pc: u64,
) -> (
    Value,
    Vec<(u64, usize, u32, bool)>,
    BTreeMap<u64, u8>,
    Vec<u8>,
) {
    let (mut d, mut routing, port) = devices();
    let mut h = Clock::default();
    prepare(&mut d, &mut routing, &mut h, pc);
    assert_eq!(d.next_deadline(0), None);
    let before = d.save();
    advance(&mut d, &mut h, pc, 5);
    assert_eq!(d.save(), before);
    port.lock().unwrap().transmit_due = Some(10);
    port.lock().unwrap().received.push_back((12, 0xa1));
    assert_eq!(d.next_deadline(5), Some(10));
    advance(&mut d, &mut h, pc, 9);
    assert_eq!(d.save(), before);
    advance(&mut d, &mut h, pc, 10);
    assert_eq!(d.channels[2].registers[2], 2);
    assert_eq!(d.next_deadline(10), Some(12));
    advance(&mut d, &mut h, pc, 12);
    assert_eq!(h.ram[&0x2000], 0xa1);
    for (at, byte) in [(20, 0xb2), (30, 0xc3)] {
        {
            let mut p = port.lock().unwrap();
            p.transmit_due = Some(at);
            p.received.push_back((at, byte));
        }
        advance(&mut d, &mut h, pc, at);
    }
    assert_eq!(port.lock().unwrap().transmitted, [0, 1, 2]);
    assert_eq!(
        (0..3).map(|i| h.ram[&(0x2000 + i)]).collect::<Vec<_>>(),
        [0xa1, 0xb2, 0xc3]
    );
    assert_eq!(h.irqs, [(30, 0, 93, true), (30, 1, 94, true)]);
    assert_eq!(d.next_deadline(30), None);
    assert_eq!(d.channels[2].registers[9], 0x40);
    assert_eq!(d.channels[3].registers[9], 0x40);
    let saved = d.save();
    assert_eq!(d.save(), saved);
    assert_eq!(d.next_deadline(30), None);
    for channel in [2, 3] {
        store(&mut d, &mut h, pc, 0x4028 + channel * 0x40, 8).unwrap();
    }
    assert_eq!(&h.irqs[2..], [(30, 0, 93, false), (30, 1, 94, false)]);
    let tx = port.lock().unwrap().transmitted.clone();
    (d.save(), h.irqs, h.ram, tx)
}
#[test]
fn direct_duplex_requests_move_distinct_bytes_and_publish_only_completions() {
    assert_eq!(trace(0x1234), trace(0xdeadbeef));
}
#[test]
fn reset_cancels_a_waiting_channel_and_route_changes_cannot_retarget_live_dma() {
    let (mut d, mut routing, port) = devices();
    let mut h = Clock::default();
    prepare(&mut d, &mut routing, &mut h, 0);
    let before = routing.save();
    assert!(
        store(&mut routing, &mut h, 0, 0x8000, 0x08080000)
            .unwrap_err()
            .contains("while active")
    );
    assert_eq!(routing.save(), before);
    store(&mut d, &mut h, 0, 0x40a8, 8).unwrap();
    port.lock().unwrap().transmit_due = Some(10);
    advance(&mut d, &mut h, 0, 10);
    assert!(port.lock().unwrap().transmitted.is_empty());
    store(&mut routing, &mut h, 0, 0x8000, 0x08080000).unwrap();
    assert_eq!(d.channels[2].registers[9], 0);
    assert!(h.irqs.is_empty());
}
#[test]
fn unsupported_activation_refuses_without_consuming_port_or_ram() {
    for (offset, value) in [
        (0x2c, 0x321116a),
        (0x0c, 1),
        (0x30, 1),
        (0x38, 0x1234),
        (8, 0),
        (4, 0x5018),
    ] {
        let (mut d, mut routing, port) = devices();
        let mut h = Clock::default();
        prepare(&mut d, &mut routing, &mut h, 0);
        store(&mut d, &mut h, 0, 0x40a8, 8).unwrap();
        store(&mut d, &mut h, 0, 0x4080 + offset, value).unwrap();
        let before = d.save();
        let ram = h.ram.clone();
        assert!(store(&mut d, &mut h, 0, 0x40a8, 1).is_err());
        assert_eq!(d.save(), before);
        assert_eq!(h.ram, ram);
        assert!(port.lock().unwrap().transmitted.is_empty());
    }
}
#[test]
fn pending_checkpoint_restores_requests_and_rejects_corrupt_state_before_mutation() {
    let (mut source, mut routing, port) = devices();
    let mut h = Clock::default();
    prepare(&mut source, &mut routing, &mut h, 0);
    port.lock().unwrap().transmit_due = Some(7);
    advance(&mut source, &mut h, 0, 7);
    let snapshot = source.save();
    let (mut restored, _, resumed_port) = devices();
    restored.restore(&snapshot).unwrap();
    assert_eq!(restored.save(), snapshot);
    assert_eq!(restored.next_deadline(7), None);
    for (field, value) in [("active", json!(false)), ("registers", json!(vec![0; 16]))] {
        let mut corrupt = snapshot.clone();
        corrupt["channels"][2][field] = value;
        assert!(restored.restore(&corrupt).is_err());
        assert_eq!(restored.save(), snapshot);
    }
    let mut corrupt = snapshot.clone();
    corrupt["wiring"]["base"] = json!(0xdead);
    assert!(restored.restore(&corrupt).is_err());
    assert_eq!(restored.save(), snapshot);
    *resumed_port.lock().unwrap() = port.lock().unwrap().clone();
    let mut resumed_host = Clock {
        now: h.now,
        ram: h.ram.clone(),
        irqs: h.irqs.clone(),
    };
    for at in [10, 20, 30] {
        for p in [&port, &resumed_port] {
            let mut p = p.lock().unwrap();
            p.transmit_due = Some(at);
            p.received.push_back((at, at as u8));
        }
        advance(&mut source, &mut h, 0x1234, at);
        advance(&mut restored, &mut resumed_host, 0xdead, at);
        assert_eq!(source.save(), restored.save());
        assert_eq!(h.ram, resumed_host.ram);
        assert_eq!(h.irqs, resumed_host.irqs);
    }
    assert_eq!(
        port.lock().unwrap().transmitted,
        resumed_port.lock().unwrap().transmitted
    );
}
