use crate::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Keyed IO-group control adjacent to the GPIO HAL.
/// XP4 1070024/107060c/10706c8 and XP5 146b030..146b607 share this law.
/// Identity is inferred from its bank enable/disable and 32 mux-byte fields;
/// no watchdog counter or timeout programming is present in these drivers.
#[derive(Default, Serialize, Deserialize)]
struct State {
    command: u32,
    status: u32,
    active: u32,
    enable: u32,
    disable: u32,
    routes: [u32; 8],
}
pub struct KeyedIoGroup {
    base: u64,
    state: State,
}
impl KeyedIoGroup {
    pub fn new(base: u64) -> Self {
        Self {
            base,
            state: State::default(),
        }
    }
}
impl Device for KeyedIoGroup {
    fn name(&self) -> &'static str {
        "keyed-io-group-control"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let offset = address.wrapping_sub(self.base);
        if size != 4 || offset & 3 != 0 {
            cx.unmodeled();
            return 0;
        }
        match offset {
            0 => self.state.command as u64,
            0xc => self.state.status as u64,
            0x18 => self.state.active as u64,
            0x20 => self.state.enable as u64,
            0x40 => self.state.disable as u64,
            0x80..=0x9c => self.state.routes[((offset - 0x80) / 4) as usize] as u64,
            _ => {
                cx.unmodeled();
                0
            }
        }
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let offset = address.wrapping_sub(self.base);
        if size != 4 || offset & 3 != 0 {
            cx.unmodeled();
            return;
        }
        let v = value as u32;
        match offset {
            0 => {
                if v & 0x7fff_ffff != 0xaa01 && v & 0x7fff_ffff != 0xaa02 {
                    cx.fail(format!("unmodeled keyed IO command {v:#x}"));
                    return;
                }
                self.state.command = v;
                self.state.active = (self.state.active | self.state.enable) & !self.state.disable;
                // Command aa01 sets bit4; command aa02 sets bit5. Preserve
                // the high command bit in the retained register value.
                self.state.status |= if v & 0xff == 1 { 1 << 4 } else { 1 << 5 };
            }
            0x10 => self.state.status &= !v, // W1C before and after each command
            0x20 => self.state.enable = v,
            0x40 => self.state.disable = v,
            0x80..=0x9c => {
                if v & 0xe0e0_e0e0 != 0 {
                    cx.fail("IO route fields exceed five bits");
                    return;
                }
                self.state.routes[((offset - 0x80) / 4) as usize] = v;
            }
            _ => cx.unmodeled(),
        }
    }
    fn save(&self) -> Value {
        serde_json::to_value(&self.state).unwrap()
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        self.state = serde_json::from_value(value.clone())
            .map_err(|e| format!("invalid keyed IO checkpoint: {e}"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_host::recording::RecordingHost;
    #[test]
    fn masks_routes_and_acknowledgments_are_independent_of_program_and_base() {
        let mut results = Vec::new();
        for (base, cpu, pc) in [(0xfff42000, 0, 0x1070450), (0xff904000, 3, 0xdeadbeef)] {
            let mut h = RecordingHost::default();
            let mut d = KeyedIoGroup::new(base);
            for (offset, value) in [
                (0x20, 0x3c008),
                (0x40, 0x20),
                (0x80, 0x1f030201),
                (0, 0xaa01),
            ] {
                let mut cx = Cx::new(&mut h, cpu, pc, 0);
                d.write(&mut cx, base + offset, 4, value);
                assert_eq!(cx.refusal(), None);
            }
            assert_eq!(
                d.read(&mut Cx::new(&mut h, cpu, pc, 0), base + 0xc, 4),
                0x10
            );
            assert_eq!(
                d.read(&mut Cx::new(&mut h, cpu, pc, 0), base + 0x18, 4),
                0x3c008
            );
            d.write(&mut Cx::new(&mut h, cpu, pc, 0), base + 0x10, 4, 0x10);
            d.write(&mut Cx::new(&mut h, cpu, pc, 0), base + 0x40, 4, 8);
            d.write(&mut Cx::new(&mut h, cpu, pc, 0), base, 4, 0x8000aa02);
            assert_eq!(
                d.read(&mut Cx::new(&mut h, cpu, pc, 0), base + 0xc, 4),
                0x20
            );
            assert_eq!(
                d.read(&mut Cx::new(&mut h, cpu, pc, 0), base + 0x18, 4),
                0x3c000
            );
            let saved = d.save();
            assert_eq!(d.save(), saved);
            d.restore(&saved).unwrap();
            assert_eq!(d.save(), saved);
            assert_eq!(d.next_deadline(h.now), None);
            let mut cx = Cx::new(&mut h, cpu, pc, 0);
            d.write(&mut cx, base + 0x80, 4, 0x20000000);
            assert!(cx.refusal().unwrap().contains("five bits"));
            assert_eq!(d.save(), saved);
            assert!(h.events.is_empty());
            assert!(h.irqs.is_empty());
            results.push(saved);
        }
        assert_eq!(results[0], results[1]);
    }
}
