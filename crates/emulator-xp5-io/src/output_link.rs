//! Polled display-output setup at 0xff9a1000.
//! Functional reset completion only; no physical link latency is claimed.
use fujimod_emulator_devices::{Cx, Device};
use serde_json::Value;

pub const BASE: u64 = 0xff9a1000;

#[derive(Default)]
pub struct Link {
    regs: [u32; 7],
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::test_host::recording::RecordingHost;

    #[test]
    fn absolute_window_checkpoint_and_refusal() {
        let mut host = RecordingHost::default();
        let mut link = Link::default();
        let mut cx = Cx::new(&mut host, 0, 0, 3);
        link.write(&mut cx, BASE + 8, 4, 0x12345678);
        assert_eq!(cx.refusal(), None);
        let state = link.save();
        assert_eq!(state, link.save());
        let mut restored = Link::default();
        restored.restore(&state).unwrap();
        assert_eq!(restored.read(&mut cx, BASE + 8, 4), 0x12345678);
        assert_eq!(restored.read(&mut cx, BASE, 4), 1);
        assert!(restored.pollable(BASE, 4));
        assert!(!restored.pollable(BASE, 1));
        assert!(restored.restore(&serde_json::json!([1])).is_err());
        assert_eq!(restored.save(), state);
        restored.read(&mut cx, BASE - 4, 4);
        assert!(cx.is_unmodeled());
        assert_eq!(cx.refusal(), Some("unmodeled register"));
    }
}

impl Device for Link {
    fn name(&self) -> &'static str {
        "display-output-link"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let Some(offset) = address
            .checked_sub(BASE)
            .filter(|o| size == 4 && o & 3 == 0 && *o <= 0x18)
        else {
            cx.unmodeled();
            return 0;
        };
        if offset == 0 {
            1
        } else {
            self.regs[offset as usize / 4] as u64
        }
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let Some(offset) = address
            .checked_sub(BASE)
            .filter(|o| size == 4 && o & 3 == 0 && *o <= 0x18 && (*o != 0 || value == 0))
        else {
            cx.unmodeled();
            return;
        };
        self.regs[offset as usize / 4] = value as u32;
    }
    fn pollable(&self, address: u64, size: u8) -> bool {
        size == 4 && address == BASE
    }
    fn save(&self) -> Value {
        serde_json::json!(self.regs)
    }
    fn restore(&mut self, state: &Value) -> Result<(), String> {
        self.regs = serde_json::from_value(state.clone())
            .map_err(|e| format!("invalid output link checkpoint: {e}"))?;
        Ok(())
    }
}
