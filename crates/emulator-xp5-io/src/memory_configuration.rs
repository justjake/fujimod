//! Bounded memory-parameter register banks and their configuration commit.
//!
//! The commit latency is a required functional input. This device does not
//! model physical training, retention memory, transfers, interrupts or resets.
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

const COMMIT: u64 = 0x704;
const WORDS: usize = 113;

#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct State {
    staged: BTreeMap<u64, u32>,
    committed: Option<BTreeMap<u64, u32>>,
    request: Option<u32>,
    pending: Option<(u64, u64)>,
    generation: u64,
}

pub struct MemoryConfiguration {
    base: u64,
    commit_ticks: u64,
    state: State,
}
impl MemoryConfiguration {
    pub fn new(base: u64, commit_ticks: u64) -> Result<Self, String> {
        if base & 0xfff != 0 || base.checked_add(0x1000).is_none() || commit_ticks == 0 {
            return Err("memory configuration needs a page and positive commit latency".into());
        }
        Ok(Self {
            base,
            commit_ticks,
            state: State::default(),
        })
    }
    fn parameter(offset: u64) -> bool {
        offset & 3 == 0
            && ([0, 0x200, 0x400, 0x500, 0x800]
                .into_iter()
                .any(|bank| (bank..=bank + 0x54).contains(&offset))
                || matches!(offset, 0x900 | 0x904 | 0x908))
    }
    fn programmed(words: &BTreeMap<u64, u32>) -> bool {
        words.len() == WORDS && words.keys().all(|offset| Self::parameter(*offset))
    }
}
impl Device for MemoryConfiguration {
    fn name(&self) -> &'static str {
        "memory-configuration"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let offset = address.wrapping_sub(self.base);
        if size == 4 && offset == COMMIT {
            if let Some(value) = self.state.request {
                return u64::from(value);
            }
        }
        cx.unmodeled();
        0
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let offset = address.wrapping_sub(self.base);
        if size != 4 || value > u64::from(u32::MAX) {
            cx.unmodeled();
            return;
        }
        if self.state.pending.is_some() {
            cx.fail("memory configuration is committing");
            return;
        }
        if Self::parameter(offset) {
            self.state.staged.insert(offset, value as u32);
            return;
        }
        if offset != COMMIT || value != 1 {
            cx.unmodeled();
            return;
        }
        if !Self::programmed(&self.state.staged) {
            cx.fail("memory configuration parameters are incomplete");
            return;
        }
        let Some(at) = cx.now().checked_add(self.commit_ticks) else {
            cx.fail("memory configuration deadline overflow");
            return;
        };
        let Some(generation) = self.state.generation.checked_add(1) else {
            cx.fail("memory configuration generation overflow");
            return;
        };
        self.state.generation = generation;
        self.state.pending = Some((at, generation));
        self.state.request = Some(1);
        cx.schedule(at, generation);
    }
    fn next_deadline(&self, _: u64) -> Option<u64> {
        self.state.pending.map(|p| p.0)
    }
    fn event(&mut self, cx: &mut Cx<'_>, token: u64) {
        let Some((at, generation)) = self.state.pending else {
            return;
        };
        if token != generation || cx.now() < at {
            return;
        }
        self.state.committed = Some(self.state.staged.clone());
        self.state.pending = None;
        self.state.request = Some(0);
    }
    fn pollable(&self, address: u64, size: u8) -> bool {
        let offset = address.wrapping_sub(self.base);
        size == 4 && offset == COMMIT && self.state.request.is_some()
    }
    fn save(&self) -> Value {
        serde_json::to_value(&self.state).unwrap()
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let state: State = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        if state.staged.keys().any(|offset| !Self::parameter(*offset))
            || state
                .committed
                .as_ref()
                .is_some_and(|words| !Self::programmed(words))
            || !matches!(state.request, None | Some(0 | 1))
            || state.request == Some(0) && state.committed.is_none()
            || state.committed.is_some() && state.generation == 0
            || state.pending.is_some_and(|(_, generation)| {
                generation != state.generation
                    || generation == 0
                    || state.request != Some(1)
                    || !Self::programmed(&state.staged)
            })
            || state.request == Some(1) && state.pending.is_none()
        {
            return Err("invalid memory configuration state".into());
        }
        self.state = state;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::test_host::recording::RecordingHost;

    fn program(device: &mut MemoryConfiguration, host: &mut RecordingHost) {
        for bank in [0, 0x200, 0x400, 0x500, 0x800] {
            for offset in (bank..bank + 0x58).step_by(4) {
                device.write(
                    &mut Cx::new(host, 0, 0, 0),
                    device.base + offset,
                    4,
                    offset ^ 0x1234,
                );
            }
        }
        for offset in [0x900, 0x904, 0x908] {
            device.write(&mut Cx::new(host, 0, 0, 0), device.base + offset, 4, 7);
        }
    }

    #[test]
    fn unknown_reset_state_widths_and_commands_refuse_without_effects() {
        let mut device = MemoryConfiguration::new(0x10000, 10).unwrap();
        let mut host = RecordingHost::default();
        for (offset, size) in [(0, 4), (COMMIT, 4), (0x100, 4), (0, 1), (0, 8)] {
            let mut cx = Cx::new(&mut host, 0, 0, 0);
            device.read(&mut cx, device.base + offset, size);
            assert!(cx.is_unmodeled());
        }
        for (offset, size, value) in [(0x100, 4, 0), (0, 1, 0), (COMMIT, 4, 2)] {
            let mut cx = Cx::new(&mut host, 0, 0, 0);
            device.write(&mut cx, device.base + offset, size, value);
            assert!(cx.is_unmodeled());
        }
        let before = device.save();
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        device.write(&mut cx, device.base + COMMIT, 4, 1);
        assert_eq!(
            cx.refusal(),
            Some("memory configuration parameters are incomplete")
        );
        assert_eq!(before, device.save());
        assert!(host.events.is_empty() && host.irqs.is_empty() && host.ram.is_empty());
    }

    #[test]
    fn commit_completes_only_at_its_declared_deadline_and_snapshots_are_observational() {
        let mut device = MemoryConfiguration::new(0x10000, 10).unwrap();
        let mut host = RecordingHost::default();
        program(&mut device, &mut host);
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        device.read(&mut cx, 0x10000, 4);
        assert!(cx.is_unmodeled());
        device.write(&mut Cx::new(&mut host, 0, 0, 0), 0x10704, 4, 1);
        assert_eq!(host.events, [(0, 10, 1)]);
        let pending = device.save();
        assert_eq!(device.save(), pending);
        host.now = 9;
        device.event(&mut Cx::new(&mut host, 1, 0, 0), 1);
        assert_eq!(device.read(&mut Cx::new(&mut host, 2, 0, 0), 0x10704, 4), 1);
        host.now = 10;
        device.event(&mut Cx::new(&mut host, 1, 0, 0), 2);
        assert_eq!(device.next_deadline(10), Some(10));
        device.restore(&pending).unwrap();
        assert_eq!(device.save(), pending);
        device.event(&mut Cx::new(&mut host, 1, 0, 0), 1);
        assert_eq!(device.read(&mut Cx::new(&mut host, 3, 0, 0), 0x10704, 4), 0);
        assert_eq!(device.next_deadline(10), None);
        let committed = device.save();
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        device.write(&mut cx, 0x10704, 4, 2);
        assert!(cx.is_unmodeled());
        assert_eq!(device.save(), committed);
        assert!(host.irqs.is_empty() && host.ram.is_empty());
    }

    #[test]
    fn banks_are_independent_and_cannot_change_during_commit() {
        let mut a = MemoryConfiguration::new(0x10000, 10).unwrap();
        let mut b = MemoryConfiguration::new(0x20000, 20).unwrap();
        let mut host = RecordingHost::default();
        program(&mut a, &mut host);
        program(&mut b, &mut host);
        a.write(&mut Cx::new(&mut host, 0, 0, 0), 0x10704, 4, 1);
        let before = a.save();
        let mut cx = Cx::new(&mut host, 1, 0, 0);
        a.write(&mut cx, 0x10000, 4, 0);
        assert_eq!(cx.refusal(), Some("memory configuration is committing"));
        assert_eq!(a.save(), before);
        assert_eq!(b.next_deadline(0), None);
        b.write(&mut Cx::new(&mut host, 2, 0, 1), 0x20704, 4, 1);
        assert_eq!(host.events, [(0, 10, 1), (1, 20, 1)]);
        host.now = 10;
        a.event(&mut Cx::new(&mut host, 0, 0, 0), 1);
        b.event(&mut Cx::new(&mut host, 0, 0, 1), 1);
        assert_eq!(a.next_deadline(10), None);
        assert_eq!(b.next_deadline(10), Some(20));
    }

    #[test]
    fn malformed_checkpoint_does_not_replace_current_state() {
        let mut device = MemoryConfiguration::new(0x10000, 10).unwrap();
        let before = device.save();
        let mut invalid = before.clone();
        invalid["request"] = serde_json::json!(0);
        assert!(device.restore(&invalid).is_err());
        assert_eq!(before, device.save());
        invalid = before.clone();
        invalid["staged"] = serde_json::json!({"256": 0});
        assert!(device.restore(&invalid).is_err());
        assert_eq!(before, device.save());
    }
}
