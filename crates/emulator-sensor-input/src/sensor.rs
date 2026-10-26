//! Eight-byte sensor command packets fetched by programmed DMA.
//! Completion is scheduled; stock code owns its busy polls, cleanup and retries.
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

#[derive(Clone, Serialize, Deserialize)]
pub struct SensorState {
    pub registers: BTreeMap<u16, u8>,
    pub commands: u64,
    pub writes: u64,
    pub reads: u64,
    #[serde(default)]
    pub image: BTreeMap<u64, u32>,
    #[serde(default)]
    pub frames: [u64; 2],
    #[serde(default)]
    pub last_dma: [(u64, u32, u32); 2],
}
impl Default for SensorState {
    fn default() -> Self {
        // Functional reset assumption: standby until stock programming clears
        // register0 bit0. Stock sleep path330a6e0 sets this bit; wake clears it.
        Self {
            registers: BTreeMap::from([(0, 1)]),
            commands: 0,
            writes: 0,
            reads: 0,
            image: BTreeMap::new(),
            frames: [0; 2],
            last_dma: [(0, 0, 0); 2],
        }
    }
}
#[derive(Clone, Copy)]
pub struct ImageRoutes {
    pub dma: Option<[u32; 2]>,
    pub timing: [u32; 2],
}
#[derive(Clone)]
pub struct Sensor(pub Arc<Mutex<SensorState>>, Option<ImageRoutes>, (u16, u8));
impl Sensor {
    pub fn new(routes: Option<ImageRoutes>, read_gate: (u16, u8)) -> Self {
        Self(Arc::default(), routes, read_gate)
    }
    pub fn image_reg(&self, offset: u64) -> u32 {
        self.0
            .lock()
            .unwrap()
            .image
            .get(&offset)
            .copied()
            .unwrap_or(0)
    }
    pub fn image_write(&self, cx: &mut Cx<'_>, offset: u64, value: u32) {
        let mut s = self.0.lock().unwrap();
        if matches!(offset, 0x64 | 0xe4 | 0x144) {
            let old = s.image.get(&offset).copied().unwrap_or(0);
            s.image.insert(offset, old & !(value & 15));
        } else {
            s.image.insert(offset, value);
        }
        let Some(routes) = self.1 else { return };
        if let Some(dma) = routes.dma {
            for (enable, pending, irq) in [(0x60, 0x64, dma[0]), (0xe0, 0xe4, dma[1])] {
                cx.set_irq(
                    irq,
                    s.image.get(&enable).copied().unwrap_or(0)
                        & s.image.get(&pending).copied().unwrap_or(0)
                        & 15
                        != 0,
                );
            }
        }
        let flags =
            s.image.get(&0x140).copied().unwrap_or(0) & s.image.get(&0x144).copied().unwrap_or(0);
        cx.set_irq(routes.timing[0], flags & 3 != 0);
        cx.set_irq(routes.timing[1], flags & 12 != 0);
    }
    pub fn timing(&self, cx: &mut Cx<'_>, start: bool) {
        let Some(routes) = self.1 else { return };
        let mut s = self.0.lock().unwrap();
        // Global sensor timing remains active even without a RAW DMA buffer.
        // 1604184(event1) ->IRQ166 ->1603d98 ->imgdc32c6bd8/15ae.
        let pending = s.image.get(&0x144).copied().unwrap_or(0) | if start { 3 } else { 12 };
        s.image.insert(0x144, pending);
        let enabled = s.image.get(&0x140).copied().unwrap_or(0);
        cx.set_irq(routes.timing[0], pending & enabled & 3 != 0);
        cx.set_irq(routes.timing[1], pending & enabled & 12 != 0);
    }
    pub fn frame(&self, cx: &mut Cx<'_>, channel: usize) -> Result<(), String> {
        let Some(routes) = self.1 else { return Ok(()) };
        let Some(dma) = routes.dma else { return Ok(()) };
        let mut s = self.0.lock().unwrap();
        let base = [0x40, 0xc0][channel];
        let reg = |o| s.image.get(&o).copied().unwrap_or(0);
        if reg(base) & 1 == 0 {
            return Ok(());
        }
        let address = u64::from(reg(base + 0x10)) | (u64::from(reg(base + 0xc) & 7) << 32);
        let width = reg(base + 4) & 0x7fff;
        let height = reg(base + 8) & 0x7fff;
        // No framebuffer override: shape and destination come from143c828/dafc.
        if address == 0 || width == 0 || height == 0 {
            return Ok(());
        }
        let size = u64::from(width) * u64::from(height) * 2;
        if size > 128 * 1024 * 1024 {
            return Err("receiver frame exceeds functional DMA limit".into());
        }
        let frame = s.frames[channel];
        let mut bytes = Vec::with_capacity(size as usize);
        for y in 0..height {
            for x in 0..width {
                // Explicit RAW14-in-LE16 moving ramp for the normal REC14 profile.
                let sample =
                    ((u64::from(x) * 31 + u64::from(y) * 17 + frame * 131) & 0x3fff) as u16;
                bytes.extend_from_slice(&sample.to_le_bytes());
            }
        }
        cx.ram_write(address, &bytes)?;
        s.frames[channel] += 1;
        s.last_dma[channel] = (address, width, height);
        let pending = base + 0x24;
        let value = s.image.get(&pending).copied().unwrap_or(0) | 1;
        s.image.insert(pending, value);
        let enabled = s.image.get(&(base + 0x20)).copied().unwrap_or(0);
        cx.set_irq(dma[channel], value & enabled & 15 != 0);
        Ok(())
    }
    fn image_supported(&self, offset: u64) -> bool {
        self.1
            .is_some_and(|r| matches!(offset, 0x140 | 0x144) || r.dma.is_some())
    }
    pub fn streaming(&self) -> bool {
        let s = self.0.lock().unwrap();
        s.writes != 0 && s.registers.get(&0).copied().unwrap_or(1) & 1 == 0
    }
}
#[derive(Default, Serialize, Deserialize)]
struct State {
    registers: BTreeMap<u64, u32>,
    response: [u32; 8],
    generation: u64,
    pending: Option<Vec<[u8; 8]>>,
    deadline: Option<u64>,
}
pub struct Command {
    base: u64,
    state: State,
    pub sensor: Sensor,
    latency_ticks: u64,
}
impl Command {
    pub fn new(base: u64, sensor: Sensor, latency_ticks: u64) -> Result<Self, String> {
        if base & 0xfff != 0 || latency_ticks == 0 {
            return Err("sensor command needs aligned base and positive latency".into());
        }
        Ok(Self {
            base,
            state: State::default(),
            sensor,
            latency_ticks,
        })
    }
    fn reg(&self, o: u64) -> u32 {
        self.state.registers.get(&o).copied().unwrap_or(0)
    }
    fn cancel(&mut self) {
        self.state.generation = self.state.generation.wrapping_add(1);
        self.state.pending = None;
        self.state.deadline = None;
        self.state.registers.insert(0x204, 0);
    }
    fn submit(&mut self, cx: &mut Cx<'_>) -> Result<(), String> {
        if self.state.pending.is_some() {
            return Err("sensor command submitted while busy".into());
        }
        let format = self.reg(0x208);
        if format & 0x7fffff00 != 0x10400
            || self.reg(0x21c) != 0
            || self.reg(0x214) != 0
            || self.reg(0x218) != 8
            || self.reg(0x20c) > 7
        {
            return Err("unsupported sensor command DMA descriptor".into());
        }
        let source = u64::from(self.reg(0x210)) | (u64::from(self.reg(0x20c)) << 32);
        let count = match format & 255 {
            0 => return Err("zero sensor packet count is unsupported".into()),
            n => n as usize,
        };
        let bytes = cx.ram_bytes(source, count * 8)?;
        let mut packets = Vec::with_capacity(count);
        for bytes in bytes.chunks_exact(8) {
            let packet: [u8; 8] = bytes.try_into().unwrap();
            if !matches!(packet[0], 0x81 | 0x82) {
                return Err(format!("unsupported sensor opcode {:02x}", packet[0]));
            }
            packets.push(packet);
        }
        let duration = self
            .latency_ticks
            .checked_mul(count as u64)
            .ok_or("sensor command duration overflow")?;
        let at = cx
            .now()
            .checked_add(duration)
            .ok_or("sensor command deadline overflow")?;
        self.state.pending = Some(packets);
        self.state.registers.insert(0x204, 1);
        self.state.generation = self.state.generation.wrapping_add(1);
        self.state.deadline = Some(at);
        cx.schedule(at, self.state.generation);
        Ok(())
    }
}
impl Device for Command {
    fn name(&self) -> &'static str {
        "imaging-sensor"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let o = address.wrapping_sub(self.base);
        self.advance(cx);
        if size != 4 || o & 3 != 0 {
            cx.unmodeled();
            return 0;
        }
        match o {
            0 => u64::from(self.reg(0) | 1),
            0x4..=0x34 | 0x40..=0x94 | 0xc0..=0x114 | 0x140 | 0x144 => {
                if !self.sensor.image_supported(o) {
                    cx.unmodeled();
                    return 0;
                }
                u64::from(self.sensor.image_reg(o))
            }
            0x180..=0x1b0 | 0x200 | 0x204 | 0x208 | 0x20c | 0x210 | 0x214 | 0x218 | 0x21c => {
                u64::from(self.reg(o))
            }
            0x224..=0x240 => u64::from(self.state.response[((0x240 - o) / 4) as usize]),
            _ => {
                cx.unmodeled();
                0
            }
        }
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let o = address.wrapping_sub(self.base);
        self.advance(cx);
        if size != 4 || o & 3 != 0 {
            cx.unmodeled();
            return;
        }
        let value = value as u32;
        match o {
            0 => {
                self.state.registers.insert(0, value | 1);
                if value & 1 == 0 {
                    self.cancel();
                }
            }
            0x200 => {
                if value > 1 {
                    cx.fail("invalid sensor command reset");
                    return;
                }
                self.cancel();
                self.state.registers.insert(o, 1);
            }
            0x204 => {
                if value == 0 {
                    self.cancel();
                } else if value == 1 {
                    if let Err(e) = self.submit(cx) {
                        cx.fail(e);
                    }
                } else {
                    cx.fail("invalid sensor command start");
                }
            }
            0x4..=0x34 | 0x40..=0x94 | 0xc0..=0x114 | 0x140 | 0x144 => {
                if !self.sensor.image_supported(o) {
                    cx.unmodeled();
                    return;
                }
                self.sensor.image_write(cx, o, value)
            }
            0x180..=0x1b0 | 0x208 | 0x20c | 0x210 | 0x214 | 0x218 | 0x21c => {
                if self.state.pending.is_some() {
                    cx.fail("reconfiguring active sensor command");
                } else {
                    self.state.registers.insert(o, value);
                }
            }
            _ => cx.unmodeled(),
        }
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
        let Some(packets) = self.state.pending.take() else {
            return;
        };
        let mut sensor = self.sensor.0.lock().unwrap();
        for packet in packets {
            let address = u16::from_be_bytes([packet[1], packet[2]]);
            sensor.commands += 1;
            if packet[0] == 0x81 {
                sensor.registers.insert(address, packet[3]);
                sensor.writes += 1;
            } else {
                let (gate, mask) = self.sensor.2;
                if sensor.registers.get(&gate).copied().unwrap_or(0) & mask == 0 {
                    cx.fail(format!(
                        "sensor read while register {gate:#x} gate is closed"
                    ));
                    return;
                }
                for (i, response) in self.state.response.iter_mut().enumerate() {
                    *response = u32::from(
                        sensor
                            .registers
                            .get(&address.wrapping_add(i as u16))
                            .copied()
                            .unwrap_or(0),
                    ) << 16;
                }
                sensor.reads += 1;
            }
        }
        self.state.registers.insert(0x204, 0);
    }
    fn save(&self) -> Value {
        serde_json::json!({"command": self.state, "sensor": *self.sensor.0.lock().unwrap(), "command_latency_ticks": self.latency_ticks})
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let state: State = serde_json::from_value(value["command"].clone())
            .map_err(|e| format!("sensor command checkpoint: {e}"))?;
        let sensor = serde_json::from_value(value["sensor"].clone())
            .map_err(|e| format!("sensor checkpoint: {e}"))?;
        let latency = value["command_latency_ticks"]
            .as_u64()
            .filter(|n| *n > 0)
            .ok_or("missing sensor command latency")?;
        if state.pending.is_some() != state.deadline.is_some() {
            return Err("sensor pending/deadline disagree".into());
        }
        self.state = state;
        *self.sensor.0.lock().unwrap() = sensor;
        self.latency_ticks = latency;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::test_host::recording::RecordingHost;
    #[test]
    fn timing_input_causes_ack_without_granting_raw_dma_support() {
        let sensor = Sensor::new(
            Some(ImageRoutes {
                dma: None,
                timing: [100, 102],
            }),
            (0x51, 8),
        );
        let mut command = Command::new(0xfff6c000, sensor.clone(), 17).unwrap();
        let mut host = RecordingHost::default();
        command.write(&mut Cx::new(&mut host, 3, 0xabad, 0), 0xfff6c140, 4, 15);
        sensor.timing(&mut Cx::new(&mut host, 0, 0, 0), true);
        assert!(host.irqs.iter().rev().find(|(i, _)| *i == 100).unwrap().1);
        assert_eq!(
            command.read(&mut Cx::new(&mut host, 0, 0, 0), 0xfff6c144, 4),
            3
        );
        command.write(&mut Cx::new(&mut host, 0, 0, 0), 0xfff6c144, 4, 3);
        assert!(!host.irqs.iter().rev().find(|(i, _)| *i == 100).unwrap().1);
        sensor.timing(&mut Cx::new(&mut host, 3, 0x1234, 0), false);
        assert!(host.irqs.iter().rev().find(|(i, _)| *i == 102).unwrap().1);
        assert_eq!(
            command.read(&mut Cx::new(&mut host, 0, 0, 0), 0xfff6c144, 4),
            12
        );
        let saved = command.save();
        {
            let mut cx = Cx::new(&mut host, 0, 0, 0);
            command.write(&mut cx, 0xfff6c040, 4, 1);
            assert!(cx.is_unmodeled());
        }
        assert_eq!(command.save(), saved);
        assert!(host.ram.is_empty());
    }
    #[test]
    fn packet_lists_read_gate_cancel_and_exact_deadline_are_program_independent() {
        fn experiment(cpu: usize, pc: u64, late: bool) -> Value {
            let sensor = Sensor::new(None, (0x51, 8));
            let mut command = Command::new(0xfff6c000, sensor.clone(), 17).unwrap();
            let mut host = RecordingHost {
                ram_base: 0x1000,
                ram: vec![
                    0x81, 0, 0x51, 8, 0, 0, 0, 0, 0x81, 0, 0, 4, 0, 0, 0, 0, 0x81, 1, 0, 0xa5, 0,
                    0, 0, 0,
                ],
                ..Default::default()
            };
            for (offset, value) in [(0x208, 0x10403), (0x210, 0x1000), (0x218, 8), (0x204, 1)] {
                let mut cx = Cx::new(&mut host, cpu, pc, 0);
                command.write(&mut cx, 0xfff6c000 + offset, 4, value);
                assert!(cx.refusal().is_none());
            }
            let saved = command.save();
            assert_eq!(command.next_deadline(0), Some(51));
            host.now = 50;
            command.event(&mut Cx::new(&mut host, cpu, pc, 0), 1);
            assert_eq!(command.save(), saved);
            assert!(!sensor.streaming());
            // The transfer owns the submitted DMA packet bytes.
            host.ram.fill(0xff);
            host.now = if late { 1000 } else { 51 };
            command.event(&mut Cx::new(&mut host, cpu, pc, 0), 1);
            assert!(sensor.streaming());
            assert_eq!(sensor.0.lock().unwrap().commands, 3);
            host.ram[..8].copy_from_slice(&[0x82, 1, 0, 0, 0, 0, 0, 0]);
            for (offset, value) in [(0x208, 0x10401), (0x204, 1)] {
                command.write(
                    &mut Cx::new(&mut host, cpu, pc, 0),
                    0xfff6c000 + offset,
                    4,
                    value,
                );
            }
            host.now += 17;
            command.advance(&mut Cx::new(&mut host, cpu, pc, 0));
            assert_eq!(
                command.read(&mut Cx::new(&mut host, cpu, pc, 0), 0xfff6c240, 4),
                0xa50000
            );
            // Cancel a pending write; its old event cannot apply it.
            host.ram[..8].copy_from_slice(&[0x81, 1, 0, 0x99, 0, 0, 0, 0]);
            command.write(&mut Cx::new(&mut host, cpu, pc, 0), 0xfff6c204, 4, 1);
            command.write(&mut Cx::new(&mut host, cpu, pc, 0), 0xfff6c204, 4, 0);
            let saved = command.save();
            host.now += 17;
            command.event(&mut Cx::new(&mut host, cpu, pc, 0), 3);
            assert_eq!(command.save(), saved);
            assert_eq!(command.next_deadline(host.now), None);
            saved
        }
        let expected = experiment(0, 0, false);
        assert_eq!(experiment(3, 0xabad, true), expected);
    }
    #[test]
    fn dma_command_waits_for_guest_time_and_restores_pending_transport() {
        let sensor = Sensor::new(None, (0x59, 4));
        let mut command = Command::new(0xff948000, sensor.clone(), 100_000).unwrap();
        let mut host = RecordingHost {
            ram_base: 0x1000,
            ram: vec![0x81, 0, 0x59, 4, 0, 0, 0, 0],
            ..Default::default()
        };
        {
            let mut cx = Cx::new(&mut host, 3, 0xabad, 0);
            for (o, v) in [(0x208, 0x10401), (0x210, 0x1000), (0x218, 8), (0x204, 1)] {
                command.write(&mut cx, 0xff948000 + o, 4, v);
            }
            assert!(cx.refusal().is_none());
        }
        let saved = command.save();
        let token = saved["command"]["generation"].as_u64().unwrap();
        host.now = 99_999;
        command.event(&mut Cx::new(&mut host, 0, 0, 0), token);
        assert_eq!(command.save(), saved);
        assert_eq!(sensor.0.lock().unwrap().writes, 0);
        command.restore(&saved).unwrap();
        host.now = 100_000;
        command.advance(&mut Cx::new(&mut host, 0, 0, 0));
        assert_eq!(sensor.0.lock().unwrap().registers[&0x59], 4);
        assert_eq!(command.next_deadline(host.now), None);
        host.ram[0] = 0x82;
        command.write(&mut Cx::new(&mut host, 0, 0, 0), 0xff948204, 4, 1);
        host.now = 200_000;
        command.advance(&mut Cx::new(&mut host, 0, 0, 0));
        assert_eq!(
            command.read(&mut Cx::new(&mut host, 0, 0, 0), 0xff948240, 4),
            4 << 16
        );
        assert_eq!(sensor.0.lock().unwrap().reads, 1);
    }
}
