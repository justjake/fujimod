//! The X-mount lens behind XP4 SPI0 and GPIO pads.
//!
//! Observed wiring and protocol:
//! - Detect (Port 0, Pin 11): Active low (0 = lens mounted, 1 = unmounted).
//! - Power outputs (Port 1, Pin 12 and Port 1, Pin 13): Both driven high by camera
//!   to supply power to the lens.
//! - Sense pads (Port 11, Pin 0; Port 11, Pin 1; Port 11, Pin 2; Port 15, Pin 0):
//!   Provide the 1/0/1/1 presence pattern before power is applied.
//! - Ready pad (Port 15, Pin 1): Carries the lens ready/busy status, wired to GIC line 55.
//!   Unpowered/starting: high (idle).
//!   Startup handshake: pulled low after startup_ticks, released high after busy_ticks.
//!   Motion/operations: pulled low during focus/aperture travel, released high on completion.
//! - SPI controller (SPI0 at 0xfff32000): 8-bit SPI mode 3 framing carrying the X-mount
//!   link protocol (4-byte commands/polls/acknowledgments and 128-byte bulk identification reads).

use crate::spi::Input;
use fujimod_emulator_devices::checkpoint::Checkpoint;
use fujimod_emulator_ip::socionext::gpio::{OutputWatcher, Pins, SharedPins, SharedWatcher};
use fujimod_emulator_parts_io::serial::{Frame, Wire};
use fujimod_emulator_parts_lens::x_mount_model::{Port, Spec, Timing};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

const KIND: &str = "xp4-lens-link";

/// One GPIO pad.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pad {
    pub port: usize,
    pub pin: usize,
}

fn default_base() -> u64 {
    0xfff3_2000
}
fn default_detect() -> Pad {
    Pad { port: 0, pin: 11 }
}
fn default_ready() -> Pad {
    Pad { port: 15, pin: 1 }
}
fn default_power() -> Vec<Pad> {
    vec![Pad { port: 1, pin: 12 }, Pad { port: 1, pin: 13 }]
}
fn default_sense() -> Vec<Pad> {
    vec![
        Pad { port: 11, pin: 0 },
        Pad { port: 11, pin: 1 },
        Pad { port: 11, pin: 2 },
        Pad { port: 15, pin: 0 },
    ]
}
fn default_sense_unpowered() -> Vec<bool> {
    vec![true, false, true, true]
}
fn default_boot_ticks() -> u64 {
    // 1 ms at 54 MHz
    54_000
}
fn default_busy_ticks() -> u64 {
    // 1 ms at 54 MHz
    54_000
}
fn default_operation_ticks() -> u64 {
    // 1 ms at 54 MHz
    54_000
}
fn default_lens() -> String {
    "xf16-80".into()
}
fn default_mounted() -> bool {
    true
}

/// Declared wiring, timings and lens model for the XP4 X-mount lens link.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default = "default_base")]
    pub base: u64,
    #[serde(default)]
    pub idle: bool,
    #[serde(default = "default_detect")]
    pub detect: Pad,
    #[serde(default = "default_ready")]
    pub ready: Pad,
    #[serde(default = "default_power")]
    pub power: Vec<Pad>,
    #[serde(default = "default_sense")]
    pub sense: Vec<Pad>,
    #[serde(default = "default_sense_unpowered")]
    pub sense_unpowered: Vec<bool>,
    #[serde(default = "default_boot_ticks")]
    pub boot_ticks: u64,
    #[serde(default = "default_busy_ticks")]
    pub busy_ticks: u64,
    #[serde(default = "default_operation_ticks")]
    pub operation_ticks: u64,
    #[serde(default = "default_lens")]
    pub lens: String,
    #[serde(default = "default_mounted")]
    pub mounted: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            base: default_base(),
            idle: false,
            detect: default_detect(),
            ready: default_ready(),
            power: default_power(),
            sense: default_sense(),
            sense_unpowered: default_sense_unpowered(),
            boot_ticks: default_boot_ticks(),
            busy_ticks: default_busy_ticks(),
            operation_ticks: default_operation_ticks(),
            lens: default_lens(),
            mounted: default_mounted(),
        }
    }
}

impl Config {
    pub fn validate(&self) -> Result<(), String> {
        let pads = [self.detect, self.ready]
            .into_iter()
            .chain(self.power.iter().copied())
            .chain(self.sense.iter().copied());
        for p in pads {
            if p.port >= 16 || p.pin >= 16 {
                return Err("Invalid pad port or pin in lens link config".into());
            }
        }
        if self.sense.len() != self.sense_unpowered.len() {
            return Err("Sense pads and sense unpowered levels count mismatch".into());
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
struct SavedState {
    kind: String,
    config: Config,
    mounted: bool,
    powered: bool,
    ready_high: bool,
}

struct State {
    config: Config,
    mounted: bool,
    powered: bool,
    ready_high: bool,
    lens: Option<Port>,
}

impl State {
    fn supplies_on(&self, pins: &Pins) -> bool {
        self.config
            .power
            .iter()
            .all(|p| pins.output(p.port, p.pin).0)
    }

    fn apply_presence(&self, pins: &mut Pins) {
        if self.mounted {
            pins.set_input(self.config.detect.port, self.config.detect.pin, false);
            for (pad, &level) in self.config.sense.iter().zip(&self.config.sense_unpowered) {
                pins.set_input(pad.port, pad.pin, level);
            }
            pins.set_input(self.config.ready.port, self.config.ready.pin, true);
        } else {
            pins.set_input(self.config.detect.port, self.config.detect.pin, true);
            pins.set_input(self.config.ready.port, self.config.ready.pin, true);
        }
    }
}

impl OutputWatcher for State {
    fn output_changed(&mut self, pins: &mut Pins, port: usize, pin: usize, _high: bool, now: u64) {
        let at = |pad: &Pad| (pad.port, pad.pin) == (port, pin);
        if !self.config.power.iter().any(at) {
            return;
        }

        let powered = self.supplies_on(pins);
        if powered == self.powered {
            return;
        }
        self.powered = powered;

        if let Some(lens) = &self.lens {
            lens.tick(now, powered);
        }

        let ready = self.config.ready;
        if powered && self.mounted {
            let low = now.saturating_add(self.config.boot_ticks);
            let high = low.saturating_add(self.config.busy_ticks);
            pins.cancel_inputs(ready.port, ready.pin);
            let _ = pins.drive_input(low, ready.port, ready.pin, false);
            let _ = pins.drive_input(high, ready.port, ready.pin, true);
            self.ready_high = true;
        } else {
            pins.cancel_inputs(ready.port, ready.pin);
            let _ = pins.drive_input(now, ready.port, ready.pin, true);
            self.ready_high = true;
        }
    }
}

/// The X-mount lens link for XP4 SPI0 and GPIO pads.
#[derive(Clone)]
pub struct LensLink {
    pins: SharedPins,
    state: Arc<Mutex<State>>,
}

impl LensLink {
    pub fn new(pins: SharedPins, config: Config) -> Result<Self, String> {
        config.validate()?;

        let lens = if config.mounted && config.lens != "none" {
            let spec = Spec::preset(&config.lens)
                .ok_or_else(|| format!("Unknown lens preset: {}", config.lens))?;
            let timing = Timing {
                ticks_per_ms: 54_000,
                startup_ticks: config.boot_ticks,
                busy_ticks: config.busy_ticks,
                min_operation_ticks: config.operation_ticks,
            };
            Some(Port::new(spec, timing)?)
        } else {
            None
        };

        let state = Arc::new(Mutex::new(State {
            mounted: config.mounted,
            powered: false,
            ready_high: true,
            config,
            lens,
        }));

        let mut bank = pins.lock().map_err(|_| "GPIO pins poisoned")?;
        state.lock().unwrap().apply_presence(&mut bank);
        let watcher: SharedWatcher = state.clone();
        bank.watch(watcher);
        drop(bank);

        Ok(Self { pins, state })
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, State>, String> {
        self.state
            .lock()
            .map_err(|_| "Lens link state poisoned".to_owned())
    }

    pub fn set_mounted(&self, mounted: bool) -> Result<(), String> {
        let mut state = self.lock()?;
        state.mounted = mounted;
        let mut bank = self.pins.lock().map_err(|_| "GPIO pins poisoned")?;
        state.apply_presence(&mut bank);
        Ok(())
    }

    pub fn set_aperture_ring(&self, ring: u16) -> Result<(), String> {
        let state = self.lock()?;
        if let Some(lens) = &state.lens {
            lens.set_aperture_ring(ring);
        }
        Ok(())
    }

    pub fn diagnostic_state(&self) -> Value {
        if let Ok(state) = self.lock() {
            let lens_info = state.lens.as_ref().map(|l| {
                json!({
                    "name": l.name(),
                    "powered": l.powered(),
                    "serving": l.serving(),
                    "ready_high": l.ready_high(),
                    "identification_reads": l.identification_reads(),
                    "extended_reads": l.extended_reads(),
                    "catalog_reads": l.catalog_reads(),
                })
            });
            json!({
                "kind": KIND,
                "mounted": state.mounted,
                "powered": state.powered,
                "ready_high": state.ready_high,
                "lens": lens_info,
            })
        } else {
            json!({"kind": KIND, "error": "poisoned"})
        }
    }
}

impl Input for LensLink {
    fn exchange(&mut self, bits: u32, tx: u16, tick: u64) -> Result<u16, String> {
        if bits != 8 {
            return Err("X-mount lens requires 8-bit SPI transfers".into());
        }

        let mut state = self.lock()?;
        if !state.mounted || !state.powered {
            return Ok(if state.config.idle { 0xff } else { 0xff });
        }

        let Some(mut lens) = state.lens.clone() else {
            return Ok(0xff);
        };

        let token = lens.begin(Frame {
            bits: 8,
            mode: 3,
            word: tx,
        })?;
        let rx = lens.finish(
            token,
            Frame {
                bits: 8,
                mode: 3,
                word: tx,
            },
            tick,
        )?;

        let ready_high = lens.ready_high();
        if ready_high != state.ready_high {
            state.ready_high = ready_high;
            let ready = state.config.ready;
            if let Ok(mut bank) = self.pins.lock() {
                bank.cancel_inputs(ready.port, ready.pin);
                let _ = bank.drive_input(tick, ready.port, ready.pin, ready_high);
                if !ready_high
                    && let Some(until) = lens.next_event_tick(tick, state.powered)
                {
                    let _ = bank.drive_input(until, ready.port, ready.pin, true);
                }
            }
        }

        Ok(rx)
    }

    fn checkpoint(&self) -> Result<Checkpoint, String> {
        let state = self.lock()?;
        let saved = SavedState {
            kind: KIND.into(),
            config: state.config.clone(),
            mounted: state.mounted,
            powered: state.powered,
            ready_high: state.ready_high,
        };
        Checkpoint::encode(KIND, &saved)
    }

    fn validate_checkpoint(&self, data: &Checkpoint) -> Result<(), String> {
        let saved: SavedState = data.decode(KIND)?;
        let state = self.lock()?;
        if saved.kind != KIND || saved.config != state.config {
            return Err("X-mount lens link checkpoint differs".into());
        }
        Ok(())
    }

    fn restore_checkpoint(&mut self, data: &Checkpoint) -> Result<(), String> {
        self.validate_checkpoint(data)?;
        let saved: SavedState = data.decode(KIND)?;
        let mut state = self.lock()?;
        state.mounted = saved.mounted;
        state.powered = saved.powered;
        state.ready_high = saved.ready_high;
        Ok(())
    }

    fn save(&self) -> Value {
        self.diagnostic_state()
    }

    fn restore(&mut self, value: &Value) -> Result<(), String> {
        if value["kind"] != KIND {
            return Err("Invalid lens link saved state".into());
        }
        Ok(())
    }
}
