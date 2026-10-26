//! Reusable part assemblies and their declared electrical attachments.

use serde::{Deserialize, Serialize};

/// Electrical installation of a reusable part assembly.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct PartAttachment {
    /// Assembly label used when constructing a device graph.
    pub label: Option<String>,
    /// Pulse-counter and timer channels consumed by the assembly.
    pub pulse: Option<crate::reference::PulseInterface>,
    /// Interrupt outputs for the pulse channels, in channel order.
    pub interrupts: Option<Vec<Interrupt>>,
    /// Interrupts raised when each motor completes its movement.
    pub completion_interrupts: Option<Vec<Interrupt>>,
    /// Pin edges routed to processor interrupt inputs.
    pub gpio_interrupts: Option<Vec<PinInterrupt>>,
    /// Home-switch GPIO pins, indexed by motor.
    pub motor_home_pins: Option<Vec<Option<Pin>>>,
    /// Encoder GPIO pins in phase order.
    pub encoder_pins: Option<Vec<Pin>>,
    /// Actuator drive GPIO pins in bit order.
    pub drive_pins: Option<Vec<Pin>>,
    /// Actuator contact GPIO pins in contact order.
    pub contact_pins: Option<Vec<Pin>>,
    /// Actuator position at reset.
    pub initial_position: Option<i32>,
    /// Pulse-to-position feedback bridge, when installed.
    pub position_bridge: Option<PositionBridge>,
    /// Zoom motor pulse and home routes.
    pub zoom: Option<ZoomAttachment>,
    /// Serial routes and the selected reusable endpoints.
    pub serial_routes: Option<Vec<SerialAttachment>>,
    /// Qualification of these declared attachment facts.
    pub basis: Option<String>,
    /// Known limits of the assembly.
    pub limits: Vec<String>,
}

/// A GPIO pad on a body.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "PartAssemblyPin"))]
pub struct Pin {
    /// GPIO port number.
    pub port: u8,
    /// Pin number within its port.
    pub pin: u8,
}

/// A processor interrupt input.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "PartAssemblyInterrupt"))]
pub struct Interrupt {
    /// Processor index.
    pub core: u8,
    /// Interrupt line on that processor.
    pub line: u32,
}

/// A pad edge connected to interrupt inputs.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "PartAssemblyPinInterrupt"))]
pub struct PinInterrupt {
    /// GPIO input pad.
    pub pin: Pin,
    /// Edge that raises the interrupt.
    pub edge: Edge,
    /// Receiving processor inputs.
    pub outputs: Vec<Interrupt>,
}

/// Electrical transition recognized by an interrupt route.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", schemars(rename = "PartAssemblyEdge"))]
pub enum Edge {
    /// Low-to-high transition.
    Rising,
    /// High-to-low transition.
    Falling,
    /// Either transition.
    Both,
}

/// Serial-controller route to an installed endpoint.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "PartAssemblySerialAttachment"))]
pub struct SerialAttachment {
    /// Physical address of the serial controller.
    pub controller_address: u64,
    /// Accepted serial register configurations.
    pub formats: Vec<SerialFormat>,
    /// GPIO conditions selecting this endpoint.
    pub selection: Vec<PinLevel>,
    /// Declared transfer latency in virtual ticks.
    pub transfer_ticks: u64,
    /// Controller queue behavior.
    pub queue: Queue,
    /// Reusable endpoint model selected by identity.
    pub endpoint_part_id: String,
}

/// SPI register values and transfer width.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "PartAssemblySerialFormat"))]
pub struct SerialFormat {
    /// Control register value.
    pub control: u32,
    /// Configuration register value.
    pub configuration: u32,
    /// Word width in bits.
    pub bits: u8,
    /// Whether this format receives data.
    pub receive: bool,
}

/// A GPIO pad level.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "PartAssemblyPinLevel"))]
pub struct PinLevel {
    /// Selected GPIO pad.
    pub pin: Pin,
    /// Required high level, or low when false.
    pub high: bool,
}

/// Supported serial queue behavior.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schema", schemars(rename = "PartAssemblyQueue"))]
pub enum Queue {
    /// One outstanding transfer.
    Single,
    /// Ordered transfer queue.
    Fifo,
}

/// A physical pulse feedback path.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "PartAssemblyPositionBridge"))]
pub struct PositionBridge {
    /// Source pulse channel.
    pub pulse_channel: u8,
    /// Position at reset.
    pub initial_position: i32,
    /// Pad carrying the pulse output.
    pub pulse_pin: Pin,
    /// Pin multiplexer function.
    pub pulse_function: u8,
    /// Position scale for one pulse step.
    pub position_units_per_step: u32,
    /// Interrupt clocking feedback.
    pub clock_interrupt: Interrupt,
}

/// Zoom motor signal routes.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename = "PartAssemblyZoomAttachment"))]
pub struct ZoomAttachment {
    /// Pulse output channel.
    pub pulse_channel: u8,
    /// Feedback counter channel.
    pub counter_channel: u8,
    /// Pad carrying the home switch.
    pub home_pin: Pin,
}

impl PartAttachment {
    /// Reject impossible pin, interrupt and serial routes before assembly.
    pub fn validate(&self) -> Result<(), String> {
        let pin = |pin: &Pin| {
            if pin.port > 15 || pin.pin > 15 {
                Err("part attachment GPIO pad is out of range".to_owned())
            } else {
                Ok(())
            }
        };
        for value in self
            .motor_home_pins
            .iter()
            .flatten()
            .flatten()
            .chain(self.encoder_pins.iter().flatten())
            .chain(self.drive_pins.iter().flatten())
            .chain(self.contact_pins.iter().flatten())
        {
            pin(value)?;
        }
        for route in self.gpio_interrupts.iter().flatten() {
            pin(&route.pin)?;
            if route.outputs.is_empty() {
                return Err("part GPIO interrupt has no destination".into());
            }
        }
        for route in self.serial_routes.iter().flatten() {
            fujimod_catalog_identity::validate_id(&route.endpoint_part_id)?;
            if route.formats.is_empty() || route.transfer_ticks == 0 {
                return Err("part serial route has no format or transfer latency".into());
            }
            for format in &route.formats {
                if !matches!(format.bits, 8 | 16 | 32) {
                    return Err("part serial route has an unsupported word width".into());
                }
            }
            for selection in &route.selection {
                pin(&selection.pin)?;
            }
        }
        if let Some(bridge) = &self.position_bridge {
            pin(&bridge.pulse_pin)?;
            if bridge.position_units_per_step == 0 {
                return Err("part position bridge has a zero scale".into());
            }
        }
        if let Some(zoom) = &self.zoom {
            pin(&zoom.home_pin)?;
        }
        Ok(())
    }
}
