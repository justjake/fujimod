//! Physical key transitions on the native session timeline.
use crate::{AdcKey, calibrated_key, calibrated_sample, host};
use fujimod_catalog::reference::{ConnectionSignal, ResolvedSelection};
use fujimod_catalog_configuration::RunInputs;
use fujimod_catalog_controls::{ElectricalInput, FirmwareControlIds, InputTarget};
use fujimod_catalog_resolution::ResolvedConfiguration;
use fujimod_emulator_session::Session;
use std::collections::{BTreeMap, VecDeque};

#[derive(Clone)]
pub struct DigitalKey {
    pub port: u8,
    pub pin: u8,
    pub active: bool,
}

pub fn digital_key(reference: &ResolvedSelection, name: &str) -> Result<Option<DigitalKey>, String> {
    let controls = reference.control_ids.clone().map(serde_json::from_value::<FirmwareControlIds>)
        .transpose().map_err(|error| format!("firmware control IDs: {error}"))?;
    let found = controls.iter().flat_map(|controls| &controls.inputs).find(|input| {
        input.debug_key.as_ref().is_some_and(|key| key.eq_ignore_ascii_case(name))
            && matches!(input.target, InputTarget::Control { .. })
    });
    let (connection_id, expected) = match found.and_then(|input| input.electrical.as_ref()) {
        Some(ElectricalInput::Gpio { connection_id, expected_level, .. }) =>
            (connection_id.as_str(), Some(*expected_level)),
        Some(_) => return Ok(None),
        None => (name, None),
    };
    let Some(connection) = reference.body.connections.iter().find(|wire| wire.id == connection_id) else {
        return Ok(None);
    };
    let [ConnectionSignal::Gpio { port, pin, active_level }] = connection.signals.as_slice() else {
        return Ok(None);
    };
    if expected.is_some_and(|expected| expected != *active_level) {
        return Err(format!("key {name} disagrees with its declared GPIO polarity"));
    }
    Ok(Some(DigitalKey { port: *port, pin: *pin, active: *active_level }))
}

enum Input {
    Digital(DigitalKey),
    Analog(AdcKey),
}

pub struct Exr2Keys {
    gpio: String,
    pending: VecDeque<(u64, Input, bool)>,
    held: BTreeMap<usize, (String, u16)>,
}

impl Exr2Keys {
    pub fn new(resolved: &ResolvedConfiguration, edges: Vec<host::KeyEdge>) -> Result<Self, String> {
        let Some(RunInputs::Exr2(inputs)) = &resolved.configuration.inputs else {
            return Err("EXR II key schedule requires EXR II input wiring".into());
        };
        let pending = edges.into_iter().map(|edge| {
            let wire = match digital_key(&resolved.reference, &edge.name)? {
                Some(pin) => Input::Digital(pin),
                None => Input::Analog(calibrated_key(resolved, inputs, &edge.name.to_ascii_uppercase())?),
            };
            Ok((host::duration_ticks(edge.delay, crate::EXR2_TICKS_PER_SECOND), wire, edge.pressed))
        }).collect::<Result<_, String>>()?;
        Ok(Self { gpio: crate::x70::node_for_block(&inputs.gpio.block_id)?, pending, held: BTreeMap::new() })
    }

    pub fn gpio_input(&self, session: &mut Session, port: u8, pin: u8, high: bool) -> Result<(), String> {
        session.gpio(&self.gpio, port, pin, high)
    }

    pub fn advance(&mut self, session: &mut Session, start: u64) -> Result<Option<u64>, String> {
        while let Some((delay, wire, press)) = self.pending.front() {
            let at = start.saturating_add(*delay);
            if session.board.now() < at {
                return Ok(Some(at));
            }
            match wire {
                Input::Digital(wire) => self.gpio_input(session, wire.port, wire.pin, wire.active == *press)?,
                Input::Analog(key) => {
                    let mut samples = session.adc_samples(&key.adc)?;
                    let sample = samples.get_mut(key.channel).ok_or("absent key ADC channel")?;
                    if *press {
                        if self.held.contains_key(&key.channel) {
                            return Err(format!("two keys hold ADC channel {}", key.channel));
                        }
                        self.held.insert(key.channel, (key.key_id.clone(), *sample));
                        *sample = calibrated_sample(session, key)?;
                    } else if let Some((name, old)) = self.held.remove(&key.channel) {
                        if name != key.key_id {
                            return Err(format!("cannot release {} while {name} holds its ADC channel", key.key_id));
                        }
                        *sample = old;
                    }
                    session.set_adc_samples(&key.adc, &samples)?;
                }
            }
            self.pending.pop_front();
        }
        Ok(None)
    }
}
