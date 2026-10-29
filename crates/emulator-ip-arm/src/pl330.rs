//! ARM PL330 DMA controller (ARM DDI 0424), bounded to programs started
//! through the debug instruction interface.
//!
//! Peripheral transfers use the same loads and stores: a peripheral's
//! request is always asserted, so DMAWFP and DMAFLUSHP complete at once and
//! DMALDP/DMASTP transfer like DMALD/DMAST. A peripheral data register is a
//! [`Source`] window.
//!
//! Eight channel threads run programs from physical RAM. A program runs whole
//! when its transfer time elapses; the time is an explicit setup cost plus a
//! byte rate. Event `n` drives interrupt line `irq_routes[n]` when its
//! INTEN bit is set. Loads may read declared read-only memories as well as
//! RAM; stores into them refuse.
use fujimod_emulator_devices::{Cx, Device, Record, Value as Field};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::sync::Arc;

/// A physical window that DMA loads read without CPU RAM.
pub trait Source: Send + Sync {
    /// Whether `[address, address + len)` lies inside this window.
    fn contains(&self, address: u64, len: usize) -> bool;
    /// Bytes at `address`.
    fn read(&self, address: u64, len: usize) -> Result<Vec<u8>, String>;
    /// Whether stores into this window reach [`Source::write`]; otherwise a
    /// program storing into it refuses before it starts.
    fn writable(&self) -> bool {
        false
    }
    /// Stores `bytes` at `address` when the transfer completes.
    fn write(&self, _address: u64, _bytes: &[u8]) -> Result<(), String> {
        Err("PL330 store into a read-only source".into())
    }
}

/// Transfer time: `setup_ticks + ceil(bytes / bytes_per_tick)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Throughput {
    pub setup_ticks: u64,
    pub bytes_per_tick: u64,
}

const CHANNELS: usize = 8;
const STEP_LIMIT: usize = 1 << 24;
const REGISTERS: u64 = 0x1000;

#[derive(Clone, Default, Serialize, Deserialize)]
struct Thread {
    pc: u32,
    sar: u32,
    dar: u32,
    ccr: u32,
    lc: [u32; 2],
    running: bool,
    due: Option<u64>,
}

/// Outcome of one program run: bytes stored, first load and store address.
struct Transfer {
    bytes: u64,
    source: u32,
    destination: u32,
}

pub struct Pl330 {
    base: u64,
    irq_routes: Vec<u32>,
    sources: Vec<Arc<dyn Source>>,
    timing: Throughput,
    threads: [Thread; CHANNELS],
    inten: u32,
    events: u32,
    debug: [u32; 2],
}

impl Pl330 {
    pub fn new(
        base: u64,
        irq_routes: Vec<u32>,
        sources: Vec<Arc<dyn Source>>,
        timing: Throughput,
    ) -> Result<Self, String> {
        if base & (REGISTERS - 1) != 0
            || irq_routes.is_empty()
            || irq_routes.len() > 32
            || irq_routes.iter().any(|irq| !(32..1020).contains(irq))
            || timing.setup_ticks == 0
            || timing.bytes_per_tick == 0
        {
            return Err("Invalid PL330 wiring or timing".into());
        }
        Ok(Self {
            base,
            irq_routes,
            sources,
            timing,
            threads: Default::default(),
            inten: 0,
            events: 0,
            debug: [0; 2],
        })
    }

    fn signal(&mut self, cx: &mut Cx<'_>, event: u32) -> Result<(), String> {
        if self.inten >> event & 1 == 0 {
            return Ok(());
        }
        let line = *self
            .irq_routes
            .get(event as usize)
            .ok_or("PL330 event has no interrupt route")?;
        self.events |= 1 << event;
        cx.set_irq(line, true);
        Ok(())
    }

    fn load(&self, cx: &Cx<'_>, address: u64, len: usize) -> Result<Vec<u8>, String> {
        match self.sources.iter().find(|s| s.contains(address, len)) {
            Some(source) => source.read(address, len),
            None => cx.ram_bytes(address, len),
        }
    }

    /// Runs channel `n` to DMAEND. Without `apply`, no memory, event or
    /// channel state changes; loaded bytes are not read.
    fn run(&mut self, cx: &mut Cx<'_>, n: usize, apply: bool) -> Result<Transfer, String> {
        let mut t = self.threads[n].clone();
        let mut fifo = VecDeque::new();
        let mut bytes = 0;
        let mut first: [Option<u32>; 2] = [None; 2];
        let fetch = |cx: &Cx<'_>, pc: u32, len: usize| cx.ram_bytes(u64::from(pc), len);
        for _ in 0..STEP_LIMIT {
            let pc = t.pc;
            let op = fetch(cx, pc, 1)?[0];
            let mut len = 1;
            match op {
                // DMAEND
                0x00 => {
                    t.running = false;
                    t.due = None;
                    if apply {
                        self.threads[n] = t;
                    }
                    return Ok(Transfer {
                        bytes,
                        source: first[0].unwrap_or(0),
                        destination: first[1].unwrap_or(0),
                    });
                }
                // DMALD, DMALDP<S|B>
                0x04 | 0x25 | 0x27 => {
                    if op != 0x04 {
                        len = 2;
                    }
                    let count = ((1u32 << (t.ccr >> 1 & 7)) * ((t.ccr >> 4 & 15) + 1)) as usize;
                    first[0].get_or_insert(t.sar);
                    if apply {
                        fifo.extend(self.load(cx, u64::from(t.sar), count)?);
                    } else {
                        fifo.extend(std::iter::repeat_n(0, count));
                    }
                    if t.ccr & 1 != 0 {
                        t.sar = t.sar.wrapping_add(count as u32);
                    }
                }
                // DMAST, DMASTP<S|B>
                0x08 | 0x29 | 0x2b => {
                    if op != 0x08 {
                        len = 2;
                    }
                    let count = ((1u32 << (t.ccr >> 15 & 7)) * ((t.ccr >> 18 & 15) + 1)) as usize;
                    if fifo.len() < count {
                        return Err("PL330 store exceeds loaded data".into());
                    }
                    let data: Vec<u8> = fifo.drain(..count).collect();
                    first[1].get_or_insert(t.dar);
                    let at = u64::from(t.dar);
                    match self.sources.iter().find(|s| s.contains(at, count)) {
                        Some(source) if !source.writable() => {
                            return Err("PL330 store into a read-only source".into());
                        }
                        Some(source) if apply => source.write(at, &data)?,
                        None if apply => cx.ram_write(at, &data)?,
                        _ => {}
                    }
                    bytes += count as u64;
                    if t.ccr >> 14 & 1 != 0 {
                        t.dar = t.dar.wrapping_add(count as u32);
                    }
                }
                // DMARMB, DMAWMB, DMANOP
                0x12 | 0x13 | 0x18 => {}
                // DMAWFP<S|P|B>, DMAFLUSHP
                0x30..=0x32 | 0x35 => len = 2,
                // DMALP lc0/lc1
                0x20 | 0x22 => {
                    len = 2;
                    t.lc[usize::from(op >> 1 & 1)] = u32::from(fetch(cx, pc + 1, 1)?[0]);
                }
                // DMALPEND lc0/lc1
                0x38 | 0x3c => {
                    let lc = usize::from(op >> 2 & 1);
                    if t.lc[lc] != 0 {
                        t.lc[lc] -= 1;
                        t.pc = pc.wrapping_sub(u32::from(fetch(cx, pc + 1, 1)?[0]));
                        continue;
                    }
                    len = 2;
                }
                // DMASEV
                0x34 => {
                    len = 2;
                    let event = u32::from(fetch(cx, pc + 1, 1)?[0] >> 3);
                    if apply {
                        self.signal(cx, event)?;
                    }
                }
                // DMAADDH SAR/DAR
                0x54 | 0x56 => {
                    len = 3;
                    let b = fetch(cx, pc + 1, 2)?;
                    let add = u32::from(u16::from_le_bytes([b[0], b[1]]));
                    if op & 2 == 0 {
                        t.sar = t.sar.wrapping_add(add);
                    } else {
                        t.dar = t.dar.wrapping_add(add);
                    }
                }
                // DMAMOV SAR/CCR/DAR
                0xbc => {
                    len = 6;
                    let b = fetch(cx, pc + 1, 5)?;
                    let v = u32::from_le_bytes([b[1], b[2], b[3], b[4]]);
                    match b[0] {
                        0 => t.sar = v,
                        1 if v >> 28 & 7 == 0 => t.ccr = v,
                        2 => t.dar = v,
                        _ => return Err("Unmodeled PL330 DMAMOV".into()),
                    }
                }
                _ => return Err(format!("Unmodeled PL330 instruction {op:#x}")),
            }
            t.pc = t.pc.wrapping_add(len);
        }
        Err("PL330 program exceeds step bound".into())
    }

    fn debug(&mut self, cx: &mut Cx<'_>) -> Result<(), String> {
        let [first, second] = self.debug;
        let (op, arg) = (first >> 16 & 0xff, first >> 24);
        match (first & 1, op) {
            // DMAGO from the manager thread.
            (0, 0xa0 | 0xa2) => {
                let n = (arg & 7) as usize;
                if self.threads[n].running {
                    return Err("PL330 DMAGO on a running channel".into());
                }
                self.threads[n].pc = second;
                let transfer = self.run(cx, n, false)?;
                let at = cx.now()
                    + self.timing.setup_ticks
                    + transfer.bytes.div_ceil(self.timing.bytes_per_tick);
                self.threads[n].running = true;
                self.threads[n].due = Some(at);
                cx.schedule(at, n as u64);
            }
            // DMAKILL on a channel thread.
            (1, 0x01) => {
                let t = &mut self.threads[(first >> 8 & 7) as usize];
                t.running = false;
                t.due = None;
            }
            // DMASEV from the manager thread.
            (0, 0x34) => self.signal(cx, arg >> 3)?,
            _ => return Err(format!("Unmodeled PL330 debug instruction {first:#x}")),
        }
        Ok(())
    }

    fn access(
        &mut self,
        cx: &mut Cx<'_>,
        o: u64,
        write: bool,
        value: u32,
    ) -> Result<Option<u32>, String> {
        let n = (o as usize >> 3) & 7;
        let c = (o as usize >> 5) & 7;
        Ok(Some(match (o, write) {
            // DSR, DBGSTATUS: manager and debug interface idle.
            (0x000 | 0xd00, false) => 0,
            (0x020, false) => self.inten,
            (0x020, true) => {
                self.inten = value;
                0
            }
            (0x024, false) => self.events,
            (0x028, false) => self.events & self.inten,
            (0x02c, true) => {
                for e in (0..32).filter(|e| (value & self.events) >> e & 1 != 0) {
                    if let Some(&line) = self.irq_routes.get(e as usize) {
                        cx.set_irq(line, false);
                    }
                }
                self.events &= !value;
                0
            }
            // FSRD, FSRC, FTRD: no faults.
            (0x030 | 0x034 | 0x038, false) => 0,
            // CSRn executing/stopped, CPCn.
            (0x100..0x140, false) if o & 7 == 0 => u32::from(self.threads[n].running),
            (0x100..0x140, false) => self.threads[n].pc,
            (0x400..0x500, false) => match o & 0x1f {
                0 => self.threads[c].sar,
                4 => self.threads[c].dar,
                8 => self.threads[c].ccr,
                0xc => self.threads[c].lc[0],
                0x10 => self.threads[c].lc[1],
                _ => return Ok(None),
            },
            (0xd04, true) if value == 0 => {
                self.debug(cx)?;
                0
            }
            (0xd08 | 0xd0c, true) => {
                self.debug[(o as usize - 0xd08) / 4] = value;
                0
            }
            _ => return Ok(None),
        }))
    }
}

impl Device for Pl330 {
    fn name(&self) -> &'static str {
        "pl330"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        if size != 4 || address & 3 != 0 {
            cx.unmodeled();
            return 0;
        }
        match self.access(cx, address - self.base, false, 0) {
            Ok(Some(v)) => u64::from(v),
            Ok(None) => {
                cx.unmodeled();
                0
            }
            Err(e) => {
                cx.fail(e);
                0
            }
        }
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        if size != 4 || address & 3 != 0 {
            cx.unmodeled();
            return;
        }
        match self.access(cx, address - self.base, true, value as u32) {
            Ok(Some(_)) => {}
            Ok(None) => cx.unmodeled(),
            Err(e) => cx.fail(e),
        }
    }
    fn pollable(&self, address: u64, _size: u8) -> bool {
        matches!(address - self.base, 0 | 0x24 | 0x28 | 0x100..0x140 | 0xd00)
    }
    fn event(&mut self, cx: &mut Cx<'_>, token: u64) {
        let n = token as usize;
        if n >= CHANNELS || !self.threads[n].due.is_some_and(|at| at <= cx.now()) {
            return;
        }
        match self.run(cx, n, true) {
            Ok(transfer) => cx.record(Record::new(
                "PL330 transfer complete",
                [
                    ("base", Field::int(self.base)),
                    ("channel", Field::int(n as u64)),
                    ("source", Field::int(u64::from(transfer.source))),
                    ("destination", Field::int(u64::from(transfer.destination))),
                    ("size", Field::int(transfer.bytes)),
                    ("tick", Field::int(cx.now())),
                ],
            )),
            Err(e) => cx.fail(e),
        }
    }
    fn save(&self) -> Value {
        json!({"threads": self.threads, "interrupt_enable": self.inten,
               "events": self.events, "debug": self.debug,
               "irq_routes": self.irq_routes, "timing": self.timing})
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let field = |k: &str| value[k].clone();
        let e = |e: serde_json::Error| e.to_string();
        if field("irq_routes") != json!(self.irq_routes) || field("timing") != json!(self.timing) {
            return Err("PL330 checkpoint wiring or timing differs".into());
        }
        self.threads = serde_json::from_value(field("threads")).map_err(e)?;
        self.inten = serde_json::from_value(field("interrupt_enable")).map_err(e)?;
        self.events = serde_json::from_value(field("events")).map_err(e)?;
        self.debug = serde_json::from_value(field("debug")).map_err(e)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::Host;

    const BASE: u64 = 0xfff18000;
    const ROM: u64 = 0xf0000000;

    struct Rom(Vec<u8>);
    impl Source for Rom {
        fn contains(&self, address: u64, len: usize) -> bool {
            address >= ROM && address + len as u64 <= ROM + self.0.len() as u64
        }
        fn read(&self, address: u64, len: usize) -> Result<Vec<u8>, String> {
            let at = (address - ROM) as usize;
            Ok(self.0[at..at + len].to_vec())
        }
    }

    #[derive(Default)]
    struct TestHost {
        now: u64,
        ram: Vec<u8>,
        irqs: Vec<(u32, bool)>,
        events: Vec<(u64, u64)>,
        records: Vec<Record>,
    }
    impl Host for TestHost {
        fn now(&self) -> u64 {
            self.now
        }
        fn schedule(&mut self, _: usize, at: u64, token: u64) {
            self.events.push((at, token));
        }
        fn set_irq(&mut self, line: u32, level: bool) {
            self.irqs.push((line, level));
        }
        fn raise_irq(&mut self, _: usize, _: u32) -> Result<(), String> {
            Err("unused".into())
        }
        fn ram_read(&self, a: u64, out: &mut [u8]) -> Result<(), String> {
            let a = a as usize;
            out.copy_from_slice(self.ram.get(a..a + out.len()).ok_or("unmapped read")?);
            Ok(())
        }
        fn ram_write(&mut self, a: u64, b: &[u8]) -> Result<(), String> {
            let a = a as usize;
            self.ram
                .get_mut(a..a + b.len())
                .ok_or("unmapped write")?
                .copy_from_slice(b);
            Ok(())
        }
        fn record(&mut self, record: Record) {
            self.records.push(record);
        }
    }
    fn write(d: &mut Pl330, h: &mut TestHost, offset: u64, value: u32) {
        let mut cx = Cx::new(h, 0, 0, 0);
        d.write(&mut cx, BASE + offset, 4, u64::from(value));
        assert_eq!(cx.refusal(), None, "{offset:#x}={value:#x}");
    }

    #[test]
    fn debug_dmago_copies_a_source_window_and_raises_the_event_route() {
        let image: Vec<u8> = (0..64).collect();
        let routes = (207..215).collect();
        let timing = Throughput {
            setup_ticks: 16,
            bytes_per_tick: 16,
        };
        let mut dma = Pl330::new(BASE, routes, vec![Arc::new(Rom(image.clone()))], timing).unwrap();
        let mut h = TestHost {
            now: 100,
            ram: vec![0; 0x2000],
            ..TestHost::default()
        };
        // DMAMOV SAR/DAR/CCR with 16-byte bursts, both addresses incrementing;
        // DMALP 4 { DMALD; DMAST }; DMAWMB; DMASEV 4; DMAEND.
        let mut program = vec![0xbc, 0];
        program.extend((ROM as u32).to_le_bytes());
        program.extend([0xbc, 2, 0, 0x10, 0, 0, 0xbc, 1]);
        program.extend(0x5c017u32.to_le_bytes());
        program.extend([0x20, 3, 0x04, 0x08, 0x38, 2, 0x13, 0x34, 4 << 3, 0]);
        h.ram[0x100..0x100 + program.len()].copy_from_slice(&program);
        for (o, v) in [
            (0x20, 1 << 4),
            (0xd08, 4 << 24 | 0xa20000),
            (0xd0c, 0x100),
            (0xd04, 0),
        ] {
            write(&mut dma, &mut h, o, v);
        }
        assert_eq!(h.events, vec![(120, 4)]);
        assert_eq!(h.ram[0x1000..0x1040], [0; 64]);
        h.now = 120;
        {
            let mut cx = Cx::new(&mut h, 0, 0, 0);
            dma.event(&mut cx, 4);
            assert_eq!(cx.refusal(), None);
        }
        assert_eq!(h.ram[0x1000..0x1040], image[..]);
        assert_eq!(h.irqs, vec![(211, true)]);
        assert_eq!(h.records.len(), 1);
        let mut cx = Cx::new(&mut h, 0, 0, 0);
        assert_eq!(dma.read(&mut cx, BASE + 0x120, 4), 0);
        drop(cx);
        let saved = dma.save();
        write(&mut dma, &mut h, 0x2c, 1 << 4);
        assert_eq!(h.irqs, vec![(211, true), (211, false)]);
        let mut resumed = Pl330::new(BASE, (207..215).collect(), vec![], timing).unwrap();
        resumed.restore(&saved).unwrap();
        assert_eq!(resumed.save(), saved);
        let mut rewired = Pl330::new(BASE, (208..216).collect(), vec![], timing).unwrap();
        assert!(rewired.restore(&saved).is_err());
    }

    #[test]
    fn stores_into_a_source_window_refuse() {
        let timing = Throughput {
            setup_ticks: 1,
            bytes_per_tick: 1,
        };
        let mut dma =
            Pl330::new(BASE, vec![207], vec![Arc::new(Rom(vec![0; 64]))], timing).unwrap();
        let mut h = TestHost {
            ram: vec![0; 0x2000],
            ..TestHost::default()
        };
        let mut program = vec![0xbc, 2];
        program.extend((ROM as u32).to_le_bytes());
        program.extend([0xbc, 1]);
        program.extend(0x5c017u32.to_le_bytes());
        program.extend([0x04, 0x08, 0]);
        h.ram[0x100..0x100 + program.len()].copy_from_slice(&program);
        write(&mut dma, &mut h, 0xd08, 0xa20000);
        write(&mut dma, &mut h, 0xd0c, 0x100);
        let mut cx = Cx::new(&mut h, 0, 0, 0);
        dma.write(&mut cx, BASE + 0xd04, 4, 0);
        assert!(cx.refusal().is_some());
    }

    struct Sink(std::sync::Mutex<Vec<(u64, Vec<u8>)>>);
    impl Source for Sink {
        fn contains(&self, address: u64, len: usize) -> bool {
            address >= ROM && address + len as u64 <= ROM + 0x100
        }
        fn read(&self, _: u64, _: usize) -> Result<Vec<u8>, String> {
            Err("unused".into())
        }
        fn writable(&self) -> bool {
            true
        }
        fn write(&self, address: u64, bytes: &[u8]) -> Result<(), String> {
            self.0.lock().unwrap().push((address, bytes.to_vec()));
            Ok(())
        }
    }

    #[test]
    fn stores_into_a_writable_window_arrive_when_the_transfer_completes() {
        let timing = Throughput {
            setup_ticks: 1,
            bytes_per_tick: 1,
        };
        let sink = Arc::new(Sink(Default::default()));
        let mut dma = Pl330::new(BASE, vec![207], vec![sink.clone()], timing).unwrap();
        let mut h = TestHost {
            ram: vec![0; 0x2000],
            ..TestHost::default()
        };
        h.ram[0x1000..0x1010].copy_from_slice(&[7; 16]);
        // DMAMOV SAR RAM, DAR window, CCR 16-byte bursts; DMALD; DMAST; DMAEND.
        let mut program = vec![0xbc, 0];
        program.extend(0x1000u32.to_le_bytes());
        program.extend([0xbc, 2]);
        program.extend((ROM as u32 + 0x20).to_le_bytes());
        program.extend([0xbc, 1]);
        program.extend(0x5c017u32.to_le_bytes());
        program.extend([0x04, 0x08, 0]);
        h.ram[0x100..0x100 + program.len()].copy_from_slice(&program);
        write(&mut dma, &mut h, 0xd08, 0xa20000);
        write(&mut dma, &mut h, 0xd0c, 0x100);
        write(&mut dma, &mut h, 0xd04, 0);
        assert!(sink.0.lock().unwrap().is_empty());
        h.now = 17;
        let mut cx = Cx::new(&mut h, 0, 0, 0);
        dma.event(&mut cx, 0);
        assert_eq!(cx.refusal(), None);
        assert_eq!(*sink.0.lock().unwrap(), [(ROM + 0x20, vec![7; 16])]);
    }

    #[test]
    fn peripheral_handshakes_complete_at_once() {
        let timing = Throughput {
            setup_ticks: 1,
            bytes_per_tick: 1,
        };
        let sink = Arc::new(Sink(Default::default()));
        let mut dma = Pl330::new(BASE, vec![207, 208], vec![sink.clone()], timing).unwrap();
        let mut h = TestHost {
            ram: vec![0; 0x2000],
            ..TestHost::default()
        };
        h.ram[0x1000..0x1004].copy_from_slice(b"lens");
        // The X-T20 lens transmit program: byte transfers into a fixed
        // register; DMALP 4 { DMALD; DMAWFP 5; DMASTPS 5 }; DMAFLUSHP 5;
        // DMAWMB; DMASEV 1; DMAEND.
        let mut program = vec![0xbc, 0];
        program.extend(0x1000u32.to_le_bytes());
        program.extend([0xbc, 2]);
        program.extend((ROM as u32 + 0x60).to_le_bytes());
        program.extend([0xbc, 1]);
        program.extend(0x0080_0201u32.to_le_bytes());
        program.extend([0x20, 3, 0x04, 0x30, 5 << 3, 0x29, 5 << 3, 0x38, 5]);
        program.extend([0x35, 5 << 3, 0x13, 0x34, 1 << 3, 0]);
        h.ram[0x100..0x100 + program.len()].copy_from_slice(&program);
        write(&mut dma, &mut h, 0x20, 2);
        write(&mut dma, &mut h, 0xd08, 0xa20000);
        write(&mut dma, &mut h, 0xd0c, 0x100);
        write(&mut dma, &mut h, 0xd04, 0);
        h.now = 5;
        let mut cx = Cx::new(&mut h, 0, 0, 0);
        dma.event(&mut cx, 0);
        assert_eq!(cx.refusal(), None);
        let stores = sink.0.lock().unwrap().clone();
        assert_eq!(stores.len(), 4);
        assert!(stores.iter().all(|(at, _)| *at == ROM + 0x60));
        let bytes: Vec<u8> = stores.into_iter().flat_map(|(_, b)| b).collect();
        assert_eq!(bytes, b"lens");
        assert_eq!(h.irqs, vec![(208, true)]);
    }
}
