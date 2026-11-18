//! The parts of a session's device graph the contact compiler reads.
use serde_json::Value;

use crate::panel::{Target, Unresolved};

/// One readable status route of a serial controller.
pub struct SerialRoute {
    pub node: String,
    pub index: usize,
    pub base: u64,
    pub readable: bool,
    pub status_registers: Vec<u8>,
    /// Initial value of each register on the route.
    pub registers: Vec<(u8, u64)>,
}

/// The GPIO and ADC owners and serial status routes, with their initial levels.
pub struct Nodes<'a> {
    pub gpio: String,
    pub adc: String,
    pub serial: Vec<SerialRoute>,
    gpio_levels: Vec<u64>,
    adc_samples: Vec<u64>,
    /// Every device node, for interfaces outside the switch panel.
    pub all: Vec<&'a Value>,
    mappings: Vec<Mapping>,
    ram: Vec<(u64, u64)>,
}

struct Mapping {
    core: u64,
    address: u64,
    physical: u64,
    size: u64,
}

fn numbers(value: &Value) -> Vec<u64> {
    value
        .as_array()
        .map(|items| items.iter().filter_map(Value::as_u64).collect())
        .unwrap_or_default()
}

pub fn kind(node: &Value) -> &str {
    node["model"]["kind"].as_str().unwrap_or_default()
}

pub fn name(node: &Value) -> String {
    node["name"].as_str().unwrap_or_default().to_owned()
}

fn routes(node: &Value, kinds: &[&str]) -> Vec<SerialRoute> {
    node["model"]["routes"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .filter(|(_, route)| {
            kinds.contains(&route["endpoint"]["kind"].as_str().unwrap_or_default())
        })
        .map(|(index, route)| {
            let options = &route["endpoint"]["options"];
            SerialRoute {
                node: name(node),
                index,
                base: route["options"]["base"].as_u64().unwrap_or(u64::MAX),
                readable: options["readable"].as_bool().unwrap_or(false),
                status_registers: numbers(&options["status_registers"])
                    .into_iter()
                    .map(|r| r as u8)
                    .collect(),
                registers: options["registers"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|pair| Some((pair[0].as_u64()? as u8, pair[1].as_u64()?)))
                    .collect(),
            }
        })
        .collect()
}

/// Readable serial status routes of every serial controller with endpoints of `kinds`.
pub fn serial_routes<'a>(nodes: &[&'a Value], kinds: &[&str]) -> Vec<(&'a Value, SerialRoute)> {
    nodes
        .iter()
        .filter(|node| kind(node) == "spi-controller")
        .flat_map(|node| {
            routes(node, kinds)
                .into_iter()
                .map(move |route| (*node, route))
        })
        .collect()
}

impl<'a> Nodes<'a> {
    /// Read a session definition's device nodes and execution mappings.
    pub fn new(definition: &'a Value) -> Result<Self, String> {
        let all: Vec<&Value> = definition["devices"]["nodes"]
            .as_array()
            .ok_or("session definition has no device nodes")?
            .iter()
            .collect();
        let one = |wanted: &str| -> Result<&Value, String> {
            let found: Vec<_> = all.iter().filter(|node| kind(node) == wanted).collect();
            match found.as_slice() {
                [node] => Ok(**node),
                _ => Err("Contact compiler requires one declared GPIO and ADC owner".into()),
            }
        };
        let gpio = one("gpio")?;
        let adc = one("adc")?;
        let serial = serial_routes(&all, &["registers"])
            .into_iter()
            .map(|(_, route)| route)
            .collect();
        let execution = &definition["execution"];
        Ok(Self {
            gpio: name(gpio),
            adc: name(adc),
            serial,
            gpio_levels: numbers(&gpio["model"]["options"]["input_levels"]),
            adc_samples: numbers(&adc["model"]["options"]["samples"]),
            mappings: execution["mappings"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|m| Mapping {
                    core: m["core"].as_u64().unwrap_or(u64::MAX),
                    address: m["address"].as_u64().unwrap_or(0),
                    physical: m["physical"].as_u64().unwrap_or(0),
                    size: m["size"].as_u64().unwrap_or(0),
                })
                .collect(),
            ram: execution["ram"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|r| {
                    (
                        r["base"].as_u64().unwrap_or(0),
                        r["size"].as_u64().unwrap_or(0),
                    )
                })
                .collect(),
            all,
        })
    }

    fn gpio_level(&self, port: u8, pin: u8) -> Result<u64, Unresolved> {
        let levels = self.gpio_levels.get(usize::from(port));
        match levels {
            Some(levels) if pin < 16 => Ok((levels >> pin) & 1),
            _ => Err("GPIO contact is outside the selected body".into()),
        }
    }

    /// The level a target presents in the body's default state.
    pub fn current(&self, target: &Target) -> Result<u64, Unresolved> {
        match target {
            Target::GpioContacts { pins, .. } => {
                let mut value = 0;
                for (index, (port, pin)) in pins.iter().enumerate() {
                    value |= self.gpio_level(*port, *pin)? << index;
                }
                Ok(value)
            }
            Target::Gpio { port, pin, .. } => self.gpio_level(*port, *pin),
            Target::SerialStatus {
                node,
                route,
                register,
                mask,
            } => {
                let route = self
                    .serial
                    .iter()
                    .find(|candidate| candidate.node == *node && candidate.index == *route)
                    .ok_or("Status bits have no unique serial status input")?;
                let value = route
                    .registers
                    .iter()
                    .find(|(known, _)| known == register)
                    .map(|(_, value)| *value)
                    .ok_or("Status bits start between contact levels")?;
                let bits = value & u64::from(*mask);
                if bits != 0 && bits != u64::from(*mask) {
                    return Err("Status bits start between contact levels".into());
                }
                Ok(u64::from(bits == u64::from(*mask)))
            }
            Target::Analog { channel, .. } => self
                .adc_samples
                .get(usize::from(*channel))
                .copied()
                .ok_or_else(|| "ADC contact is outside the selected body".into()),
        }
    }

    /// The one physical RAM address processor 0 reaches at `address` for `size` bytes.
    pub fn physical(&self, address: u64, size: u64) -> Result<u64, String> {
        let mut candidates: Vec<u64> = self
            .mappings
            .iter()
            .filter(|m| m.core == 0 && m.address <= address && address + size <= m.address + m.size)
            .map(|m| m.physical + address - m.address)
            .collect();
        if candidates.is_empty() {
            candidates.push(address);
        }
        candidates.sort_unstable();
        candidates.dedup();
        candidates.retain(|a| {
            self.ram
                .iter()
                .any(|(base, length)| *base <= *a && a + size <= base + length)
        });
        match candidates.as_slice() {
            [one] => Ok(*one),
            _ => Err("Control observation is not uniquely mapped physical RAM".into()),
        }
    }
}
