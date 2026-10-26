//! XP4 12-channel ADC. Kernel 5ac48..5ae90 supplies the register contract.
//! Samples and the functional conversion clock are explicit board inputs.
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::Value;
pub const BASE: u64 = 0xfff40000;
#[derive(Clone, Serialize, Deserialize)]
struct State {
    control: u32,
    divider: u32,
    gate: u32,
    status: u32,
    trim: u32,
    samples: [u16; 12],
    deadline: Option<u64>,
    last_edge: Option<u64>,
}
pub struct Adc {
    inputs: [u16; 12],
    sample_ticks: u64,
    state: State,
}
impl Adc {
    pub fn new(inputs: [u16; 12], sample_ticks: u64) -> Result<Self, String> {
        if inputs.iter().any(|v| *v > 4095) || !(1..=1024).contains(&sample_ticks) {
            return Err(
                "XP4 ADC requires twelve 12-bit inputs and an explicit conversion clock".into(),
            );
        }
        Ok(Self {
            inputs,
            sample_ticks,
            state: State {
                control: 0,
                divider: 0,
                gate: 0,
                status: 0,
                trim: 0,
                samples: [0; 12],
                deadline: None,
                last_edge: None,
            },
        })
    }
    pub fn set_input(
        &mut self,
        cx: &mut Cx<'_>,
        channel: usize,
        sample: u16,
    ) -> Result<(), String> {
        if channel >= 12 || sample > 4095 {
            return Err("Invalid XP4 external ADC sample".into());
        }
        self.advance(cx);
        if self.inputs[channel] != sample {
            self.inputs[channel] = sample;
            if self.state.control & 2 != 0 && self.mask() & (1 << channel) != 0 {
                self.arm(cx, false);
            }
        }
        Ok(())
    }
    fn mask(&self) -> u32 {
        self.state.control >> 16 & 0xfff
    }
    fn period(&self) -> u64 {
        self.sample_ticks
            * (u64::from(self.state.divider) + 1)
            * u64::from(self.mask().count_ones())
    }
    fn arm(&mut self, cx: &mut Cx<'_>, restart: bool) {
        if restart {
            self.state.deadline = None;
            self.state.last_edge = None;
        }
        if self.state.control & 1 == 0 || self.state.gate == 0 || self.mask() == 0 {
            return;
        }
        if self.state.deadline.is_some() {
            return;
        }
        let now = cx.now();
        let period = self.period();
        let delay = self
            .state
            .last_edge
            .map_or(period, |edge| period - (now - edge) % period);
        self.state.deadline = now.checked_add(delay);
        if self.state.deadline.is_none() {
            cx.fail("XP4 ADC deadline overflow");
        }
    }
}
impl Device for Adc {
    fn name(&self) -> &'static str {
        "xp4-adc"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        self.advance(cx);
        let offset = address.wrapping_sub(BASE);
        if size != 4 || offset & 3 != 0 {
            cx.unmodeled();
            return 0;
        }
        u64::from(match offset {
            0 => self.state.control,
            4 => self.state.divider,
            8 => self.state.gate,
            0xc => self.state.status,
            0x10..=0x3c => u32::from(self.state.samples[(offset as usize - 0x10) / 4]),
            0x40 => self.state.trim,
            _ => {
                cx.unmodeled();
                0
            }
        })
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        self.advance(cx);
        let offset = address.wrapping_sub(BASE);
        if size != 4 || offset & 3 != 0 || value > u64::from(u32::MAX) {
            cx.unmodeled();
            return;
        }
        let value = value as u32;
        match offset {
            0 if value & !0x0fff0003 == 0 => {
                if value == self.state.control {
                    return;
                }
                self.state.control = value;
                self.arm(cx, true);
            }
            4 if value < 0x100000 => {
                self.state.divider = value;
                self.arm(cx, true);
            }
            8 if value <= 1 => {
                self.state.gate = value;
                self.arm(cx, true);
            }
            0xc if value <= 1 => {
                self.state.status &= !value;
                if self.state.control & 2 != 0 {
                    self.arm(cx, false);
                }
            }
            0x40 if value <= 255 => self.state.trim = value,
            _ => cx.unmodeled(),
        }
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        let Some(at) = self.state.deadline.filter(|at| *at <= cx.now()) else {
            return;
        };
        for n in 0..12 {
            if self.mask() & (1 << n) != 0 {
                self.state.samples[n] = self.inputs[n];
            }
        }
        self.state.status = 1;
        self.state.deadline = None;
        self.state.last_edge = Some(at);
        if self.state.control & 2 == 0 {
            self.state.control &= !1;
        }
        // With fixed external inputs and the completion flag already set,
        // further continuous sweeps have no observable effect. A W1C schedules
        // the next actual clock edge, rather than a new polling interval.
    }
    fn next_deadline(&self, _: u64) -> Option<u64> {
        self.state.deadline
    }
    fn save(&self) -> Value {
        serde_json::json!({"inputs":self.inputs,"sample_ticks":self.sample_ticks,"adc":self.state})
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let state: State =
            serde_json::from_value(value["adc"].clone()).map_err(|e| e.to_string())?;
        let inputs: [u16; 12] =
            serde_json::from_value(value["inputs"].clone()).map_err(|e| e.to_string())?;
        if inputs.iter().any(|v| *v > 4095)
            || value["sample_ticks"] != self.sample_ticks
            || state.control & !0x0fff0003 != 0
            || state.divider >= 0x100000
            || state.gate > 1
            || state.status > 1
            || state.trim > 255
            || state.samples.iter().any(|v| *v > 4095)
            || (state.deadline.is_some()
                && (state.control & 1 == 0 || state.gate == 0 || state.control & 0x0fff0000 == 0))
        {
            return Err("Invalid XP4 ADC checkpoint or conversion clock".into());
        }
        self.inputs = inputs;
        self.state = state;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::{Host, Record};
    #[derive(Default)]
    struct Clock {
        now: u64,
    }
    impl Host for Clock {
        fn now(&self) -> u64 {
            self.now
        }
        fn schedule(&mut self, _: usize, _: u64, _: u64) {
            unreachable!()
        }
        fn set_irq(&mut self, _: u32, _: bool) {
            unreachable!()
        }
        fn raise_irq(&mut self, _: usize, _: u32) -> Result<(), String> {
            unreachable!()
        }
        fn ram_read(&self, _: u64, _: &mut [u8]) -> Result<(), String> {
            unreachable!()
        }
        fn ram_write(&mut self, _: u64, _: &[u8]) -> Result<(), String> {
            unreachable!()
        }
        fn record(&mut self, _: Record) {}
    }
    fn experiment(pc: u64, noisy: bool) -> Value {
        let mut m = Adc::new(std::array::from_fn(|n| (n as u16 + 1) * 100), 16).unwrap();
        let mut h = Clock::default();
        for (o, v) in [(8, 1), (0, 0x05550003)] {
            m.write(&mut Cx::new(&mut h, 3, pc, 0), BASE + o, 4, v);
        }
        assert_eq!(m.next_deadline(0), Some(96));
        for tick in 0..=96 {
            h.now = tick;
            let before = m.save();
            assert_eq!(m.save(), before);
            if noisy || tick == 96 {
                m.advance(&mut Cx::new(&mut h, 0, pc + 4, 0));
            }
            if tick < 96 {
                assert_eq!(m.save(), before);
            }
        }
        assert_eq!(
            m.state.samples,
            [100, 0, 300, 0, 500, 0, 700, 0, 900, 0, 1100, 0]
        );
        assert_eq!(m.state.status, 1);
        assert_eq!(m.next_deadline(96), None);
        h.now = 150;
        m.write(&mut Cx::new(&mut h, 0, pc, 0), BASE + 0xc, 4, 1);
        assert_eq!(m.next_deadline(150), Some(192));
        let saved = m.save();
        let mut restored = Adc::new(m.inputs, 16).unwrap();
        restored.restore(&saved).unwrap();
        h.now = 192;
        restored.advance(&mut Cx::new(&mut h, 1, pc, 0));
        assert_eq!(restored.state.status, 1);
        restored.save()
    }
    #[test]
    fn fixed_inputs_mask_deadlines_and_inspection_ignore_program_state() {
        assert_eq!(experiment(0x1000, false), experiment(0xdead0000, true));
    }
    #[test]
    fn invalid_input_refuses() {
        assert!(Adc::new([4096; 12], 16).is_err());
        assert!(Adc::new([0; 12], 0).is_err());
    }
    #[test]
    fn changed_physical_input_is_sampled_on_the_next_clock_edge() {
        let mut m = Adc::new([0; 12], 16).unwrap();
        let mut h = Clock::default();
        for (offset, value) in [(8, 1), (0, 0x00080003)] {
            m.write(&mut Cx::new(&mut h, 0, 0, 0), BASE + offset, 4, value);
        }
        h.now = 16;
        m.advance(&mut Cx::new(&mut h, 0, 0, 0));
        assert_eq!(m.next_deadline(16), None);
        h.now = 23;
        m.set_input(&mut Cx::new(&mut h, 3, 0x1234, 0), 3, 1024)
            .unwrap();
        assert_eq!(m.next_deadline(23), Some(32));
        assert_eq!(m.state.samples[3], 0);
        let saved = m.save();
        let mut resumed = Adc::new([0; 12], 16).unwrap();
        resumed.restore(&saved).unwrap();
        assert_eq!(resumed.save(), saved);
        h.now = 31;
        m.advance(&mut Cx::new(&mut h, 1, 0, 0));
        resumed.advance(&mut Cx::new(&mut h, 1, 0, 0));
        assert_eq!(m.save(), saved);
        assert_eq!(resumed.save(), saved);
        h.now = 32;
        m.advance(&mut Cx::new(&mut h, 1, 0, 0));
        resumed.advance(&mut Cx::new(&mut h, 1, 0, 0));
        assert_eq!(resumed.save(), m.save());
        assert_eq!(m.state.samples[3], 1024);
        assert_eq!(m.next_deadline(32), None);
        m.set_input(&mut Cx::new(&mut h, 1, 0, 0), 2, 2048).unwrap();
        assert_eq!(m.next_deadline(32), None);
        let saved = m.save();
        assert!(m.set_input(&mut Cx::new(&mut h, 1, 0, 0), 12, 0).is_err());
        assert!(m.set_input(&mut Cx::new(&mut h, 1, 0, 0), 3, 4096).is_err());
        assert_eq!(m.save(), saved);
    }
}
