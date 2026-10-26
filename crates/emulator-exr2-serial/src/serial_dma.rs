//! Graph nodes for clocked byte serial, request DMA, and selector devices.
//! The fixed external bit level and every channel/request/IRQ connection are
//! constructor inputs. Guest descriptor addresses are the only RAM authority.
use fujimod_emulator_devices::bus::InterruptLine;
use fujimod_emulator_devices::dma::Direction;
use crate::{dma::{peripheral, routing}, spi::byte_fifo};
use fujimod_emulator_parts_io::serial::{InputLevel, Wire};
use fujimod_emulator_parts_lens::x_mount_link;
use fujimod_emulator_devices::{Cx, Device, graph::Mapping};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
#[derive(Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Input {
    Fixed {
        high: bool,
    },
    XMountStartup {
        configuration: x_mount_link::Configuration,
    },
}
impl Input {
    fn wire(&self) -> Result<Box<dyn Wire>, String> {
        Ok(match self {
            Self::Fixed { high } => Box::new(InputLevel(*high)),
            Self::XMountStartup { configuration } => {
                Box::new(x_mount_link::Link::new(configuration.clone())?)
            }
        })
    }
}
#[derive(Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub serial_base: u64,
    pub dma_base: u64,
    pub routing_base: u64,
    pub channels: [usize; 2],
    pub transmit_request: u8,
    pub receive_request: u8,
    pub frame_ticks: u64,
    pub irq_targets: Vec<(usize, usize, u32)>,
}
pub(crate) struct Transport {
    wiring: Options,
    windows: Vec<(u64, u64)>,
    serial: byte_fifo::ByteFifo,
    dma: peripheral::PeripheralChannels,
    routing: routing::Block98500,
}
impl Transport {
    pub(crate) fn windows(&self) -> &[(u64, u64)] {
        &self.windows
    }
    fn new(wiring: Options, input: Input) -> Result<Self, String> {
        Self::with_wire(wiring, input.wire()?)
    }
    fn with_wire(wiring: Options, wire: Box<dyn Wire>) -> Result<Self, String> {
        if wiring.channels.iter().any(|c| *c >= 8)
            || wiring.channels[0] == wiring.channels[1]
            || wiring.transmit_request >= 32
            || wiring.receive_request >= 32
            || wiring.transmit_request == wiring.receive_request
            || wiring.irq_targets.iter().any(|(channel, cpu, line)| {
                !wiring.channels.contains(channel) || *cpu > 1 || *line >= 128
            })
        {
            return Err("invalid EXR II serial/DMA channel, request or IRQ wiring".into());
        }
        let serial = byte_fifo::ByteFifo::new(wiring.serial_base, wiring.frame_ticks, wire)?;
        let port = serial.port();
        let shared = routing::SharedRouting::default();
        let connections = BTreeMap::from([
            (
                wiring.transmit_request,
                peripheral::Connection {
                    address: wiring.serial_base + 0x1c,
                    direction: Direction::Transmit,
                    port: port.clone(),
                },
            ),
            (
                wiring.receive_request,
                peripheral::Connection {
                    address: wiring.serial_base + 0x18,
                    direction: Direction::Receive,
                    port,
                },
            ),
        ]);
        let mut interrupts: [Vec<InterruptLine>; 8] = std::array::from_fn(|_| vec![]);
        for &(channel, core, line) in &wiring.irq_targets {
            let irq = InterruptLine::Core { core, line };
            if interrupts[channel].contains(&irq) {
                return Err("duplicate EXR II serial/DMA IRQ wire".into());
            }
            interrupts[channel].push(irq);
        }
        let dma = peripheral::PeripheralChannels::new(
            wiring.dma_base,
            shared.clone(),
            connections,
            interrupts,
        )?;
        let routing = routing::Block98500::new(wiring.routing_base, shared)?;
        let mut windows = vec![
            (wiring.serial_base, wiring.serial_base + 0x24),
            (wiring.routing_base, wiring.routing_base + 8),
        ];
        windows.extend(wiring.channels.iter().map(|c| {
            let base = wiring.dma_base + (*c as u64) * 0x40;
            (base, base + 0x40)
        }));
        windows.sort_unstable();
        if windows.windows(2).any(|p| p[0].1 > p[1].0) {
            return Err("EXR II serial/DMA windows overlap".into());
        }
        Ok(Self {
            wiring,
            windows,
            serial,
            dma,
            routing,
        })
    }
    fn next(&self, now: u64) -> Option<u64> {
        [self.serial.next_deadline(now), self.dma.next_deadline(now)]
            .into_iter()
            .flatten()
            .min()
    }
    fn device(&mut self, address: u64, size: u8) -> Result<&mut dyn Device, String> {
        if self.wiring.serial_base <= address && address < self.wiring.serial_base + 0x24 {
            return Ok(&mut self.serial);
        }
        if self.wiring.routing_base <= address && address < self.wiring.routing_base + 8 {
            return Ok(&mut self.routing);
        }
        if self.contains(address, size) {
            return Ok(&mut self.dma);
        }
        Err("EXR II serial/DMA address is outside its selected windows".into())
    }
    fn contains(&self, address: u64, size: u8) -> bool {
        if size != 4 || address & 3 != 0 {
            return false;
        }
        (self.wiring.serial_base <= address && address < self.wiring.serial_base + 0x24)
            || (self.wiring.routing_base <= address && address < self.wiring.routing_base + 8)
            || self.wiring.channels.iter().any(|c| {
                let base = self.wiring.dma_base + (*c as u64) * 0x40;
                base <= address && address < base + 0x40
            })
    }
}
impl Device for Transport {
    fn name(&self) -> &'static str {
        "exr2-serial-dma-transport"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        match self.device(address, size) {
            Ok(device) => device.read(cx, address, size),
            Err(error) => {
                cx.fail(error);
                0
            }
        }
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        match self.device(address, size) {
            Ok(device) => device.write(cx, address, size, value),
            Err(error) => cx.fail(error),
        }
    }
    fn next_deadline(&self, now: u64) -> Option<u64> {
        self.next(now)
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        let now = cx.now();
        let mut cursor = now;
        while let Some(at) = self.next(cursor).filter(|at| *at <= now) {
            cursor = at;
            cx.at_tick(at, |cx| self.serial.advance(cx));
            if let Some(error) = cx.take_refusal() {
                cx.fail(error);
                return;
            }
            cx.at_tick(at, |cx| self.dma.advance(cx));
            if let Some(error) = cx.take_refusal() {
                cx.fail(error);
                return;
            }
        }
    }

    fn checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        Ok(
            fujimod_emulator_devices::checkpoint::Checkpoint::encode(self.name(), &self.wiring)?
                .with_children(vec![self.serial.checkpoint()?, self.dma.checkpoint()?]),
        )
    }
    fn validate_checkpoint(&self, saved: &fujimod_emulator_devices::checkpoint::Checkpoint) -> Result<(), String> {
        let wiring: Options = saved.decode(self.name())?;
        if wiring != self.wiring || saved.children().len() != 2 || !saved.blobs().is_empty() {
            return Err("serial/DMA checkpoint wiring differs".into());
        }
        self.serial.validate_checkpoint(&saved.children()[0])?;
        self.dma.validate_checkpoint(&saved.children()[1])
    }
    fn restore_checkpoint(&mut self, saved: &fujimod_emulator_devices::checkpoint::Checkpoint) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        self.serial.restore_checkpoint(&saved.children()[0])?;
        self.dma.restore_checkpoint(&saved.children()[1])
    }
    fn save(&self) -> Value {
        json!({"wiring":self.wiring,"serial":self.serial.save(),"dma":self.dma.save(),"routing":self.routing.save()})
    }
    fn fingerprint(&self) -> Result<Value, String> {
        Ok(self.save())
    }
}

/// Creates independent SPI, selector, and peripheral-DMA nodes for a board.
pub struct Nodes {
    transport: Transport,
}
impl Nodes {
    pub fn new(wiring: Options, input: Input) -> Result<Self, String> {
        Ok(Self {
            transport: Transport::new(wiring, input)?,
        })
    }

    pub(crate) fn with_wire(wiring: Options, wire: Box<dyn Wire>) -> Result<Self, String> {
        Ok(Self {
            transport: Transport::with_wire(wiring, wire)?,
        })
    }

    pub(crate) fn into_transport(self) -> Transport {
        self.transport
    }

    pub fn windows(&self) -> &[(u64, u64)] {
        self.transport.windows()
    }

    pub fn devices(self) -> Vec<(Vec<Mapping>, Box<dyn Device>)> {
        let Transport {
            wiring,
            serial,
            dma,
            routing,
            ..
        } = self.transport;
        let serial_window = Mapping {
            base: wiring.serial_base,
            size: 0x24,
        };
        let routing_window = Mapping {
            base: wiring.routing_base,
            size: 8,
        };
        let dma_windows = wiring
            .channels
            .iter()
            .map(|channel| Mapping {
                base: wiring.dma_base + (*channel as u64) * 0x40,
                size: 0x40,
            })
            .collect();
        vec![
            (vec![serial_window], Box::new(serial) as Box<dyn Device>),
            (vec![routing_window], Box::new(routing) as Box<dyn Device>),
            (dma_windows, Box::new(dma) as Box<dyn Device>),
        ]
    }
}
#[cfg(test)]
mod tests;
