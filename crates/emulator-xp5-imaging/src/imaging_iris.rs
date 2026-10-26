//! Functional SPI5 aperture pulse completion at the stock IRQ boundary.
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::VecDeque;

#[derive(Default, Serialize, Deserialize)]
struct Motion {
    generation: u64,
    armed: bool,
    word: u16,
    completions: u64,
    #[serde(default)]
    batches: VecDeque<u16>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::test_host::recording::RecordingHost;
    #[test]
    fn stock_three_batch_command_survives_restore_and_stop_cancels() {
        let mut iris = Iris::new(crate::serial_sink::Sink::new(0));
        let mut host = RecordingHost::default();
        for word in [0x2c00, 0x2c04, 0x2cc4, 0x2c04] {
            iris.write(&mut Cx::new(&mut host, 0, 0, 0), 8, 4, word);
        }
        let mut restored = Iris::new(crate::serial_sink::Sink::new(0));
        restored.restore(&iris.save()).unwrap();
        while !host.events.is_empty() {
            let (_, at, token) = host.events.remove(0);
            host.now = at;
            restored.event(&mut Cx::new(&mut host, 0, 0, 0), token);
        }
        assert_eq!(
            host.irqs
                .iter()
                .filter(|(_, high)| *high)
                .map(|(irq, _)| *irq)
                .collect::<Vec<_>>(),
            [125, 125, 125]
        );
        assert_eq!(host.now, 7_080_000);
        restored.write(&mut Cx::new(&mut host, 0, 0, 0), 8, 4, 0x2cc4);
        restored.write(&mut Cx::new(&mut host, 0, 0, 0), 8, 4, 0x2c04);
        let (_, _, token) = host.events.remove(0);
        restored.write(&mut Cx::new(&mut host, 0, 0, 0), 8, 4, 0x2400);
        restored.event(&mut Cx::new(&mut host, 0, 0, 0), token);
        assert_eq!(
            host.irqs
                .iter()
                .filter(|(_, high)| *high)
                .map(|(irq, _)| *irq)
                .collect::<Vec<_>>(),
            [125, 125, 125]
        );
        assert!(host.events.is_empty());
    }
}

pub struct Iris {
    serial: crate::serial_sink::Sink,
    motion: Motion,
}
impl Iris {
    pub fn new(serial: crate::serial_sink::Sink) -> Self {
        Self {
            serial,
            motion: Motion::default(),
        }
    }
    fn schedule(&mut self, cx: &mut Cx<'_>) {
        let word = *self.motion.batches.front().expect("iris batch queue empty");
        self.motion.word = word;
        self.motion.armed = true;
        cx.schedule(
            cx.now() + (1000 + u64::from(word & 0x3ff) * 20) * 1000,
            self.motion.generation,
        );
    }
}
impl Device for Iris {
    fn name(&self) -> &'static str {
        "virtual-audio-spi"
    }
    fn read(&mut self, cx: &mut Cx<'_>, offset: u64, size: u8) -> u64 {
        self.serial.read(cx, offset, size)
    }
    fn write(&mut self, cx: &mut Cx<'_>, offset: u64, size: u8, value: u64) {
        self.serial.write(cx, offset, size, value);
        let offset = offset.wrapping_sub(self.serial.base());
        let word = value as u16;
        if offset != 8 || size != 4 || word & 0xf000 != 0x2000 {
            return;
        }
        // 164933c fields b/c/d: enable11, direction10, pulse count low10.
        // The firmware first writes2c00, then finite2c04/2cc4 batches.
        let pulses = word & 0x3ff;
        if word & 0x800 == 0 || pulses == 0 {
            self.motion.generation = self.motion.generation.wrapping_add(1);
            self.motion.armed = false;
            self.motion.batches.clear();
            self.motion.word = word;
            return;
        }
        if self.motion.batches.len() >= 0x44 {
            cx.fail("iris pulse batch queue exceeds stock command capacity");
            return;
        }
        // The command queues four, 196, and four pulse batches before the first
        // end IRQ. Preserve each command so every batch completes.
        self.motion.batches.push_back(word);
        if !self.motion.armed {
            self.motion.generation = self.motion.generation.wrapping_add(1);
            self.schedule(cx);
        }
        cx.log(&format!(
            "virtual iris pulse batch {word:04x} pulses{pulses}"
        ));
    }
    fn event(&mut self, cx: &mut Cx<'_>, token: u64) {
        if !self.motion.armed || token != self.motion.generation {
            return;
        }
        self.motion.armed = false;
        self.motion.batches.pop_front();
        self.motion.completions += 1;
        // Logical interrupt14 routes to GIC125. Completion advances the pulse
        // count and cancels the watchdog.
        cx.set_irq(125, true);
        cx.set_irq(125, false);
        cx.log(&format!(
            "virtual iris batch {:04x} complete IRQ125",
            self.motion.word
        ));
        if !self.motion.batches.is_empty() {
            self.schedule(cx);
        }
    }
    fn save(&self) -> Value {
        serde_json::json!({"serial": self.serial.save(), "iris": self.motion})
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let motion = serde_json::from_value(value["iris"].clone())
            .map_err(|e| format!("invalid iris checkpoint: {e}"))?;
        self.serial.restore(&value["serial"])?;
        self.motion = motion;
        Ok(())
    }
}
