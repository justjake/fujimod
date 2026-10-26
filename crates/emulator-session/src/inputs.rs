//! Host electrical transitions through the checked device graph.
use super::Session;
use super::contact_driver::{ControlChoice, ControlInput, QueueState};
use fujimod_emulator_devices::parts::pins::Pin;
use fujimod_emulator_devices::parts::switch_panel::{Signal, Spec, SwitchPanel};
use std::collections::BTreeMap;
use fujimod_emulator_devices::capability::{AnalogInputs, SerialStatus, TouchPanel};

/// Host-visible controls and their current positions.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
#[derive(tsify::Tsify)]
#[tsify(hashmap_as_object, missing_as_null)]
pub struct ControlState {
    pub inputs: Vec<ControlInput>,
    #[tsify(type = "Record<string, string>")]
    pub positions: BTreeMap<String, String>,
    pub input_queue: Option<QueueState>,
}

impl Session {
    pub fn adc_samples(&self, node: &str) -> Result<Vec<u16>, String> {
        self.board
            .inspect_capability::<dyn AnalogInputs, _>(self.node(node)?, |adc| Ok(adc.samples().to_vec()))
    }
    pub fn set_adc_samples(&mut self, node: &str, samples: &[u16]) -> Result<(), String> {
        if samples.len() != self.adc_samples(node)?.len() {
            return Err("ADC input channel count differs".into());
        }
        self.board
            .edit_capability::<dyn AnalogInputs, _>(self.node(node)?, |adc| {
                for (channel, &value) in samples.iter().enumerate() {
                    adc.set_sample(channel, value)?;
                }
                Ok(())
            })
    }
    pub fn configure_controls(&mut self, spec: Spec) -> Result<(), String> {
        if self.controls.is_some() {
            return Err("physical controls are already connected".into());
        }
        let panel = SwitchPanel::new(spec)?;
        // Check every compiled position's endpoints, including choices that
        // are not initially selected. No unavailable target can masquerade as
        // a usable control merely because its initial value was harmless.
        for circuit in &panel.spec().circuits {
            for case in &circuit.cases {
                self.admit_signals(&case.signals)?;
            }
        }
        self.apply_signals(&panel.signals()?)?;
        self.controls = Some(panel);
        Ok(())
    }
    fn admit_signals(&self, signals: &[Signal]) -> Result<(), String> {
        let mut pins: BTreeMap<&str, Vec<(Pin, bool)>> = BTreeMap::new();
        for signal in signals {
            match signal {
                Signal::Gpio { node, pin, high } => {
                    pins.entry(node).or_default().push((*pin, *high))
                }
                Signal::Analog { node, channel, .. } => {
                    if *channel >= self.adc_samples(node)?.len() {
                        return Err("control targets an absent analog channel".into());
                    }
                }
                Signal::SerialStatus {
                    node,
                    route,
                    register,
                    ..
                } => {
                    self.board
                        .inspect_capability::<dyn SerialStatus, _>(
                            self.node(node)?,
                            |serial| serial.status_value(*route, *register).map(|_| ()),
                        )?;
                }
            }
        }
        for (node, changes) in pins {
            self.resources.pads(node)?.admit_requests(&changes)?;
            self.node(node)?;
        }
        Ok(())
    }
    fn apply_signals(&mut self, signals: &[Signal]) -> Result<(), String> {
        self.admit_signals(signals)?;
        let mut pins: BTreeMap<&str, Vec<(Pin, bool)>> = BTreeMap::new();
        let mut analog: BTreeMap<&str, Vec<(usize, u16)>> = BTreeMap::new();
        for signal in signals {
            match signal {
                Signal::Gpio { node, pin, high } => {
                    pins.entry(node).or_default().push((*pin, *high))
                }
                Signal::Analog {
                    node,
                    channel,
                    value,
                } => analog.entry(node).or_default().push((*channel, *value)),
                Signal::SerialStatus {
                    node,
                    route,
                    register,
                    mask,
                    value,
                } => {
                    self.board
                        .edit_capability::<dyn SerialStatus, _>(
                            self.node(node)?,
                            |serial| {
                                let old = serial.status_value(*route, *register)?;
                                serial.status_input(*route, *register, (old & !mask) | value)
                            },
                        )?;
                }
            }
        }
        for (node, changes) in pins {
            self.resources.pads(node)?.request_many(&changes)?;
            self.board.inputs_changed(self.node(node)?)?;
        }
        for (node, changes) in analog {
            self.board
                .edit_capability::<dyn AnalogInputs, _>(self.node(node)?, |adc| {
                    for &(channel, value) in &changes {
                        adc.set_sample(channel, value)?;
                    }
                    Ok(())
                })?;
        }
        Ok(())
    }
    pub fn set_position(&mut self, id: &str, position: &str) -> Result<(), String> {
        self.admit_position(id, position)?;
        self.apply_contact_position(id, position)
    }
    pub fn admit_position(&self, id: &str, position: &str) -> Result<(), String> {
        if self.contact_driver.as_ref().is_some_and(|d| d.owns(id)) {
            return Err("wait for queued physical transitions before setting this position".into());
        }
        self.controls
            .as_ref()
            .ok_or("physical controls are not connected")?
            .transition(id, position)
            .map(|_| ())
    }
    pub(super) fn apply_contact_position(
        &mut self,
        id: &str,
        position: &str,
    ) -> Result<(), String> {
        let next = self
            .controls
            .as_ref()
            .ok_or("physical controls are not connected")?
            .transition(id, position)?;
        self.apply_signals(&next.signals()?)?;
        self.controls = Some(next);
        Ok(())
    }
    pub fn control_state(&self) -> Result<ControlState, String> {
        let panel = self
            .controls
            .as_ref()
            .ok_or("physical controls are not connected")?;
        let inputs = self
            .contact_driver
            .as_ref()
            .map(|d| d.spec.catalog.clone())
            .unwrap_or_else(|| {
                panel
                    .spec()
                    .controls
                    .iter()
                    .map(|c| ControlInput {
                        id: c.id.clone(),
                        label: c.label.clone(),
                        kind: c.kind.clone(),
                        supported: c.unavailable.is_none(),
                        unavailable: c.unavailable.clone(),
                        positions: Some(
                            c.positions
                                .iter()
                                .map(|p| ControlChoice {
                                    id: p.clone(),
                                    label: p.clone(),
                                })
                                .collect(),
                        ),
                        initial: Some(c.initial.clone()),
                        relative: None,
                        width: None,
                        height: None,
                        stages: None,
                    })
                    .collect()
            });
        Ok(ControlState {
            inputs,
            positions: panel.positions().clone(),
            input_queue: self.contact_driver.as_ref().map(|d| d.state()),
        })
    }
}

impl Session {
    pub fn admit_touch(
        &self,
        node: &str,
        script: &[fujimod_emulator_devices::parts::touch_input::ContactInput],
    ) -> Result<(), String> {
        self.board
            .inspect_capability::<dyn TouchPanel, _>(
                self.node(node)?,
                |bus| bus.admit_touch(self.board.now(), script),
            )
    }
    pub fn touch(
        &mut self,
        node: &str,
        script: &[fujimod_emulator_devices::parts::touch_input::ContactInput],
    ) -> Result<(), String> {
        let now = self.board.now();
        self.board
            .edit_capability::<dyn TouchPanel, _>(self.node(node)?, |bus| {
                bus.touch(now, script)
            })
    }
    pub fn rotation(&self, id: &str, steps: i32) -> Result<(String, i32), String> {
        use super::contact_driver::Interface;
        let (node, sign) = self
            .contact_driver
            .as_ref()
            .and_then(|d| {
                d.spec.interfaces.iter().find_map(|i| match i {
                    Interface::Rotary {
                        id: found,
                        node,
                        cycles_per_step,
                    } if found == id => Some((node, *cycles_per_step)),
                    _ => None,
                })
            })
            .ok_or("physical rotary control is unavailable")?;
        let cycles = steps.checked_mul(sign).ok_or("rotary step overflow")?;
        self.resources
            .encoder(node)?
            .admit_cycles(self.board.now(), cycles)?;
        Ok((node.clone(), cycles))
    }
    pub fn touch_input(
        &self,
        id: &str,
        phase: &str,
        x: u16,
        y: u16,
    ) -> Result<
        (
            String,
            Vec<fujimod_emulator_devices::parts::touch_input::ContactInput>,
        ),
        String,
    > {
        use super::contact_driver::Interface;
        use fujimod_emulator_devices::parts::touch_input::{ContactInput, Phase};
        let (node, width, height, offsets) = self
            .contact_driver
            .as_ref()
            .and_then(|d| {
                d.spec.interfaces.iter().find_map(|i| match i {
                    Interface::Touch {
                        id: found,
                        node,
                        width,
                        height,
                        offsets,
                    } if found == id => Some((node, *width, *height, *offsets)),
                    _ => None,
                })
            })
            .ok_or("physical touch input is unavailable")?;
        if x >= width || y >= height {
            return Err("touch coordinates escape the declared display".into());
        }
        let now = self.board.now();
        let mut event = ContactInput {
            tick: now,
            contact: 0,
            phase: match phase {
                "press" | "tap" => Phase::Down,
                "drag" => Phase::Move,
                "release" => Phase::Up,
                _ => return Err("unknown touch phase".into()),
            },
            x: (i32::from(x) - i32::from(offsets[0])).clamp(0, 4095) as u16,
            y: (i32::from(y) - i32::from(offsets[1])).clamp(0, 4095) as u16,
        };
        let mut script = vec![event];
        if phase == "tap" {
            let period = self
                .board
                .inspect_capability::<dyn TouchPanel, _>(
                    self.node(node)?,
                    |bus| bus.touch_period(),
                )?;
            event.tick = now.checked_add(period).ok_or("touch deadline overflow")?;
            event.phase = Phase::Up;
            script.push(event);
        }
        self.admit_touch(node, &script)?;
        Ok((node.clone(), script))
    }
}
