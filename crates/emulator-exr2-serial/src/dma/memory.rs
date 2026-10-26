//! EXR II channel 0 memory DMA at FFF02000; global configuration at +300.
//!
//! Supports uniform fill and linear copy, with physical IRQ 5B; active bit 0
//! reports transfer progress.
//! Alternate descriptor words are retained but inactive in supported modes.
//!
//! Delay max(2, bytes/32), atomic completion, fill-seed latching at START and
//! copy-source reads at completion are declared, unmeasured timing laws. Copy
//! reads use bounded scratch storage and memmove ordering for overlapping DRAM
//! aliases; RAM changes before completion are visible. Operation selection
//! depends only on guest registers, memory and wired request lines. Channel 1
//! and peripheral audio handshakes belong to their separately wired DMA model.
use fujimod_emulator_devices::exr2::irq_routes::{Irq, IrqRoutes};
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

pub const BASE: u64 = 0xfff02000;
pub const IRQ: u32 = 0x5b;
const MAX_BYTES: u32 = 0x08000000;
const SCRATCH_BYTES: usize = 0x8000;
const STATUS: usize = 0x24 / 4;
const CONTROL: usize = 0x28 / 4;
const CONFIG: usize = 0x2c / 4;

#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {}

pub struct Wiring {
    pub base: u64,
    pub routes: IrqRoutes,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
enum Operation {
    Fill([u8; 4]),
    Copy { source: u32 },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pending {
    due: u64,
    destination: u32,
    count: u32,
    operation: Operation,
    irq: Option<Irq>,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    registers: [u32; 14],
    global: u32,
    pending: Option<Pending>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    base: u64,
    state: State,
}

pub struct MemoryDma {
    base: u64,
    routes: IrqRoutes,
    state: State,
    scratch: Box<[u8]>,
    /// The completion status word, published for side-effect-free polling.
    status: Arc<AtomicU32>,
}
impl MemoryDma {
    pub fn new(_options: Options, wiring: Wiring) -> Result<Self, String> {
        if wiring.base & 0x3f != 0 || wiring.base.checked_add(0x304).is_none() {
            return Err("memory DMA requires an aligned channel register window".into());
        }
        Ok(Self {
            base: wiring.base,
            routes: wiring.routes,
            state: State::default(),
            scratch: vec![0; SCRATCH_BYTES].into_boxed_slice(),
            status: Arc::new(AtomicU32::new(0)),
        })
    }
    fn publish_status(&self) {
        self.status
            .store(self.state.registers[STATUS], Ordering::Release);
    }
    fn offset(&self, address: u64, size: u8) -> Option<usize> {
        let offset = address.checked_sub(self.base)?;
        (size == 4
            && matches!(
                offset,
                0x00 | 0x04 | 0x08 | 0x0c | 0x10 | 0x14 | 0x24 | 0x28 | 0x2c | 0x30 | 0x34 | 0x300
            ))
        .then_some(offset as usize)
    }
    fn span(address: u32, count: u32) -> Result<(), String> {
        if count == 0 || count > MAX_BYTES || u64::from(address) + u64::from(count) > 1u64 << 32 {
            Err("invalid memory DMA byte span".into())
        } else {
            Ok(())
        }
    }
    fn canonical(address: u32) -> u64 {
        // The EXR II uncached DRAM alias maps +40000000 onto the low bus.
        u64::from(match address {
            0x40000000..0x80000000 => address - 0x40000000,
            _ => address,
        })
    }
    fn start(&mut self, cx: &mut Cx<'_>, control: u32) -> Result<(), String> {
        if !matches!(control, 1 | 5) || self.state.pending.is_some() {
            return Err("unsupported memory DMA control or start while busy".into());
        }
        let r = &self.state.registers;
        let config = r[CONFIG];
        let source_width = (config >> 12) & 15;
        let destination_width = (config >> 16) & 15;
        let mode = config & !0xff000;
        if !matches!(mode, 0x1500400 | 0x500400 | 0x400400)
            || !(2..=5).contains(&source_width)
            || source_width != destination_width
        {
            return Err(format!("unsupported memory DMA mode {config:#x}"));
        }
        let application = mode != 0x1500400;
        if r[0x30 / 4] != 0
            || r[0x34 / 4] != 0
            || (!application && r[3..=5].iter().any(|v| *v != 0))
        {
            return Err("unsupported memory DMA strides or loader alternate descriptor".into());
        }
        let (source, destination, count) = (r[0], r[1], r[2]);
        Self::span(destination, count)?;
        Self::span(source, if mode == 0x400400 { count } else { 32 })?;
        let irq = if application {
            Some(
                self.routes
                    .latch(IRQ)
                    .map_err(|_| "application memory DMA IRQ bank missing or ambiguous")?,
            )
        } else {
            None
        };
        let operation = if mode == 0x400400 {
            Operation::Copy { source }
        } else {
            let mut pattern = [0; 32];
            cx.ram_read(u64::from(source), &mut pattern)?;
            if pattern.chunks_exact(4).any(|word| word != &pattern[..4]) {
                return Err("nonuniform source is outside memory DMA fill mode".into());
            }
            Operation::Fill(pattern[..4].try_into().unwrap())
        };
        let due = cx
            .now()
            .checked_add(2.max(u64::from(count) / 32))
            .ok_or("memory DMA deadline overflow")?;
        self.state.pending = Some(Pending {
            due,
            destination,
            count,
            operation,
            irq,
        });
        self.state.registers[CONTROL] = control;
        self.state.registers[STATUS] = 1;
        Ok(())
    }
    fn transfer(&mut self, cx: &mut Cx<'_>, pending: &Pending) -> Result<(), String> {
        let count = pending.count as usize;
        if let Some(irq) = pending.irq {
            self.routes
                .validate(irq)
                .map_err(|_| "memory DMA IRQ bank changed during transfer")?;
        }
        match pending.operation {
            Operation::Fill(pattern) => {
                for chunk in self.scratch.chunks_exact_mut(4) {
                    chunk.copy_from_slice(&pattern);
                }
                let mut cursor = 0;
                while cursor < count {
                    let size = (count - cursor).min(self.scratch.len());
                    cx.ram_write(
                        u64::from(pending.destination) + cursor as u64,
                        &self.scratch[..size],
                    )?;
                    cursor += size;
                }
            }
            Operation::Copy { source } => {
                let src = Self::canonical(source);
                let dst = Self::canonical(pending.destination);
                let backward = src < dst && dst < src + u64::from(pending.count);
                let mut remaining = count;
                while remaining != 0 {
                    let size = remaining.min(self.scratch.len());
                    let cursor = if backward {
                        remaining - size
                    } else {
                        count - remaining
                    };
                    cx.ram_read(u64::from(source) + cursor as u64, &mut self.scratch[..size])?;
                    cx.ram_write(
                        u64::from(pending.destination) + cursor as u64,
                        &self.scratch[..size],
                    )?;
                    remaining -= size;
                }
            }
        }
        Ok(())
    }
    fn valid_state(state: &State) -> bool {
        if state.global != 0
            || state.registers[6..9].iter().any(|v| *v != 0)
            || !matches!(state.registers[CONTROL], 0 | 1 | 5 | 8)
        {
            return false;
        }
        match &state.pending {
            None => matches!(state.registers[STATUS], 0 | 0x40),
            Some(p) => {
                state.registers[STATUS] == 1
                    && p.due >= 2
                    && matches!(state.registers[CONTROL], 1 | 5)
                    && Self::span(p.destination, p.count).is_ok()
                    && match p.operation {
                        Operation::Copy { source } => {
                            Self::span(source, p.count).is_ok() && p.irq.is_some()
                        }
                        Operation::Fill(_) => true,
                    }
                    && p.irq.is_none_or(|irq| irq.core < 2 && irq.line == IRQ)
            }
        }
    }
}
impl Device for MemoryDma {
    fn checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        fujimod_emulator_devices::checkpoint::Checkpoint::encode(
            self.name(),
            &Saved {
                base: self.base,
                state: self.state.clone(),
            },
        )
    }
    fn validate_checkpoint(
        &self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<(), String> {
        let saved: Saved = saved.decode(self.name())?;
        if saved.base != self.base || !Self::valid_state(&saved.state) {
            return Err("invalid memory DMA checkpoint or wiring".into());
        }
        Ok(())
    }
    fn restore_checkpoint(
        &mut self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        let saved: Saved = saved.decode(self.name())?;
        self.state = saved.state;
        self.publish_status();
        Ok(())
    }
    fn fingerprint(&self) -> Result<Value, String> {
        let mut state = self.save();
        state["routes"] = self.routes.fingerprint();
        Ok(state)
    }
    fn name(&self) -> &'static str {
        "exr2-memory-dma"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        self.advance(cx);
        let Some(offset) = self.offset(address, size) else {
            cx.unmodeled();
            return 0;
        };
        u64::from(if offset == 0x300 {
            self.state.global
        } else {
            self.state.registers[offset / 4]
        })
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        self.advance(cx);
        if cx.refusal().is_some() {
            return;
        }
        let Some(offset) = self.offset(address, size) else {
            cx.unmodeled();
            return;
        };
        if value > u64::from(u32::MAX) {
            cx.fail("memory DMA value exceeds 32 bits");
            return;
        }
        let value = value as u32;
        match offset {
            0x24 => cx.fail("memory DMA completion status is read-only"),
            0x300 if value != 0 => cx.fail("unsupported global memory DMA configuration"),
            0x300 => self.state.global = value,
            0x28 if value == 8 => {
                self.state.pending = None;
                self.state.registers[CONTROL] = value;
                self.state.registers[STATUS] = 0;
            }
            0x28 => {
                if let Err(e) = self.start(cx, value) {
                    cx.fail(e);
                }
            }
            _ => self.state.registers[offset / 4] = value,
        }
        self.publish_status();
    }
    fn fast_reads(
        &self,
        window_base: u64,
        window_size: u64,
    ) -> Vec<fujimod_emulator_devices::bus::FastReadWord> {
        let status = self.base + 0x24;
        if status >= window_base && status + 4 <= window_base.saturating_add(window_size) {
            vec![fujimod_emulator_devices::bus::FastReadWord::atomic_u32(
                status - window_base,
                self.status.clone(),
            )]
        } else {
            Vec::new()
        }
    }
    fn pollable(&self, address: u64, size: u8) -> bool {
        self.offset(address, size).is_some()
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        let Some(pending) = self
            .state
            .pending
            .as_ref()
            .filter(|p| p.due <= cx.now())
            .cloned()
        else {
            return;
        };
        if let Err(e) = self.transfer(cx, &pending) {
            cx.fail(e);
            return;
        }
        self.state.pending = None;
        self.state.registers[STATUS] = 0x40;
        self.publish_status();
        if let Some(irq) = pending.irq {
            if let Err(e) = irq.raise(cx) {
                cx.fail(e);
            }
        }
    }
    fn next_deadline(&self, _: u64) -> Option<u64> {
        self.state.pending.as_ref().map(|p| p.due)
    }
    fn save(&self) -> Value {
        serde_json::json!({ "base": self.base, "state": self.state })
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let Saved { base, state } =
            serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        if base != self.base {
            return Err("memory DMA checkpoint has different wiring".into());
        }
        if !Self::valid_state(&state) {
            return Err("invalid memory DMA checkpoint".into());
        }
        self.state = state;
        self.publish_status();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::{Host, Record};

    struct Memory {
        now: u64,
        bytes: Vec<u8>,
        irqs: Vec<(usize, u32)>,
        irq_bytes: Vec<u8>,
    }
    impl Default for Memory {
        fn default() -> Self {
            Self {
                now: 0,
                bytes: vec![0; 0x40000],
                irqs: vec![],
                irq_bytes: vec![],
            }
        }
    }
    impl Memory {
        fn range(&self, address: u64, count: usize) -> Result<std::ops::Range<usize>, String> {
            let address = u32::try_from(address).map_err(|_| "DMA address exceeds bus")?;
            let start = MemoryDma::canonical(address) as usize;
            let end = start.checked_add(count).ok_or("DMA span overflow")?;
            if end > self.bytes.len() {
                return Err("DMA span is not RAM".into());
            }
            Ok(start..end)
        }
    }
    impl Host for Memory {
        fn now(&self) -> u64 {
            self.now
        }
        fn schedule(&mut self, _: usize, _: u64, _: u64) {
            panic!("deadline API expected");
        }
        fn set_irq(&mut self, _: u32, _: bool) {
            panic!("per-core IRQ expected");
        }
        fn raise_irq(&mut self, cpu: usize, line: u32) -> Result<(), String> {
            self.irqs.push((cpu, line));
            self.irq_bytes = self.bytes.clone();
            Ok(())
        }
        fn ram_read(&self, address: u64, out: &mut [u8]) -> Result<(), String> {
            out.copy_from_slice(&self.bytes[self.range(address, out.len())?]);
            Ok(())
        }
        fn ram_write(&mut self, address: u64, bytes: &[u8]) -> Result<(), String> {
            let range = self.range(address, bytes.len())?;
            self.bytes[range].copy_from_slice(bytes);
            Ok(())
        }
        fn record(&mut self, _: Record) {
            panic!("no allocation on access path");
        }
    }
    fn model() -> MemoryDma {
        MemoryDma::new(
            Options {},
            Wiring {
                base: BASE,
                routes: IrqRoutes::default(),
            },
        )
        .unwrap()
    }
    fn write(d: &mut MemoryDma, h: &mut Memory, offset: u64, value: u32) -> Result<(), String> {
        let mut cx = Cx::new(h, 1, 0xdeadbeef, 0);
        d.write(&mut cx, BASE + offset, 4, u64::from(value));
        cx.take_refusal().map_or(Ok(()), Err)
    }
    fn advance(d: &mut MemoryDma, h: &mut Memory) -> Result<(), String> {
        let mut cx = Cx::new(h, 0, 0, 0);
        d.advance(&mut cx);
        cx.take_refusal().map_or(Ok(()), Err)
    }
    fn configure(
        d: &mut MemoryDma,
        h: &mut Memory,
        mode: u32,
        source: u32,
        destination: u32,
        count: u32,
    ) {
        for (offset, value) in [
            (0, source),
            (4, destination),
            (8, count),
            (0x2c, mode | 0x55000),
        ] {
            write(d, h, offset, value).unwrap();
        }
    }
    #[test]
    fn aligned_register_words_read_back_and_status_and_unknown_accesses_refuse() {
        let mut d = model();
        let mut h = Memory::default();
        for offset in [0x00, 0x04, 0x08, 0x0c, 0x10, 0x14, 0x2c, 0x30, 0x34] {
            let value = offset as u32 + 0x1234;
            write(&mut d, &mut h, offset, value).unwrap();
            let mut cx = Cx::new(&mut h, 0, 0x1234, 0);
            assert_eq!(d.read(&mut cx, BASE + offset, 4), u64::from(value));
            assert_eq!(cx.refusal(), None);
            assert!(d.pollable(BASE + offset, 4));
        }
        write(&mut d, &mut h, 0x28, 8).unwrap();
        write(&mut d, &mut h, 0x300, 0).unwrap();
        for offset in [0x24, 0x300] {
            assert_eq!(d.read(&mut Cx::new(&mut h, 0, 0, 0), BASE + offset, 4), 0);
        }
        let saved = d.save();
        for (offset, size) in [(0, 2), (1, 4), (0x18, 4), (0x40, 4), (0x200, 4)] {
            let mut cx = Cx::new(&mut h, 0, 0, 0);
            d.read(&mut cx, BASE + offset, size);
            assert!(cx.is_unmodeled());
            assert!(!d.pollable(BASE + offset, size));
            assert_eq!(d.save(), saved);
        }
        let mut cx = Cx::new(&mut h, 0, 0, 0);
        d.write(&mut cx, BASE, 4, 1u64 << 32);
        assert!(cx.refusal().is_some());
        assert_eq!(d.save(), saved);
    }
    #[test]
    fn completion_status_is_published_for_polling_without_device_reads() {
        let mut d = model();
        let mut h = Memory::default();
        assert!(d.fast_reads(BASE + 0x40, 0x40).is_empty());
        let reads = d.fast_reads(BASE, 0x40);
        assert_eq!(reads.len(), 1);
        assert_eq!(reads[0].offset, 0x24);
        let fujimod_emulator_devices::bus::FastReadValue::AtomicU32(word) = &reads[0].value else {
            panic!("memory DMA status is an atomic fast read");
        };
        let status = || word.load(Ordering::Acquire);
        assert_eq!(status(), 0);
        h.now = 100;
        configure(&mut d, &mut h, 0x1500400, 0x40001000, 0x2000, 81);
        write(&mut d, &mut h, 0x28, 5).unwrap();
        assert_eq!(status(), 1);
        h.now = 102;
        advance(&mut d, &mut h).unwrap();
        assert_eq!(status(), 0x40);
        let saved = d.checkpoint().unwrap();
        write(&mut d, &mut h, 0x28, 8).unwrap();
        assert_eq!(status(), 0);
        d.restore_checkpoint(&saved).unwrap();
        assert_eq!(status(), 0x40);
    }
    #[test]
    fn fill_latches_seed_then_commits_at_exact_deadline_independent_of_early_advances() {
        fn run(noisy: bool) -> (Value, Vec<u8>) {
            let mut d = model();
            let mut h = Memory::default();
            h.now = 100;
            h.bytes[0x1000..0x1020].fill(0xa5);
            configure(&mut d, &mut h, 0x1500400, 0x40001000, 0x2000, 81);
            write(&mut d, &mut h, 0x28, 5).unwrap();
            assert_eq!(d.next_deadline(0), Some(102));
            assert_eq!(d.state.registers[STATUS], 1);
            let saved = d.save();
            h.bytes[0x1000..0x1020].fill(0x77);
            if noisy {
                for at in [100, 101, 101] {
                    h.now = at;
                    advance(&mut d, &mut h).unwrap();
                    assert_eq!(d.save(), saved);
                }
            }
            assert_eq!(&h.bytes[0x2000..0x2051], &[0; 81]);
            h.now = 102;
            advance(&mut d, &mut h).unwrap();
            assert_eq!(&h.bytes[0x2000..0x2051], &[0xa5; 81]);
            assert_eq!(d.state.registers[STATUS], 0x40);
            assert_eq!(d.next_deadline(102), None);
            assert!(h.irqs.is_empty());
            (d.save(), h.bytes)
        }
        assert_eq!(run(false), run(true));
    }
    #[test]
    fn copy_reads_at_completion_and_publishes_data_before_irq() {
        let mut d = model();
        let mut h = Memory::default();
        d.routes.set(IRQ, Some(1)).unwrap();
        h.bytes[0x1000..0x1040].fill(0x10);
        configure(&mut d, &mut h, 0x400400, 0x1000, 0x2000, 64);
        // Application alternate descriptors are retained but inactive here.
        write(&mut d, &mut h, 0x0c, 0x1234).unwrap();
        write(&mut d, &mut h, 0x28, 1).unwrap();
        h.bytes[0x1000..0x1040].fill(0xb7);
        h.now = 2;
        advance(&mut d, &mut h).unwrap();
        assert_eq!(&h.bytes[0x2000..0x2040], &[0xb7; 64]);
        assert_eq!(&h.irq_bytes[0x2000..0x2040], &[0xb7; 64]);
        assert_eq!(h.irqs, [(1, IRQ)]);
        advance(&mut d, &mut h).unwrap();
        assert_eq!(h.irqs.len(), 1);
    }
    #[test]
    fn application_fill_uses_its_captured_irq_and_abort_cancels_pending_bytes() {
        let mut d = model();
        let mut h = Memory::default();
        d.routes.set(IRQ, Some(0)).unwrap();
        h.bytes[0x1000..0x1020].fill(0x91);
        configure(&mut d, &mut h, 0x500400, 0x1000, 0x2000, 64);
        write(&mut d, &mut h, 0x28, 5).unwrap();
        write(&mut d, &mut h, 0x28, 8).unwrap();
        h.now = 20;
        advance(&mut d, &mut h).unwrap();
        assert_eq!(&h.bytes[0x2000..0x2040], &[0; 64]);
        assert_eq!(d.state.registers[STATUS], 0);
        assert!(h.irqs.is_empty());
        assert_eq!(d.next_deadline(20), None);
        write(&mut d, &mut h, 0x28, 1).unwrap();
        h.now = 22;
        advance(&mut d, &mut h).unwrap();
        assert_eq!(&h.bytes[0x2000..0x2040], &[0x91; 64]);
        assert_eq!(h.irqs, [(0, IRQ)]);
    }
    #[test]
    fn overlapping_copy_handles_both_directions_and_the_uncached_alias() {
        for (source, destination) in [
            (0x1000, 0x1010),
            (0x1010, 0x1000),
            (0x40001000, 0x1010),
            (0x1000, 0x40001010),
        ] {
            let mut d = model();
            let mut h = Memory::default();
            d.routes.set(IRQ, Some(0)).unwrap();
            let count = SCRATCH_BYTES + 37;
            for (i, byte) in h.bytes.iter_mut().enumerate() {
                *byte = i.wrapping_mul(29) as u8;
            }
            let mut expected = h.bytes.clone();
            expected.copy_within(
                MemoryDma::canonical(source) as usize
                    ..MemoryDma::canonical(source) as usize + count,
                MemoryDma::canonical(destination) as usize,
            );
            configure(&mut d, &mut h, 0x400400, source, destination, count as u32);
            write(&mut d, &mut h, 0x28, 5).unwrap();
            h.now = u64::from(count as u32) / 32;
            advance(&mut d, &mut h).unwrap();
            assert_eq!(h.bytes, expected);
        }
    }
    #[test]
    fn unsupported_configuration_refuses_without_starting_work() {
        let mut d = model();
        let mut h = Memory::default();
        configure(&mut d, &mut h, 0x400400, 0x1000, 0x2000, 64);
        assert!(write(&mut d, &mut h, 0x28, 1).unwrap_err().contains("IRQ"));
        d.routes.set(IRQ, Some(0)).unwrap();
        for (offset, bad, fixed) in [
            (8, 0, 64),
            (8, MAX_BYTES + 1, 64),
            (0x30, 1, 0),
            (0x34, 1, 0),
            (0x2c, 0x10455400, 0x455400),
            (0x2c, 0x451400, 0x455400),
            (0x2c, 0x445400, 0x455400),
            (4, 0xfffffff0, 0x2000),
        ] {
            write(&mut d, &mut h, offset, bad).unwrap();
            assert!(write(&mut d, &mut h, 0x28, 1).is_err());
            assert_eq!(d.next_deadline(0), None);
            write(&mut d, &mut h, offset, fixed).unwrap();
        }
        assert!(
            write(&mut d, &mut h, 0x24, 0)
                .unwrap_err()
                .contains("read-only")
        );
        assert!(write(&mut d, &mut h, 0x300, 1).is_err());
        assert!(write(&mut d, &mut h, 0x28, 0).is_err());
        assert!(write(&mut d, &mut h, 0x40, 0).is_err());
        write(&mut d, &mut h, 0x300, 0).unwrap();
        write(&mut d, &mut h, 0x28, 1).unwrap();
        assert!(write(&mut d, &mut h, 0x28, 5).unwrap_err().contains("busy"));
    }
    #[test]
    fn fill_refuses_nonuniform_seed_and_loader_alternate_descriptors() {
        let mut d = model();
        let mut h = Memory::default();
        configure(&mut d, &mut h, 0x1500400, 0x1000, 0x2000, 64);
        h.bytes[0x1001] = 2;
        assert!(
            write(&mut d, &mut h, 0x28, 5)
                .unwrap_err()
                .contains("nonuniform")
        );
        h.bytes[0x1001] = 0;
        write(&mut d, &mut h, 0x10, 0x1234).unwrap();
        assert!(
            write(&mut d, &mut h, 0x28, 5)
                .unwrap_err()
                .contains("alternate")
        );
    }
    #[test]
    fn masking_the_selected_bank_preserves_completion_and_data_before_irq() {
        let mut d = model();
        let mut h = Memory::default();
        d.routes.set(IRQ, Some(0)).unwrap();
        h.bytes[0x1000..0x1040].fill(0x71);
        configure(&mut d, &mut h, 0x400400, 0x1000, 0x2000, 64);
        write(&mut d, &mut h, 0x28, 1).unwrap();
        d.routes.set(IRQ, None).unwrap();
        h.now = 2;
        advance(&mut d, &mut h).unwrap();
        assert_eq!(&h.bytes[0x2000..0x2040], &[0x71; 64]);
        assert_eq!(&h.irq_bytes[0x2000..0x2040], &[0x71; 64]);
        assert_eq!(d.state.registers[STATUS], 0x40);
        assert_eq!(h.irqs, [(0, IRQ)]);
        advance(&mut d, &mut h).unwrap();
        assert_eq!(h.irqs.len(), 1);
    }
    #[test]
    fn ambiguous_enables_still_refuse_a_latched_memory_completion() {
        use fujimod_emulator_devices::exr2::irq::EnableOutput;
        use fujimod_emulator_devices::exr2::irq_routes::RouteOutput;
        let mut d = model();
        let mut h = Memory::default();
        d.routes.set(IRQ, Some(0)).unwrap();
        configure(&mut d, &mut h, 0x400400, 0x1000, 0x2000, 64);
        write(&mut d, &mut h, 0x28, 1).unwrap();
        let peer = RouteOutput::new(d.routes.clone(), 1).unwrap();
        let mut enabled = [0; 4];
        enabled[IRQ as usize / 32] = 1 << (IRQ % 32);
        peer.publish(enabled);
        h.now = 2;
        assert!(advance(&mut d, &mut h).is_err());
        assert_eq!(&h.bytes[0x2000..0x2040], &[0; 64]);
        assert!(h.irqs.is_empty());
    }
    #[test]
    fn changed_irq_owner_or_unmapped_ram_refuses_before_completion_or_irq() {
        for reroute in [false, true] {
            let mut d = model();
            let mut h = Memory::default();
            d.routes.set(IRQ, Some(0)).unwrap();
            configure(
                &mut d,
                &mut h,
                0x400400,
                if reroute { 0x1000 } else { 0x100000 },
                0x2000,
                64,
            );
            write(&mut d, &mut h, 0x28, 1).unwrap();
            if reroute {
                d.routes.set(IRQ, Some(1)).unwrap();
            }
            h.now = 2;
            assert!(advance(&mut d, &mut h).is_err());
            assert_eq!(d.state.registers[STATUS], 1);
            assert!(h.irqs.is_empty());
            assert_eq!(&h.bytes[0x2000..0x2040], &[0; 64]);
        }
    }
    #[test]
    fn corrupt_checkpoints_refuse_atomically() {
        let mut d = model();
        let mut h = Memory::default();
        d.routes.set(IRQ, Some(0)).unwrap();
        configure(&mut d, &mut h, 0x400400, 0x1000, 0x2000, 64);
        write(&mut d, &mut h, 0x28, 1).unwrap();
        let saved = d.save();
        let corruptions: &[(&[&str], Value)] = &[
            (&["state", "global"], 1.into()),
            (&["state", "pending", "due"], 0.into()),
            (&["state", "pending", "count"], 0.into()),
            (&["state", "pending", "count"], (MAX_BYTES + 1).into()),
            (&["state", "pending", "destination"], 0xfffffff0u32.into()),
            (
                &["state", "pending", "operation", "Copy", "source"],
                0xfffffff0u32.into(),
            ),
            (&["state", "pending", "irq", "core"], 2.into()),
            (&["state", "pending", "irq", "line"], (IRQ + 1).into()),
            (&["state", "pending", "irq"], Value::Null),
            (&["base"], (BASE + 0x40).into()),
            (&["unknown"], true.into()),
        ];
        for (path, value) in corruptions {
            let mut corrupt = saved.clone();
            let mut target = &mut corrupt;
            for key in *path {
                target = &mut target[*key];
            }
            *target = value.clone();
            assert!(d.restore(&corrupt).is_err(), "accepted {path:?}");
            assert_eq!(d.save(), saved);
        }
        for (index, value) in [(6, 1), (CONTROL, 2), (STATUS, 0)] {
            let mut corrupt = saved.clone();
            corrupt["state"]["registers"][index] = value.into();
            assert!(d.restore(&corrupt).is_err());
            assert_eq!(d.save(), saved);
        }
    }
    #[test]
    fn pending_checkpoint_is_observational_and_restores_exact_deadline() {
        let mut d = model();
        let mut restored = model();
        let mut h = Memory::default();
        configure(&mut d, &mut h, 0x1500400, 0x1000, 0x2000, 64);
        write(&mut d, &mut h, 0x28, 5).unwrap();
        let saved = d.save();
        assert_eq!(saved, d.save());
        let checkpoint = d.checkpoint().unwrap();
        restored.validate_checkpoint(&checkpoint).unwrap();
        restored.restore_checkpoint(&checkpoint).unwrap();
        assert_eq!(d.fingerprint().unwrap(), restored.fingerprint().unwrap());
        let mut bad: Saved = checkpoint.decode(d.name()).unwrap();
        bad.state.pending.as_mut().unwrap().count = 0;
        let bad = fujimod_emulator_devices::checkpoint::Checkpoint::encode(d.name(), &bad).unwrap();
        assert!(restored.validate_checkpoint(&bad).is_err());
        assert!(restored.restore_checkpoint(&bad).is_err());
        assert_eq!(restored.next_deadline(100), Some(2));
        assert_eq!(restored.save(), saved);
        let mut invalid = saved.clone();
        invalid["state"]["registers"][STATUS] = 0.into();
        assert!(restored.restore(&invalid).is_err());
        assert_eq!(restored.save(), saved);
        h.now = 2;
        advance(&mut d, &mut h).unwrap();
        advance(&mut restored, &mut h).unwrap();
        assert_eq!(restored.save(), d.save());
        assert!(serde_json::from_str::<Options>(r#"{"unknown":true}"#).is_err());
        assert!(
            MemoryDma::new(
                Options {},
                Wiring {
                    base: BASE + 4,
                    routes: IrqRoutes::default()
                }
            )
            .is_err()
        );
    }
}
