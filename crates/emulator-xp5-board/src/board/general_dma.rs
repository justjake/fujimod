//! Checked XP5 serial FIFO, DMA request/address views and interrupt wiring.
use fujimod_emulator_devices::bus::InterruptLine;
use fujimod_emulator_devices::dma::Direction;
use fujimod_emulator_devices::graph::{DeviceGraph, DeviceWindow};
use fujimod_emulator_ip_arm::pl022::{Clock, Controller};
use fujimod_emulator_ip::socionext::xdmac::{Connection, Pair};
use fujimod_emulator_parts_io::serial::{InputLevel, Unconnected, Wire};
use fujimod_emulator_xp5_storage::{dma_registers, general_dma::Global};
use serde::Deserialize;
use std::collections::BTreeMap;

pub fn owns(kind: &str) -> bool {
    matches!(
        kind,
        "pl022" | "general-dma" | "dma-address-high" | "dma-request-routing"
    )
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SerialWiring {
    source_hz: u64,
    ticks_per_second: u64,
    receive_request: u8,
    transmit_request: u8,
    wire: WireInput,
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
enum WireInput {
    Unconnected,
    InputLevel {
        high: bool,
    },
    GpioSelected {
        endpoints: Vec<super::serial::Connection>,
    },
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DmaWiring {
    companion_irq_base: u32,
}
fn options<T: serde::de::DeserializeOwned>(w: &DeviceWindow) -> Result<T, String> {
    toml::Value::Table(w.options.clone())
        .try_into()
        .map_err(|e| format!("{} wiring: {e}", w.kind))
}
pub fn register_all(
    bus: &mut DeviceGraph,
    windows: &[DeviceWindow],
    companion: Option<usize>,
    pins: fujimod_emulator_devices::xp5::pins::SharedPins,
) -> Result<(), String> {
    let banks = dma_registers::banks();
    let mut connections = BTreeMap::new();
    let mut serials = vec![];
    for w in windows.iter().filter(|w| w.kind == "pl022") {
        if w.base != 0xff911000 || w.size != 0x1000 {
            return Err("invalid XP5 PL022 window".into());
        }
        let wiring: SerialWiring = options(w)?;
        if wiring.receive_request >= 63
            || wiring.transmit_request >= 63
            || wiring.receive_request == wiring.transmit_request
        {
            return Err("invalid XP5 serial DMA request wiring".into());
        }
        let wire: Box<dyn Wire> = match wiring.wire {
            WireInput::Unconnected => Box::new(Unconnected),
            WireInput::InputLevel { high } => Box::new(InputLevel(high)),
            WireInput::GpioSelected { endpoints } => {
                super::serial::gpio_wire(pins.clone(), endpoints)?
            }
        };
        let serial = Controller::new(
            w.base,
            Clock {
                source_hz: wiring.source_hz,
                ticks_per_second: wiring.ticks_per_second,
            },
            vec![],
            wire,
        )?;
        let port = serial.port();
        for (request, direction) in [
            (wiring.receive_request, Direction::Receive),
            (wiring.transmit_request, Direction::Transmit),
        ] {
            if connections
                .insert(
                    request,
                    Connection {
                        address: w.base + 8,
                        direction,
                        port: port.clone(),
                    },
                )
                .is_some()
            {
                return Err("duplicate DMA request wire".into());
            }
        }
        serials.push(bus.register(w.base, w.size, Box::new(serial))?);
    }
    let mut pairs = vec![];
    let mut views = vec![];
    for w in windows.iter().filter(|w| w.kind == "general-dma") {
        if !pairs.is_empty() || w.base != 0xff930000 || w.size != 0x8000 {
            return Err("invalid or duplicate general-DMA window".into());
        }
        let wiring: DmaWiring = options(w)?;
        let core = companion.ok_or("general-DMA IRQ wiring requires the companion")?;
        if wiring.companion_irq_base.checked_add(15).is_none() {
            return Err("DMA IRQ range overflow".into());
        }
        for (bank, route) in banks.iter().enumerate() {
            let base = w.base + bank as u64 * 0x1000;
            let interrupts = std::array::from_fn(|lane| {
                vec![InterruptLine::Core {
                    core,
                    line: wiring.companion_irq_base + (bank * 2 + lane) as u32,
                }]
            });
            pairs.push(bus.register(
                base,
                0x1000,
                Box::new(Pair::new(
                    base,
                    route.clone(),
                    connections.clone(),
                    interrupts,
                )?),
            )?);
        }
        bus.register(0xff90e564, 24, Box::new(Global::default()))?;
    }
    for w in windows
        .iter()
        .filter(|w| matches!(w.kind.as_str(), "dma-address-high" | "dma-request-routing"))
    {
        if !w.options.is_empty() {
            return Err("unknown DMA register-view options".into());
        }
        let device: Box<dyn fujimod_emulator_devices::Device> = match w.kind.as_str() {
            "dma-address-high" if w.base == 0xff90e748 && w.size == 0x40 => {
                Box::new(dma_registers::AddressHigh::new(banks.clone()))
            }
            "dma-request-routing" if w.base == 0xff90e500 && w.size == 0x20 => {
                Box::new(dma_registers::RequestRouting::new(banks.clone()))
            }
            _ => return Err("invalid DMA register-view window".into()),
        };
        views.push(bus.register(w.base, w.size, device)?);
    }
    if pairs.is_empty() && (!serials.is_empty() || !views.is_empty()) {
        return Err("serial DMA wiring requires the channel banks".into());
    }
    if !pairs.is_empty() && views.len() != 2 {
        return Err("general-DMA requires both register views".into());
    }
    let pins = windows
        .iter()
        .filter(|w| matches!(w.kind.as_str(), "gpio" | "gpio-port3"))
        .map(|w| bus.at(w.base))
        .collect::<Result<Vec<_>, _>>()?;
    for &pair in &pairs {
        bus.depend_on(pair, serials.iter().chain(&views).copied().collect())?;
    }
    for serial in serials {
        bus.depend_on(serial, pairs.iter().chain(&pins).copied().collect())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
