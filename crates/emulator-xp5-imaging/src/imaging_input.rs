//! XP5 receiver PHY and input-engine ready gates.
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Default, Serialize, Deserialize)]
pub struct Phy {
    regs: BTreeMap<u64, u32>,
    #[serde(default)]
    generation: u64,
    deadline: Option<u64>,
}
impl Phy {
    fn known(o: u64) -> bool {
        matches!(o, 0 | 4 | 8 | 12) || (0x100..0x500).contains(&o) && o % 0x100 <= 0x48
    }
}
impl Device for Phy {
    fn name(&self) -> &'static str {
        "imaging-phy"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, n: u8) -> u64 {
        let o = address.wrapping_sub(0xff944000);
        self.advance(cx);
        if n != 4 || o & 3 != 0 || !Self::known(o) {
            cx.unmodeled();
            return 0;
        }
        u64::from(self.regs.get(&o).copied().unwrap_or(0))
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, n: u8, v: u64) {
        let o = address.wrapping_sub(0xff944000);
        self.advance(cx);
        if n != 4 || o & 3 != 0 || !Self::known(o) {
            cx.unmodeled();
            return;
        }
        self.regs.insert(o, v as u32);
        if o == 0 {
            // 319a338 writes the control word, then waits for bit0 to become1.
            // Functional reset/ready completion; the silicon latency is unknown.
            self.regs.insert(0, v as u32 & !1);
            self.generation = self.generation.wrapping_add(1);
            self.deadline = Some(cx.now() + 100_000);
            cx.schedule(self.deadline.unwrap(), self.generation);
        }
    }
    fn event(&mut self, cx: &mut Cx<'_>, token: u64) {
        if token == self.generation {
            self.advance(cx);
        }
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        if self.deadline.is_some_and(|at| at <= cx.now()) {
            self.deadline = None;
            *self.regs.entry(0).or_default() |= 1;
        }
    }
    fn next_deadline(&self, _: u64) -> Option<u64> {
        self.deadline
    }
    fn save(&self) -> Value {
        serde_json::to_value(self).unwrap()
    }
    fn restore(&mut self, v: &Value) -> Result<(), String> {
        *self = serde_json::from_value(v.clone())
            .map_err(|e| format!("receiver PHY checkpoint: {e}"))?;
        Ok(())
    }
}

/// ff946000 input engine reset/ready gate (2f37170/2f3719c).
/// Other input-engine configuration remains visible in explore reports.
#[derive(Default, Serialize, Deserialize)]
pub struct Input {
    ready: bool,
    generation: u64,
    #[serde(default)]
    running: bool,
    #[serde(default)]
    pending: u32,
    #[serde(default)]
    frames: u64,
    deadline: Option<u64>,
    frame_completion: bool,
}
impl Device for Input {
    fn name(&self) -> &'static str {
        "imaging-input"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let offset = address.wrapping_sub(0xff946000);
        self.advance(cx);
        if size != 4 {
            cx.unmodeled();
            return 0;
        }
        match offset {
            0 => u64::from(self.ready || self.generation == 0),
            4 => u64::from(self.running),
            0x10c => u64::from(self.pending),
            _ => {
                cx.unmodeled();
                0
            }
        }
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let offset = address.wrapping_sub(0xff946000);
        self.advance(cx);
        if size == 4 && offset == 0 {
            self.frame_completion = false;
            self.ready = value & 1 != 0;
            self.generation = self.generation.wrapping_add(1);
            self.deadline = None;
            if !self.ready {
                self.deadline = Some(cx.now() + 100_000);
                cx.schedule(self.deadline.unwrap(), self.generation);
            }
        } else if size == 4 && offset == 4 {
            self.frame_completion = true;
            self.generation = self.generation.wrapping_add(1);
            self.deadline = None;
            self.running = value & 1 != 0;
            if self.running {
                self.deadline = Some(cx.now() + 1_000_000);
                cx.schedule(self.deadline.unwrap(), self.generation | (1 << 63));
            }
        } else if size == 4 && offset == 0x10c {
            // 318bc5c clears every pending cause by writing sixteen zero flags.
            self.pending &= value as u32 & 0xffff;
            cx.set_irq(229, self.pending != 0);
        } else {
            cx.unmodeled();
        }
    }
    fn event(&mut self, cx: &mut Cx<'_>, token: u64) {
        if token & !(1 << 63) == self.generation {
            self.advance(cx);
        }
    }
    fn next_deadline(&self, _: u64) -> Option<u64> {
        self.deadline
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        if !self.deadline.is_some_and(|at| at <= cx.now()) {
            return;
        }
        self.deadline = None;
        if self.frame_completion && self.running {
            self.running = false;
            self.pending |= 1;
            self.frames += 1;
            // Descriptor35eff84=14 ->19e78c0 ->INTID229. Bit0 maps to
            // 331ebb4 status[15] ->331ea44 ->stock event100 in3320c10.
            cx.set_irq(229, true);
        } else {
            self.ready = true;
        }
    }
    fn save(&self) -> Value {
        serde_json::to_value(self).unwrap()
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        *self = serde_json::from_value(value.clone())
            .map_err(|e| format!("input engine checkpoint: {e}"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::test_host::recording::RecordingHost;
    fn irq(host: &RecordingHost, line: u32) -> bool {
        host.irqs
            .iter()
            .rev()
            .find(|(n, _)| *n == line)
            .is_some_and(|(_, v)| *v)
    }
    #[test]
    fn phy_and_input_reset_wait_for_their_guest_deadlines() {
        let mut phy = Phy::default();
        let mut input = Input::default();
        let mut host = RecordingHost::default();
        phy.write(&mut Cx::new(&mut host, 0, 0, 0), 0xff944000, 4, 0);
        input.write(&mut Cx::new(&mut host, 0, 0, 0), 0xff946000, 4, 0);
        host.now = 99_999;
        phy.advance(&mut Cx::new(&mut host, 0, 0, 0));
        input.advance(&mut Cx::new(&mut host, 0, 0, 0));
        assert_eq!(
            phy.read(&mut Cx::new(&mut host, 0, 0, 0), 0xff944000, 4) & 1,
            0
        );
        assert_eq!(
            input.read(&mut Cx::new(&mut host, 0, 0, 0), 0xff946000, 4),
            0
        );
        host.now = 100_000;
        assert_eq!(
            phy.read(&mut Cx::new(&mut host, 0, 0, 0), 0xff944000, 4) & 1,
            1
        );
        assert_eq!(
            input.read(&mut Cx::new(&mut host, 0, 0, 0), 0xff946000, 4),
            1
        );
        input.write(&mut Cx::new(&mut host, 0, 0, 0), 0xff946004, 4, 1);
        host.now = 1_099_999;
        input.advance(&mut Cx::new(&mut host, 0, 0, 0));
        assert!(!irq(&host, 229));
        host.now = 1_100_000;
        input.advance(&mut Cx::new(&mut host, 0, 0, 0));
        assert!(irq(&host, 229));
        input.write(&mut Cx::new(&mut host, 0, 0, 0), 0xff94610c, 4, 0);
        assert!(!irq(&host, 229));
        // Reset replaces a pending frame completion, rather than delivering
        // the canceled job's interrupt at the reset deadline.
        input.write(&mut Cx::new(&mut host, 0, 0, 0), 0xff946004, 4, 1);
        input.write(&mut Cx::new(&mut host, 0, 0, 0), 0xff946000, 4, 0);
        host.now = 1_200_000;
        input.advance(&mut Cx::new(&mut host, 0, 0, 0));
        assert_eq!(
            input.read(&mut Cx::new(&mut host, 0, 0, 0), 0xff946000, 4),
            1
        );
        assert!(!irq(&host, 229));
        let mut paf = crate::imaging::paf::Paf::default();
        paf.write(&mut Cx::new(&mut host, 0, 0, 0), 0xff976000, 4, 0);
        host.now = 1_299_999;
        assert_eq!(
            paf.read(&mut Cx::new(&mut host, 0, 0, 0), 0xff976000, 4) & 0x10000,
            0
        );
        host.now = 1_300_000;
        assert_eq!(
            paf.read(&mut Cx::new(&mut host, 0, 0, 0), 0xff976000, 4) & 0x10000,
            0x10000
        );
    }
}
