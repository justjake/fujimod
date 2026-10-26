//! The kernel timer block at 0xfff91000.
//!
//! Native register and deadline laws for the
//! kernel tick timer (+0x00..0x08), two polled/application channels
//! (+0x10, +0x20) and the application timer (+0x30). Units are board
//! virtual ticks; modes and prescalers are software-derived, not hardware
//! validated. `tick` only advances elapsed-time bookkeeping unless a timer
//! expires, which one later tick reproduces exactly.

use fujimod_emulator_devices::record::{Action, Record, Result, Value};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, RwLock};

pub const BASE: u64 = 0xfff91000;
const COUNTER_OFFSETS: [u64; 4] = [0, 0x10, 0x20, 0x30];
const MAIN_ASSUMPTION: &str = "0x23 periodic mode; shared IRQ0x20 fanout inferred";
const APPLICATION_ASSUMPTION: &str =
    "Up-count/reload inferred; IRQ routed to firmware-enabled banks; prescaler units unknown";

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolledTimer {
    pub reload: u64,
    pub control: u64,
    pub count: u64,
    pub acknowledgments: u64,
    pub phase: u64,
    pub counter_written: bool,
    pub application_mode: bool,
    pub expirations: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApplicationTimer {
    pub reload: u64,
    pub control: u64,
    pub count: u64,
    pub expirations: u64,
    pub counter_written: bool,
    pub phase: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SystemTimer {
    pub targets: Vec<u8>,
    pub reload: u64,
    pub control: u64,
    pub count: u64,
    pub last_tick: u64,
    pub expirations: u64,
    pub count_written: bool,
    pub phase: u64,
    pub polled: [PolledTimer; 2],
    pub application: ApplicationTimer,
}

#[derive(Clone)]
struct TimerReadSnapshot {
    state: SystemTimer,
    valid_until: Option<u64>,
}
impl TimerReadSnapshot {
    fn new(state: &SystemTimer) -> std::result::Result<Self, String> {
        let valid_until = state
            .next_event_tick(state.last_tick)
            .map_err(|error| error.0)?;
        Ok(Self {
            state: state.clone(),
            valid_until,
        })
    }

    fn read_count(&self, counter: u64, now: u64) -> Option<u32> {
        if now < self.state.last_tick || self.valid_until.is_some_and(|at| now >= at) {
            return None;
        }
        let elapsed = now - self.state.last_tick;
        let read = |phase: u64, control: u64| {
            let counts = (phase as u128 + elapsed as u128) / divider(control) as u128;
            u64::try_from(counts).ok()
        };
        let value = match counter {
            0 => {
                if self.state.control == 0 || self.state.reload == 0 {
                    self.state.count
                } else {
                    let counts = read(self.state.phase, self.state.control)?;
                    if self.state.control == 0x63 {
                        ((self.state.count as u128 + counts as u128)
                            % (self.state.reload as u128 + 1)) as u64
                    } else if self.state.control == 0x23 {
                        let remaining = if self.state.count != 0 {
                            self.state.count
                        } else {
                            self.state.reload
                        };
                        if counts < remaining {
                            remaining - counts
                        } else {
                            self.state.reload - ((counts - remaining) % self.state.reload)
                        }
                    } else {
                        return None;
                    }
                }
            }
            1 | 2 => {
                let timer = &self.state.polled[counter as usize - 1];
                if timer.control == 0 {
                    timer.count
                } else {
                    let counts = read(timer.phase, timer.control)?;
                    if timer.application_mode {
                        if timer.reload == 0 {
                            return None;
                        }
                        ((timer.count as u128 + counts as u128) % timer.reload as u128) as u64
                    } else {
                        timer.count.wrapping_add(counts) & 0xffff_ffff
                    }
                }
            }
            3 => {
                let timer = &self.state.application;
                if timer.control == 0 || timer.reload == 0 {
                    timer.count
                } else {
                    let counts = read(timer.phase, timer.control)?;
                    ((timer.count as u128 + counts as u128) % timer.reload as u128) as u64
                }
            }
            _ => return None,
        };
        u32::try_from(value).ok()
    }
}

pub fn divider(control: u64) -> u64 {
    // Control bits 4..6 select a power-of-two divider.
    1 << ((control >> 4) & 7)
}

fn record(name: &'static str, fields: Vec<(&'static str, Value)>) -> Action {
    Action::Record(Record::new(name, fields))
}

fn write_record(
    name: &'static str,
    cpu_id: i64,
    channel: Option<i64>,
    address: u64,
    value: u32,
) -> Action {
    let mut fields = vec![("cpu_id", Value::int(cpu_id))];
    if let Some(channel) = channel {
        fields.push(("channel", Value::int(channel)));
    }
    fields.push(("address", Value::hex(address as u128)));
    fields.push(("value", Value::hex(value as u128)));
    record(name, fields)
}

impl SystemTimer {
    pub fn new(targets: Vec<u8>) -> std::result::Result<Self, String> {
        if targets.is_empty() || targets.iter().any(|t| *t > 1) {
            return Err("Timer targets must contain CPU 0 and/or 1".into());
        }
        Ok(SystemTimer {
            targets,
            reload: 0,
            control: 0,
            count: 0,
            last_tick: 0,
            expirations: 0,
            count_written: false,
            phase: 0,
            polled: Default::default(),
            application: Default::default(),
        })
    }

    fn advance_to<E: Effects>(
        &mut self,
        now: u64,
        enabled: [u128; 2],
        effects: &mut E,
    ) -> Result<()> {
        if now < self.last_tick {
            return Err("Virtual time moved backwards".into());
        }
        let elapsed = now - self.last_tick;
        self.last_tick = now;
        for channel in 1..=2 {
            let timer = &mut self.polled[channel - 1];
            if timer.control != 0 {
                let total = timer.phase as u128 + elapsed as u128;
                let d = divider(timer.control);
                let counts = (total / d as u128) as u64;
                timer.phase = (total % d as u128) as u64;
                if timer.application_mode {
                    advance_application(
                        channel as u64,
                        &mut timer.reload,
                        &mut timer.count,
                        &mut timer.expirations,
                        counts,
                        now,
                        enabled,
                        effects,
                    )?;
                } else {
                    timer.count = timer.count.wrapping_add(counts) & 0xffff_ffff;
                }
            }
        }
        let timer = &mut self.application;
        if elapsed != 0 && timer.control != 0 && timer.reload != 0 {
            let total = timer.phase as u128 + elapsed as u128;
            let d = divider(timer.control);
            timer.phase = (total % d as u128) as u64;
            advance_application(
                3,
                &mut timer.reload,
                &mut timer.count,
                &mut timer.expirations,
                (total / d as u128) as u64,
                now,
                enabled,
                effects,
            )?;
        }
        if elapsed == 0 || self.control == 0 || self.reload == 0 {
            return Ok(());
        }
        let total = self.phase as u128 + elapsed as u128;
        let d = divider(self.control);
        let elapsed = (total / d as u128) as u64;
        self.phase = (total % d as u128) as u64;
        if elapsed == 0 {
            return Ok(());
        }
        if self.control == 0x63 {
            // Boot up-counts from zero; no IRQ in this mode (inferred).
            self.count =
                ((self.count as u128 + elapsed as u128) % (self.reload as u128 + 1)) as u64;
            return Ok(());
        }
        let remaining = if self.count != 0 {
            self.count
        } else {
            self.reload
        };
        if elapsed < remaining {
            self.count = remaining - elapsed;
            return Ok(());
        }
        let expirations = 1 + (elapsed - remaining) / self.reload;
        self.count = self.reload - ((elapsed - remaining) % self.reload);
        self.expirations = self.expirations.saturating_add(expirations);
        for &cpu in &self.targets {
            effects.irq(cpu, 0x20)?;
        }
        effects.record(|| {
            record(
                "Modeled timer expiry",
                vec![
                    ("expirations", Value::int(expirations)),
                    ("virtual_ticks", Value::int(now)),
                    (
                        "targets",
                        Value::List(self.targets.iter().map(|t| Value::int(*t as i64)).collect()),
                    ),
                    ("assumption", Value::str(MAIN_ASSUMPTION)),
                ],
            )
        });
        Ok(())
    }

    /// Next modeled IRQ expiry; free-running and polled timers do not wake WFI.
    pub fn next_event_tick(&self, now: u64) -> Result<Option<u64>> {
        if now < self.last_tick {
            return Err("Virtual time moved backwards".into());
        }
        let mut next = None;
        let mut offer = |due: i128| -> Result<()> {
            let at = due.max(now as i128);
            let at = u64::try_from(at).map_err(|_| "timer deadline exceeds guest time")?;
            next = Some(next.map_or(at, |previous: u64| previous.min(at)));
            Ok(())
        };
        if self.control == 0x23 && self.reload != 0 {
            let remaining = if self.count != 0 {
                self.count
            } else {
                self.reload
            };
            offer(
                self.last_tick as i128 + remaining as i128 * divider(self.control) as i128
                    - self.phase as i128,
            )?;
        }
        let mut application = |reload: u64, count: u64, control: u64, phase: u64| -> Result<()> {
            if control != 0 && reload == 0 {
                return Err("enabled application timer has zero reload".into());
            }
            if control != 0 && reload != 0 {
                offer(
                    self.last_tick as i128
                        + (reload as i128 - count as i128) * divider(control) as i128
                        - phase as i128,
                )?;
            }
            Ok(())
        };
        for timer in self.polled.iter().filter(|t| t.application_mode) {
            application(timer.reload, timer.count, timer.control, timer.phase)?;
        }
        let a = &self.application;
        application(a.reload, a.count, a.control, a.phase)?;
        Ok(next)
    }

    /// A register read after the caller advances the timer to the access tick.
    pub fn read(&self, address: u64) -> Result<u32> {
        let offset = address - BASE;
        let register = offset & 0xf;
        if (0x10..0x40).contains(&offset) {
            let (count, reload, control, kind) = if offset >= 0x30 {
                let t = &self.application;
                (t.count, t.reload, t.control, "application timer")
            } else {
                let t = &self.polled[(offset >> 4) as usize - 1];
                (t.count, t.reload, t.control, "polled timer")
            };
            return match register {
                0 => Ok(count as u32),
                4 => Ok(reload as u32),
                8 => Ok(control as u32),
                _ => Err(format!("Unmodeled {kind} read {address:#x}").into()),
            };
        }
        match offset {
            0 => Ok(self.count as u32),
            4 => Ok(self.reload as u32),
            8 => Ok(self.control as u32),
            _ => Err(format!("Unmodeled timer read {address:#x}").into()),
        }
    }

    /// A register write after the caller's tick; `now` (the board's tick, if
    /// any) becomes the last tick after a successful write.
    fn program<E: Effects>(
        &mut self,
        cpu_id: i64,
        address: u64,
        value: u32,
        now: Option<u64>,
        effects: &mut E,
    ) -> Result<()> {
        let offset = address - BASE;
        let register = offset & 0xf;
        let v = value as u64;
        if (0x30..0x40).contains(&offset) {
            let timer = &mut self.application;
            if register == 0 && value == 0 {
                timer.count = 0;
                timer.counter_written = true;
                timer.phase = 0;
            } else if register == 4 {
                timer.reload = v;
            } else if register == 8 && (value == 0 || (3..0x74).step_by(0x10).any(|x| x == value)) {
                if value != 0 && (timer.reload == 0 || !timer.counter_written) {
                    return Err(
                        "Application timer start without observed counter-zero/load sequence"
                            .into(),
                    );
                }
                timer.control = v;
                timer.phase = 0;
            } else {
                return Err(
                    format!("Unmodeled application timer write {address:#x}={value:#x}").into(),
                );
            }
            effects.record(|| {
                write_record(
                    "Application timer register write",
                    cpu_id,
                    Some(3),
                    address,
                    value,
                )
            });
        } else if (0x10..0x30).contains(&offset) {
            let channel = offset >> 4;
            let timer = &mut self.polled[channel as usize - 1];
            if register == 0 && value == 0 && timer.control == 0 {
                // Zeroing a disabled channel's counter selects finite-reload mode.
                timer.count = 0;
                timer.phase = 0;
                timer.counter_written = true;
                timer.application_mode = true;
            } else if register == 4
                && (timer.application_mode || value == 0 || value == 0xffff_ffff)
            {
                timer.reload = v;
            } else if register == 8 && (value == 0 || (3..0x74).step_by(0x10).any(|x| x == value)) {
                if value != 0 {
                    if timer.control != 0 {
                        return Err("Timer channel start while already enabled".into());
                    }
                    if timer.counter_written && timer.reload != 0 {
                        timer.application_mode = true;
                        timer.counter_written = false;
                    } else if timer.reload == 0xffff_ffff && (value == 0x23 || value == 0x53) {
                        timer.application_mode = false;
                        timer.count = 0;
                    } else {
                        return Err("Timer channel start lacks observed application reset/load or loader maximum-load sequence".into());
                    }
                    timer.phase = 0;
                }
                timer.control = v;
                if value == 0 {
                    timer.phase = 0;
                    timer.counter_written = false;
                }
            } else if register == 0xc && value == 1 {
                if timer.control != 0 || timer.reload != 0 {
                    return Err(
                        "Polled timer acknowledge before observed stop/clear sequence".into(),
                    );
                }
                timer.count = 0;
                timer.phase = 0;
                timer.application_mode = false;
                timer.counter_written = false;
                timer.acknowledgments = timer.acknowledgments.saturating_add(1);
            } else {
                return Err(format!("Unmodeled polled timer write {address:#x}={value:#x}").into());
            }
            effects.record(|| {
                write_record(
                    "Timer channel register write",
                    cpu_id,
                    Some(channel as i64),
                    address,
                    value,
                )
            });
        } else {
            if offset == 0 {
                // Mode 0x63 requires an explicit zero write to the counter.
                if value != 0 {
                    return Err(format!("Unmodeled timer counter write {value:#x}").into());
                }
                self.count = 0;
                self.count_written = true;
                self.phase = 0;
            } else if offset == 4 {
                self.reload = v;
                if (self.control == 0 && !self.count_written)
                    || (self.control == 0x23 && self.count == 0)
                {
                    self.count = v;
                }
            } else if offset == 8 {
                if !matches!(value, 0 | 0x23 | 0x63) {
                    return Err(format!("Unmodeled timer control {value:#x}").into());
                }
                if value == 0x63 && !self.count_written {
                    return Err("Loader timer mode requires its explicit counter-zero write".into());
                }
                self.control = v;
                self.phase = 0;
                if value != 0x63 {
                    self.count_written = false;
                }
            } else {
                return Err(format!("Unmodeled timer write {address:#x}").into());
            }
            effects.record(|| write_record("Timer register write", cpu_id, None, address, value));
        }
        if let Some(now) = now {
            self.last_tick = now;
        }
        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
fn advance_application<E: Effects>(
    channel: u64,
    reload: &mut u64,
    count: &mut u64,
    expirations: &mut u64,
    counts: u64,
    now: u64,
    enabled: [u128; 2],
    effects: &mut E,
) -> Result<()> {
    if *reload == 0 {
        return Err("Enabled application timer has zero reload".into());
    }
    let total = *count as u128 + counts as u128;
    let expired = (total / *reload as u128) as u64;
    *count = (total % *reload as u128) as u64;
    if expired == 0 {
        return Ok(());
    }
    *expirations = expirations.saturating_add(expired);
    let line = 0x20 + channel as u8;
    for cpu in 0..2 {
        if enabled[cpu as usize] & (1u128 << line) != 0 {
            effects.irq(cpu, line)?;
        }
    }
    effects.record(|| {
        record(
            "Modeled application timer expiry",
            vec![
                ("channel", Value::int(channel as i64)),
                ("line", Value::int(line as i64)),
                ("expirations", Value::int(expired)),
                (
                    "targets",
                    Value::List(
                        (0..2)
                            .filter(|c| enabled[*c as usize] & (1u128 << line) != 0)
                            .map(Value::int)
                            .collect(),
                    ),
                ),
                ("virtual_ticks", Value::int(now)),
                ("assumption", Value::str(APPLICATION_ASSUMPTION)),
            ],
        )
    });
    Ok(())
}

trait Effects {
    fn irq(&mut self, cpu: u8, line: u8) -> Result<()>;
    fn record(&mut self, make: impl FnOnce() -> Action);
}
struct Connected<'call, 'host>(&'call mut fujimod_emulator_devices::Cx<'host>);
impl Effects for Connected<'_, '_> {
    fn irq(&mut self, cpu: u8, line: u8) -> Result<()> {
        self.0
            .raise_irq(cpu as usize, line as u32)
            .map_err(Into::into)
    }
    fn record(&mut self, _: impl FnOnce() -> Action) {}
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub main_targets: Vec<u8>,
}
pub struct Wiring {
    pub controllers: Vec<(usize, fujimod_emulator_devices::exr2::irq::Inputs)>,
    pub start_tick: u64,
}
/// Four hardware timer channels connected directly to per-core interrupt pins.
pub struct Timers {
    state: SystemTimer,
    controllers: [Option<fujimod_emulator_devices::exr2::irq::Inputs>; 2],
    read_snapshot: Arc<RwLock<Arc<TimerReadSnapshot>>>,
}
impl Timers {
    pub fn new(options: Options, wiring: Wiring) -> std::result::Result<Self, String> {
        let mut controllers: [Option<fujimod_emulator_devices::exr2::irq::Inputs>; 2] = Default::default();
        for (core, inputs) in wiring.controllers {
            if core >= 2 || controllers[core].is_some() {
                return Err("invalid timer interrupt wiring".into());
            }
            controllers[core] = Some(inputs);
        }
        if options.main_targets.iter().enumerate().any(|(i, &core)| {
            core >= 2
                || controllers[core as usize].is_none()
                || options.main_targets[..i].contains(&core)
        }) {
            return Err("timer targets require unique connected controllers".into());
        }
        let mut state = SystemTimer::new(options.main_targets)?;
        state.last_tick = wiring.start_tick;
        let read_snapshot = Arc::new(RwLock::new(Arc::new(TimerReadSnapshot::new(&state)?)));
        Ok(Self {
            state,
            controllers,
            read_snapshot,
        })
    }
    fn publish_read_snapshot(&self) -> std::result::Result<(), String> {
        let snapshot = Arc::new(TimerReadSnapshot::new(&self.state)?);
        let mut published = self
            .read_snapshot
            .write()
            .map_err(|_| "timer read snapshot is poisoned")?;
        *published = snapshot;
        Ok(())
    }
    fn enabled(&self) -> [u128; 2] {
        std::array::from_fn(|core| {
            self.controllers[core]
                .as_ref()
                .map_or(0, fujimod_emulator_devices::exr2::irq::Inputs::enabled)
        })
    }
    fn settle(&mut self, cx: &mut fujimod_emulator_devices::Cx<'_>) -> bool {
        if let Err(error) = self
            .state
            .advance_to(cx.now(), self.enabled(), &mut Connected(cx))
        {
            cx.fail(error.0);
            return false;
        }
        if let Err(error) = self.publish_read_snapshot() {
            cx.fail(error);
            return false;
        }
        true
    }
    fn address(address: u64, size: u8, write: bool) -> bool {
        let Some(offset) = address.checked_sub(BASE) else {
            return false;
        };
        size == 4
            && offset < 0x40
            && offset.is_multiple_of(4)
            && (offset & 15 != 12 || write && matches!(offset, 0x1c | 0x2c))
    }

    fn validate_state(&self, state: &SystemTimer) -> std::result::Result<(), String> {
        let valid = |count: u64, reload: u64, control: u64, phase: u64| {
            count <= u32::MAX as u64
                && reload <= u32::MAX as u64
                && control <= u32::MAX as u64
                && phase < divider(control)
        };
        if state.targets != self.state.targets
            || !matches!(state.control, 0 | 0x23 | 0x63)
            || !valid(state.count, state.reload, state.control, state.phase)
            || state.polled.iter().any(|timer| {
                !valid(timer.count, timer.reload, timer.control, timer.phase)
                    || timer.control != 0
                        && (!(3..0x74).step_by(0x10).any(|value| value == timer.control)
                            || timer.application_mode && timer.reload == 0)
            })
            || !valid(
                state.application.count,
                state.application.reload,
                state.application.control,
                state.application.phase,
            )
            || state.application.control != 0
                && (!(3..0x74)
                    .step_by(0x10)
                    .any(|value| value == state.application.control)
                    || state.application.reload == 0)
        {
            return Err("invalid timer state".into());
        }
        state
            .next_event_tick(state.last_tick)
            .map_err(|error| error.0)?;
        Ok(())
    }
}
impl fujimod_emulator_devices::Device for Timers {
    fn checkpoint(&self) -> std::result::Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
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
        let state: SystemTimer = saved.decode(self.name())?;
        self.validate_state(&state)?;
        self.state = state;
        self.publish_read_snapshot()
    }

    fn fingerprint(&self) -> std::result::Result<serde_json::Value, String> {
        Ok(self.save())
    }

    fn name(&self) -> &'static str {
        "exr2-system-timers"
    }
    fn read(&mut self, cx: &mut fujimod_emulator_devices::Cx<'_>, address: u64, size: u8) -> u64 {
        if !Self::address(address, size, false) {
            cx.unmodeled();
            return 0;
        }
        if !self.settle(cx) {
            return 0;
        }
        match self.state.read(address) {
            Ok(value) => value as u64,
            Err(error) => {
                cx.fail(error.0);
                0
            }
        }
    }
    fn write(&mut self, cx: &mut fujimod_emulator_devices::Cx<'_>, address: u64, size: u8, value: u64) {
        if !Self::address(address, size, true) {
            cx.unmodeled();
            return;
        }
        if !self.settle(cx) {
            return;
        }
        let result = u32::try_from(value)
            .map_err(|_| "timer word exceeds 32 bits".into())
            .and_then(|value| {
                self.state.program(
                    cx.cpu as i64,
                    address,
                    value,
                    Some(cx.now()),
                    &mut Connected(cx),
                )
            });
        if let Err(error) = result {
            cx.fail(error.0);
        }
        if let Err(error) = self.state.next_event_tick(self.state.last_tick) {
            cx.fail(error.0);
        }
        if let Err(error) = self.publish_read_snapshot() {
            cx.fail(error);
        }
    }
    fn next_deadline(&self, now: u64) -> Option<u64> {
        match self.state.next_event_tick(self.state.last_tick) {
            Ok(at) => at,
            Err(_) => Some(now),
        }
    }
    fn advance(&mut self, cx: &mut fujimod_emulator_devices::Cx<'_>) {
        match self.state.next_event_tick(self.state.last_tick) {
            Ok(Some(at)) if at <= cx.now() => {
                self.settle(cx);
            }
            Err(error) => cx.fail(error.0),
            _ => (),
        }
    }
    fn pollable(&self, address: u64, size: u8) -> bool {
        Self::address(address, size, false)
    }
    fn fast_reads(&self, window_base: u64, window_size: u64) -> Vec<fujimod_emulator_devices::bus::FastReadWord> {
        let Some(window_end) = window_base.checked_add(window_size) else {
            return Vec::new();
        };
        COUNTER_OFFSETS
            .into_iter()
            .filter_map(|register| {
                let address = BASE.checked_add(register)?;
                let end = address.checked_add(4)?;
                (address >= window_base && end <= window_end).then(|| {
                    let counter = register / 0x10;
                    let snapshot = self.read_snapshot.clone();
                    fujimod_emulator_devices::bus::FastReadWord::clocked_u32(
                        address - window_base,
                        Arc::new(move |now| {
                            let snapshot = snapshot.read().ok()?.clone();
                            snapshot.read_count(counter, now)
                        }),
                    )
                })
            })
            .collect()
    }
    fn save(&self) -> serde_json::Value {
        serde_json::to_value(&self.state).expect("timer state")
    }
    fn restore(&mut self, value: &serde_json::Value) -> std::result::Result<(), String> {
        let state: SystemTimer =
            serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        self.validate_state(&state)?;
        self.state = state;
        self.publish_read_snapshot()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::bus::FastReadValue;
    use fujimod_emulator_devices::{Cx, Device, Host};
    #[derive(Default)]
    struct Clock {
        now: u64,
        irqs: Vec<(usize, u32)>,
    }
    impl Host for Clock {
        fn now(&self) -> u64 {
            self.now
        }
        fn schedule(&mut self, _: usize, _: u64, _: u64) {
            panic!("timer scheduled a synthetic event")
        }
        fn set_irq(&mut self, _: u32, _: bool) {
            panic!("unexpected level IRQ")
        }
        fn raise_irq(&mut self, core: usize, line: u32) -> std::result::Result<(), String> {
            self.irqs.push((core, line));
            Ok(())
        }
        fn ram_read(&self, _: u64, _: &mut [u8]) -> std::result::Result<(), String> {
            panic!("timer read RAM")
        }
        fn ram_write(&mut self, _: u64, _: &[u8]) -> std::result::Result<(), String> {
            panic!("timer wrote RAM")
        }
        fn record(&mut self, _: fujimod_emulator_devices::Record) {
            panic!("hot-path timer record")
        }
    }
    fn timers(start_tick: u64) -> Timers {
        Timers::new(
            Options {
                main_targets: vec![0, 1],
            },
            Wiring {
                controllers: vec![
                    (0, fujimod_emulator_devices::exr2::irq::Inputs::default()),
                    (1, fujimod_emulator_devices::exr2::irq::Inputs::default()),
                ],
                start_tick,
            },
        )
        .unwrap()
    }
    fn put(timer: &mut Timers, host: &mut Clock, offset: u64, value: u32) {
        let mut cx = Cx::new(host, 0, 0, 0);
        timer.write(&mut cx, BASE + offset, 4, value as u64);
        assert_eq!(cx.refusal(), None);
    }
    fn get(timer: &mut Timers, host: &mut Clock, offset: u64) -> u64 {
        let mut cx = Cx::new(host, 0, 0, 0);
        let value = timer.read(&mut cx, BASE + offset, 4);
        assert_eq!(cx.refusal(), None);
        value
    }
    fn fast_counter(timer: &Timers, offset: u64) -> Arc<dyn Fn(u64) -> Option<u32> + Send + Sync> {
        timer
            .fast_reads(BASE, 0x40)
            .into_iter()
            .find(|word| word.offset == offset)
            .and_then(|word| match word.value {
                FastReadValue::ClockedU32(read) => Some(read),
                _ => None,
            })
            .expect("timer counter has a clocked fast read")
    }
    #[derive(Default)]
    struct NoEffects;
    impl Effects for NoEffects {
        fn irq(&mut self, _: u8, _: u8) -> Result<()> {
            Ok(())
        }
        fn record(&mut self, _: impl FnOnce() -> Action) {}
    }
    fn settled_counter(timer: &Timers, offset: u64, now: u64) -> u32 {
        let mut state = timer.state.clone();
        state
            .advance_to(now, [0; 2], &mut NoEffects)
            .expect("counter settles before its deadline");
        state.read(BASE + offset).expect("counter register reads")
    }

    #[test]
    fn clocked_reads_match_up_down_and_divided_counters_before_real_deadlines() {
        let check = |timer: Timers, offset: u64, end: u64| {
            let read = fast_counter(&timer, offset);
            for now in (100..end).step_by(7) {
                assert_eq!(read(now), Some(settled_counter(&timer, offset, now)));
            }
            assert_eq!(read(end), None, "counter at {offset:#x} deadline");
        };

        let (mut main, mut main_host) = (
            timers(100),
            Clock {
                now: 100,
                ..Clock::default()
            },
        );
        put(&mut main, &mut main_host, 4, 5);
        put(&mut main, &mut main_host, 8, 0x23);
        check(main, 0, 120);

        let (mut polled, mut polled_host) = (
            timers(100),
            Clock {
                now: 100,
                ..Clock::default()
            },
        );
        put(&mut polled, &mut polled_host, 0x10, 0);
        put(&mut polled, &mut polled_host, 0x14, 9);
        put(&mut polled, &mut polled_host, 0x18, 0x33);
        check(polled, 0x10, 172);

        let (mut application, mut application_host) = (
            timers(100),
            Clock {
                now: 100,
                ..Clock::default()
            },
        );
        put(&mut application, &mut application_host, 0x30, 0);
        put(&mut application, &mut application_host, 0x34, 11);
        put(&mut application, &mut application_host, 0x38, 0x53);
        check(application, 0x30, 452);
    }

    #[test]
    fn clocked_reads_refresh_after_write_stop_deadline_and_restore() {
        let (mut timer, mut host) = (
            timers(100),
            Clock {
                now: 100,
                ..Clock::default()
            },
        );
        put(&mut timer, &mut host, 0x30, 0);
        put(&mut timer, &mut host, 0x34, 7);
        put(&mut timer, &mut host, 0x38, 0x33);
        let read = fast_counter(&timer, 0x30);
        assert_eq!(read(123), Some(2));

        host.now = 123;
        put(&mut timer, &mut host, 0x38, 0);
        assert_eq!(read(123), Some(2));
        assert_eq!(read(500), Some(2));

        let mut saved = timer.save();
        saved["last_tick"] = serde_json::json!(1000);
        saved["application"]["reload"] = serde_json::json!(9);
        saved["application"]["count"] = serde_json::json!(4);
        saved["application"]["control"] = serde_json::json!(0x23);
        saved["application"]["phase"] = serde_json::json!(2);
        timer.restore(&saved).unwrap();
        assert_eq!(read(1005), Some(5));
        assert_eq!(read(1018), None);

        let checkpoint = timer.checkpoint().unwrap();
        host.now = 1005;
        put(&mut timer, &mut host, 0x38, 0);
        timer.restore_checkpoint(&checkpoint).unwrap();
        assert_eq!(read(1005), Some(5));
        assert_eq!(read(1018), None);
    }
    #[test]
    fn periodic_deadline_preserves_fractional_phase_and_early_ticks_are_pure() {
        let (mut timer, mut host) = (timers(0), Clock::default());
        put(&mut timer, &mut host, 4, 3);
        put(&mut timer, &mut host, 8, 0x23);
        assert_eq!(timer.next_deadline(0), Some(12));
        let before = timer.save();
        host.now = 5;
        timer.advance(&mut Cx::new(&mut host, 0, 0, 0));
        assert_eq!(timer.save(), before);
        assert_eq!(get(&mut timer, &mut host, 0), 2);
        assert_eq!(timer.next_deadline(5), Some(12));
        host.now = 12;
        timer.advance(&mut Cx::new(&mut host, 0, 0, 0));
        assert_eq!(host.irqs, [(0, 32), (1, 32)]);
        assert_eq!(timer.next_deadline(12), Some(24));
        let before = timer.save();
        assert_eq!(timer.save(), before);
        timer.restore(&before).unwrap();
        assert_eq!(timer.save(), before);
    }
    #[test]
    fn polled_clocks_advance_on_access_without_deadlines_or_events() {
        let (mut timer, mut host) = (
            timers(100),
            Clock {
                now: 100,
                ..Clock::default()
            },
        );
        for (base, control) in [(0x10, 0x23), (0x20, 0x53)] {
            put(&mut timer, &mut host, base + 4, u32::MAX);
            put(&mut timer, &mut host, base + 8, control);
        }
        assert_eq!(timer.next_deadline(100), None);
        host.now = 200;
        assert_eq!(get(&mut timer, &mut host, 0x10), 25);
        assert_eq!(get(&mut timer, &mut host, 0x20), 3);
        assert!(host.irqs.is_empty());
    }
    #[test]
    fn application_channels_use_controller_mask_outputs() {
        use fujimod_emulator_devices::exr2::irq::{Arbitration, Inputs, InterruptController, Options as IrqOptions, Wiring as IrqWiring};
        let mut host = Clock::default();
        let inputs = [Inputs::default(), Inputs::default()];
        for core in 0..2 {
            let base = 0xfffb3000 + core as u64 * 0x1000;
            let mut controller = InterruptController::new(
                IrqOptions {
                    arbitration: Arbitration::ActiveBitmap,
                },
                IrqWiring {
                    base,
                    inputs: inputs[core].clone(),
                    enables: None,
                    software: vec![],
                },
            )
            .unwrap();
            controller.write(
                &mut Cx::new(&mut host, 0, 0, 0),
                base + 0x84,
                4,
                if core == 0 { 4 } else { 10 },
            );
            assert_eq!(
                inputs[core].enabled(),
                (if core == 0 { 4u128 } else { 10 }) << 32
            );
        }
        let mut timer = Timers::new(
            Options {
                main_targets: vec![0, 1],
            },
            Wiring {
                controllers: inputs.into_iter().enumerate().collect(),
                start_tick: 0,
            },
        )
        .unwrap();
        for channel in 1..=3 {
            put(&mut timer, &mut host, channel * 16, 0);
            put(&mut timer, &mut host, channel * 16 + 4, 5);
            put(&mut timer, &mut host, channel * 16 + 8, 3);
        }
        assert_eq!(timer.next_deadline(0), Some(5));
        host.now = 5;
        timer.advance(&mut Cx::new(&mut host, 0, 0, 0));
        assert_eq!(host.irqs, [(1, 33), (0, 34), (1, 35)]);
        assert_eq!(timer.next_deadline(5), Some(10));
    }
    #[test]
    fn deadline_overflow_and_invalid_restores_refuse() {
        let mut timer = timers(u64::MAX - 1);
        let mut host = Clock {
            now: u64::MAX - 1,
            ..Clock::default()
        };
        put(&mut timer, &mut host, 4, 3);
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        timer.write(&mut cx, BASE + 8, 4, 0x23);
        assert!(cx.refusal().unwrap().contains("deadline exceeds"));
        let before = timers(0).save();
        let mut bad = before.clone();
        bad["phase"] = serde_json::json!(100);
        assert!(timer.restore(&bad).is_err());
        assert!(
            Timers::new(
                Options {
                    main_targets: vec![0]
                },
                Wiring {
                    controllers: vec![],
                    start_tick: 0
                }
            )
            .is_err()
        );
    }
}
