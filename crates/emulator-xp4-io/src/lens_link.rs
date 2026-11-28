//! The XP4 SPI and GPIO connection to an X-mount lens.
//!
//! The body supplies power over two GPIO outputs. The lens drives detect and
//! sense, and pulls ready low during startup and operations. The GPIO bank
//! applies the scheduled ready changes and routes the ready interrupt.
use crate::spi::Input;
use fujimod_emulator_devices::{checkpoint::Checkpoint, parts::gpio::InputIrq};
use fujimod_emulator_ip::socionext::gpio::{OutputWatcher, Pins, SharedPins, SharedWatcher};
use fujimod_emulator_parts_io::serial::{Frame, Wire};
pub use fujimod_emulator_parts_lens::x_mount_model::Timing;
use fujimod_emulator_parts_lens::x_mount_model::{Port, Spec, UNDRIVEN_BYTE};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

const KIND: &str = "xp4-lens-link";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pad {
    pub port: usize,
    pub pin: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Signal {
    pub port: usize,
    pub pin: usize,
    pub active_high: bool,
}

/// Explicit wiring and model inputs for one XP4 lens connector.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub base: u64,
    pub miso_high: bool,
    pub detect: Signal,
    pub ready: InputIrq,
    pub supplies: [Signal; 2],
    pub sense: Vec<Pad>,
    pub sense_unpowered: Vec<bool>,
    pub timing: Timing,
    pub lens: String,
    pub mounted: bool,
}

impl Config {
    fn validate(&self) -> Result<(), String> {
        let pads = [
            (self.detect.port, self.detect.pin),
            (self.ready.port, self.ready.pin),
        ]
        .into_iter()
        .chain(self.supplies.iter().map(|pad| (pad.port, pad.pin)))
        .chain(self.sense.iter().map(|pad| (pad.port, pad.pin)));
        let mut seen = std::collections::BTreeSet::new();
        for (port, pin) in pads {
            if port >= 16 || pin >= 16 || !seen.insert((port, pin)) {
                return Err("Invalid or repeated GPIO pad in XP4 lens wiring".into());
            }
        }
        if self.sense.is_empty() || self.sense.len() != self.sense_unpowered.len() {
            return Err("XP4 lens sense pads and levels must match".into());
        }
        if !(32..512).contains(&self.ready.line) {
            return Err("XP4 lens ready requires a shared GIC input".into());
        }
        Ok(())
    }
}

struct State {
    config: Config,
    lens: Port,
}

impl State {
    fn supplies_on(&self, pins: &Pins) -> bool {
        self.config.supplies.iter().all(|pad| {
            let output = pins.register(pad.port, 0) | pins.register(pad.port, 1);
            (output & (1 << pad.pin) != 0) == pad.active_high
        })
    }

    fn apply_presence(&self, pins: &mut Pins) {
        let detect = self.config.detect;
        pins.set_input(
            detect.port,
            detect.pin,
            self.config.mounted == detect.active_high,
        );
        if self.config.mounted {
            for (pad, &high) in self.config.sense.iter().zip(&self.config.sense_unpowered) {
                pins.set_input(pad.port, pad.pin, high);
            }
        }
        let ready = self.config.ready;
        pins.set_input(ready.port, ready.pin, true);
    }
}

impl OutputWatcher for State {
    fn output_changed(&mut self, pins: &mut Pins, port: usize, pin: usize, _high: bool, now: u64) {
        if !self.config.mounted
            || !self
                .config
                .supplies
                .iter()
                .any(|pad| (pad.port, pad.pin) == (port, pin))
        {
            return;
        }
        let powered = self.supplies_on(pins);
        if powered == self.lens.powered() {
            return;
        }
        self.lens.tick(now, powered);

        let ready = self.config.ready;
        pins.cancel_inputs(ready.port, ready.pin);
        if powered {
            let low = now.saturating_add(self.config.timing.startup_ticks);
            let high = low.saturating_add(self.config.timing.busy_ticks);
            pins.drive_input(low, ready.port, ready.pin, false).unwrap();
            pins.drive_input(high, ready.port, ready.pin, true).unwrap();
        } else {
            pins.drive_input(now, ready.port, ready.pin, true).unwrap();
        }
    }
}

/// The SPI input and the external aperture-ring input share the same lens.
#[derive(Clone)]
pub struct LensLink {
    pins: SharedPins,
    state: Arc<Mutex<State>>,
}

impl LensLink {
    pub fn new(pins: SharedPins, config: Config) -> Result<Self, String> {
        config.validate()?;
        let spec = Spec::preset(&config.lens)
            .ok_or_else(|| format!("Unknown X-mount lens preset {}", config.lens))?;
        let lens = Port::new(spec, config.timing)?;
        let state = State { config, lens };
        let mut bank = pins.lock().map_err(|_| "GPIO pins poisoned")?;
        state.apply_presence(&mut bank);
        let state = Arc::new(Mutex::new(state));
        let watcher: SharedWatcher = state.clone();
        bank.watch(0, watcher.clone());
        bank.watch(1, watcher);
        drop(bank);
        Ok(Self { pins, state })
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, State>, String> {
        self.state
            .lock()
            .map_err(|_| "XP4 lens link poisoned".into())
    }

    pub fn set_aperture_ring(&self, ring: u16) -> Result<(), String> {
        let state = self.lock()?;
        if !state.config.mounted {
            return Err("XP4 lens mount is empty".into());
        }
        state.lens.set_aperture_ring(ring);
        Ok(())
    }
}

impl Input for LensLink {
    fn exchange(&mut self, bits: u32, tx: u16, tick: u64) -> Result<u16, String> {
        self.exchange_in_mode(bits, 3, tx, tick)
    }

    fn exchange_in_mode(
        &mut self,
        bits: u32,
        mode: u32,
        tx: u16,
        tick: u64,
    ) -> Result<u16, String> {
        if bits != 8 || mode != 3 {
            return Err("X-mount lens requires 8-bit SPI mode 3 transfers".into());
        }

        // GPIO output watchers take these locks in the same order.
        let mut pins = self.pins.lock().map_err(|_| "GPIO pins poisoned")?;
        let mut state = self.lock()?;
        let powered = state.lens.powered();
        state.lens.tick(tick, powered);
        if !state.lens.serving() {
            return Ok(if state.config.miso_high {
                UNDRIVEN_BYTE
            } else {
                0
            });
        }

        let previous = state.lens.ready_release_tick();
        let frame = Frame {
            bits: 8,
            mode: 3,
            word: tx,
        };
        let token = state.lens.begin(frame)?;
        let rx = state.lens.finish(token, frame, tick)?;
        let release = state.lens.ready_release_tick();
        if release != previous
            && let Some(until) = release
        {
            let ready = state.config.ready;
            pins.cancel_inputs(ready.port, ready.pin);
            pins.drive_input(tick, ready.port, ready.pin, false)?;
            pins.drive_input(until, ready.port, ready.pin, true)?;
        }
        Ok(rx)
    }

    fn checkpoint(&self) -> Result<Checkpoint, String> {
        let state = self.lock()?;
        Ok(Checkpoint::encode(KIND, &state.config)?.with_children(vec![state.lens.checkpoint()?]))
    }

    fn validate_checkpoint(&self, data: &Checkpoint) -> Result<(), String> {
        let state = self.lock()?;
        let [lens] = data.children() else {
            return Err("XP4 lens link checkpoint requires one lens".into());
        };
        if data.decode::<Config>(KIND)? != state.config || !data.blobs().is_empty() {
            return Err("XP4 lens link checkpoint differs".into());
        }
        state.lens.validate_checkpoint(lens)
    }

    fn restore_checkpoint(&mut self, data: &Checkpoint) -> Result<(), String> {
        self.validate_checkpoint(data)?;
        self.lock()?.lens.restore_checkpoint(&data.children()[0])
    }

    fn save(&self) -> Value {
        let state = self.state.lock().unwrap();
        json!({"kind": KIND, "config": state.config, "lens": state.lens.save()})
    }

    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let mut state = self.lock()?;
        if value["kind"] != KIND || value["config"] != json!(state.config) {
            return Err("XP4 lens link saved state differs".into());
        }
        state.lens.restore(&value["lens"])
    }
}
