//! Declared X-mount power/reset, serial pads and ready interrupt connection.
//!
//! Pad mux/direction/enable registers and interrupt sense registers are the
//! only control inputs. Startup duration and electrical interpretation remain
//! explicit inferred board inputs; no optical operation is qualified here.
use fujimod_emulator_devices::exr2::{gpio, irq};
use crate::serial_dma;
use fujimod_emulator_devices::{Cx, Device, checkpoint::Checkpoint, parts::pins::{Pin, Pins}};
use fujimod_emulator_parts_lens::{x_mount_lens, x_mount_link};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pad {
    pub pin: Pin,
    pub mux: u32,
}
impl Pad {
    fn validate(self, input: bool) -> Result<(), String> {
        self.pin.validate()?;
        if self.mux > 3 || (input && self.pin.port >= 16) {
            return Err("X-mount pad requires a physical GPIO function".into());
        }
        Ok(())
    }
    fn driven(self, pads: &gpio::Pads) -> Result<Option<bool>, String> {
        Ok(pads
            .pad_state(self.pin)?
            .driven_for_mux(self.mux)?
            .map(|v| v != 0))
    }
    fn input_enabled(self, pads: &gpio::Pads) -> Result<bool, String> {
        let state = pads.pad_state(self.pin)?;
        Ok(state.direction == Some(0) && state.enabled == Some(1) && state.mux == Some(self.mux))
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub supplies: [Pad; 2],
    pub reset: Pad,
    pub ready: Pad,
    pub ready_line: u32,
    pub clock: Pad,
    pub transmit: Pad,
    pub receive: Pad,
}
impl Options {
    fn validate(&self) -> Result<(), String> {
        let pads = [
            self.supplies[0],
            self.supplies[1],
            self.reset,
            self.ready,
            self.clock,
            self.transmit,
            self.receive,
        ];
        for (i, pad) in pads.iter().copied().enumerate() {
            pad.validate(i == 3 || i == 6)?;
            if pads[..i].iter().any(|other| other.pin == pad.pin) {
                return Err("X-mount connector requires distinct physical pads".into());
            }
        }
        if self.ready_line >= 128 {
            return Err("X-mount ready IRQ exceeds 128 sources".into());
        }
        Ok(())
    }
}
/// Edge selector 1/2/3 is rising/falling/either; level selector 1/2 is high/low.
/// An enabled unknown selector refuses rather than inventing an assertion.
fn signal(edge: bool, sense: u8, previous: bool, high: bool) -> Result<(bool, bool), String> {
    match (edge, sense) {
        (true, 1) => Ok((false, !previous && high)),
        (true, 2) => Ok((false, previous && !high)),
        (true, 3) => Ok((false, previous != high)),
        (false, 1) => Ok((true, high)),
        (false, 2) => Ok((true, !high)),
        _ => Err("unmodeled GPIO interrupt sense".into()),
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    levels: Vec<bool>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    options: Options,
    cores: Vec<usize>,
    state: State,
}

pub struct XMountLens {
    options: Options,
    gpio: gpio::Pads,
    controllers: Vec<(usize, irq::Inputs)>,
    port: x_mount_lens::Port,
    transport: serial_dma::Transport,
    state: State,
}
impl XMountLens {
    pub fn new(
        transport: serial_dma::Options,
        configuration: x_mount_link::Configuration,
        startup_ticks: u64,
        options: Options,
        gpio: gpio::Pads,
        controllers: Vec<(usize, irq::Inputs)>,
    ) -> Result<Self, String> {
        options.validate()?;
        if controllers.is_empty()
            || controllers.iter().enumerate().any(|(i, (core, _))| {
                *core > 1 || controllers[..i].iter().any(|(other, _)| other == core)
            })
        {
            return Err("X-mount ready requires unique per-core interrupt controllers".into());
        }
        let port = x_mount_lens::Port::new(configuration, startup_ticks)?;
        let transport =
            serial_dma::Nodes::with_wire(transport, Box::new(port.clone()))?.into_transport();
        let state = State {
            levels: vec![false; controllers.len()],
        };
        Ok(Self {
            options,
            gpio,
            controllers,
            port,
            transport,
            state,
        })
    }
    pub fn windows(&self) -> &[(u64, u64)] {
        self.transport.windows()
    }
    fn pins(&self) -> Result<x_mount_lens::Pins, String> {
        Ok(x_mount_lens::Pins {
            supplies: [
                self.options.supplies[0].driven(&self.gpio)?,
                self.options.supplies[1].driven(&self.gpio)?,
            ],
            reset_released: self.options.reset.driven(&self.gpio)?,
        })
    }
    fn transport_configured(&self) -> Result<bool, String> {
        Ok(self.options.clock.driven(&self.gpio)?.is_some()
            && self.options.transmit.driven(&self.gpio)?.is_some()
            && self.options.receive.input_enabled(&self.gpio)?)
    }
    fn signals(&self) -> Result<Vec<(bool, bool)>, String> {
        let enabled = self.options.ready.input_enabled(&self.gpio)?;
        let previous = self.gpio.input_level(self.options.ready.pin)?;
        let high = self.port.ready_high();
        self.controllers
            .iter()
            .map(|(_, inputs)| {
                let source = inputs.source(self.options.ready_line)?;
                if !enabled || !source.enabled {
                    return Ok((true, false));
                }
                signal(
                    source.source_config,
                    if source.source_config {
                        source.mode140
                    } else {
                        source.mode180
                    }
                    .unwrap_or(0),
                    previous,
                    high,
                )
            })
            .collect()
    }
    fn connector_deadline(&self, now: u64) -> Result<Option<u64>, String> {
        if (!self.port.ready_high() && !self.transport_configured()?)
            || self.gpio.input_level(self.options.ready.pin)? != self.port.ready_high()
            || self
                .signals()?
                .iter()
                .zip(&self.state.levels)
                .any(|(&(level, active), &before)| {
                    (level && active != before) || (!level && (active || before))
                })
        {
            return Ok(Some(now));
        }
        Ok(self.port.next_event_tick(now, self.pins()?))
    }
    fn update_connector(&mut self, cx: &mut Cx<'_>) -> Result<(), String> {
        self.port.tick(cx.now(), self.pins()?)?;
        if !self.port.ready_high() && !self.transport_configured()? {
            return Err(
                "Powered X-mount lens requires its declared SPI clock/TX/RX pad configuration"
                    .into(),
            );
        }
        let signals = self.signals()?;
        self.gpio
            .input(cx, self.options.ready.pin, self.port.ready_high())?;
        for (i, ((_, inputs), (level, active))) in self.controllers.iter().zip(signals).enumerate()
        {
            let high = level && active;
            inputs.level(self.options.ready_line, high)?;
            self.state.levels[i] = high;
            if !level && active {
                inputs.pulse(self.options.ready_line)?;
            }
        }
        Ok(())
    }
    fn saved(&self) -> Saved {
        Saved {
            options: self.options.clone(),
            cores: self.controllers.iter().map(|(core, _)| *core).collect(),
            state: State {
                levels: self.state.levels.clone(),
            },
        }
    }
}
impl Device for XMountLens {
    fn name(&self) -> &'static str {
        "exr2-x-mount-lens"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        self.transport.read(cx, address, size)
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        self.transport.write(cx, address, size, value);
    }
    fn next_deadline(&self, now: u64) -> Option<u64> {
        [
            self.connector_deadline(now).unwrap_or(Some(now)),
            self.transport.next_deadline(now),
        ]
        .into_iter()
        .flatten()
        .min()
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        let now = cx.now();
        let mut cursor = now;
        while let Some(at) = self.next_deadline(cursor).filter(|at| *at <= now) {
            cursor = at;
            cx.at_tick(at, |cx| {
                if let Err(error) = self.update_connector(cx) {
                    cx.fail(error);
                    return;
                }
                self.transport.advance(cx);
            });
            if cx.refusal().is_some() {
                return;
            }
        }
    }
    fn checkpoint(&self) -> Result<Checkpoint, String> {
        Ok(Checkpoint::encode(self.name(), &self.saved())?
            .with_children(vec![self.transport.checkpoint()?]))
    }
    fn validate_checkpoint(&self, saved: &Checkpoint) -> Result<(), String> {
        let candidate: Saved = saved.decode(self.name())?;
        if candidate.options != self.options
            || candidate.cores != self.saved().cores
            || candidate.state.levels.len() != self.controllers.len()
            || saved.children().len() != 1
            || !saved.blobs().is_empty()
        {
            return Err("X-mount connector checkpoint wiring differs".into());
        }
        self.transport.validate_checkpoint(&saved.children()[0])
    }
    fn restore_checkpoint(&mut self, saved: &Checkpoint) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        self.transport.restore_checkpoint(&saved.children()[0])?;
        self.state = saved.decode::<Saved>(self.name())?.state;
        Ok(())
    }
    fn save(&self) -> Value {
        json!({"connector":self.saved(),"transport":self.transport.save()})
    }
    fn fingerprint(&self) -> Result<Value, String> {
        Ok(self.save())
    }
}
#[cfg(test)]
mod tests;
