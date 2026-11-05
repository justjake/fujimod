//! Explicit selection of a body's physical key model.
//!
//! Each implementation is a separate module with its own declared wiring. A
//! body names exactly one; nothing here chooses by firmware or camera identity.
use crate::{ladder_keys, resistor_keys};
use fujimod_emulator_devices::parts::controls::Controls;
use fujimod_emulator_devices::parts::pins::DriveInput;
use serde::Deserialize;
use std::{sync::Arc, time::Duration};

pub use resistor_keys::Key;

/// A key model the session can queue physical edges into.
pub trait KeyInput: Controls {
    /// Begin guest time at `now`; edges queued earlier become relative to it.
    fn start(&self, now: Duration);
    /// Queue one physical edge. Edges the model has no declared wiring or
    /// band for are refused, never approximated.
    fn enqueue(&self, at: Duration, key: Key, pressed: bool) -> Result<(), String>;
}

impl KeyInput for resistor_keys::ResistorKeys {
    fn start(&self, now: Duration) {
        resistor_keys::ResistorKeys::start(self, now)
    }
    fn enqueue(&self, at: Duration, key: Key, pressed: bool) -> Result<(), String> {
        resistor_keys::ResistorKeys::enqueue(self, at, key, pressed)
    }
}

impl KeyInput for ladder_keys::LadderKeys {
    fn start(&self, now: Duration) {
        ladder_keys::LadderKeys::start(self, now)
    }
    fn enqueue(&self, at: Duration, key: Key, pressed: bool) -> Result<(), String> {
        ladder_keys::LadderKeys::enqueue(self, at, key, pressed)
    }
}

/// The declared key model of one body. Transports that accept a model as
/// data read it as the model's options tagged with its `kind`
/// (`resistor-keys` or `ladder-keys`).
#[derive(Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ControlsOptions {
    /// GPIO shutter and dial pads plus seven keys sampled on two ADC channels.
    ResistorKeys(resistor_keys::Options),
    /// Keys read as ADC resistor-ladder bands or power-controller register
    /// ladders, each with its own declared band.
    LadderKeys(ladder_keys::Options),
}

impl ControlsOptions {
    /// Construct the declared model. GPIO-driving models take the board's pins.
    pub fn open(self, pins: Arc<dyn DriveInput>) -> Result<Arc<dyn KeyInput>, String> {
        Ok(match self {
            Self::ResistorKeys(options) => Arc::new(resistor_keys::ResistorKeys::new(pins, options)?),
            Self::LadderKeys(options) => Arc::new(ladder_keys::LadderKeys::new(options)?),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::xp5::pins::Pins;
    use serde_json::json;

    // Synthetic ladder: the band and sample are test inputs.
    fn ladder() -> serde_json::Value {
        json!({"kind":"ladder-keys","ladders":[{"wire":{"kind":"power-controller-register","register":52,"released":1023},
            "levels":1024,"rungs":[{"key":"Menu","band_low":527,"band_high":582,"sample":554,"basis":"test fixture"}]}]})
    }

    #[test]
    fn a_tagged_model_opens_its_own_implementation() {
        let options: ControlsOptions = serde_json::from_value(ladder()).unwrap();
        let keys = options.open(Pins::shared()).unwrap();
        keys.start(Duration::ZERO);
        assert_eq!(keys.register_sample(52), Some(1023));
        keys.enqueue(Duration::from_nanos(1), Key::Menu, true).unwrap();
        keys.advance(1);
        assert_eq!(keys.register_sample(52), Some(554));
        let error = keys.enqueue(Duration::from_nanos(1), Key::Back, true).unwrap_err();
        assert!(error.contains("no declared ladder rung"), "{error}");
    }

    #[test]
    fn unknown_kinds_and_fields_are_refused() {
        let mut value = ladder();
        value["kind"] = json!("x100vi");
        assert!(serde_json::from_value::<ControlsOptions>(value).is_err());
        let mut value = ladder();
        value["resistor_keys"] = json!({});
        assert!(serde_json::from_value::<ControlsOptions>(value).is_err());
    }
}
