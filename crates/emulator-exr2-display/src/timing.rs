use fujimod_emulator_devices::bus::FastReadWord;
use fujimod_emulator_devices::record::Result;
use fujimod_emulator_devices::{Record, Unsupported, Value, WordMap};
use serde::{Deserialize, Serialize};
use std::sync::{
    Arc,
    atomic::{AtomicU32, Ordering},
};

pub const BASES: [u64; 2] = [0xfff6a000, 0xfff6d000];
pub const EXTERNAL: &str = "declared-external-phase-clock-unmeasured";
/// The external logic wire that holds the output boundary interrupt while high.
pub const OUTPUT_HOLD_WIRE: &str = "display_output_hold";
/// Profile parsing happens at the host boundary, never on register accesses.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum PhaseProfile {
    Bit16,
    ConfiguredClocked,
    External,
    Other(String),
}
impl PhaseProfile {
    pub fn from_name(name: String) -> Self {
        match name.as_str() {
            "bit16" => Self::Bit16,
            "configured-clocked" => Self::ConfiguredClocked,
            EXTERNAL => Self::External,
            _ => Self::Other(name),
        }
    }
    pub fn as_str(&self) -> &str {
        match self {
            Self::Bit16 => "bit16",
            Self::ConfiguredClocked => "configured-clocked",
            Self::External => EXTERNAL,
            Self::Other(name) => name,
        }
    }
}
pub const MASKS: [(u32, u32); 14] = [
    (0, 0x101ff),
    (4, 0x0fff0fff),
    (8, 0x1fff1fff),
    (0xc, 0x0fff0fff),
    (0x10, 0x1fff1fff),
    (0x14, 0x0fff0fff),
    (0x18, 0xff),
    (0x1c, 0x1fff),
    (0x20, 0x0fff0fff),
    (0x24, 0x1fff),
    (0x28, 0x0fff0fff),
    (0x2c, 0x1fff),
    (0x34, 0x1fff0007),
    (0x38, 0x07ff0fff),
];
pub const IRQS: [[u8; 3]; 2] = [[0x34, 0x34, 0x35], [0x37, 0x37, 0x38]];

pub fn address(address: u64, size: u32) -> Result<(usize, u32)> {
    address_in(&BASES, address, size)
}

/// Bank decode for a board that places the two timing banks elsewhere.
pub fn address_in(bases: &[u64; 2], address: u64, size: u32) -> Result<(usize, u32)> {
    for (i, base) in bases.iter().enumerate() {
        if (*base..*base + 0x1000).contains(&address) {
            let o = (address - base) as u32;
            if size != 4
                || o & 3 != 0
                || (o != 0x30 && !MASKS.iter().any(|(k, _)| *k == o))
                || i == 0 && (o == 0x24 || o == 0x28)
            {
                return Err(Unsupported(format!(
                    "Unknown display timing register/access {address:#x}/{size}"
                )));
            }
            return Ok((i, o));
        }
    }
    Err(Unsupported(format!(
        "Unknown display timing bank {address:#x}"
    )))
}

/// Dependencies are read only where their values are needed; notably a clock
/// read can settle SystemControl and record before the cadence-change record.
pub trait TimingHost {
    fn enabled(&self) -> [u128; 2];
    fn clock_ready(&mut self) -> Result<bool>;
    fn record(&mut self, record: Record) -> Result<()>;
    fn queue_irq(&mut self, cpu: u8, line: u8) -> Result<()>;
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimingBank {
    pub registers: WordMap,
    pub status: u32,
    pub enabled: bool,
    pub next_phase_index: usize,
    pub frame_start: i64,
    pub stop_due: Option<i64>,
    pub phases: [i64; 3],
    pub frames: i64,
    pub stop_completions: i64,
    pub inferred_running: bool,
}
impl Default for TimingBank {
    fn default() -> Self {
        Self {
            registers: WordMap::default(),
            status: 2,
            enabled: false,
            next_phase_index: 0,
            frame_start: 0,
            stop_due: None,
            phases: [0; 3],
            frames: 0,
            stop_completions: 0,
            inferred_running: false,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Timing {
    pub banks: [TimingBank; 2],
    pub phase_profile: PhaseProfile,
    pub frame_period_ticks: Option<i64>,
    pub stop_delay_ticks: i64,
    /// Python permits True as a one-tick stop delay; preserve its state type.
    pub stop_delay_bool: bool,
    pub last_tick: i64,
    pub phase_order: [usize; 3],
    pub offsets: Option<[i64; 3]>,
    /// Board wiring, not guest state.
    #[serde(skip, default = "exr2_bases")]
    pub bases: [u64; 2],
    #[serde(skip, default = "exr2_irqs")]
    pub irqs: [[u8; 3]; 2],
    /// Frame-boundary interrupts withheld per bank while the output hold is
    /// high. Delivered once on release; absent from saved state when zero.
    #[serde(default, skip_serializing_if = "no_withheld")]
    pub withheld: [u64; 2],
}
fn no_withheld(withheld: &[u64; 2]) -> bool {
    withheld == &[0, 0]
}
fn exr2_bases() -> [u64; 2] {
    BASES
}
fn exr2_irqs() -> [[u8; 3]; 2] {
    IRQS
}
impl Timing {
    pub fn new(period: Option<i64>, delay: i64, profile: String, now: i64) -> Self {
        Self {
            banks: Default::default(),
            phase_profile: PhaseProfile::from_name(profile),
            frame_period_ticks: period,
            stop_delay_ticks: delay,
            stop_delay_bool: false,
            last_tick: now,
            phase_order: [1, 2, 0],
            offsets: period.map(|p| [p / 2, p * 3 / 4, p]),
            bases: BASES,
            irqs: IRQS,
            withheld: [0; 2],
        }
    }
    fn log(
        host: &mut dyn TimingHost,
        name: &'static str,
        fields: Vec<(&'static str, Value)>,
    ) -> Result<()> {
        host.record(Record::new(name, fields))
    }
    pub fn targets(&self, i: usize, phase: usize, host: &dyn TimingHost) -> Result<(u8, Vec<u8>)> {
        let line = self.irqs[i][phase];
        let targets = host
            .enabled()
            .iter()
            .enumerate()
            .filter(|(_, bits)| **bits & (1u128 << line) != 0)
            .map(|(i, _)| i as u8)
            .collect::<Vec<_>>();
        if targets.len() > 1 {
            return Err(Unsupported(format!(
                "Ambiguous display phase{phase} physical IRQ{line:#x}: enabled in both CPU banks"
            )));
        }
        Ok((line, targets))
    }
    pub fn validate_routes(&self, i: usize, host: &dyn TimingHost) -> Result<()> {
        for p in 0..3 {
            self.targets(i, p, host)?;
        }
        Ok(())
    }
    pub fn configured(&self, i: usize) -> bool {
        let r = &self.banks[i].registers;
        MASKS
            .iter()
            .all(|(k, _)| i == 0 && (*k == 0x24 || *k == 0x28) || r.get(*k).is_some())
            && r.get_or_zero(4) & 0xfff != 0
            && r.get_or_zero(4) & 0x0fff0000 != 0
            && r.get_or_zero(0x2c) != 0
    }
    pub fn running(&self, i: usize) -> bool {
        if self.phase_profile == PhaseProfile::ConfiguredClocked
            || self.phase_profile == PhaseProfile::External
        {
            self.banks[i].inferred_running
        } else {
            self.banks[i].enabled
        }
    }
    pub fn refresh(&mut self, i: usize, now: i64, host: &mut dyn TimingHost) -> Result<()> {
        let external = self.phase_profile == PhaseProfile::External;
        if !external && self.phase_profile != PhaseProfile::ConfiguredClocked {
            return Ok(());
        }
        let candidate = if external {
            self.banks[i].registers.get_or_zero(0) & 0x100 != 0
        } else {
            self.configured(i)
        };
        let running = candidate && host.clock_ready()?;
        if running && !self.banks[i].inferred_running {
            self.validate_routes(i, host)?;
        }
        if running != self.banks[i].inferred_running {
            let b = &mut self.banks[i];
            b.inferred_running = running;
            if running {
                b.next_phase_index = 0;
                b.frame_start = now;
            }
            Self::log(
                host,
                if external {
                    "Display external clock input changed"
                } else {
                    "Display configured-clocked cadence changed"
                },
                vec![
                    ("instance", Value::int(i as i64)),
                    ("running", Value::Bool(running)),
                    ("phase_profile", Value::str(self.phase_profile.as_str())),
                    (
                        "evidence",
                        Value::str(if external {
                            "declared external waveform; calibrated timing/dimensions remain unresolved"
                        } else {
                            "opt-in inference from complete timing programming and requested/ready clock48"
                        }),
                    ),
                    (
                        "virtual_frame_period_ticks",
                        Value::opt(self.frame_period_ticks),
                    ),
                ],
            )?;
        }
        Ok(())
    }
    pub fn phase_due(&self, i: usize) -> Option<i64> {
        if !self.running(i) {
            return None;
        }
        self.offsets
            .map(|offsets| self.banks[i].frame_start + offsets[self.banks[i].next_phase_index])
    }
    pub fn emit_phase(
        &mut self,
        i: usize,
        phase: usize,
        due: i64,
        held: bool,
        host: &mut dyn TimingHost,
    ) -> Result<()> {
        let b = &mut self.banks[i];
        if phase == 0 {
            b.status |= 2;
            b.frames += 1;
        } else if phase == 1 {
            b.status &= !2;
        }
        b.phases[phase] += 1;
        if phase == 0 && held {
            self.withheld[i] += 1;
            return Self::log(
                host,
                "Display output boundary withheld",
                vec![
                    ("instance", Value::int(i as i64)),
                    ("source_tick", Value::int(due)),
                ],
            );
        }
        let (line, targets) = self.targets(i, phase, host)?;
        Self::log(
            host,
            "Display virtual phase",
            vec![
                ("instance", Value::int(i as i64)),
                ("phase", Value::int(phase as i64)),
                ("source_tick", Value::int(due)),
                ("physical_irq", Value::hex(line as u128)),
                (
                    "target_cpus",
                    Value::List(targets.iter().map(|c| Value::int(*c)).collect()),
                ),
                (
                    "timing_evidence",
                    Value::str(if self.phase_profile == PhaseProfile::External {
                        "declared external waveform, no calibrated timing inference"
                    } else {
                        "explicit inferred virtual cadence"
                    }),
                ),
            ],
        )?;
        for cpu in targets {
            host.queue_irq(cpu, line)?;
        }
        Ok(())
    }
    /// Deliver each bank's withheld frame-boundary interrupt once. The output
    /// presents one frame per boundary, however many boundaries were held.
    pub fn release_withheld(&mut self, host: &mut dyn TimingHost) -> Result<()> {
        for i in 0..2 {
            if self.withheld[i] == 0 {
                continue;
            }
            let count = std::mem::take(&mut self.withheld[i]);
            let (line, targets) = self.targets(i, 0, host)?;
            Self::log(
                host,
                "Display output boundary delivered",
                vec![
                    ("instance", Value::int(i as i64)),
                    ("withheld", Value::int(count as i64)),
                    ("physical_irq", Value::hex(line as u128)),
                ],
            )?;
            for cpu in targets {
                host.queue_irq(cpu, line)?;
            }
        }
        Ok(())
    }
    pub fn has_withheld(&self) -> bool {
        self.withheld.iter().any(|count| *count > 0)
    }
    /// Advance to `now`. While `held` is set, frame-boundary interrupts are
    /// withheld instead of delivered.
    pub fn tick(&mut self, now: i64, held: bool, host: &mut dyn TimingHost) -> Result<()> {
        if now < self.last_tick {
            return Err("Display virtual time moved backwards".into());
        }
        for i in 0..2 {
            if let Some(due) = self.banks[i].stop_due.filter(|due| *due <= now) {
                let b = &mut self.banks[i];
                b.stop_due = None;
                b.status |= 2;
                b.stop_completions += 1;
                Self::log(
                    host,
                    "Display timing stopped at phase0",
                    vec![
                        ("instance", Value::int(i as i64)),
                        ("source_tick", Value::int(due)),
                    ],
                )?;
            }
            self.refresh(i, now, host)?;
            let mut events = 0;
            while let Some(due) = self.phase_due(i).filter(|due| *due <= now) {
                self.emit_phase(
                    i,
                    self.phase_order[self.banks[i].next_phase_index],
                    due,
                    held,
                    host,
                )?;
                let b = &mut self.banks[i];
                b.next_phase_index += 1;
                if b.next_phase_index == 3 {
                    b.next_phase_index = 0;
                    b.frame_start += self.frame_period_ticks.unwrap();
                }
                events += 1;
                if events > 10000 {
                    return Err("Display phase catch-up exceeds explicit event bound".into());
                }
            }
        }
        self.last_tick = now;
        Ok(())
    }
    pub fn read(
        &mut self,
        a: u64,
        size: u32,
        now: i64,
        held: bool,
        host: &mut dyn TimingHost,
    ) -> Result<u32> {
        let (i, o) = address_in(&self.bases, a, size)?;
        if now < self.last_tick {
            return Err("Display virtual time moved backwards".into());
        }
        // A status read between events is pure, including checkpoint state.
        // Due work and clock-peer transitions remain explicit boundaries.
        if self.deadline_with_clock(now, host.clock_ready()?).is_some_and(|at| at <= now) {
            self.tick(now, held, host)?;
        }
        Ok(if o == 0x30 {
            self.banks[i].status
        } else {
            self.banks[i].registers.get_or_zero(o)
        })
    }
    #[allow(clippy::too_many_arguments)] // CPU/access/timing and host dependencies are explicit.
    pub fn write(
        &mut self,
        cpu: i64,
        a: u64,
        size: u32,
        value: i128,
        now: i64,
        held: bool,
        host: &mut dyn TimingHost,
    ) -> Result<()> {
        let (i, o) = address_in(&self.bases, a, size)?;
        if o == 0x30 {
            return Err(
                "Display timing phase status is read-only in the recovered contract".into(),
            );
        }
        let mask = MASKS.iter().find(|(k, _)| *k == o).unwrap().1;
        if !(0..=0xffffffff).contains(&value) || value & !(mask as i128) != 0 {
            let v = if value < 0 {
                format!("-{:#x}", value.unsigned_abs())
            } else {
                format!("{value:#x}")
            };
            return Err(Unsupported(format!(
                "Unrecovered display timing bits {a:#x}={v}"
            )));
        }
        self.tick(now, held, host)?;
        let v = value as u32;
        if o == 0 && v & 0x10000 != 0 {
            self.validate_routes(i, host)?;
        }
        self.banks[i].registers.insert(o, v);
        Self::log(
            host,
            "Display timing register write",
            vec![
                ("instance", Value::int(i as i64)),
                ("cpu", Value::int(cpu)),
                ("address", Value::hex(a as u128)),
                ("value", Value::hex(v as u128)),
            ],
        )?;
        if o != 0 {
            return self.refresh(i, now, host);
        }
        let enabled = v & 0x10000 != 0;
        if self.phase_profile == PhaseProfile::ConfiguredClocked
            && !enabled
            && self.banks[i].inferred_running
        {
            self.banks[i].stop_due = Some(now + self.stop_delay_ticks);
            Self::log(
                host,
                "Display configured-clocked stop requested",
                vec![
                    ("instance", Value::int(i as i64)),
                    ("due_virtual_tick", Value::opt(self.banks[i].stop_due)),
                    (
                        "cadence_continues",
                        Value::str("explicit profile inference, bit16 meaning unresolved"),
                    ),
                ],
            )?;
        }
        let b = &mut self.banks[i];
        if enabled && !b.enabled {
            b.enabled = true;
            b.stop_due = None;
            b.next_phase_index = 0;
            b.frame_start = now;
            Self::log(
                host,
                "Display timing enabled",
                vec![
                    ("instance", Value::int(i as i64)),
                    (
                        "virtual_frame_period_ticks",
                        Value::opt(self.frame_period_ticks),
                    ),
                ],
            )?;
        } else if !enabled && b.enabled {
            b.enabled = false;
            b.stop_due = Some(now + self.stop_delay_ticks);
            Self::log(
                host,
                "Display timing stop requested",
                vec![
                    ("instance", Value::int(i as i64)),
                    ("due_virtual_tick", Value::opt(b.stop_due)),
                ],
            )?;
        } else if !enabled && b.status & 2 == 0 && b.stop_due.is_none() {
            b.stop_due = Some(now + self.stop_delay_ticks);
        }
        // Same-value writes configure without restarting an active frame.
        self.refresh(i, now, host)
    }
    pub fn next_event_tick(&self) -> Option<i64> {
        (0..2)
            .flat_map(|i| [self.banks[i].stop_due, self.phase_due(i)])
            .flatten()
            .min()
    }

    /// A clock dependency can admit or stop cadence before any phase exists.
    /// Query settled clock state only; the due tick commits the transition.
    pub fn deadline_with_clock(&self, now: i64, clock_ready: bool) -> Option<i64> {
        let transition = (0..2).any(|i| {
            let candidate = match self.phase_profile {
                PhaseProfile::ConfiguredClocked => self.configured(i),
                PhaseProfile::External => self.banks[i].registers.get_or_zero(0) & 0x100 != 0,
                _ => return false,
            };
            (candidate && clock_ready) != self.banks[i].inferred_running
        });
        self.next_event_tick()
            .into_iter()
            .chain(transition.then_some(now))
            .min()
    }
}

/// Declared display clock gating, independent of operating mode or callbacks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClockMode {
    Bit16,
    ConfiguredClocked,
    External,
}
#[derive(Clone, Copy, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub mode: ClockMode,
    pub frame_ticks: Option<u64>,
    pub stop_ticks: u64,
    /// Connect the external output-hold wire, which withholds frame-boundary
    /// (phase 0) interrupts while it is high.
    #[serde(default)]
    pub output_hold: bool,
}
pub struct Wiring {
    pub clock: fujimod_emulator_devices::exr2::clock::Output,
    pub controllers: Vec<(usize, fujimod_emulator_devices::exr2::irq::Inputs)>,
    pub start_tick: u64,
    pub output_hold: Option<fujimod_emulator_devices::signals::Level>,
}
pub struct DisplayTiming {
    publication_generation: u64,
    state: Timing,
    status_snapshots: [Arc<AtomicU32>; 2],
    options: Options,
    clock: fujimod_emulator_devices::exr2::clock::Output,
    controllers: [fujimod_emulator_devices::exr2::irq::Inputs; 2],
    output_hold: Option<fujimod_emulator_devices::signals::Level>,
    ceiling: u64,
}
struct Connected<'a, 'b> {
    cx: &'a mut fujimod_emulator_devices::Cx<'b>,
    clock: &'a fujimod_emulator_devices::exr2::clock::Output,
    controllers: &'a [fujimod_emulator_devices::exr2::irq::Inputs; 2],
}
impl TimingHost for Connected<'_, '_> {
    fn enabled(&self) -> [u128; 2] {
        std::array::from_fn(|i| self.controllers[i].enabled())
    }
    fn clock_ready(&mut self) -> Result<bool> {
        Ok(self.clock.ready())
    }
    fn record(&mut self, record: Record) -> Result<()> {
        self.cx.record(record);
        Ok(())
    }
    fn queue_irq(&mut self, cpu: u8, line: u8) -> Result<()> {
        self.cx
            .raise_irq(cpu as usize, line as u32)
            .map_err(Into::into)
    }
}
impl DisplayTiming {
    pub fn new(options: Options, wiring: Wiring) -> std::result::Result<Self, String> {
        if options.stop_ticks == 0
            || options.stop_ticks > i64::MAX as u64
            || options
                .frame_ticks
                .is_some_and(|t| t < 4 || t > i64::MAX as u64 / 3)
            || options.mode == ClockMode::External && options.frame_ticks.is_none()
        {
            return Err("display timing requires bounded physical clock/stop delays".into());
        }
        let ceiling = i64::MAX as u64 - options.stop_ticks.max(options.frame_ticks.unwrap_or(0));
        if wiring.start_tick > ceiling {
            return Err("display initial clock exceeds its time range".into());
        }
        let mut controllers: [Option<fujimod_emulator_devices::exr2::irq::Inputs>; 2] = Default::default();
        for (core, pins) in wiring.controllers {
            if core >= 2 || controllers[core].is_some() {
                return Err("invalid display IRQ connections".into());
            }
            controllers[core] = Some(pins);
        }
        let controllers = controllers
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .ok_or("display requires both controller connections")?
            .try_into()
            .ok()
            .unwrap();
        let profile = match options.mode {
            ClockMode::Bit16 => "bit16",
            ClockMode::ConfiguredClocked => "configured-clocked",
            ClockMode::External => EXTERNAL,
        };
        let state = Timing::new(
            options.frame_ticks.map(|v| v as i64),
            options.stop_ticks as i64,
            profile.into(),
            wiring.start_tick as i64,
        );
        let status_snapshots =
            std::array::from_fn(|i| Arc::new(AtomicU32::new(state.banks[i].status)));
        Ok(Self {
            publication_generation: 0,
            state,
            status_snapshots,
            options,
            clock: wiring.clock,
            controllers,
            output_hold: wiring.output_hold,
            ceiling,
        })
    }
    /// Whether the external output hold is high.
    fn output_hold_high(&self) -> bool {
        self.output_hold.as_ref().is_some_and(|hold| hold.high())
    }
    fn publish_status(&self) {
        for (snapshot, bank) in self.status_snapshots.iter().zip(&self.state.banks) {
            snapshot.store(bank.status, Ordering::Release);
        }
    }
    fn validate_state(&self, state: &Timing) -> std::result::Result<(), String> {
        if state.phase_profile != self.state.phase_profile
            || state.frame_period_ticks != self.options.frame_ticks.map(|v| v as i64)
            || state.stop_delay_ticks != self.options.stop_ticks as i64
            || state.stop_delay_bool
            || state.phase_order != [1, 2, 0]
            || state.offsets != self.state.offsets
            || state.last_tick < 0
            || state.last_tick as u64 > self.ceiling
            || state.banks.iter().enumerate().any(|(i, b)| {
                b.next_phase_index >= 3
                    || b.status & !7 != 0
                    || b.frame_start < 0
                    || b.frame_start as u64 > self.ceiling
                    || b.phases.iter().any(|v| *v < 0)
                    || b.frames < 0
                    || b.stop_completions < 0
                    || b.stop_due.is_some_and(|t| t < 0)
                    || b.registers.items().any(|(o, v)| {
                        i == 0 && matches!(o, 0x24 | 0x28)
                            || MASKS
                                .iter()
                                .find(|(a, _)| *a == o)
                                .is_none_or(|(_, m)| v & !m != 0)
                    })
            })
        {
            return Err("invalid display timing state".into());
        }
        Ok(())
    }
    fn time(&self, cx: &mut fujimod_emulator_devices::Cx<'_>) -> Option<i64> {
        if cx.now() > self.ceiling {
            cx.fail("display timing exceeds its guest clock range");
            None
        } else {
            Some(cx.now() as i64)
        }
    }
}
impl DisplayTiming {
    fn publication_state(&self) -> Timing {
        self.state.clone()
    }
}
impl DisplayTiming {
    fn read_published(
        &mut self,
        cx: &mut fujimod_emulator_devices::Cx<'_>,
        a: u64,
        size: u8,
    ) -> u64 {
        if address(a, size as u32).is_err() {
            cx.unmodeled();
            return 0;
        }
        let Some(now) = self.time(cx) else { return 0 };
        let held = self.output_hold_high();
        let result = {
            let mut host = Connected {
                cx,
                clock: &self.clock,
                controllers: &self.controllers,
            };
            self.state.read(a, size as u32, now, held, &mut host)
        };
        self.publish_status();
        match result {
            Ok(value) => value.into(),
            Err(error) => {
                cx.fail(error.0);
                0
            }
        }
    }
    fn write_published(
        &mut self,
        cx: &mut fujimod_emulator_devices::Cx<'_>,
        a: u64,
        size: u8,
        value: u64,
    ) {
        if address(a, size as u32).is_err() {
            cx.unmodeled();
            return;
        }
        let Some(now) = self.time(cx) else { return };
        let cpu = cx.cpu as i64;
        let held = self.output_hold_high();
        let result = {
            let mut host = Connected {
                cx,
                clock: &self.clock,
                controllers: &self.controllers,
            };
            self.state
                .write(cpu, a, size as u32, value.into(), now, held, &mut host)
        };
        self.publish_status();
        if let Err(error) = result {
            cx.fail(error.0);
        }
    }
    fn advance_published(&mut self, cx: &mut fujimod_emulator_devices::Cx<'_>) {
        let Some(now) = self.time(cx) else { return };
        let held = self.output_hold_high();
        let delivers = !held && self.state.has_withheld();
        if !delivers
            && !self
                .state
                .deadline_with_clock(now, self.clock.ready())
                .is_some_and(|due| due <= now)
        {
            return;
        }
        let result = {
            let mut host = Connected {
                cx,
                clock: &self.clock,
                controllers: &self.controllers,
            };
            let released = if delivers {
                self.state.release_withheld(&mut host)
            } else {
                Ok(())
            };
            released.and_then(|()| self.state.tick(now, held, &mut host))
        };
        self.publish_status();
        if let Err(error) = result {
            cx.fail(error.0);
        }
    }
}
impl fujimod_emulator_devices::Device for DisplayTiming {
    fn publication_generation(&self) -> Option<u64> {
        Some(self.publication_generation)
    }
    fn checkpoint(
        &self,
    ) -> std::result::Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        fujimod_emulator_devices::checkpoint::Checkpoint::encode(self.name(), &self.state)
    }
    fn validate_checkpoint(
        &self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> std::result::Result<(), String> {
        self.validate_state(&saved.decode(self.name())?)
    }
    fn restore_checkpoint(
        &mut self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> std::result::Result<(), String> {
        let state: Timing = saved.decode(self.name())?;
        self.validate_state(&state)?;
        if self.state != state {
            self.publication_generation = self.publication_generation.wrapping_add(1);
        }
        self.state = state;
        self.publish_status();
        Ok(())
    }

    fn fingerprint(&self) -> std::result::Result<serde_json::Value, String> {
        Ok(self.save())
    }

    fn name(&self) -> &'static str {
        "display-timing"
    }
    fn display_frame_count(&self, output: usize) -> std::result::Result<u64, String> {
        self.state
            .banks
            .get(output)
            .map(|bank| bank.frames as u64)
            .ok_or_else(|| "invalid display timing output".into())
    }
    fn fast_reads(&self, window_base: u64, window_size: u64) -> Vec<FastReadWord> {
        let Some(window_end) = window_base.checked_add(window_size) else {
            return Vec::new();
        };
        BASES
            .iter()
            .enumerate()
            .filter_map(|(bank, base)| {
                let address = base.checked_add(0x30)?;
                let end = address.checked_add(4)?;
                (address >= window_base && end <= window_end).then(|| {
                    FastReadWord::atomic_u32(
                        address - window_base,
                        self.status_snapshots[bank].clone(),
                    )
                })
            })
            .collect()
    }
    fn read(&mut self, cx: &mut fujimod_emulator_devices::Cx<'_>, a: u64, size: u8) -> u64 {
        if self
            .next_deadline(cx.now())
            .is_none_or(|due| due > cx.now())
        {
            return self.read_published(cx, a, size);
        }
        let before = self.publication_state();
        let result = self.read_published(cx, a, size);
        if self.publication_state() != before {
            self.publication_generation = self.publication_generation.wrapping_add(1);
        }
        result
    }
    fn write(&mut self, cx: &mut fujimod_emulator_devices::Cx<'_>, a: u64, size: u8, value: u64) {
        let before = self.publication_state();
        self.write_published(cx, a, size, value);
        if self.publication_state() != before {
            self.publication_generation = self.publication_generation.wrapping_add(1);
        }
    }
    fn advance(&mut self, cx: &mut fujimod_emulator_devices::Cx<'_>) {
        if (self.output_hold_high() || !self.state.has_withheld())
            && self
                .next_deadline(cx.now())
                .is_none_or(|due| due > cx.now())
        {
            return self.advance_published(cx);
        }
        let before = self.publication_state();
        self.advance_published(cx);
        if self.publication_state() != before {
            self.publication_generation = self.publication_generation.wrapping_add(1);
        }
    }
    fn next_deadline(&self, now: u64) -> Option<u64> {
        if now > self.ceiling {
            return Some(now);
        }
        self.state
            .deadline_with_clock(now as i64, self.clock.ready())
            .map(|v| v as u64)
    }
    fn pollable(&self, address: u64, size: u8) -> bool {
        self::address(address, size as u32).is_ok()
    }
    fn save(&self) -> serde_json::Value {
        serde_json::to_value(&self.state).expect("typed display timing state")
    }
    fn restore(&mut self, value: &serde_json::Value) -> std::result::Result<(), String> {
        let state: Timing = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        self.validate_state(&state)?;
        if self.state != state {
            self.publication_generation = self.publication_generation.wrapping_add(1);
        }
        self.state = state;
        self.publish_status();
        Ok(())
    }
}

#[cfg(test)]
mod native_tests {
    use super::*;
    use fujimod_emulator_devices::test_host::{TestHost, advance, write};
    use fujimod_emulator_devices::{
        Device,
        exr2::{clock, irq},
    };
    fn fixture() -> (DisplayTiming, clock::Clocks, TestHost) {
        let mut host = TestHost::new();
        let clock = clock::Clocks::new(
            clock::Options {
                output_settle_ticks: 5,
                reset_ready: true,
            },
            (),
        )
        .unwrap();
        let inputs = [irq::Inputs::default(), irq::Inputs::default()];
        let mut controller = irq::InterruptController::new(
            irq::Options {
                arbitration: irq::Arbitration::ActiveBitmap,
                edge_latch: irq::EdgeLatch::Always,
                level_edges: irq::LevelEdges::Ignored,
            },
            irq::Wiring {
                base: 0xfffb4000,
                inputs: inputs[1].clone(),
                enables: None,
                software: vec![],
            },
        )
        .unwrap();
        write(&mut controller, &mut host, 0xfffb4084, 3 << 23);
        let timing = DisplayTiming::new(
            Options {
                mode: ClockMode::ConfiguredClocked,
                frame_ticks: Some(12),
                stop_ticks: 2,
                output_hold: false,
            },
            Wiring {
                clock: clock.output(),
                controllers: inputs.into_iter().enumerate().collect(),
                start_tick: 0,
                output_hold: None,
            },
        )
        .unwrap();
        (timing, clock, host)
    }
    #[test]
    fn frame_count_and_publication_generation_match_saved_changes() {
        let (mut timing, _, mut host) = fixture();
        assert_eq!(timing.display_frame_count(0).unwrap(), 0);
        assert_eq!(timing.display_frame_count(1).unwrap(), 0);
        assert!(timing.display_frame_count(2).is_err());
        for (now, address, value) in [
            (0, BASES[1] + 4, 0x10001),
            (0, BASES[1] + 4, 0x10001),
            (1, BASES[1] + 4, 0x10001),
        ] {
            host.now = now;
            let before = timing.save();
            let generation = timing.publication_generation();
            write(&mut timing, &mut host, address, value);
            assert_eq!(
                timing.publication_generation() != generation,
                timing.save() != before
            );
        }
        let mut saved = timing.save();
        saved["banks"][0]["frames"] = 3.into();
        saved["banks"][1]["frames"] = 7.into();
        let generation = timing.publication_generation();
        timing.restore(&saved).unwrap();
        assert_ne!(timing.publication_generation(), generation);
        assert_eq!(timing.display_frame_count(0).unwrap(), 3);
        assert_eq!(timing.display_frame_count(1).unwrap(), 7);
        let generation = timing.publication_generation();
        timing.restore(&saved).unwrap();
        assert_eq!(timing.publication_generation(), generation);
    }
    #[test]
    fn pure_status_reads_leave_checkpoint_state_unchanged_between_deadlines() {
        let (mut timing, _, mut host) = fixture();
        let before = timing.save();
        let generation = timing.publication_generation();
        for now in [1, 2, 100, 10000] {
            host.now = now;
            let mut cx = host.cx();
            assert_eq!(timing.read(&mut cx, BASES[0] + 0x30, 4), 2);
            assert!(cx.refusal().is_none());
            assert_eq!(timing.save(), before);
            assert_eq!(timing.publication_generation(), generation);
        }
    }
    #[test]
    fn clock_settling_wakes_cadence_and_routes_exact_phase_edges_without_python() {
        let (mut timing, mut clock, mut host) = fixture();
        let status = timing
            .fast_reads(BASES[1], 0x1000)
            .into_iter()
            .find(|read| read.offset == 0x30)
            .and_then(|read| match read.value {
                fujimod_emulator_devices::bus::FastReadValue::AtomicU32(value) => Some(value),
                fujimod_emulator_devices::bus::FastReadValue::AtomicU16(_)
                | fujimod_emulator_devices::bus::FastReadValue::ClockedU32(_)
                | fujimod_emulator_devices::bus::FastReadValue::ConsumingU32(_) => None,
            })
            .unwrap();
        let status_value = || status.load(Ordering::Acquire);
        assert_eq!(status_value(), 2);
        for (offset, _) in MASKS {
            let value = match offset {
                4 => 0x10001,
                0x2c => 1,
                _ => 0,
            };
            write(&mut timing, &mut host, BASES[1] + offset as u64, value);
        }
        assert_eq!(timing.next_deadline(0), None);
        write(&mut clock, &mut host, clock::BASE + 0x48, 1 << 28);
        assert_eq!(clock.next_deadline(0), Some(5));
        advance(&mut clock, &mut host, 5);
        assert_eq!(timing.next_deadline(5), Some(5));
        advance(&mut timing, &mut host, 5);
        assert_eq!(timing.next_deadline(5), Some(11));
        let saved = timing.save();
        advance(&mut timing, &mut host, 10);
        assert_eq!(timing.save(), saved);
        for tick in [11, 14, 17] {
            let generation = timing.publication_generation();
            advance(&mut timing, &mut host, tick);
            assert_ne!(timing.publication_generation(), generation);
            assert_eq!(status_value(), if tick == 17 { 2 } else { 0 });
        }
        assert_eq!(host.irqs, [(1, 55), (1, 56), (1, 55)]);
        let checkpoint = timing.checkpoint().unwrap();
        host.now = 23;
        write(&mut timing, &mut host, BASES[1] + 0x34, 0);
        assert_eq!(status_value(), 0);
        host.now = 17;
        timing.restore_checkpoint(&checkpoint).unwrap();
        assert_eq!(status_value(), 2);
        let saved = timing.save();
        timing.restore(&saved).unwrap();
        assert_eq!(timing.save(), saved);
        write(&mut clock, &mut host, clock::BASE + 0x48, 0);
        assert_eq!(timing.next_deadline(17), Some(17));
        advance(&mut timing, &mut host, 17);
        assert_eq!(timing.next_deadline(17), None);
    }
    #[test]
    fn malformed_routes_times_and_saved_registers_refuse() {
        let (mut timing, clock, mut host) = fixture();
        assert!(
            DisplayTiming::new(
                Options {
                    mode: ClockMode::External,
                    frame_ticks: None,
                    stop_ticks: 2,
                    output_hold: false,
                },
                Wiring {
                    clock: clock.output(),
                    controllers: vec![],
                    start_tick: 0,
                    output_hold: None,
                }
            )
            .is_err()
        );
        let mut state = timing.save();
        state["banks"][1]["next_phase_index"] = serde_json::json!(3);
        assert!(timing.restore(&state).is_err());
        for (address, size) in [
            (BASES[0] + 0x24, 4),
            (BASES[1] + 0x30, 2),
            (clock::BASE + 0x48, 4),
        ] {
            let mut cx = host.cx();
            timing.read(&mut cx, address, size);
            assert!(cx.is_unmodeled());
        }
        host.now = u64::MAX;
        timing.advance(&mut host.cx());
        assert!(timing.next_deadline(u64::MAX).is_some());
    }
}
#[cfg(test)]
mod output_hold_tests {
    use super::*;

    /// Records delivered interrupts; CPU 0 has every line enabled.
    #[derive(Default)]
    struct Recorder {
        irqs: Vec<(u8, u8)>,
    }
    impl TimingHost for Recorder {
        fn enabled(&self) -> [u128; 2] {
            [u128::MAX, 0]
        }
        fn clock_ready(&mut self) -> Result<bool> {
            Ok(true)
        }
        fn record(&mut self, _record: Record) -> Result<()> {
            Ok(())
        }
        fn queue_irq(&mut self, cpu: u8, line: u8) -> Result<()> {
            self.irqs.push((cpu, line));
            Ok(())
        }
    }

    const PHASE0: u8 = 0x40;
    const PHASE1: u8 = 0x41;
    const PHASE2: u8 = 0x42;

    /// One running bank with a frame every 12 ticks and distinct phase lines.
    /// The bit-16 profile takes its cadence from the bank's enable flag.
    fn running_timing() -> Timing {
        let mut timing = Timing::new(Some(12), 2, "bit16".into(), 0);
        timing.irqs = [[PHASE0, PHASE1, PHASE2], [0x50, 0x51, 0x52]];
        timing.banks[0].enabled = true;
        timing
    }

    fn count(host: &Recorder, line: u8) -> usize {
        host.irqs.iter().filter(|(_, l)| *l == line).count()
    }

    #[test]
    fn held_frame_boundaries_are_withheld_and_released_once() {
        let mut timing = running_timing();
        let mut host = Recorder::default();

        // Two frames pass while the hold is high: their boundaries are
        // withheld, and the other phases still reach the guest.
        timing.tick(12, true, &mut host).unwrap();
        timing.tick(24, true, &mut host).unwrap();
        assert_eq!(count(&host, PHASE0), 0);
        assert_eq!(timing.withheld, [2, 0]);
        assert_eq!(count(&host, PHASE1), 2);
        assert_eq!(count(&host, PHASE2), 2);

        // Release delivers one frame boundary for the held frames, and the
        // cadence then resumes with its boundaries delivered again.
        assert!(timing.has_withheld());
        timing.release_withheld(&mut host).unwrap();
        assert_eq!(count(&host, PHASE0), 1);
        assert_eq!(timing.withheld, [0, 0]);
        assert!(!timing.has_withheld());
        timing.tick(36, false, &mut host).unwrap();
        assert_eq!(count(&host, PHASE0), 2);
        assert_eq!(count(&host, PHASE1), 3);
    }

    #[test]
    fn saved_state_omits_withheld_counts_when_none_are_held() {
        let timing = running_timing();
        let saved = serde_json::to_value(&timing).unwrap();
        assert!(saved.get("withheld").is_none());
        let restored: Timing = serde_json::from_value(saved).unwrap();
        assert_eq!(restored.withheld, [0, 0]);
    }
}
