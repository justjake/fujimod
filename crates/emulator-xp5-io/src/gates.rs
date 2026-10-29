//! Recovered reset/gate latches and request/status mirrors. Initial values
//! are explicitly zero session assumptions, not hardware reset measurements.
use fujimod_emulator_devices::{Cx, Device};
use serde_json::Value;
use std::collections::BTreeMap;
pub struct Gates {
    power: bool,
    configuration_word: Option<u32>,
    regs: BTreeMap<u64, u32>,
    panel_clocks: Option<fujimod_emulator_devices::xp5::panel_clocks::Clocks>,
    companion_cpu: Option<usize>,
}
impl Gates {
    pub fn new(power: bool) -> Self {
        Self::with_panel_clocks(power, None)
    }
    pub fn with_panel_clocks(
        power: bool,
        panel_clocks: Option<fujimod_emulator_devices::xp5::panel_clocks::Clocks>,
    ) -> Self {
        Self {
            power,
            configuration_word: None,
            regs: BTreeMap::new(),
            panel_clocks,
            companion_cpu: None,
        }
    }
    /// BIS reset bank10: c928 bit21 clear holds reset, set releases it.
    pub fn with_companion(mut self, cpu: Option<usize>) -> Self {
        self.companion_cpu = cpu;
        self
    }
    /// Board-supplied read-only configuration input; no reset value is inferred.
    pub fn with_configuration_word(mut self, value: Option<u32>) -> Self {
        self.configuration_word = value;
        self
    }
    fn drive_reset(&self, cx: &mut Cx<'_>) {
        if !self.power {
            if let Some(cpu) = self.companion_cpu {
                if let Err(e) = cx.reset_core(
                    cpu,
                    self.regs.get(&0x928).copied().unwrap_or(0) & 0x200000 == 0,
                ) {
                    cx.fail(e);
                }
            }
        }
    }
    fn latch(&self, o: u64) -> bool {
        if self.power {
            matches!(
                o,
                0x510 | 0xb00 | 0xb04 | 0xb08 | 0xb0c | 0xb10 | 0xb1c | 0xb44
            )
        } else {
            (0x300..=0x334).contains(&o)
                || (0x900..=0x944).contains(&o)
                || matches!(o, 0x150 | 0x154 | 0x600 | 0x604 | 0x608 | 0x700)
        }
    }
    fn status(&self, o: u64) -> Option<u64> {
        if self.power && matches!(o, 0xb20 | 0xb24 | 0xb28 | 0xb2c | 0xb30 | 0xb3c | 0xb64) {
            Some(o - 0x20)
        } else {
            None
        }
    }
}
impl Device for Gates {
    fn name(&self) -> &'static str {
        if self.power {
            "power-gates"
        } else {
            "clock-reset-gates"
        }
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, n: u8) -> u64 {
        let o = address.wrapping_sub(if self.power { 0xff90d000 } else { 0xff90c000 });
        if n != 4 || o & 3 != 0 {
            cx.unmodeled();
            return 0;
        }
        if !self.power && o == 4 {
            if let Some(value) = self.configuration_word {
                return u64::from(value);
            }
            cx.unmodeled();
            return 0;
        }
        let a = if self.latch(o) {
            o
        } else if let Some(a) = self.status(o) {
            a
        } else {
            cx.unmodeled();
            return 0;
        };
        self.regs.get(&a).copied().unwrap_or(0) as u64
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, n: u8, v: u64) {
        let o = address.wrapping_sub(if self.power { 0xff90d000 } else { 0xff90c000 });
        if n != 4 || o & 3 != 0 || !self.latch(o) {
            cx.unmodeled();
            return;
        }
        let previous = self.regs.insert(o, v as u32).unwrap_or(0);
        if !self.power && o == 0x928 && (previous ^ v as u32) & 0x200000 != 0 {
            self.drive_reset(cx);
        }
        if !self.power && o == 0x608 {
            if let Some(clocks) = &self.panel_clocks {
                clocks.selector(v as u32);
            }
        }
    }
    fn save(&self) -> Value {
        serde_json::to_value(&self.regs).unwrap()
    }
    fn restore(&mut self, v: &Value) -> Result<(), String> {
        self.regs = serde_json::from_value(v.clone())
            .map_err(|e| format!("invalid gate checkpoint: {e}"))?;
        // CPU reset state is restored by the framework with its CPU snapshot.
        if !self.power {
            if let Some(clocks) = &self.panel_clocks {
                clocks.selector(self.regs.get(&0x608).copied().unwrap_or(0));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::test_host::recording::RecordingHost;

    #[test]
    fn reset_output_is_explicit_and_checkpoint_reads_are_observational() {
        let mut gates = Gates::new(false).with_companion(Some(4));
        let mut host = RecordingHost::default();
        gates.write(&mut Cx::new(&mut host, 0, 0, 0), 0xff90c928, 4, 0x200000);
        gates.write(&mut Cx::new(&mut host, 0, 0, 0), 0xff90c928, 4, 0);
        assert_eq!(host.resets, [(4, false), (4, true)]);
        let state = gates.save();
        gates.restore(&state).unwrap();
        assert_eq!(state, gates.save());
        assert_eq!(host.resets, [(4, false), (4, true)]);
    }

    #[test]
    fn configuration_input_is_read_only_and_independent_of_requester() {
        for selector in 0..32 {
            let value = 0x9753_2460 | selector;
            let mut gates = Gates::new(false).with_configuration_word(Some(value));
            let mut host = RecordingHost::default();
            let saved = gates.save();
            for core in 0..4 {
                let mut cx = Cx::new(&mut host, core, 0, 0);
                assert_eq!(gates.read(&mut cx, 0xff90c004, 4), u64::from(value));
                assert!(!cx.is_unmodeled());
            }
            let mut cx = Cx::new(&mut host, 0, 0, 0);
            gates.write(&mut cx, 0xff90c004, 4, 0);
            assert!(cx.is_unmodeled());
            assert_eq!(
                gates.read(&mut Cx::new(&mut host, 0, 0, 0), 0xff90c004, 4),
                u64::from(value)
            );
            gates.restore(&saved).unwrap();
            assert_eq!(saved, gates.save());
            assert!(host.events.is_empty());
            assert!(host.irqs.is_empty());
        }
    }

    #[test]
    fn missing_input_and_unestablished_access_widths_refuse() {
        let mut host = RecordingHost::default();
        for input in [None, Some(31)] {
            let mut gates = Gates::new(false).with_configuration_word(input);
            for width in [1, 2, 4, 8] {
                let mut cx = Cx::new(&mut host, 0, 0, 0);
                gates.read(&mut cx, 0xff90c004, width);
                assert_eq!(cx.is_unmodeled(), input.is_none() || width != 4);
            }
        }
        let mut power = Gates::new(true).with_configuration_word(Some(31));
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        power.read(&mut cx, 0xff90d004, 4);
        assert!(cx.is_unmodeled());
    }
}
