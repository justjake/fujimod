//! XP5 preview input-buffer and RIFP processor maps.
use fujimod_emulator_sensor_input::{input_buffer::{InputBuffer, Output, Wiring}, statistics::{Planes, Statistics}};
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
#[derive(Clone, Default)]
pub struct Io(Arc<Mutex<State>>);
pub enum Preview {
    Input(InputBuffer),
    Rifp(Rifp),
}
pub struct Rifp {
    state: State,
    io: Io,
    delay: u64,
    source_luma: u8,
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
                "completion_us" | "frame_period_us" | "source_luma"
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
        if rifp {
            return Ok(Self::Rifp(Rifp {
                state: State::default(),
                io,
                delay,
                source_luma,
            }));
        }
        Ok(Self::Input(InputBuffer::new(
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
        )?))
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
            cx.unmodeled();
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
        if address != 0 && io.r(0x70) & 0x80000000 != 0 && row >= 4 && row % 4 == 0 && rows != 0 {
            if let Err(e) = fujimod_emulator_imaging::frame::paint(cx, address, row / 2, rows, row, self.source_luma) {
                cx.fail(e);
                return;
            }
        }
        self.state.regs.insert(4, 0);
        self.state.regs.insert(0x144, self.state.r(0x144) | 1);
        self.state.frames += 1;
        self.irq(cx);
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
        let o = address.wrapping_sub(0xffd01000);
        if n != 4 || o & 3 != 0 || o > 0xd8 {
            cx.unmodeled();
            return 0;
        }
        u64::from(self.0.lock().unwrap().r(o))
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, n: u8, v: u64) {
        let o = address.wrapping_sub(0xffd01000);
        if n != 4 || o & 3 != 0 || o > 0xd8 {
            cx.unmodeled();
            return;
        }
        self.0.lock().unwrap().regs.insert(o, v as u32);
    }
    fn save(&self) -> Value {
        serde_json::to_value(&*self.0.lock().unwrap()).unwrap()
    }
    fn restore(&mut self, v: &Value) -> Result<(), String> {
        *self.0.lock().unwrap() =
            serde_json::from_value(v.clone()).map_err(|e| format!("RIFP IO checkpoint: {e}"))?;
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
}
