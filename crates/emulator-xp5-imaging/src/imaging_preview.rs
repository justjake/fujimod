//! XP5 preview input-buffer and RIFP processor maps.
use fujimod_emulator_sensor_input::{input_buffer::{InputBuffer, Output, Wiring}, statistics::{Planes, Statistics}};
use fujimod_emulator_devices::checkpoint::Checkpoint;
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

#[derive(Default, Serialize, Deserialize)]
struct State {
    regs: BTreeMap<u64, u32>,
    generation: u64,
    armed: bool,
    frames: u64,
    deadline: Option<u64>,
}
impl State {
    fn r(&self, o: u64) -> u32 {
        self.regs.get(&o).copied().unwrap_or(0)
    }
}
/// The RIFP I/O DMA at 0xffd01000 (channel 1 of the stock driver 0x2c720f8 / 0x2c6e478,
/// channel table 0x32fa998, ops table 0x32fac38). The X-T5 4.31 still path only uses it in
/// register-list mode (+0x4c bit 0 = 1): the recf register queue (0x13c0e54 / 0x13c0e94,
/// entries of `value: u32, address: u32`) is read from DRAM and each entry becomes one
/// 32-bit MMIO write, which is how the queued RIFP, RCIF/RCIZ and RESZ settings and the
/// RIFP start (0xffd02004 = 1) reach the hardware.
///
/// * +0x00 run/stop handshake: 0x2c72da4 writes 0 and polls bit 0 until it reads 1.
/// * +0x40 bit 0 START (ops +0x50, 0x2c6e97c), +0x44 bit 0 interrupt enable (ops +0x60,
///   0x2c6e9e0), +0x48 bit 0 done status, write-one-to-clear (ISR 0x2c72d28 -> 0x2c6ea44).
/// * Source (queue): +0x50 address bits 31..0, +0x54 bit 31 enable and bits 2..0 address
///   bits 34..32, +0x58 pitch 8, +0x60 line bytes 8, +0x64 lines = entry count.
/// * Destination: +0x6c = 0, +0x70 bit 31, +0x74 pitch 4, +0x7c line bytes 4, +0x80 = count.
/// * Done raises INTID 230 (logical 0x15, 0x18ea918) while +0x44 bit 0 is set.
///
/// The engine is opt-in (`with_queue_completion_ticks`); without it, and for every other
/// aligned word below +0xdc, the window stores its last write. The RIFP model reads the
/// destination words +0x6c..+0x80.
#[derive(Clone, Default)]
pub struct Io(Arc<Mutex<State>>, Option<u64>);

const QUEUE_IRQ: u32 = 230;

impl Io {
    /// Enable the register-list engine: ticks between a START and its done status. The
    /// queued writes themselves are performed at the START.
    pub fn with_queue_completion_ticks(mut self, ticks: u64) -> Self {
        self.1 = Some(ticks);
        self
    }
    fn irq(state: &State, cx: &mut Cx<'_>) {
        cx.set_irq(QUEUE_IRQ, state.r(0x48) & state.r(0x44) & 1 != 0);
    }
    /// Read the queue and post its writes; returns false after a refusal.
    fn start(&self, state: &mut State, cx: &mut Cx<'_>) -> bool {
        if state.r(0x4c) & 1 != 1 {
            cx.unsupported("rifp-io: only the register-list mode (+0x4c bit 0 = 1) is modeled");
            return false;
        }
        let source = state.r(0x54);
        let count = state.r(0x64) & 0xffff;
        if source & 0x8000_0000 == 0
            || state.r(0x60) & 0x1ffff != 8
            || state.r(0x7c) & 0x1ffff != 4
            || state.r(0x80) & 0xffff != count
        {
            cx.unsupported("rifp-io: register-list geometry is not 8-byte entries to 4-byte writes");
            return false;
        }
        let address = u64::from(state.r(0x50)) | (u64::from(source & 7) << 32);
        let mut entries = vec![0; count as usize * 8];
        if let Err(e) = fujimod_emulator_devices::dma::bus_read(&[], cx, address, &mut entries) {
            cx.fail(format!("rifp-io register list at {address:#x}: {e}"));
            return false;
        }
        for entry in entries.chunks_exact(8) {
            let value = u32::from_le_bytes(entry[..4].try_into().unwrap());
            let target = u32::from_le_bytes(entry[4..].try_into().unwrap());
            cx.post_mmio_write(u64::from(target), 4, u64::from(value));
        }
        let Some(at) = cx.now().checked_add(self.1.unwrap_or(0)) else {
            cx.fail("rifp-io deadline overflow");
            return false;
        };
        state.armed = true;
        state.generation = state.generation.wrapping_add(1);
        state.deadline = Some(at);
        state.frames += 1;
        cx.schedule(at, state.generation);
        true
    }
}
pub enum Preview {
    Input(InputBuffer),
    Rifp(Rifp),
}
pub struct Rifp {
    state: State,
    io: Io,
    delay: u64,
    source_luma: u8,
    /// Bring-up option: unmodeled aligned words read back their last write (in `state.regs`).
    retain_words: bool,
}
impl Preview {
    pub fn new(rifp: bool, io: Io, options: &toml::Table) -> Result<Self, String> {
        let delay = options
            .get(if rifp {
                "completion_us"
            } else {
                "frame_period_us"
            })
            .and_then(toml::Value::as_integer)
            .and_then(|n| u64::try_from(n).ok())
            .filter(|n| *n > 0)
            .and_then(|n| n.checked_mul(1000))
            .ok_or("preview needs a positive bounded interval")?;
        if options.keys().any(|key| {
            !matches!(
                key.as_str(),
                "completion_us" | "frame_period_us" | "source_luma" | "retain_words"
            )
        }) {
            return Err("unknown preview option".into());
        }
        let source_luma = options
            .get("source_luma")
            .and_then(toml::Value::as_integer)
            .and_then(|n| u8::try_from(n).ok())
            .filter(|n| (16..=235).contains(n))
            .ok_or("preview requires a declared limited-range source_luma")?;
        let retain_words = match options.get("retain_words") {
            None => false,
            Some(value) => value.as_bool().ok_or("preview retain_words must be a boolean")?,
        };
        if rifp {
            return Ok(Self::Rifp(Rifp {
                state: State::default(),
                io,
                delay,
                source_luma,
                retain_words,
            }));
        }
        let input = InputBuffer::new(
            Wiring {
                base: 0xff974000,
                irq: 170,
                status: 0x20,
                irq_control: 0x500,
                ready: 0x504,
                max_offset: 0x508,
                write_limit: 0x500,
                enables: [0x7ff, 0x3f],
                causes: [0x7ff, 0x3f],
                outputs: vec![
                    Output {
                        enable_word: 0,
                        bit: 1 << 7,
                        low: 0x400,
                        high: 0x404,
                        high_mask: 7,
                        columns: 7,
                        rows: 7,
                        planes: Planes::Color,
                    },
                    Output {
                        enable_word: 0,
                        bit: 1 << 9,
                        low: 0x428,
                        high: 0x42c,
                        high_mask: 7,
                        columns: 16,
                        rows: 16,
                        planes: Planes::Ae,
                    },
                ],
                reset_ticks: 0,
                period_ticks: Some(delay),
            },
            None,
        )?;
        Ok(Self::Input(if retain_words {
            input.with_retained_window()
        } else {
            input
        }))
    }
    pub fn with_statistics(mut self, statistics: Statistics) -> Self {
        if let Self::Input(input) = &mut self {
            input.set_statistics(statistics);
        }
        self
    }
    fn device(&mut self) -> &mut dyn Device {
        match self {
            Self::Input(v) => v,
            Self::Rifp(v) => v,
        }
    }
}
impl Device for Preview {
    fn name(&self) -> &'static str {
        match self {
            Self::Input(_) => "imaging-input-buffer",
            Self::Rifp(_) => "imaging-rifp",
        }
    }
    fn read(&mut self, cx: &mut Cx<'_>, a: u64, n: u8) -> u64 {
        self.device().read(cx, a, n)
    }
    fn write(&mut self, cx: &mut Cx<'_>, a: u64, n: u8, v: u64) {
        self.device().write(cx, a, n, v);
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        self.device().advance(cx);
    }
    fn event(&mut self, cx: &mut Cx<'_>, token: u64) {
        self.device().event(cx, token);
    }
    fn next_deadline(&self, now: u64) -> Option<u64> {
        match self {
            Self::Input(v) => v.next_deadline(now),
            Self::Rifp(v) => v.next_deadline(now),
        }
    }
    fn checkpoint(&self) -> Result<Checkpoint, String> {
        match self {
            Self::Input(input) => input.checkpoint(),
            Self::Rifp(rifp) => rifp.checkpoint(),
        }
    }
    fn validate_checkpoint(&self, saved: &Checkpoint) -> Result<(), String> {
        match self {
            Self::Input(input) => input.validate_checkpoint(saved),
            Self::Rifp(rifp) => rifp.validate_checkpoint(saved),
        }
    }
    fn restore_checkpoint(&mut self, saved: &Checkpoint) -> Result<(), String> {
        self.device().restore_checkpoint(saved)
    }
    fn save(&self) -> Value {
        match self {
            Self::Input(v) => v.save(),
            Self::Rifp(v) => v.save(),
        }
    }
    fn restore(&mut self, v: &Value) -> Result<(), String> {
        self.device().restore(v)
    }
}
impl Rifp {
    fn irq(&self, cx: &mut Cx<'_>) {
        cx.set_irq(202, self.state.r(0x144) & self.state.r(0x148) & 1 != 0);
    }
}
impl Device for Rifp {
    fn name(&self) -> &'static str {
        "imaging-rifp"
    }
    fn read(&mut self, cx: &mut Cx<'_>, a: u64, n: u8) -> u64 {
        self.advance(cx);
        let o = a.wrapping_sub(0xffd02000);
        if n != 4 || o & 3 != 0 || o > 0x14c {
            if self.retain_words && n == 4 && o & 3 == 0 && o < 0x1000 {
                return u64::from(self.state.r(o));
            }
            cx.unmodeled();
            return 0;
        }
        u64::from(match o {
            0 => self.state.r(0) | 1,
            0x140 => {
                if self.state.armed {
                    0
                } else {
                    0x101010
                }
            }
            _ => self.state.r(o),
        })
    }
    fn write(&mut self, cx: &mut Cx<'_>, a: u64, n: u8, v: u64) {
        self.advance(cx);
        let o = a.wrapping_sub(0xffd02000);
        let v = v as u32;
        if n != 4 || o & 3 != 0 || o > 0x14c {
            if self.retain_words && n == 4 && o & 3 == 0 && o < 0x1000 {
                self.state.regs.insert(o, v);
            } else {
                cx.unmodeled();
            }
            return;
        }
        if o == 0 && v & 1 == 0 {
            self.state.armed = false;
            self.state.deadline = None;
            self.state.generation = self.state.generation.wrapping_add(1);
            self.state.regs.insert(0, 1);
            self.state.regs.insert(0x144, 0);
        } else if o == 0x14c {
            self.state
                .regs
                .insert(0x144, self.state.r(0x144) & !(v & 1));
        } else {
            self.state.regs.insert(o, v);
            if o == 4 && v & 1 != 0 {
                self.state.armed = true;
                self.state.generation = self.state.generation.wrapping_add(1);
                let Some(at) = cx.now().checked_add(self.delay) else {
                    cx.fail("preview deadline overflow");
                    return;
                };
                self.state.deadline = Some(at);
                cx.schedule(at, self.state.generation);
            }
        }
        self.irq(cx);
    }
    fn event(&mut self, cx: &mut Cx<'_>, token: u64) {
        if token == self.state.generation {
            self.advance(cx);
        }
    }
    fn next_deadline(&self, _: u64) -> Option<u64> {
        self.state.deadline
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        if !self.state.deadline.is_some_and(|at| at <= cx.now()) {
            return;
        }
        self.state.deadline = None;
        self.state.armed = false;
        let io = self.io.0.lock().unwrap();
        let address = u64::from(io.r(0x6c)) | (u64::from(io.r(0x70) & 7) << 32);
        let row = io.r(0x7c) & 0x1ffff;
        let rows = io.r(0x80) & 0xffff;
        if address != 0
            && io.r(0x70) & 0x80000000 != 0
            && row >= 4
            && row.is_multiple_of(4)
            && rows != 0
            && let Err(e) = fujimod_emulator_imaging::frame::paint(
                cx,
                address,
                row / 2,
                rows,
                row,
                self.source_luma,
            )
        {
            cx.fail(e);
            return;
        }
        self.state.regs.insert(4, 0);
        self.state.regs.insert(0x144, self.state.r(0x144) | 1);
        self.state.frames += 1;
        self.irq(cx);
    }
    fn checkpoint(&self) -> Result<Checkpoint, String> {
        Checkpoint::encode(self.name(), &self.state)
    }
    fn validate_checkpoint(&self, saved: &Checkpoint) -> Result<(), String> {
        let state: State = saved.decode(self.name())?;
        if state.armed != state.deadline.is_some() {
            return Err("RIFP active/deadline disagree".into());
        }
        Ok(())
    }
    fn restore_checkpoint(&mut self, saved: &Checkpoint) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        self.state = saved.decode(self.name())?;
        Ok(())
    }
    fn save(&self) -> Value {
        serde_json::json!({"state":self.state})
    }
    fn restore(&mut self, v: &Value) -> Result<(), String> {
        let s: State = serde_json::from_value(v["state"].clone())
            .map_err(|e| format!("RIFP checkpoint: {e}"))?;
        if s.armed != s.deadline.is_some() {
            return Err("RIFP active/deadline disagree".into());
        }
        self.state = s;
        Ok(())
    }
}

impl Device for Io {
    fn name(&self) -> &'static str {
        "imaging-rifp-io"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, n: u8) -> u64 {
        self.advance(cx);
        let o = address.wrapping_sub(0xffd01000);
        if n != 4 || o & 3 != 0 || o > 0xd8 {
            cx.unmodeled();
            return 0;
        }
        let state = self.0.lock().unwrap();
        u64::from(match o {
            // Stopped/ready acknowledge of the run/stop handshake.
            0 if self.1.is_some() => state.r(0) | 1,
            _ => state.r(o),
        })
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, n: u8, v: u64) {
        self.advance(cx);
        let o = address.wrapping_sub(0xffd01000);
        if n != 4 || o & 3 != 0 || o > 0xd8 {
            cx.unmodeled();
            return;
        }
        let v = v as u32;
        let mut state = self.0.lock().unwrap();
        if self.1.is_none() {
            state.regs.insert(o, v);
            return;
        }
        match o {
            0x48 => {
                let status = state.r(0x48) & !(v & 1);
                state.regs.insert(0x48, status);
            }
            0x40 if v & 1 != 0 && !state.armed => {
                state.regs.insert(0x40, v);
                if !self.start(&mut state, cx) {
                    return;
                }
            }
            _ => {
                state.regs.insert(o, v);
                if o == 0 && v & 1 == 0 && state.armed {
                    // Stop: a pending done status is dropped.
                    state.armed = false;
                    state.deadline = None;
                    state.generation = state.generation.wrapping_add(1);
                    state.regs.insert(0x40, 0);
                }
            }
        }
        if matches!(o, 0x00 | 0x40 | 0x44 | 0x48) {
            Self::irq(&state, cx);
        }
    }
    fn event(&mut self, cx: &mut Cx<'_>, token: u64) {
        if token == self.0.lock().unwrap().generation {
            self.advance(cx);
        }
    }
    fn next_deadline(&self, _: u64) -> Option<u64> {
        self.0.lock().unwrap().deadline
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        let mut state = self.0.lock().unwrap();
        if !state.deadline.is_some_and(|at| at <= cx.now()) {
            return;
        }
        state.deadline = None;
        state.armed = false;
        state.regs.insert(0x40, 0);
        let status = state.r(0x48) | 1;
        state.regs.insert(0x48, status);
        Self::irq(&state, cx);
    }
    fn checkpoint(&self) -> Result<Checkpoint, String> {
        Checkpoint::encode(self.name(), &*self.0.lock().unwrap())
    }
    fn validate_checkpoint(&self, saved: &Checkpoint) -> Result<(), String> {
        let state: State = saved.decode(self.name())?;
        if state.armed != state.deadline.is_some() {
            return Err("RIFP IO active/deadline disagree".into());
        }
        Ok(())
    }
    fn restore_checkpoint(&mut self, saved: &Checkpoint) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        *self.0.lock().unwrap() = saved.decode(self.name())?;
        Ok(())
    }
    fn save(&self) -> Value {
        serde_json::to_value(&*self.0.lock().unwrap()).unwrap()
    }
    fn restore(&mut self, v: &Value) -> Result<(), String> {
        let state: State =
            serde_json::from_value(v.clone()).map_err(|e| format!("RIFP IO checkpoint: {e}"))?;
        if state.armed != state.deadline.is_some() {
            return Err("RIFP IO active/deadline disagree".into());
        }
        *self.0.lock().unwrap() = state;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::test_host::recording::RecordingHost;
    #[test]
    fn ibuf_status_is_guest_w1c_and_statistics_use_programmed_dma() {
        let statistics = Statistics::new(
            super::super::STATISTICS_BASE,
            super::super::STATISTICS_BANK_SIZE,
        )
        .unwrap();
        let mut preview = Preview::new(
            false,
            Io::default(),
            &toml::from_str("frame_period_us=1000\nsource_luma=128").unwrap(),
        )
        .unwrap()
        .with_statistics(statistics.clone());
        let mut host = RecordingHost {
            ram_base: 0x1_0000_0000,
            ram: vec![0; 0x2000],
            ..Default::default()
        };
        {
            let mut cx = Cx::new(&mut host, 2, 0xbeef, 0);
            for (o, v) in [
                (0x400, 0x100),
                (0x404, 1),
                (0x500, 0x100),
                (4, (1 << 23) | (1 << 7)),
            ] {
                preview.write(&mut cx, 0xff974000 + o, 4, v);
            }
            assert!(cx.refusal().is_none());
        }
        host.now = 999_999;
        preview.advance(&mut Cx::new(&mut host, 0, 0, 0));
        assert!(host.ram.iter().all(|v| *v == 0));
        host.now = 1_000_000;
        preview.advance(&mut Cx::new(&mut host, 0, 0, 0));
        assert!(host.irqs.contains(&(170, true)));
        assert!(host.ram[0x100..0x110].iter().any(|v| *v != 0));
        let before = preview.save();
        let events = host.events.len();
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        assert_eq!(preview.read(&mut cx, 0xff974020, 4), 1 << 7);
        assert_eq!(preview.read(&mut cx, 0xff974020, 4), 1 << 7);
        assert_eq!(preview.save(), before);
        assert_eq!(events, host.events.len());
        preview.write(&mut Cx::new(&mut host, 0, 0, 0), 0xff974020, 4, 1 << 7);
        assert_eq!(host.irqs.last(), Some(&(170, false)));
        assert_eq!(statistics.save()["frames"], 1);
        assert_eq!(preview.next_deadline(host.now), Some(2_000_000));
    }
    #[test]
    fn rifp_fills_only_its_programmed_high_address_output_on_completion() {
        let mut io = Io::default();
        let mut rifp = Preview::new(
            true,
            io.clone(),
            &toml::from_str("completion_us=100\nsource_luma=128").unwrap(),
        )
        .unwrap();
        let mut host = RecordingHost {
            ram_base: 0x1_0000_0200,
            ram: vec![0x55; 64],
            ..Default::default()
        };
        {
            let mut cx = Cx::new(&mut host, 0, 0, 0);
            for (o, v) in [(0x6c, 0x200), (0x70, 0x80000001), (0x7c, 8), (0x80, 4)] {
                io.write(&mut cx, 0xffd01000 + o, 4, v);
            }
            rifp.write(&mut cx, 0xffd02148, 4, 1);
            rifp.write(&mut cx, 0xffd02004, 4, 1);
        }
        let saved = rifp.save();
        host.now = 99_999;
        rifp.advance(&mut Cx::new(&mut host, 0, 0, 0));
        assert_eq!(rifp.save(), saved);
        rifp.restore(&saved).unwrap();
        host.now = 100_000;
        rifp.advance(&mut Cx::new(&mut host, 0, 0, 0));
        assert_eq!(&host.ram[..32], &[128; 32]);
        assert_eq!(&host.ram[32..], &[0x55; 32]);
        assert_eq!(rifp.next_deadline(host.now), None);
        assert!(host.irqs.contains(&(202, true)));
        rifp.write(&mut Cx::new(&mut host, 0, 0, 0), 0xffd0214c, 4, 1);
        assert_eq!(host.irqs.last(), Some(&(202, false)));
    }
    /// Program the stock register-list descriptor (0x2c51f98 -> 0x2c720f8 -> 0x2c6e478)
    /// for `count` entries at `queue`.
    fn program_register_list(io: &mut Io, cx: &mut Cx<'_>, queue: u64, count: u32) {
        for (o, v) in [
            (0x4c, 1),
            (0x50, queue as u32),
            (0x54, 0x8000_0000 | (queue >> 32) as u32),
            (0x58, 8),
            (0x5c, 0),
            (0x60, 8),
            (0x64, count),
            (0x68, 0),
            (0x6c, 0),
            (0x70, 0x8000_0000),
            (0x74, 4),
            (0x78, 0),
            (0x7c, 4),
            (0x80, count),
            (0x84, 0),
        ] {
            io.write(cx, 0xffd01000 + o, 4, u64::from(v));
        }
    }
    #[test]
    fn register_list_start_posts_each_queued_write_then_raises_done() {
        let mut io = Io::default().with_queue_completion_ticks(5_000);
        let mut host = RecordingHost {
            ram_base: 0x1_2000_0000,
            ram: vec![0; 0x100],
            ..Default::default()
        };
        // Entries as 0x13c0e54 stores them: value at +0, address at +4.
        for (i, (value, address)) in [(1u32, 0xffd02148u32), (0x1234, 0xffd0b050), (1, 0xffd02004)]
            .into_iter()
            .enumerate()
        {
            host.ram[i * 8..i * 8 + 4].copy_from_slice(&value.to_le_bytes());
            host.ram[i * 8 + 4..i * 8 + 8].copy_from_slice(&address.to_le_bytes());
        }
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        // Run/stop handshake (0x2c72da4): write 0, bit 0 then reads 1.
        io.write(&mut cx, 0xffd01000, 4, 0);
        assert_eq!(io.read(&mut cx, 0xffd01000, 4) & 1, 1);
        program_register_list(&mut io, &mut cx, 0x1_2000_0000, 3);
        io.write(&mut cx, 0xffd01044, 4, 1);
        assert!(cx.take_posted_writes().is_empty());
        io.write(&mut cx, 0xffd01040, 4, 1);
        assert!(cx.refusal().is_none());
        let posted: Vec<_> = cx
            .take_posted_writes()
            .into_iter()
            .map(|w| (w.address, w.size, w.value))
            .collect();
        assert_eq!(
            posted,
            [(0xffd02148, 4, 1), (0xffd0b050, 4, 0x1234), (0xffd02004, 4, 1)]
        );
        drop(cx);
        assert_eq!(io.next_deadline(0), Some(5_000));
        assert!(!host.irqs.contains(&(230, true)));
        let saved = io.save();
        let mut copy = Io::default();
        copy.restore(&saved).unwrap();
        // Without the engine the window stays a plain register file.
        let mut plain = Io::default();
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        plain.write(&mut cx, 0xffd01040, 4, 1);
        assert_eq!(plain.read(&mut cx, 0xffd01000, 4), 0);
        assert!(cx.take_posted_writes().is_empty() && cx.refusal().is_none());
        drop(cx);
        host.now = 5_000;
        let token = host.events.last().unwrap().2;
        io.event(&mut Cx::new(&mut host, usize::MAX, 0, 0), token);
        assert_eq!(host.irqs.last(), Some(&(230, true)));
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        assert_eq!(io.read(&mut cx, 0xffd01048, 4), 1);
        assert_eq!(io.read(&mut cx, 0xffd01040, 4), 0);
        // ISR 0x2c72d28 acknowledges with 1 at +0x48.
        io.write(&mut cx, 0xffd01048, 4, 1);
        drop(cx);
        assert_eq!(host.irqs.last(), Some(&(230, false)));
        assert_eq!(io.next_deadline(host.now), None);
    }
    #[test]
    fn rifp_io_refuses_a_start_outside_register_list_mode() {
        let mut io = Io::default().with_queue_completion_ticks(1);
        let mut host = RecordingHost::default();
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        io.write(&mut cx, 0xffd01040, 4, 1);
        assert!(cx.is_unmodeled());
        assert!(cx.take_posted_writes().is_empty());
    }
}
