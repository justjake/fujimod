//! X-Processor Pro display timing banks, reusing the EXR II timing law.
//! The banks keep the EXR II register layout at board-declared bases.
//! No clock gate is modeled: the cadence starts once timing is programmed.
use fujimod_emulator_devices::{Cx, Device, Record};
use fujimod_emulator_exr2_display::timing::{Timing, TimingHost, address_in};
use serde_json::Value;

pub struct XppTiming {
    state: Timing,
}

struct Host<'a, 'b>(&'a mut Cx<'b>);

impl TimingHost for Host<'_, '_> {
    fn enabled(&self) -> [u128; 2] {
        // The GIC distributor routes; the timing core sees one target.
        [u128::MAX, 0]
    }
    fn clock_ready(&mut self) -> fujimod_emulator_devices::record::Result<bool> {
        Ok(true)
    }
    fn record(&mut self, record: Record) -> fujimod_emulator_devices::record::Result<()> {
        self.0.record(record);
        Ok(())
    }
    fn queue_irq(&mut self, _: u8, line: u8) -> fujimod_emulator_devices::record::Result<()> {
        self.0.set_irq(line.into(), true);
        self.0.set_irq(line.into(), false);
        Ok(())
    }
}

impl XppTiming {
    /// `irqs` are each bank's GIC lines for phases 0, 1 and 2.
    pub fn new(bases: [u64; 2], irqs: [[u8; 3]; 2], frame_ticks: u64, stop_ticks: u64) -> Result<Self, String> {
        if bases[0] & 0xfff != 0
            || bases[1] & 0xfff != 0
            || bases[0].abs_diff(bases[1]) < 0x1000
            || irqs.iter().flatten().any(|l| *l < 32)
            || !(4..=i64::MAX as u64 / 3).contains(&frame_ticks)
            || !(1..=i64::MAX as u64).contains(&stop_ticks)
        {
            return Err("Invalid X-Processor Pro display timing wiring".into());
        }
        let mut state = Timing::new(Some(frame_ticks as i64), stop_ticks as i64, "configured-clocked".into(), 0);
        state.bases = bases;
        state.irqs = irqs;
        Ok(Self { state })
    }
    pub fn windows(&self) -> [(u64, u64); 2] {
        self.state.bases.map(|b| (b, b + 0x1000))
    }
}

impl Device for XppTiming {
    fn name(&self) -> &'static str {
        "xpp-display-timing"
    }
    fn read(&mut self, cx: &mut Cx<'_>, a: u64, size: u8) -> u64 {
        if address_in(&self.state.bases, a, size.into()).is_err() {
            cx.unmodeled();
            return 0;
        }
        let now = cx.now() as i64;
        match self.state.read(a, size.into(), now, false, &mut Host(cx)) {
            Ok(value) => value.into(),
            Err(error) => {
                cx.fail(error.0);
                0
            }
        }
    }
    fn write(&mut self, cx: &mut Cx<'_>, a: u64, size: u8, value: u64) {
        if address_in(&self.state.bases, a, size.into()).is_err() {
            cx.unmodeled();
            return;
        }
        let (now, cpu) = (cx.now() as i64, cx.cpu as i64);
        if let Err(error) = self.state.write(cpu, a, size.into(), value.into(), now, false, &mut Host(cx)) {
            cx.fail(error.0);
        }
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        let now = cx.now() as i64;
        if self
            .state
            .deadline_with_clock(now, true)
            .is_some_and(|due| due <= now)
            && let Err(error) = self.state.tick(now, false, &mut Host(cx))
        {
            cx.fail(error.0);
        }
    }
    fn next_deadline(&self, now: u64) -> Option<u64> {
        self.state.deadline_with_clock(now as i64, true).map(|v| v as u64)
    }
    fn save(&self) -> Value {
        serde_json::to_value(&self.state).unwrap()
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let mut state: Timing = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        if state.frame_period_ticks != self.state.frame_period_ticks
            || state.stop_delay_ticks != self.state.stop_delay_ticks
            || state.banks.iter().any(|b| b.next_phase_index >= 3)
        {
            return Err("Invalid X-Processor Pro display timing checkpoint".into());
        }
        (state.bases, state.irqs) = (self.state.bases, self.state.irqs);
        self.state = state;
        Ok(())
    }
}
