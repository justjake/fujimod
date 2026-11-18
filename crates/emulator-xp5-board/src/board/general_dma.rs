//! Checked XP5 serial FIFO, DMA request/address views and interrupt wiring.
use fujimod_emulator_devices::bus::InterruptLine;
use fujimod_emulator_devices::dma::Direction;
use fujimod_emulator_devices::graph::{DeviceGraph, DeviceWindow};
use fujimod_emulator_ip_arm::pl022::{Clock, Controller};
use fujimod_emulator_ip::socionext::xdmac::{Connection, Pair};
use fujimod_emulator_parts_io::serial::{InputLevel, Unconnected, Wire};
use fujimod_emulator_xp5_storage::{dma_registers, flash_storage, general_dma::Global};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

pub fn owns(kind: &str) -> bool {
    matches!(
        kind,
        "pl022" | "general-dma" | "dma-address-high" | "dma-request-routing"
    )
}
/// A companion-side SSP instance. The DMA request pair and the companion interrupt line
/// are board facts a declaration may lack: without the pair a DMA request for the
/// instance has no connected peripheral and refuses, and without the line a raised
/// interrupt refuses unless the declaration states that the interrupt output reaches no
/// modeled input (`companion_irq_unconnected`).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SerialWiring {
    source_hz: u64,
    ticks_per_second: u64,
    #[serde(default)]
    receive_request: Option<u8>,
    #[serde(default)]
    transmit_request: Option<u8>,
    #[serde(default)]
    companion_irq_line: Option<u32>,
    #[serde(default)]
    companion_irq_unconnected: bool,
    wire: WireInput,
}
/// SSP instances 0..9 of the SoC (`spi0`..`spi9`), one 4 KiB window each.
const SSP_INSTANCES: std::ops::Range<u64> = 0xff910000..0xff91a000;
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
    /// Main-processor GIC INTID of channel 0 (bank 0, lane 0). Channel
    /// `bank * 2 + lane` raises `a53_irq_base + channel` in addition to its
    /// companion NVIC line. Absent leaves the banks wired to the companion only.
    #[serde(default)]
    a53_irq_base: Option<u32>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InputCaptureWiring {
    #[serde(default)]
    dma_transmit: Vec<InputCaptureTransmit>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InputCaptureTransmit {
    channel: u8,
    request: u8,
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
    flash: Option<&flash_storage::Medium>,
) -> Result<(), String> {
    let banks = dma_registers::banks();
    let mut connections = BTreeMap::new();
    let mut serials = vec![];
    for w in windows.iter().filter(|w| w.kind == "pl022") {
        if !SSP_INSTANCES.contains(&w.base) || w.base & 0xfff != 0 || w.size != 0x1000 {
            return Err("invalid XP5 PL022 window".into());
        }
        let wiring: SerialWiring = options(w)?;
        let requests = match (wiring.receive_request, wiring.transmit_request) {
            (Some(receive), Some(transmit))
                if receive < 63 && transmit < 63 && receive != transmit =>
            {
                vec![(receive, Direction::Receive), (transmit, Direction::Transmit)]
            }
            (None, None) => vec![],
            _ => return Err("invalid XP5 serial DMA request wiring".into()),
        };
        let interrupts = match wiring.companion_irq_line {
            Some(line) => vec![InterruptLine::Core {
                core: companion.ok_or("PL022 IRQ wiring requires the companion")?,
                line,
            }],
            None => vec![],
        };
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
            interrupts,
            wire,
        )?;
        let serial = if wiring.companion_irq_unconnected {
            serial.with_unconnected_interrupt()?
        } else {
            serial
        };
        let port = serial.port();
        for (request, direction) in requests {
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
    for w in windows.iter().filter(|w| w.kind == "input-capture") {
        let channels = match (w.base, w.size) {
            (0xff928000, 0x1000) => 5,
            (0xff929000, 0x1000) => 3,
            _ => return Err("invalid input-capture DMA window".into()),
        };
        let wiring: InputCaptureWiring = options(w)?;
        for transmit in wiring.dma_transmit {
            if transmit.channel >= channels || transmit.request >= 63 {
                return Err("invalid input-capture DMA request wiring".into());
            }
            let port: fujimod_emulator_devices::dma::SharedPort =
                Arc::new(Mutex::new(fujimod_emulator_xp5_io::input_capture::TransmitSink));
            if connections
                .insert(
                    transmit.request,
                    Connection {
                        address: w.base + u64::from(transmit.channel) * 0x20 + 4,
                        direction: Direction::Transmit,
                        port,
                    },
                )
                .is_some()
            {
                return Err("duplicate DMA request wire".into());
            }
        }
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
        // GIC SPIs start at INTID 32 and the architecture ends them at 1019.
        if wiring
            .a53_irq_base
            .is_some_and(|base| base < 32 || base > 1019 - 15)
        {
            return Err("general-DMA A53 IRQ base must leave 16 SPIs in 32..1019".into());
        }
        for (bank, route) in banks.iter().enumerate() {
            let base = w.base + bank as u64 * 0x1000;
            let interrupts = std::array::from_fn(|lane| {
                let channel = (bank * 2 + lane) as u32;
                let mut lines = vec![InterruptLine::Core {
                    core,
                    line: wiring.companion_irq_base + channel,
                }];
                lines.extend(
                    wiring
                        .a53_irq_base
                        .map(|base| InterruptLine::Shared(base + channel)),
                );
                lines
            });
            pairs.push(bus.register(
                base,
                0x1000,
                // Memory-to-memory copies read the serial-flash CPU aperture from its backing.
                Box::new(
                    Pair::new(base, route.clone(), connections.clone(), interrupts)?
                        .with_read_windows(flash.map(flash_storage::Medium::read_window).into_iter().collect()),
                ),
            )?);
        }
        // e560 is an M4-read retained word whose reset value is a session assumption.
        bus.register(0xff90e560, 28, Box::new(Global::default()))?;
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
