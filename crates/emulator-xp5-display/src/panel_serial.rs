//! Descriptor3 at19e49a8 is SPI9, ff919000,16-bit CPOL/CPHA1/div16,
//! CS port15 pin13 is active-low; received words are discarded.
//! Model only this write-only panel transaction; no invented panel ID/readback.
use fujimod_emulator_devices::xp5::pins::SharedPins;
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[derive(Default, Serialize, Deserialize)]
struct State {
    control: u32,
    enable: u32,
    divider: u32,
    interrupt_enable: u32,
    causes: u32,
    pending: Option<u16>,
    rx: Option<u16>,
    generation: u64,
    words: Vec<u16>,
    transfers: u64,
    deadline: Option<u64>,
}
pub struct PanelSerial {
    s: State,
    pins: SharedPins,
    source_hz: u64,
    chip_select: (usize, usize),
}
impl PanelSerial {
    pub fn new(
        pins: SharedPins,
        chip_select: (usize, usize),
        source_hz: u64,
    ) -> Result<Self, String> {
        if chip_select.0 >= 20 || chip_select.1 >= 16 || source_hz == 0 {
            return Err("invalid panel serial wiring/clock".into());
        }
        Ok(Self {
            s: State::default(),
            pins,
            source_hz,
            chip_select,
        })
    }
    fn complete(&mut self, cx: &mut Cx<'_>, word: u16) {
        self.s.transfers += 1;
        if self.s.words.len() == 4096 {
            self.s.words.remove(0);
        }
        self.s.words.push(word);
        self.s.rx = Some(0); // Stock write-only path discards the dummy.
        cx.log(&format!("panel SPI9 tx{word:#06x}"));
    }
}
impl Device for PanelSerial {
    fn name(&self) -> &'static str {
        "spi9-write-only-panel"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, n: u8) -> u64 {
        let o = address.wrapping_sub(0xff919000);
        self.advance(cx);
        if n != 4 {
            cx.unmodeled();
            return 0;
        }
        match o {
            0 => self.s.control as u64,
            4 => self.s.enable as u64,
            8 => self.s.rx.take().unwrap_or(0) as u64,
            //1467c2c: TFE bit0, TNF bit1, RNE bit2, BSY bit4.
            0xc => {
                2 | u64::from(self.s.pending.is_none())
                    | (u64::from(self.s.rx.is_some()) << 2)
                    | (u64::from(self.s.pending.is_some()) << 4)
            }
            0x10 => self.s.divider as u64,
            0x14 => self.s.causes as u64,
            0x24 => self.s.interrupt_enable as u64,
            _ => {
                cx.unmodeled();
                0
            }
        }
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, n: u8, v: u64) {
        let o = address.wrapping_sub(0xff919000);
        self.advance(cx);
        if n != 4 {
            cx.unmodeled();
            return;
        }
        if self.s.pending.is_some() && o != 0x14 {
            cx.fail("SPI9 register changed during panel transfer");
            return;
        }
        match o {
            0 if v & !0xcf == 0 => self.s.control = v as u32,
            4 if v & !0xf == 0 => self.s.enable = v as u32,
            0x10 if (2..=254).contains(&v) && v & 1 == 0 => self.s.divider = v as u32,
            0x14 if v & !15 == 0 => self.s.causes &= !(v as u32),
            0x20 if v & !3 == 0 => {
                if v & 1 != 0 {
                    self.s.rx = None;
                }
            }
            0x24 if v == 0 => self.s.interrupt_enable = 0,
            8 if v <= 0xffff => {
                if self.s.control & 0xff != 0xcf || self.s.enable & 2 == 0 || self.s.divider == 0 {
                    cx.fail("unsupported/unconfigured SPI9 panel mode");
                    return;
                }
                if self.s.rx.is_some() {
                    cx.fail("SPI9 unread receive word");
                    return;
                }
                if self
                    .pins
                    .lock()
                    .unwrap()
                    .output_level(self.chip_select.0, self.chip_select.1)
                    != 0
                {
                    cx.fail("SPI9 panel requires its declared active-low GPIO chip select");
                    return;
                }
                self.s.pending = Some(v as u16);
                self.s.generation += 1;
                // Divider times16 bits at the explicitly declared serial source.
                // Source frequency is a model input, not a camera measurement.
                let ns = (self.s.divider as u64 * 16 * 1_000_000_000).div_ceil(self.source_hz);
                let Some(at) = cx.now().checked_add(ns) else {
                    cx.fail("panel serial deadline overflow");
                    self.s.pending = None;
                    return;
                };
                self.s.deadline = Some(at);
                cx.schedule(at, self.s.generation);
            }
            0 | 4 | 8 | 0x10 | 0x14 | 0x20 | 0x24 => {
                cx.fail(format!("unsupported SPI9 panel register{o:#x}={v:#x}"))
            }
            _ => cx.unmodeled(),
        }
    }
    fn event(&mut self, cx: &mut Cx<'_>, token: u64) {
        if token == self.s.generation {
            self.advance(cx);
        }
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        if self.s.deadline.is_some_and(|at| at <= cx.now()) {
            self.s.deadline = None;
            if let Some(word) = self.s.pending.take() {
                self.complete(cx, word);
            }
        }
    }
    fn next_deadline(&self, _: u64) -> Option<u64> {
        self.s.deadline
    }
    fn save(&self) -> Value {
        serde_json::to_value(&self.s).unwrap()
    }
    fn restore(&mut self, v: &Value) -> Result<(), String> {
        let state: State = serde_json::from_value(v.clone())
            .map_err(|e| format!("panel serial checkpoint: {e}"))?;
        if state.pending.is_some() != state.deadline.is_some() {
            return Err("panel serial pending word/deadline disagree".into());
        }
        self.s = state;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_xp5_io::gpio::Gpio;
    use fujimod_emulator_devices::xp5::pins::Pins;
    use fujimod_emulator_devices::test_host::recording::RecordingHost;
    const BASE: u64 = 0xff919000;
    #[test]
    fn panel_uses_gpio_drive_and_serial_clock_across_early_ticks_and_restore() {
        let pins = Pins::shared();
        let mut gpio = Gpio::new(0xff907000, pins.clone(), false, Vec::new()).unwrap();
        let mut panel = PanelSerial::new(pins.clone(), (15, 13), 48_000_000).unwrap();
        let mut host = RecordingHost {
            now: 100,
            ..Default::default()
        }; // No firmware RAM.
        {
            let mut cx = Cx::new(&mut host, 2, 0xabc, 0);
            for (o, v) in [(0, 0xcf), (4, 2), (0x10, 16), (8, 0x401)] {
                panel.write(&mut cx, BASE + o, 4, v);
            }
            assert_eq!(panel.read(&mut cx, BASE + 0xc, 4), 0x12);
            assert!(cx.refusal().is_none());
        }
        let saved = panel.save();
        let at = panel.next_deadline(100).unwrap();
        assert_eq!(at, 100 + 5334);
        let token = saved["generation"].as_u64().unwrap();
        host.now = at - 1;
        panel.event(&mut Cx::new(&mut host, 0, 0, 0), token);
        assert_eq!(panel.save(), saved);
        panel.restore(&saved).unwrap();
        host.now = at;
        panel.event(&mut Cx::new(&mut host, 0, 0, 0), token - 1);
        assert_eq!(panel.save(), saved);
        panel.advance(&mut Cx::new(&mut host, 0, 0, 0));
        assert_eq!(panel.next_deadline(at), None);
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        assert_eq!(panel.read(&mut cx, BASE + 0xc, 4), 7);
        assert_eq!(panel.read(&mut cx, BASE + 8, 4), 0);
        assert_eq!(panel.read(&mut cx, BASE + 0xc, 4), 3);
        assert_eq!(panel.save()["transfers"], 1);
        // Changing an input-sampling mode cannot select the panel. Its CS
        // wire follows the output-data latch written by the lower GPIO driver.
        gpio.write(&mut cx, 0xff9073c0, 4, (1 << 29) | (1 << 13));
        assert_eq!(pins.lock().unwrap().output_level(15, 13), 1);
        panel.write(&mut cx, BASE + 8, 4, 0x1234);
        assert!(cx.refusal().unwrap().contains("chip select"));
        assert_eq!(panel.next_deadline(at), None);
        assert_eq!(panel.save()["transfers"], 1);
    }
}
