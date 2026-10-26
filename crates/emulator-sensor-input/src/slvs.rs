//! SLVS-EC causes, frame phases and explicit sensor/input-engine wiring.
use super::sensor::Sensor;
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Default, Serialize, Deserialize)]
struct State {
    regs: BTreeMap<u64, u32>,
    generation: u64,
    armed: bool,
    frames: u64,
    #[serde(default)]
    active: bool,
    frame_deadline: Option<u64>,
    end_deadline: Option<u64>,
    reset_deadline: Option<u64>,
}
/// Per-link control field positions and levels, selected by the processor map.
#[derive(Clone, Copy)]
pub struct Control {
    pub run_mask: u32,
    pub run_value: u32,
    pub reset_pulse: u32,
    pub reset_ticks: u64,
}
pub struct Wiring {
    pub base: u64,
    pub normal_irqs: Vec<u32>,
    pub error_irqs: Vec<u32>,
    pub output_channels: Vec<usize>,
    pub timing_output: bool,
    pub control: Control,
}
pub struct Receiver {
    state: State,
    sensor: Sensor,
    wiring: Wiring,
    period_ticks: u64,
    active_ticks: u64,
    input_buffer: Option<super::input_buffer::InputBuffer>,
}
impl Receiver {
    pub fn new(
        sensor: Sensor,
        wiring: Wiring,
        period_ticks: u64,
        active_ticks: u64,
    ) -> Result<Self, String> {
        if wiring.base & 0xfff != 0
            || !(1..=2).contains(&wiring.normal_irqs.len())
            || wiring.error_irqs.len() != wiring.normal_irqs.len()
            || wiring.output_channels.iter().any(|c| *c > 1)
            || period_ticks < 2
            || active_ticks == 0
            || active_ticks >= period_ticks
            || wiring.control.run_value & !wiring.control.run_mask != 0
            || wiring.control.reset_pulse != 0 && wiring.control.reset_ticks == 0
        {
            return Err("invalid SLVS receiver wiring/timing".into());
        }
        Ok(Self {
            state: State::default(),
            sensor,
            wiring,
            period_ticks,
            active_ticks,
            input_buffer: None,
        })
    }
    pub fn with_input_buffer(mut self, input: super::input_buffer::InputBuffer) -> Self {
        self.input_buffer = Some(input);
        self
    }
    fn reg(&self, o: u64) -> u32 {
        self.state.regs.get(&o).copied().unwrap_or_else(|| {
            // 330b720 calls 319992c4 to set bit4, preserving reset bit0.
            // 1457308 rejects bit4=1/bit0=0; a zero reset word silently
            // prevents the stock receiver initializer from starting a link.
            if o == 0 {
                if self.links() == 2 { 0x10001 } else { 1 }
            } else {
                0
            }
        })
    }
    fn enabled(&self, link: usize) -> bool {
        let shift = link * 16;
        self.reg(0) >> shift & self.wiring.control.run_mask == self.wiring.control.run_value
            && self.state.reset_deadline.is_none()
    }
    fn links(&self) -> usize {
        self.wiring.normal_irqs.len()
    }
    fn irq(&self, cx: &mut Cx<'_>) {
        for link in 0..self.links() {
            let base = 0x100 + link as u64 * 0x100;
            let intid = self.wiring.normal_irqs[link];
            cx.set_irq(intid, self.reg(base + 8) & self.reg(base + 12) & 0xf3 != 0);
            // Functional healthy link: no synthetic receiver error causes.
            cx.set_irq(self.wiring.error_irqs[link], false);
        }
    }
    fn active_source(&self) -> bool {
        (0..self.links()).any(|i| self.enabled(i)) && self.sensor.streaming()
    }
    fn arm(&mut self, cx: &mut Cx<'_>) {
        let now = cx.now();
        self.arm_at(cx, now);
    }
    fn arm_at(&mut self, cx: &mut Cx<'_>, anchor: u64) {
        let enabled = self.active_source();
        if enabled && !self.state.armed {
            self.state.armed = true;
            self.state.generation = self.state.generation.wrapping_add(1);
            let at = anchor.saturating_add(self.period_ticks);
            self.state.frame_deadline = Some(at);
            cx.schedule(at, self.state.generation);
        } else if !enabled && self.state.armed {
            self.state.armed = false;
            self.state.active = false;
            self.state.generation = self.state.generation.wrapping_add(1);
            self.state.frame_deadline = None;
            self.state.end_deadline = None;
        }
    }
    fn known(&self, o: u64) -> bool {
        if o <= 0x40 {
            return true;
        }
        (0..self.links()).any(|link| {
            o.checked_sub(link as u64 * 0x100)
                .is_some_and(|v| (0x100..=0x150).contains(&v))
        })
    }
}
impl Device for Receiver {
    fn name(&self) -> &'static str {
        "imaging-receiver"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let o = address.wrapping_sub(self.wiring.base);
        self.advance(cx);
        if size != 4 || o & 3 != 0 || !self.known(o) {
            cx.unmodeled();
            return 0;
        }
        if o % 0x100 == 0x44 && o >= 0x100 {
            return self.state.frames & 0x1fff;
        }
        if o % 0x100 == 0x48 && o >= 0x100 {
            let n = self.state.frames & 0x1fff;
            return n | n << 16;
        }
        u64::from(self.reg(o))
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let o = address.wrapping_sub(self.wiring.base);
        self.advance(cx);
        if size != 4 || o & 3 != 0 || !self.known(o) {
            cx.unmodeled();
            return;
        }
        let value = value as u32;
        if o == 0 {
            let mask = (0..self.links()).fold(0, |mask, link| {
                mask | self.wiring.control.reset_pulse << (link * 16)
            });
            if value & mask != 0 {
                let Some(at) = cx.now().checked_add(self.wiring.control.reset_ticks) else {
                    cx.fail("SLVS reset deadline overflow");
                    return;
                };
                self.state.reset_deadline = Some(at);
            }
        }
        if o >= 0x100 && o % 0x100 == 8 {
            self.state.regs.insert(o, self.reg(o) & !(value & 0xf3));
        } else {
            self.state.regs.insert(o, value);
        }
        self.irq(cx);
        self.arm(cx);
        if o == 0
            && let Some(at) = self.state.reset_deadline
        {
            cx.schedule(at, self.state.generation);
        }
    }
    fn event(&mut self, cx: &mut Cx<'_>, token: u64) {
        if token & !(1 << 63) == self.state.generation {
            self.advance(cx);
        }
    }
    fn next_deadline(&self, now: u64) -> Option<u64> {
        if self.active_source() != self.state.armed {
            return Some(now);
        }
        self.state
            .frame_deadline
            .into_iter()
            .chain(self.state.end_deadline)
            .chain(self.state.reset_deadline)
            .min()
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        let anchor = self
            .state
            .reset_deadline
            .filter(|at| *at <= cx.now())
            .unwrap_or(cx.now());
        if self.state.reset_deadline.is_some_and(|at| at <= cx.now()) {
            self.state.reset_deadline = None;
            let mask = (0..self.links()).fold(0, |mask, link| {
                mask | self.wiring.control.reset_pulse << (link * 16)
            });
            self.state.regs.insert(0, self.reg(0) & !mask);
        }
        self.arm_at(cx, anchor);
        loop {
            let next = self
                .state
                .frame_deadline
                .into_iter()
                .map(|at| (at, false))
                .chain(self.state.end_deadline.map(|at| (at, true)))
                .min();
            let Some((at, end)) = next.filter(|(at, _)| *at <= cx.now()) else {
                break;
            };
            if end {
                self.state.end_deadline = None;
            }
            if end && self.state.active {
                self.state.active = false;
                if let Some(input) = &self.input_buffer {
                    if let Err(error) = input.frame(cx) {
                        cx.fail(error);
                        return;
                    }
                }
                if self.wiring.timing_output {
                    self.sensor.timing(cx, false);
                }
                for &channel in &self.wiring.output_channels {
                    if let Err(e) = self.sensor.frame(cx, channel) {
                        cx.fail(e);
                        return;
                    }
                }
                self.state.frames += 1;
                for link in 0..self.links() {
                    if self.enabled(link) {
                        let cause = 0x108 + link as u64 * 0x100;
                        // Bits1/0 map to the two frame synchronization event flags
                        // in3307b7c (param+10/+14): shutdown/wake waits can advance.
                        self.state.regs.insert(cause, self.reg(cause) | 2);
                    }
                }
                self.irq(cx);
            } else if !end && self.sensor.streaming() {
                self.state.active = true;
                if self.wiring.timing_output {
                    self.sensor.timing(cx, true);
                }
                for link in 0..self.links() {
                    if self.enabled(link) {
                        let cause = 0x108 + link as u64 * 0x100;
                        self.state.regs.insert(cause, self.reg(cause) | 1);
                    }
                }
                self.irq(cx);
                let deadline = at.saturating_add(self.active_ticks);
                self.state.end_deadline = Some(deadline);
                cx.schedule(deadline, self.state.generation | (1 << 63));
            }
            if !end {
                let deadline = at.saturating_add(self.period_ticks);
                self.state.frame_deadline = Some(deadline);
                cx.schedule(deadline, self.state.generation);
            }
        }
    }
    fn save(&self) -> Value {
        serde_json::json!({"state":self.state,"period_ticks":self.period_ticks,"active_ticks":self.active_ticks})
    }
    fn restore(&mut self, v: &Value) -> Result<(), String> {
        let state: State = serde_json::from_value(v["state"].clone())
            .map_err(|e| format!("receiver checkpoint: {e}"))?;
        let period = v["period_ticks"]
            .as_u64()
            .filter(|n| *n > 0)
            .ok_or("invalid receiver period")?;
        let active = v["active_ticks"]
            .as_u64()
            .filter(|n| *n > 0 && *n < period)
            .ok_or("invalid receiver active period")?;
        self.state = state;
        self.period_ticks = period;
        self.active_ticks = active;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::test_host::recording::RecordingHost;
    const BASE: u64 = 0xff940000;
    const PERIOD: u64 = 16_667_000;
    const ACTIVE: u64 = PERIOD / 2;
    #[test]
    fn reset_pulse_stop_level_exact_phases_and_ack_do_not_depend_on_program() {
        for (cpu, pc) in [(0, 0), (3, 0x12345678)] {
            let sensor = Sensor::new(None, (0x51, 8));
            {
                let mut s = sensor.0.lock().unwrap();
                s.writes = 1;
                s.registers.insert(0, 4);
            }
            let mut rx = Receiver::new(
                sensor,
                Wiring {
                    base: 0xfff69000,
                    normal_irqs: vec![296, 298],
                    error_irqs: vec![297, 297],
                    output_channels: vec![],
                    timing_output: false,
                    control: Control {
                        run_mask: 0x11,
                        run_value: 0,
                        reset_pulse: 0x10,
                        reset_ticks: 8,
                    },
                },
                100,
                50,
            )
            .unwrap();
            let mut host = RecordingHost::default();
            assert_eq!(
                rx.read(&mut Cx::new(&mut host, cpu, pc, 0), 0xfff69000, 4),
                0x10001
            );
            rx.write(&mut Cx::new(&mut host, cpu, pc, 0), 0xfff69000, 4, 0x10011);
            assert_eq!(rx.next_deadline(0), Some(8));
            let saved = rx.save();
            host.now = 7;
            rx.event(&mut Cx::new(&mut host, cpu, pc, 0), 0);
            assert_eq!(rx.save(), saved);
            host.now = 8;
            rx.advance(&mut Cx::new(&mut host, cpu, pc, 0));
            assert_eq!(
                rx.read(&mut Cx::new(&mut host, cpu, pc, 0), 0xfff69000, 4),
                0x10001
            );
            assert_eq!(rx.next_deadline(8), None);
            rx.write(&mut Cx::new(&mut host, cpu, pc, 0), 0xfff6910c, 4, 3);
            rx.write(&mut Cx::new(&mut host, cpu, pc, 0), 0xfff69000, 4, 0x10000);
            assert_eq!(rx.next_deadline(8), Some(108));
            let saved = rx.save();
            host.now = 107;
            rx.event(&mut Cx::new(&mut host, cpu, pc, 0), 1);
            assert_eq!(rx.save(), saved);
            host.now = 108;
            rx.advance(&mut Cx::new(&mut host, cpu, pc, 0));
            assert!(irq(&host, 296));
            assert_eq!(rx.next_deadline(108), Some(158));
            rx.write(&mut Cx::new(&mut host, cpu, pc, 0), 0xfff69108, 4, 1);
            assert!(!irq(&host, 296));
            host.now = 158;
            rx.advance(&mut Cx::new(&mut host, cpu, pc, 0));
            assert_eq!(
                rx.read(&mut Cx::new(&mut host, cpu, pc, 0), 0xfff69108, 4),
                2
            );
            rx.write(&mut Cx::new(&mut host, cpu, pc, 0), 0xfff69108, 4, 2);
            rx.write(&mut Cx::new(&mut host, cpu, pc, 0), 0xfff69000, 4, 0x10001);
            assert!(!irq(&host, 296));
            assert_eq!(rx.next_deadline(host.now), None);
            assert!(host.ram.is_empty());
        }
    }
    fn rig(streaming: bool) -> (Sensor, Receiver, RecordingHost) {
        let sensor = Sensor::new(
            Some(super::super::sensor::ImageRoutes {
                dma: Some([162, 163]),
                timing: [166, 164],
            }),
            (0x59, 4),
        );
        {
            let mut s = sensor.0.lock().unwrap();
            s.image = BTreeMap::from([
                (0x40, 1),
                (0x44, 4),
                (0x48, 2),
                (0x4c, 1),
                (0x50, 0x100),
                (0x60, 1),
            ]);
            if streaming {
                s.registers.insert(0, 0x14);
                s.writes = 1;
            }
        }
        let receiver = Receiver::new(
            sensor.clone(),
            Wiring {
                base: BASE,
                normal_irqs: vec![154, 156],
                error_irqs: vec![155, 157],
                output_channels: vec![0, 1],
                timing_output: true,
                control: Control {
                    run_mask: 0x11,
                    run_value: 0x11,
                    reset_pulse: 0,
                    reset_ticks: 0,
                },
            },
            PERIOD,
            ACTIVE,
        )
        .unwrap();
        let host = RecordingHost {
            ram: vec![0x55; 0x1000],
            ram_base: 0x1_0000_0000,
            ..Default::default()
        };
        (sensor, receiver, host)
    }
    fn irq(host: &RecordingHost, line: u32) -> bool {
        host.irqs
            .iter()
            .rev()
            .find(|(n, _)| *n == line)
            .is_some_and(|(_, v)| *v)
    }
    fn enable(receiver: &mut Receiver, host: &mut RecordingHost) {
        let mut cx = Cx::new(host, 2, 0xfeed, 0);
        receiver.write(&mut cx, BASE + 0x10c, 4, 3);
        receiver.write(&mut cx, BASE, 4, 0x11);
        assert!(cx.refusal().is_none());
    }
    #[test]
    fn standby_dma_high_address_irq_ack_and_cancel() {
        let (sensor, mut receiver, mut host) = rig(false);
        enable(&mut receiver, &mut host);
        assert_eq!(receiver.next_deadline(0), None);
        assert!(host.events.is_empty()); // Guest-controlled standby is not polled.
        {
            let mut s = sensor.0.lock().unwrap();
            s.registers.insert(0, 0x14);
            s.writes = 1;
        }
        assert_eq!(receiver.next_deadline(0), Some(0));
        receiver.advance(&mut Cx::new(&mut host, 0, 0, 0));
        assert_eq!(receiver.next_deadline(0), Some(PERIOD));
        let initial = receiver.save();
        let token = initial["state"]["generation"].as_u64().unwrap();
        host.now = PERIOD - 1;
        receiver.event(&mut Cx::new(&mut host, 0, 0, 0), token);
        assert_eq!(receiver.save(), initial);
        assert_eq!(&host.ram[0x100..0x110], &[0x55; 16]);
        host.now = PERIOD;
        receiver.advance(&mut Cx::new(&mut host, 0, 0, 0));
        assert!(irq(&host, 154));
        host.now = PERIOD + ACTIVE;
        receiver.advance(&mut Cx::new(&mut host, 0, 0, 0));
        assert_eq!(
            &host.ram[0x100..0x110],
            &[0, 0, 31, 0, 62, 0, 93, 0, 17, 0, 48, 0, 79, 0, 110, 0]
        );
        assert_eq!(sensor.0.lock().unwrap().last_dma[0], (0x1_0000_0100, 4, 2));
        assert!(irq(&host, 162));
        receiver.write(&mut Cx::new(&mut host, 0, 0, 0), BASE + 0x108, 4, 3);
        sensor.image_write(&mut Cx::new(&mut host, 0, 0, 0), 0x64, 1);
        assert!(!irq(&host, 154));
        assert!(!irq(&host, 162));
        host.now = PERIOD * 2 + ACTIVE;
        receiver.advance(&mut Cx::new(&mut host, 0, 0, 0));
        assert_eq!(&host.ram[0x100..0x102], &131u16.to_le_bytes());
        receiver.write(&mut Cx::new(&mut host, 0, 0, 0), BASE, 4, 0);
        host.now = PERIOD * 4;
        receiver.event(&mut Cx::new(&mut host, 0, 0, 0), token);
        assert_eq!(receiver.next_deadline(host.now), None);
        assert_eq!(sensor.0.lock().unwrap().frames[0], 2);
        let saved = receiver.save();
        receiver.restore(&saved).unwrap();
        assert!(receiver.restore(&Value::Null).is_err());
        assert_eq!(receiver.save(), saved);
    }
    #[test]
    fn delayed_frame_service_has_the_same_state_and_pixels_as_each_clock_edge() {
        let (sensor_a, mut a, mut ah) = rig(true);
        let (sensor_b, mut b, mut bh) = rig(true);
        enable(&mut a, &mut ah);
        enable(&mut b, &mut bh);
        for frame in 1..=4 {
            ah.now = frame * PERIOD;
            a.advance(&mut Cx::new(&mut ah, 0, 0, 0));
            ah.now = frame * PERIOD + ACTIVE;
            a.advance(&mut Cx::new(&mut ah, 0, 0, 0));
        }
        bh.now = 4 * PERIOD + ACTIVE;
        b.advance(&mut Cx::new(&mut bh, 0, 0, 0));
        assert_eq!(a.save(), b.save());
        assert_eq!(ah.ram, bh.ram);
        assert_eq!(
            serde_json::to_value(&*sensor_a.0.lock().unwrap()).unwrap(),
            serde_json::to_value(&*sensor_b.0.lock().unwrap()).unwrap()
        );
        sensor_b.0.lock().unwrap().registers.insert(0, 1);
        assert_eq!(b.next_deadline(bh.now), Some(bh.now));
        b.advance(&mut Cx::new(&mut bh, 0, 0, 0));
        assert_eq!(b.next_deadline(bh.now), None);
    }
}
