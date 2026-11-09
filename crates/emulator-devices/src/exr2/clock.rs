//! FFF88000 clock control. Ready registers expose reset state and output
//! settling state through guest accesses. Reset readiness and settling latency
//! are declared inputs. SDHCI clock words belong to their SD hosts and are
//! outside this block. FFF8941C retains the pad-timing RMW fields; zero
//! power-on state is an unmeasured input default. Analog edge timing and
//! frequency changes are not modeled.
//!
//! FFF88084 bit 0 holds the supply on. The boot ROM sets it at reset, then
//! copies boot-header bit 0 of +0x10 into it before loading stage 0; NAND
//! headers clear it and the stock loader sets it again once running. While it
//! stays clear past the release limit the supply drops and execution stops,
//! which is how a slow card load fails on the camera.
//!
//! The X30 stock sleep entry (0x1220c88 and its SRAM code 0x1220988) also
//! programs FFF88080 bits 0-1 (low-power mode, written 2 before it polls the
//! bit 16 ready), FFF88088 (wake timer, written before WFI) and FFF8808C
//! (sleep control, read-modify-written). Their zero power-on values are
//! unmeasured inputs; they are kept apart from the boot registers so stock
//! runs that never sleep keep their saved state unchanged.
use crate::{Cx, Device};
use serde::{Deserialize, Serialize};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU32, Ordering},
};

pub const BASE: u64 = 0xfff88000;
/// SD pad timing latch.
pub const SD_TIMING: u64 = 0xfff8941c;
const OUTPUT: usize = 0x48 / 4;
const REQUEST: u32 = 1 << 28;
const READY: u32 = 1 << 24;
const POWER_HOLD: usize = 0x84 / 4;
const SLEEP_MODE: u64 = BASE + 0x80;
const SLEEP_MODE_BITS: u32 = 3;
const WAKE_TIMER: u64 = BASE + 0x88;
const SLEEP_CONTROL: u64 = BASE + 0x8c;
/// Ticks the supply survives with the hold released. Unmeasured: above every
/// observed stock release (at most 13,430 ticks from ROM release to loader
/// re-hold) and well below a multi-megabyte card load (5,370,052 ticks).
pub const POWER_HOLD_RELEASE_LIMIT_TICKS: u64 = 100_000;
/// Register-derived output pins; observing them never settles the clock.
#[derive(Clone, Default)]
pub struct Output(
    Arc<AtomicU32>,
    Arc<Mutex<Option<Arc<Mutex<super::wake_alarm::WakeAlarm>>>>>,
    Arc<AtomicBool>,
);
impl Output {
    /// SD0 host register clock gate.
    pub fn sd0_gate(&self) -> Arc<AtomicBool> {
        self.2.clone()
    }
    pub fn ready(&self) -> bool {
        self.0.load(Ordering::Relaxed) & (REQUEST | READY) == REQUEST | READY
    }
    /// Attach the power controller wired to this supply-hold output.
    pub fn attach_power(
        &self,
        power: Arc<Mutex<super::wake_alarm::WakeAlarm>>,
    ) -> Result<(), String> {
        let mut slot = self.1.lock().map_err(|_| "power connection poisoned")?;
        if slot.is_some() {
            return Err("power controller already connected".into());
        }
        *slot = Some(power);
        Ok(())
    }
    /// The explicitly attached power controller, if this board has one.
    pub fn power_alarm(&self) -> Result<Option<Arc<Mutex<super::wake_alarm::WakeAlarm>>>, String> {
        Ok(self
            .1
            .lock()
            .map_err(|_| "power connection poisoned")?
            .clone())
    }
}
#[derive(Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub output_settle_ticks: u64,
    pub reset_ready: bool,
}
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Sleep {
    mode: u32,
    wake_timer: u32,
    control: u32,
}
impl Sleep {
    fn is_reset(&self) -> bool {
        *self == Self::default()
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    registers: Vec<u32>,
    output_due: Option<u64>,
    sd_timing: u32,
    power_off_due: Option<u64>,
    #[serde(default, skip_serializing_if = "Sleep::is_reset")]
    sleep: Sleep,
}
pub struct Clocks {
    options: Options,
    state: State,
    output: Output,
}
impl Clocks {
    pub fn new(options: Options, _: ()) -> Result<Self, String> {
        if options.output_settle_ticks == 0 {
            return Err("clock control requires positive settling time".into());
        }
        let mut registers = vec![0; 34];
        registers[0x80 / 4] = u32::from(options.reset_ready) << 16;
        Ok(Self {
            options,
            output: Output::default(),
            state: State {
                registers,
                output_due: None,
                sd_timing: 0,
                power_off_due: None,
                sleep: Sleep::default(),
            },
        })
    }
    pub fn output(&self) -> Output {
        self.output.clone()
    }
    fn publish(&self) {
        self.output
            .0
            .store(self.state.registers[OUTPUT], Ordering::Relaxed);
        self.output.2.store(self.state.registers[0x30 / 4] & 0x200 != 0, Ordering::Relaxed);
    }
    fn index(address: u64, size: u8) -> Option<usize> {
        let offset = address.checked_sub(BASE)?;
        (size == 4
            && offset.is_multiple_of(4)
            && matches!(
                offset,
                0..=0x38 | 0x40 | 0x48 | 0x50 | 0x54 | 0x58 | 0x5c | 0x80 | 0x84
            ))
        .then_some((offset / 4) as usize)
    }
}
impl Clocks {
    fn validate_state(&self, state: &State) -> Result<(), String> {
        if state.registers.len() != 34
            || state.registers[32] != u32::from(self.options.reset_ready) << 16
            || state.registers.iter().enumerate().any(|(i, &v)| {
                Self::index(BASE + i as u64 * 4, 4).is_none() && v != 0
                    || i <= 8 && v & REQUEST != 0
            })
            || state.registers[OUTPUT] & !(REQUEST | READY) != 0
            || state.registers[OUTPUT] & READY != 0 && state.registers[OUTPUT] & REQUEST == 0
            || state.output_due.is_some()
                != (state.registers[OUTPUT] & (REQUEST | READY) == REQUEST)
            || state.power_off_due.is_some() && state.registers[POWER_HOLD] & 1 != 0
            || state.sleep.mode & !SLEEP_MODE_BITS != 0
        {
            return Err("invalid clock control state".into());
        }
        Ok(())
    }
}
impl Device for Clocks {
    fn checkpoint(&self) -> Result<crate::checkpoint::Checkpoint, String> {
        crate::checkpoint::Checkpoint::encode(self.name(), &self.state)
    }
    fn validate_checkpoint(&self, saved: &crate::checkpoint::Checkpoint) -> Result<(), String> {
        self.validate_state(&saved.decode(self.name())?)
    }
    fn restore_checkpoint(&mut self, saved: &crate::checkpoint::Checkpoint) -> Result<(), String> {
        let state = saved.decode(self.name())?;
        self.validate_state(&state)?;
        self.state = state;
        self.publish();
        Ok(())
    }

    fn fingerprint(&self) -> std::result::Result<serde_json::Value, String> {
        Ok(self.save())
    }

    fn name(&self) -> &'static str {
        "clock-control"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        if address == SD_TIMING && size == 4 {
            return self.state.sd_timing.into();
        }
        if size == 4 && matches!(address, WAKE_TIMER | SLEEP_CONTROL) {
            self.advance(cx);
            return u64::from(if address == WAKE_TIMER {
                self.state.sleep.wake_timer
            } else {
                self.state.sleep.control
            });
        }
        let Some(index) = Self::index(address, size) else {
            cx.unmodeled();
            return 0;
        };
        self.advance(cx);
        let value = self.state.registers[index]
            | if index == 0x80 / 4 { self.state.sleep.mode } else { 0 };
        u64::from(if matches!(index, 14 | 16) && value & REQUEST != 0 {
            value | READY
        } else {
            value
        })
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        if address == SD_TIMING && size == 4 {
            match u32::try_from(value) {
                Ok(value) => self.state.sd_timing = value,
                Err(_) => cx.fail("SD timing word exceeds 32 bits"),
            }
            return;
        }
        if size == 4 && matches!(address, WAKE_TIMER | SLEEP_CONTROL) {
            self.advance(cx);
            let Ok(value) = u32::try_from(value) else {
                cx.fail("clock word exceeds 32 bits");
                return;
            };
            if address == WAKE_TIMER {
                self.state.sleep.wake_timer = value;
            } else {
                self.state.sleep.control = value;
            }
            return;
        }
        let Some(index) = Self::index(address, size) else {
            cx.unmodeled();
            return;
        };
        self.advance(cx);
        if address == SLEEP_MODE {
            if value & !u64::from(SLEEP_MODE_BITS) != u64::from(self.state.registers[index]) {
                cx.fail("clock-ready status is read-only; only low-power mode bits 0-1 are writable");
                return;
            }
            self.state.sleep.mode = value as u32 & SLEEP_MODE_BITS;
            return;
        }
        if value > u32::MAX as u64 {
            cx.fail("clock word exceeds 32 bits");
            return;
        }
        let mut value = value as u32;
        if index == OUTPUT {
            let old = self.state.registers[index];
            if value & !(REQUEST | READY) != 0 || value & READY != 0 && old & READY == 0 {
                cx.fail("output clock accepts only its request and an already asserted ready bit");
                return;
            }
            if (value ^ old) & REQUEST != 0 {
                let due = if value & REQUEST != 0 {
                    let Some(due) = cx.now().checked_add(self.options.output_settle_ticks) else {
                        cx.fail("output clock deadline overflow");
                        return;
                    };
                    Some(due)
                } else {
                    None
                };
                self.state.registers[index] = value & REQUEST;
                self.state.output_due = due;
                self.publish();
            }
            return;
        }
        if index <= 0x20 / 4 {
            value &= !REQUEST;
        }
        if index == POWER_HOLD {
            let power = match self.output.power_alarm() {
                Ok(power) => power,
                Err(error) => {
                    cx.fail(error);
                    return;
                }
            };
            if let Some(power) = power {
                let result = power
                    .lock()
                    .map_err(|_| "power alarm poisoned".to_string())
                    .and_then(|mut power| power.hold(cx.now(), value & 1 != 0));
                if let Err(error) = result {
                    cx.fail(error);
                    return;
                }
            }
            if value & 1 != 0 {
                self.state.power_off_due = None;
            } else if self.state.power_off_due.is_none() {
                let Some(due) = cx.now().checked_add(POWER_HOLD_RELEASE_LIMIT_TICKS) else {
                    cx.fail("power hold deadline overflow");
                    return;
                };
                self.state.power_off_due = Some(due);
            }
        }
        self.state.registers[index] = value;
        self.publish();
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        if self.state.output_due.is_some_and(|at| at <= cx.now()) {
            self.state.registers[OUTPUT] |= READY;
            self.state.output_due = None;
            self.publish();
        }
        if self.state.power_off_due.is_some_and(|at| at <= cx.now()) {
            match self.output.power_alarm() {
                Ok(Some(power)) => {
                    let result = power.lock().map_err(|_| "power alarm poisoned".to_string())
                        .and_then(|mut alarm| alarm.advance(cx.now()));
                    if let Err(error) = result { cx.fail(error); return; }
                    if let Err(error) = cx.power_off() { cx.fail(error); return; }
                    self.state.power_off_due = None;
                }
                Ok(None) => cx.fail(format!(
                    "supply off: power hold FFF88084 bit 0 released for {POWER_HOLD_RELEASE_LIMIT_TICKS} ticks"
                )),
                Err(error) => cx.fail(error),
            }
        }
    }
    fn next_deadline(&self, _: u64) -> Option<u64> {
        [self.state.output_due, self.state.power_off_due]
            .into_iter()
            .flatten()
            .min()
    }
    fn pollable(&self, address: u64, size: u8) -> bool {
        Self::index(address, size).is_some()
            || size == 4 && matches!(address, SD_TIMING | WAKE_TIMER | SLEEP_CONTROL)
    }
    fn save(&self) -> serde_json::Value {
        serde_json::to_value(&self.state).unwrap()
    }
    fn restore(&mut self, value: &serde_json::Value) -> Result<(), String> {
        let state: State = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        self.validate_state(&state)?;
        self.state = state;
        self.publish();
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_host::{TestHost, advance, read, write};
    fn clock() -> Clocks {
        Clocks::new(
            Options {
                output_settle_ticks: 5,
                reset_ready: true,
            },
            (),
        )
        .unwrap()
    }
    #[test]
    fn declared_power_peer_receives_the_actual_hold_writes() {
        use super::super::wake_alarm::{Supply, WakeAlarm};
        let mut clock = clock();
        let power = Arc::new(Mutex::new(
            WakeAlarm::new(12_000_000, POWER_HOLD_RELEASE_LIMIT_TICKS).unwrap(),
        ));
        clock.output().attach_power(power.clone()).unwrap();
        assert!(clock.output().attach_power(power.clone()).is_err());
        let mut host = TestHost::new();
        write(&mut clock, &mut host, BASE + 0x84, 1);
        write(&mut clock, &mut host, BASE + 0x84, 0);
        let mut peer = power.lock().unwrap();
        assert_eq!(peer.next_deadline(), Some(POWER_HOLD_RELEASE_LIMIT_TICKS));
        peer.advance(POWER_HOLD_RELEASE_LIMIT_TICKS).unwrap();
        assert_eq!(peer.supply(), Supply::Off);
    }
    #[test]
    fn connected_power_off_pauses_the_runtime_at_the_release_deadline() {
        use super::super::wake_alarm::{Supply, WakeAlarm};
        let mut clock = clock();
        let power = Arc::new(Mutex::new(
            WakeAlarm::new(12_000_000, POWER_HOLD_RELEASE_LIMIT_TICKS).unwrap(),
        ));
        clock.output().attach_power(power.clone()).unwrap();
        let mut host = TestHost::new();
        write(&mut clock, &mut host, BASE + 0x84, 0);
        advance(&mut clock, &mut host, POWER_HOLD_RELEASE_LIMIT_TICKS);
        assert_eq!(host.effects, ["power-off"]);
        assert_eq!(power.lock().unwrap().supply(), Supply::Off);
        assert_eq!(clock.next_deadline(host.now), None);
    }
    #[test]
    fn released_power_hold_cuts_the_supply_unless_held_again() {
        let hold = BASE + 0x84;
        let mut clock = clock();
        let mut host = TestHost::new();
        write(&mut clock, &mut host, hold, 1);
        write(&mut clock, &mut host, hold, 0);
        assert_eq!(clock.next_deadline(0), Some(POWER_HOLD_RELEASE_LIMIT_TICKS));
        advance(&mut clock, &mut host, 13_430);
        write(&mut clock, &mut host, hold, 1);
        assert_eq!(clock.next_deadline(host.now), None);
        write(&mut clock, &mut host, hold, 0);
        let due = host.now + POWER_HOLD_RELEASE_LIMIT_TICKS;
        let saved = clock.save();
        let mut restored = self::clock();
        restored.restore(&saved).unwrap();
        assert_eq!(restored.next_deadline(host.now), Some(due));
        host.now = due;
        let mut cx = host.cx();
        restored.advance(&mut cx);
        assert!(
            cx.refusal()
                .is_some_and(|reason| reason.contains("supply off"))
        );
    }
    #[test]
    fn sd_timing_rmw_preserves_other_fields_and_restores_without_a_deadline() {
        let mut clock = clock();
        let mut host = TestHost::new();
        write(&mut clock, &mut host, SD_TIMING, 0xa5a40025);
        let old = read(&mut clock, &mut host, SD_TIMING) as u32;
        let programmed = (old & 0xfffc003f) | (9 << 6) | (0x83 << 10);
        write(&mut clock, &mut host, SD_TIMING, programmed);
        assert_eq!(read(&mut clock, &mut host, SD_TIMING), programmed as u64);
        assert_eq!(programmed & 0xfffc003f, old & 0xfffc003f);
        let saved = clock.save();
        let mut restored = self::clock();
        restored.restore(&saved).unwrap();
        assert_eq!(read(&mut restored, &mut host, SD_TIMING), programmed as u64);
        assert_eq!(restored.next_deadline(0), None);
        assert_eq!(restored.save(), saved);
        let mut cx = host.cx();
        restored.write(&mut cx, SD_TIMING, 2, 0);
        assert!(cx.is_unmodeled());
        assert_eq!(restored.save(), saved);
    }
    #[test]
    fn output_request_settles_at_its_guest_deadline_and_cancels_on_disable() {
        let mut clock = clock();
        let mut host = TestHost::new();
        write(&mut clock, &mut host, BASE + 0x48, REQUEST);
        assert_eq!(clock.next_deadline(0), Some(5));
        let before = clock.save();
        advance(&mut clock, &mut host, 4);
        assert_eq!(clock.save(), before);
        assert_eq!(read(&mut clock, &mut host, BASE + 0x48), REQUEST as u64);
        advance(&mut clock, &mut host, 5);
        assert_eq!(
            read(&mut clock, &mut host, BASE + 0x48),
            (REQUEST | READY) as u64
        );
        write(&mut clock, &mut host, BASE + 0x48, REQUEST);
        assert_eq!(clock.next_deadline(5), None);
        assert_eq!(
            read(&mut clock, &mut host, BASE + 0x48),
            (REQUEST | READY) as u64
        );
        write(&mut clock, &mut host, BASE + 0x48, 0);
        write(&mut clock, &mut host, BASE + 0x48, REQUEST);
        assert_eq!(clock.next_deadline(5), Some(10));
        write(&mut clock, &mut host, BASE + 0x48, 0);
        advance(&mut clock, &mut host, 100);
        assert_eq!(read(&mut clock, &mut host, BASE + 0x48), 0);
    }
    #[test]
    fn divider_status_and_ownership_are_explicit_and_inspection_is_pure() {
        let mut clock = clock();
        let mut host = TestHost::new();
        for offset in [0, 4, 0x20] {
            write(&mut clock, &mut host, BASE + offset, REQUEST | 7);
            assert_eq!(read(&mut clock, &mut host, BASE + offset), 7);
        }
        for offset in [0x38, 0x40] {
            write(&mut clock, &mut host, BASE + offset, REQUEST);
            assert_eq!(
                read(&mut clock, &mut host, BASE + offset),
                (REQUEST | READY) as u64
            );
        }
        for address in [
            0xfff5002c,
            0xfff5102c,
            BASE + 0x3c,
            BASE + 0x44,
            BASE + 0x100,
        ] {
            let mut cx = host.cx();
            clock.read(&mut cx, address, 4);
            assert!(cx.is_unmodeled());
        }
        write(&mut clock, &mut host, BASE + 0x48, REQUEST);
        host.now = 10;
        let saved = clock.save();
        assert_eq!(clock.save(), saved);
        assert_eq!(clock.next_deadline(10), Some(5));
        assert_eq!(saved["registers"][OUTPUT], REQUEST);
        clock.restore(&saved).unwrap();
        assert_eq!(clock.save(), saved);
    }
    #[test]
    fn invented_ready_bits_invalid_states_and_deadline_overflow_refuse() {
        let mut clock = clock();
        let mut host = TestHost::new();
        for (address, value) in [(BASE + 0x48, READY), (BASE + 0x48, 1), (BASE + 0x80, 0)] {
            let before = clock.save();
            let mut cx = host.cx();
            clock.write(&mut cx, address, 4, value as u64);
            assert!(cx.refusal().is_some());
            assert_eq!(clock.save(), before);
        }
        host.now = u64::MAX;
        let mut cx = host.cx();
        clock.write(&mut cx, BASE + 0x48, 4, REQUEST as u64);
        assert!(cx.refusal().unwrap().contains("overflow"));
        let mut state = clock.save();
        state["output_due"] = serde_json::json!(1);
        assert!(clock.restore(&state).is_err());
    }
}
