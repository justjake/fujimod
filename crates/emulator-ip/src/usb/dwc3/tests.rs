use super::*;
use fujimod_emulator_devices::{Host, Record};

#[derive(Clone)]
struct Board {
    now: u64,
    pc: u64,
    ram: Vec<u8>,
    irq: bool,
}
impl Host for Board {
    fn now(&self) -> u64 {
        self.now
    }
    fn schedule(&mut self, _: usize, _: u64, _: u64) {}
    fn set_irq(&mut self, line: u32, high: bool) {
        assert_eq!(line, 246);
        self.irq = high;
    }
    fn raise_irq(&mut self, _: usize, _: u32) -> Result<(), String> {
        Err("not wired".into())
    }
    fn ram_read(&self, address: u64, out: &mut [u8]) -> Result<(), String> {
        let start = usize::try_from(address).map_err(|_| "DMA address")?;
        let end = start.checked_add(out.len()).ok_or("DMA overflow")?;
        out.copy_from_slice(self.ram.get(start..end).ok_or("DMA outside RAM")?);
        Ok(())
    }
    fn ram_write(&mut self, address: u64, bytes: &[u8]) -> Result<(), String> {
        let start = usize::try_from(address).map_err(|_| "DMA address")?;
        let end = start.checked_add(bytes.len()).ok_or("DMA overflow")?;
        self.ram
            .get_mut(start..end)
            .ok_or("DMA outside RAM")?
            .copy_from_slice(bytes);
        Ok(())
    }
    fn record(&mut self, _: Record) {}
}
fn config() -> Config {
    Config {
        base: 0xffb00000,
        irq: 246,
        command_ticks: 8,
        transaction_ticks: 4,
        speed: 0,
    }
}
fn board(pc: u64) -> Board {
    Board {
        now: 0,
        pc,
        ram: vec![0; 0x5000],
        irq: false,
    }
}
fn write(d: &mut Dwc3, b: &mut Board, offset: u32, value: u32) -> Result<(), String> {
    let pc = b.pc;
    let mut cx = Cx::new(b, 0, pc, 0);
    d.write(
        &mut cx,
        config().base + u64::from(offset),
        4,
        u64::from(value),
    );
    cx.take_refusal().map_or(Ok(()), Err)
}
fn tick(d: &mut Dwc3, b: &mut Board, at: u64) {
    b.now = at;
    let pc = b.pc;
    let mut cx = Cx::new(b, 0, pc, 0);
    d.event(&mut cx, 0);
    assert_eq!(cx.refusal(), None);
}
fn command(d: &mut Dwc3, b: &mut Board, ep: usize, code: u32, parameters: [u32; 3]) {
    let base = 0xc800 + ep as u32 * 16;
    for (n, value) in parameters.into_iter().enumerate() {
        write(d, b, base + n as u32 * 4, value).unwrap();
    }
    write(d, b, base + 12, ACTIVE | code).unwrap();
    let at = b.now + 8;
    assert_eq!(d.next_deadline(b.now), Some(at));
    tick(d, b, at - 1);
    assert_ne!(d.reg(base + 12) & ACTIVE, 0);
    tick(d, b, at);
    assert_eq!(d.reg(base + 12) & ACTIVE, 0);
}
fn endpoint(d: &mut Dwc3, b: &mut Board, ep: usize, maximum: u32) {
    command(
        d,
        b,
        ep,
        1,
        [0, 0x500, maximum << 3 | if ep < 2 { 0 } else { 2 << 1 }],
    );
    command(d, b, ep, 2, [0, 0, 1]);
    write(d, b, 0xc720, d.reg(0xc720) | 1 << ep).unwrap();
}
fn trb(b: &mut Board, address: usize, buffer: u32, size: u32, kind: u32) {
    for (n, word) in [buffer, 0, size, 0x803 | kind << 4].into_iter().enumerate() {
        b.ram[address + n * 4..address + n * 4 + 4].copy_from_slice(&word.to_le_bytes());
    }
}
fn start(d: &mut Dwc3, b: &mut Board, ep: usize, address: u32) {
    command(d, b, ep, 6, [0, address, 0]);
}
fn ready(pc: u64) -> (Dwc3, Board) {
    let mut d = Dwc3::new(config()).unwrap();
    let mut b = board(pc);
    write(&mut d, &mut b, 0xc400, 0x1000).unwrap();
    write(&mut d, &mut b, 0xc408, 0x100).unwrap();
    write(&mut d, &mut b, 0xc708, 7).unwrap();
    write(&mut d, &mut b, DCTL, RUN).unwrap();
    tick(&mut d, &mut b, 8);
    d.link(8, true).unwrap();
    tick(&mut d, &mut b, 12);
    assert!(b.irq);
    assert_eq!(
        u32::from_le_bytes(b.ram[0x1000..0x1004].try_into().unwrap()),
        0x201
    );
    write(&mut d, &mut b, 0xc40c, 4).unwrap();
    assert!(!b.irq);
    d.link(b.now, true).unwrap();
    let at = b.now + 4;
    tick(&mut d, &mut b, at);
    assert_eq!(d.reg(0xc40c), 0);
    assert!(!b.irq);
    (d, b)
}

#[test]
fn real_dma_packets_deadlines_irqs_and_checkpoint_are_program_independent() {
    fn run(pc: u64, late: u64) -> (serde_json::Value, Vec<u8>) {
        let (mut d, mut b) = ready(pc);
        endpoint(&mut d, &mut b, 2, 8);
        trb(&mut b, 0x2000, 0x3000, 12, 1);
        start(&mut d, &mut b, 2, 0x2000);
        let first = d
            .transaction(b.now, 2, false, Some(b"abcdefgh".to_vec()))
            .unwrap();
        let second = d
            .transaction(b.now, 2, false, Some(b"ijkl".to_vec()))
            .unwrap();
        let deadline = b.now + 4;
        let saved = d.save();
        let ram = b.ram.clone();
        tick(&mut d, &mut b, deadline - 1);
        assert_eq!(d.save(), saved);
        assert_eq!(b.ram, ram);
        assert_eq!(d.reply(first), None);
        let mut restored = Dwc3::new(config()).unwrap();
        restored.restore(&saved).unwrap();
        assert_eq!(restored.save(), saved);
        tick(&mut restored, &mut b, deadline + late);
        assert_eq!(restored.next_deadline(b.now), None);
        assert_eq!(restored.reply(first), Some(&Reply::Ack));
        assert_eq!(restored.reply(second), Some(&Reply::Ack));
        let observed = restored.save();
        assert_eq!(restored.reply(first), Some(&Reply::Ack));
        assert_eq!(restored.save(), observed);
        assert_eq!(&b.ram[0x3000..0x300c], b"abcdefghijkl");
        assert_eq!(&b.ram[0x2000..0x2004], &0x3000u32.to_le_bytes());
        assert_eq!(&b.ram[0x2008..0x200c], &[0; 4]);
        assert_eq!(b.ram[0x200c] & 1, 0);
        assert!(b.irq);
        write(&mut restored, &mut b, 0xc40c, 4).unwrap();
        assert!(!b.irq);
        restored.consume_reply(first).unwrap();
        restored.consume_reply(second).unwrap();
        (restored.save(), b.ram)
    }
    assert_eq!(run(0x11111110, 0), run(0x99999998, 17));
}

#[test]
fn control_setup_data_and_status_use_wire_bytes_and_guest_trbs() {
    let (mut d, mut b) = ready(0x1234);
    endpoint(&mut d, &mut b, 0, 64);
    endpoint(&mut d, &mut b, 1, 64);
    trb(&mut b, 0x2000, 0x3000, 8, 2);
    start(&mut d, &mut b, 0, 0x2000);
    command(&mut d, &mut b, 0, 4, [0; 3]);
    command(&mut d, &mut b, 1, 4, [0; 3]);
    let setup = d
        .transaction(b.now, 0, true, Some(vec![0x80, 6, 0, 1, 0, 0, 5, 0]))
        .unwrap();
    let at = b.now + 4;
    tick(&mut d, &mut b, at);
    assert_eq!(d.reply(setup), Some(&Reply::Ack));
    assert!(!d.state.endpoints[0].stalled);
    assert!(!d.state.endpoints[1].stalled);
    assert_eq!(d.state.control.remaining, 5);
    assert!(d.state.control.data_in);
    assert!(!d.state.control.status);
    let not_ready = d.transaction(b.now, 1, false, None).unwrap();
    tick(&mut d, &mut b, at + 4);
    assert_eq!(d.reply(not_ready), Some(&Reply::Nak));
    assert_eq!(
        u32::from_le_bytes(b.ram[0x1008..0x100c].try_into().unwrap()) >> 12 & 3,
        1
    );
    b.ram[0x3100..0x3105].copy_from_slice(b"hello");
    trb(&mut b, 0x2020, 0x3100, 5, 5);
    start(&mut d, &mut b, 1, 0x2020);
    let data = d.transaction(b.now, 1, false, None).unwrap();
    let at = b.now + 4;
    tick(&mut d, &mut b, at);
    assert_eq!(d.reply(data), Some(&Reply::Data(b"hello".to_vec())));
    assert!(d.state.control.status);
    let not_ready = d.transaction(b.now, 0, false, Some(vec![])).unwrap();
    tick(&mut d, &mut b, at + 4);
    assert_eq!(d.reply(not_ready), Some(&Reply::Nak));
    assert_eq!(
        u32::from_le_bytes(b.ram[0x1010..0x1014].try_into().unwrap()) >> 12 & 3,
        2
    );
    trb(&mut b, 0x2040, 0x3200, 0, 4);
    start(&mut d, &mut b, 0, 0x2040);
    let status = d.transaction(b.now, 0, false, Some(vec![])).unwrap();
    let at = b.now + 4;
    tick(&mut d, &mut b, at);
    assert_eq!(d.reply(status), Some(&Reply::Ack));
    assert!(!d.state.control.status);
    assert_eq!(d.state.control.remaining, 0);
}

#[test]
fn invalid_saved_transactions_refuse_without_changing_state() {
    let (mut d, mut b) = ready(0x5554);
    endpoint(&mut d, &mut b, 2, 64);
    d.transaction(b.now, 2, false, Some(vec![1])).unwrap();
    let before = d.save();
    for (pointer, bad) in [
        (
            "/state/pending/0/operation/Transaction/ep",
            serde_json::json!(32),
        ),
        (
            "/state/pending/0/operation/Transaction/id",
            serde_json::json!(0),
        ),
        ("/state/pending/0/order", serde_json::json!(0)),
        ("/state/ring_cursor", serde_json::json!(0x100)),
        ("/state/endpoints/2/cursor", serde_json::json!(1)),
        ("/config/irq", serde_json::json!(245)),
    ] {
        let mut state = before.clone();
        *state.pointer_mut(pointer).unwrap() = bad;
        assert!(d.restore(&state).is_err(), "{pointer}");
        assert_eq!(d.save(), before);
    }
    let mut extra = before.clone();
    extra["state"]["registers"]["4294967295"] = serde_json::json!(1);
    assert!(d.restore(&extra).is_err());
    assert_eq!(d.save(), before);
    let pc = b.pc;
    let mut cx = Cx::new(&mut b, 0, pc, 0);
    d.read(&mut cx, config().base + 0xc718, 4);
    assert!(cx.is_unmodeled());
    assert_eq!(d.save(), before);
}

#[test]
fn reset_and_generic_commands_complete_only_at_their_deadlines() {
    let mut d = Dwc3::new(config()).unwrap();
    let mut b = board(0x1110);
    write(&mut d, &mut b, DCTL, RESET).unwrap();
    let saved = d.save();
    tick(&mut d, &mut b, 7);
    assert_eq!(d.save(), saved);
    let mut restored = Dwc3::new(config()).unwrap();
    restored.restore(&saved).unwrap();
    tick(&mut restored, &mut b, 8);
    assert_eq!(restored.reg(DCTL), 0);
    assert!(!b.irq);
    write(&mut restored, &mut b, 0xc710, 0x7d).unwrap();
    write(&mut restored, &mut b, 0xc714, 0x402).unwrap();
    tick(&mut restored, &mut b, 15);
    assert_eq!(restored.reg(0xc714), 0x402);
    tick(&mut restored, &mut b, 16);
    assert_eq!(restored.reg(0xc714), 2);
    assert_eq!(restored.state.periodic_parameters, 0x7d);
    assert!(write(&mut restored, &mut b, 0xc714, 0x403).is_err());
}

#[test]
fn in_transactions_preserve_owned_trbs_until_completion_and_honor_halt() {
    let (mut d, mut b) = ready(0x9988);
    endpoint(&mut d, &mut b, 3, 8);
    b.ram[0x3000..0x300b].copy_from_slice(b"abcdefghijk");
    trb(&mut b, 0x2000, 0x3000, 11, 1);
    start(&mut d, &mut b, 3, 0x2000);
    let first = d.transaction(b.now, 3, false, None).unwrap();
    let at = b.now + 4;
    tick(&mut d, &mut b, at);
    assert_eq!(d.reply(first), Some(&Reply::Data(b"abcdefgh".to_vec())));
    assert_eq!(&b.ram[0x2008..0x200c], &11u32.to_le_bytes());
    assert_ne!(b.ram[0x200c] & 1, 0);
    assert!(!b.irq);
    let second = d.transaction(b.now, 3, false, None).unwrap();
    tick(&mut d, &mut b, at + 4);
    assert_eq!(d.reply(second), Some(&Reply::Data(b"ijk".to_vec())));
    assert_eq!(&b.ram[0x2008..0x200c], &[0; 4]);
    assert_eq!(b.ram[0x200c] & 1, 0);
    assert!(b.irq);
    write(&mut d, &mut b, 0xc40c, 4).unwrap();
    command(&mut d, &mut b, 3, 4, [0; 3]);
    let halted = d.transaction(b.now, 3, false, None).unwrap();
    let at = b.now + 4;
    tick(&mut d, &mut b, at);
    assert_eq!(d.reply(halted), Some(&Reply::Stall));
    assert!(!b.irq);
    command(&mut d, &mut b, 3, 5, [0; 3]);
    assert!(!d.state.endpoints[3].stalled);
}

#[test]
fn event_ring_wraps_and_irq_remains_high_until_all_bytes_are_acknowledged() {
    let (mut d, mut b) = ready(0x1350);
    write(&mut d, &mut b, 0xc408, 8).unwrap();
    d.link(b.now, false).unwrap();
    d.link(b.now, true).unwrap();
    let at = b.now + 4;
    tick(&mut d, &mut b, at);
    assert_eq!(d.reg(0xc40c), 8);
    assert_eq!(d.state.ring_cursor, 0);
    assert!(b.irq);
    write(&mut d, &mut b, 0xc40c, 4).unwrap();
    assert!(b.irq);
    d.bus_reset(b.now).unwrap();
    tick(&mut d, &mut b, at + 4);
    assert_eq!(d.reg(0xc40c), 8);
    assert_eq!(d.state.ring_cursor, 4);
    assert_eq!(&b.ram[0x1000..0x1004], &0x101u32.to_le_bytes());
    write(&mut d, &mut b, 0xc40c, 8).unwrap();
    assert!(!b.irq);
    let before = d.save();
    assert!(write(&mut d, &mut b, 0xc40c, 4).is_err());
    assert_eq!(d.save(), before);
}
