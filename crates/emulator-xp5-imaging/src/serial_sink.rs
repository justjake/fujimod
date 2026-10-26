//! Write-only virtual SPI3/4/5/8 sinks for screen-first boots. The FIFO
//! completes bounded transfers; RX0 is a dummy, not an accessory reply.
use fujimod_emulator_devices::{Cx, Device};
use serde_json::Value;
pub struct Sink {
    base: u64,
    regs: [u32; 10],
    rx: Option<u16>,
    motion_generation: u64,
    motion_closed: Option<bool>,
    finder_pins: Option<fujimod_emulator_devices::xp5::pins::SharedPins>,
}
impl Sink {
    pub fn new(base: u64) -> Self {
        Self {
            base,
            regs: [0; 10],
            rx: None,
            motion_generation: 0,
            motion_closed: None,
            finder_pins: None,
        }
    }
    pub fn base(&self) -> u64 {
        self.base
    }
    pub fn with_finder(mut self, pins: fujimod_emulator_devices::xp5::pins::SharedPins) -> Self {
        self.finder_pins = Some(pins);
        self
    }
    fn finder_word(&mut self, cx: &mut Cx<'_>, word: u16) {
        if self.finder_pins.is_none() || word & 0xf000 != 0xa000 {
            return;
        }
        // 164d6ec fields1f/20/21: word18, finite bit11, direction10,
        // doubled pulse count low10. Actual cold close writes a832.
        self.motion_generation += 1;
        self.motion_closed = None;
        let pulses = word & 0x3ff;
        if word & 0x800 != 0 && pulses != 0 {
            let closed = word & 0x400 == 0;
            self.motion_closed = Some(closed);
            // Functional virtual timing, not measured actuator latency.
            cx.schedule(
                cx.now() + (1000 + pulses as u64 * 20) * 1000,
                self.motion_generation,
            );
            cx.log(&format!(
                "virtual finder motion pulses{pulses} closed{closed}"
            ));
        }
    }
}
impl Device for Sink {
    fn name(&self) -> &'static str {
        "virtual-audio-spi"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, n: u8) -> u64 {
        let o = address.wrapping_sub(self.base);
        if n != 4 || o & 3 != 0 || o > 0x24 {
            cx.unmodeled();
            return 0;
        }
        match o {
            // PL022 TFE0/TNF1/RNE2: idle RX is empty, not permanently
            // ready. RX drains before an analog batch.
            0xc => 3 | ((self.rx.is_some() as u64) << 2),
            8 => self.rx.take().unwrap_or(0) as u64,
            0x14 | 0x20 => 0,
            _ => self.regs[o as usize / 4] as u64,
        }
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, n: u8, v: u64) {
        let o = address.wrapping_sub(self.base);
        if n != 4 || o & 3 != 0 || o > 0x24 {
            cx.unmodeled();
            return;
        }
        match o {
            8 => {
                self.rx = Some(0); // Existing write-only virtual sink dummy.
                self.finder_word(cx, v as u16);
            }
            0x20 if v & 1 != 0 => self.rx = None,
            _ => self.regs[o as usize / 4] = v as u32,
        }
    }

    fn event(&mut self, cx: &mut Cx<'_>, token: u64) {
        if token != self.motion_generation {
            return;
        }
        if let (Some(pins), Some(closed)) = (&self.finder_pins, self.motion_closed.take()) {
            let mut pins = pins.lock().unwrap();
            // Input46/resourcef is GPIO0.15 position; logical IRQ1b row
            // 1ba25fc is GPIO1.12, selectorb6/INTID128 (164cdd4 callback).
            pins.set_input(0, 15, closed);
            pins.set_input(1, 12, true);
            cx.set_irq(128, true);
            cx.set_irq(128, false);
            pins.set_input(1, 12, false);
            cx.log("virtual finder motion complete IRQ128");
        }
    }
    fn save(&self) -> Value {
        serde_json::json!({"regs":self.regs,"rx":self.rx,"motion_generation":self.motion_generation,"motion_closed":self.motion_closed})
    }
    fn restore(&mut self, s: &Value) -> Result<(), String> {
        let regs = serde_json::from_value(s["regs"].clone())
            .map_err(|e| format!("invalid SPI checkpoint: {e}"))?;
        let rx = serde_json::from_value(s["rx"].clone())
            .map_err(|e| format!("invalid SPI RX checkpoint: {e}"))?;
        let generation = s["motion_generation"]
            .as_u64()
            .ok_or("invalid finder generation")?;
        let closed = serde_json::from_value(s["motion_closed"].clone())
            .map_err(|e| format!("invalid finder position checkpoint: {e}"))?;
        self.regs = regs;
        self.rx = rx;
        self.motion_generation = generation;
        self.motion_closed = closed;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::test_host::recording::RecordingHost;
    #[test]
    fn stock_fifo_flush_word_completion_and_checkpoint() {
        let mut s = Sink::new(0);
        let mut h = RecordingHost::default();
        assert_eq!(s.read(&mut Cx::new(&mut h, 0, 0, 0), 0xc, 4), 3);
        s.write(&mut Cx::new(&mut h, 0, 0, 0), 8, 4, 0x1234);
        assert_eq!(s.read(&mut Cx::new(&mut h, 0, 0, 0), 0xc, 4), 7);
        let mut restored = Sink::new(0);
        restored.restore(&s.save()).unwrap();
        assert_eq!(restored.read(&mut Cx::new(&mut h, 0, 0, 0), 0xc, 4), 7);
        assert_eq!(restored.read(&mut Cx::new(&mut h, 0, 0, 0), 8, 4), 0);
        assert_eq!(restored.read(&mut Cx::new(&mut h, 0, 0, 0), 0xc, 4), 3);
        restored.write(&mut Cx::new(&mut h, 0, 0, 0), 8, 4, 0x5678);
        restored.write(&mut Cx::new(&mut h, 0, 0, 0), 0x20, 4, 3);
        assert_eq!(restored.read(&mut Cx::new(&mut h, 0, 0, 0), 0xc, 4), 3);
        assert!(
            restored
                .restore(&serde_json::to_value([0u32; 10]).unwrap())
                .is_err()
        );
        assert_eq!(restored.read(&mut Cx::new(&mut h, 0, 0, 0), 0xc, 4), 3);
    }
    #[test]
    fn observed_finite_close_irq_polarity_checkpoint_and_stop() {
        let pins = fujimod_emulator_devices::xp5::pins::Pins::shared();
        pins.lock().unwrap().set_input(0, 15, false);
        let mut s = Sink::new(0).with_finder(pins.clone());
        let mut h = RecordingHost::default();
        s.write(&mut Cx::new(&mut h, 0, 0, 0), 8, 4, 0xa832);
        let (_, at, token) = h.events.pop().unwrap();
        assert_eq!(at, 2_000_000);
        let mut restored = Sink::new(0).with_finder(pins.clone());
        restored.restore(&s.save()).unwrap();
        h.now = at;
        restored.event(&mut Cx::new(&mut h, 0, 0, 0), token);
        assert_eq!(pins.lock().unwrap().level(0, 15), 1);
        assert_eq!(
            h.irqs
                .iter()
                .filter(|(_, high)| *high)
                .map(|(irq, _)| *irq)
                .collect::<Vec<_>>(),
            vec![128]
        );
        restored.event(&mut Cx::new(&mut h, 0, 0, 0), token);
        assert_eq!(
            h.irqs
                .iter()
                .filter(|(_, high)| *high)
                .map(|(irq, _)| *irq)
                .collect::<Vec<_>>(),
            vec![128]
        );
        restored.write(&mut Cx::new(&mut h, 0, 0, 0), 8, 4, 0xac32);
        let (_, _, token) = h.events.pop().unwrap();
        restored.write(&mut Cx::new(&mut h, 0, 0, 0), 8, 4, 0xa000);
        restored.event(&mut Cx::new(&mut h, 0, 0, 0), token);
        assert_eq!(
            h.irqs
                .iter()
                .filter(|(_, high)| *high)
                .map(|(irq, _)| *irq)
                .collect::<Vec<_>>(),
            vec![128]
        );
    }
}
