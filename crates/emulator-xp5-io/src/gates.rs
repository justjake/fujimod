//! Recovered reset/gate latches and request/status mirrors. Initial values
//! are explicitly zero session assumptions, not hardware reset measurements.
use fujimod_emulator_devices::bus::FastReadWord;
use fujimod_emulator_devices::checkpoint::Checkpoint;
use fujimod_emulator_devices::{Cx, Device};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::{
    Arc,
    atomic::{AtomicU32, Ordering},
};
pub struct Gates {
    power: bool,
    configuration_word: Option<u32>,
    regs: BTreeMap<u64, u32>,
    mirrors: BTreeMap<u64, Arc<AtomicU32>>,
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
        let mut gates = Self {
            power,
            configuration_word: None,
            regs: BTreeMap::new(),
            mirrors: BTreeMap::new(),
            panel_clocks,
            companion_cpu: None,
        };
        for offset in (0u64..=0xb64).step_by(4) {
            if gates.latch(offset) {
                gates.mirrors.insert(offset, Arc::new(AtomicU32::new(0)));
            }
        }
        gates
    }
    fn base(&self) -> u64 {
        if self.power { 0xff90d000 } else { 0xff90c000 }
    }
    fn publish_mirrors(&self) {
        for (&offset, mirror) in &self.mirrors {
            mirror.store(
                self.regs.get(&offset).copied().unwrap_or(0),
                Ordering::Release,
            );
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
        if !self.power
            && let Some(cpu) = self.companion_cpu
            && let Err(e) = cx.reset_core(
                cpu,
                self.regs.get(&0x928).copied().unwrap_or(0) & 0x200000 == 0,
            )
        {
            cx.fail(e);
        }
    }
    fn latch(&self, o: u64) -> bool {
        if self.power {
            matches!(
                o,
                0x510 | 0xb00 | 0xb04 | 0xb08 | 0xb0c | 0xb10 | 0xb1c | 0xb44
            )
        } else {
            // 0x110: the fourth register write of the X-T5 boot header (0xff90c110 = 0xc, after
            // 0xff90c150 and 0xff90c154), which an SD card stage repeats. A latch like those two;
            // its meaning is not known.
            (0x300..=0x334).contains(&o)
                || (0x900..=0x944).contains(&o)
                || matches!(o, 0x110 | 0x150 | 0x154 | 0x600 | 0x604 | 0x608 | 0x700)
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
    fn fast_reads(&self, window_base: u64, window_size: u64) -> Vec<FastReadWord> {
        let Some(window_end) = window_base.checked_add(window_size) else {
            return Vec::new();
        };
        let base = self.base();
        let mut words = Vec::new();
        let mut push = |reg: u64, atom: Arc<AtomicU32>| {
            if let Some(address) = base.checked_add(reg)
                && let Some(end) = address.checked_add(4)
                && address >= window_base
                && end <= window_end
            {
                words.push(FastReadWord::atomic_u32(address - window_base, atom));
            }
        };
        if !self.power
            && let Some(value) = self.configuration_word
        {
            push(4, Arc::new(AtomicU32::new(value)));
        }
        for (&offset, mirror) in &self.mirrors {
            push(offset, mirror.clone());
        }
        if self.power {
            for offset in (0u64..=0xb64).step_by(4) {
                if let Some(latch) = self.status(offset)
                    && let Some(mirror) = self.mirrors.get(&latch)
                {
                    push(offset, mirror.clone());
                }
            }
        }
        words.sort_by_key(|word| word.offset);
        words
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, n: u8) -> u64 {
        let o = address.wrapping_sub(self.base());
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
        let o = address.wrapping_sub(self.base());
        if n != 4 || o & 3 != 0 || !self.latch(o) {
            cx.unmodeled();
            return;
        }
        let previous = self.regs.insert(o, v as u32).unwrap_or(0);
        if let Some(mirror) = self.mirrors.get(&o) {
            mirror.store(v as u32, Ordering::Release);
        }
        if !self.power && o == 0x928 && (previous ^ v as u32) & 0x200000 != 0 {
            self.drive_reset(cx);
        }
        if !self.power
            && o == 0x608
            && let Some(clocks) = &self.panel_clocks
        {
            clocks.selector(v as u32);
        }
    }
    fn checkpoint(&self) -> Result<Checkpoint, String> {
        Checkpoint::encode(self.name(), &self.regs)
    }
    fn validate_checkpoint(&self, saved: &Checkpoint) -> Result<(), String> {
        let registers: BTreeMap<u64, u32> = saved.decode(self.name())?;
        if registers.keys().any(|&offset| !self.latch(offset)) {
            return Err("clock gate snapshot has an unknown latch".into());
        }
        Ok(())
    }
    fn restore_checkpoint(&mut self, saved: &Checkpoint) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        self.regs = saved.decode(self.name())?;
        self.publish_mirrors();
        if !self.power && let Some(clocks) = &self.panel_clocks {
            clocks.selector(self.regs.get(&0x608).copied().unwrap_or(0));
        }
        Ok(())
    }
    fn save(&self) -> Value {
        serde_json::to_value(&self.regs).unwrap()
    }
    fn restore(&mut self, v: &Value) -> Result<(), String> {
        self.regs = serde_json::from_value(v.clone())
            .map_err(|e| format!("invalid gate checkpoint: {e}"))?;
        self.publish_mirrors();
        // CPU reset state is restored by the framework with its CPU snapshot.
        if !self.power
            && let Some(clocks) = &self.panel_clocks
        {
            clocks.selector(self.regs.get(&0x608).copied().unwrap_or(0));
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
    fn boot_header_register_writes_latch_and_other_offsets_stay_unmodeled() {
        let mut gates = Gates::new(false);
        let mut host = RecordingHost::default();
        for (offset, value) in [(0x150u64, 0x1fu64), (0x154, 0x1f), (0x110, 0xc)] {
            let mut cx = Cx::new(&mut host, 0, 0, 0);
            gates.write(&mut cx, 0xff90c000 + offset, 4, value);
            assert!(!cx.is_unmodeled(), "{offset:#x}");
            let mut cx = Cx::new(&mut host, 0, 0, 0);
            assert_eq!(gates.read(&mut cx, 0xff90c000 + offset, 4), value);
            assert!(!cx.is_unmodeled(), "{offset:#x}");
        }
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        gates.write(&mut cx, 0xff90c114, 4, 1);
        assert!(cx.is_unmodeled());
        let state = gates.save();
        gates.restore(&state).unwrap();
        assert_eq!(state, gates.save());
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

    #[test]
    fn fast_reads_cover_exact_valid_offsets_and_track_writes_and_restores() {
        use fujimod_emulator_devices::bus::FastReadValue;

        let mut host = RecordingHost::default();
        for (power, config_word) in [(false, None), (false, Some(0x1357_2468)), (true, Some(31))] {
            let mut gates = Gates::new(power).with_configuration_word(config_word);
            let base = gates.base();
            let fast: BTreeMap<u64, Arc<AtomicU32>> = gates
                .fast_reads(base, 0x1000)
                .into_iter()
                .map(|word| {
                    let FastReadValue::AtomicU32(atom) = word.value else {
                        panic!("expected AtomicU32 at {:#x}", word.offset);
                    };
                    (word.offset, atom)
                })
                .collect();

            // Every modeled offset in 0..0x1000 must be published, and no unmodeled offset may be.
            let assert_parity = |gates: &mut Gates, host: &mut RecordingHost| {
                for offset in (0u64..0x1000).step_by(4) {
                    let mut cx = Cx::new(host, 0, 0, 0);
                    let expected = gates.read(&mut cx, base + offset, 4);
                    if cx.is_unmodeled() {
                        assert!(!fast.contains_key(&offset), "power={power} offset={offset:#x}");
                    } else {
                        assert_eq!(
                            u64::from(fast[&offset].load(Ordering::Acquire)),
                            expected,
                            "power={power} offset={offset:#x}"
                        );
                    }
                }
            };
            assert_parity(&mut gates, &mut host);

            let initial_checkpoint = gates.checkpoint().unwrap();
            let initial_json = gates.save();
            let sample_writes: &[(u64, u64)] = if power {
                &[(0x510, 0x11), (0xb00, 0xaa), (0xb44, 0xbb)]
            } else {
                &[(0x110, 0xc), (0x304, 0x22), (0x928, 0x200000)]
            };
            for &(offset, value) in sample_writes {
                let mut cx = Cx::new(&mut host, 0, 0, 0);
                gates.write(&mut cx, base + offset, 4, value);
                assert!(!cx.is_unmodeled());
            }
            if power {
                assert_eq!(fast[&0xb20].load(Ordering::Acquire), 0xaa);
                assert_eq!(fast[&0xb64].load(Ordering::Acquire), 0xbb);
            }
            assert_parity(&mut gates, &mut host);

            gates.restore(&initial_json).unwrap();
            assert_parity(&mut gates, &mut host);

            let &(offset, value) = &sample_writes[0];
            let mut cx = Cx::new(&mut host, 0, 0, 0);
            gates.write(&mut cx, base + offset, 4, value);
            assert_parity(&mut gates, &mut host);

            gates.restore_checkpoint(&initial_checkpoint).unwrap();
            assert_parity(&mut gates, &mut host);
        }
    }
}
