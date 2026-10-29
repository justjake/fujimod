//! X-Processor Pro topology on the shared native execution graph.
//! The caller supplies checked wiring, assets, and physical contact inputs.
use crate::{boot, clocks, flash, sd, serial, timer};
use fujimod_emulator::board::{
    self, Board, InterruptMap, InterruptRegisters,
    bus::{Bus, Mode},
    factory,
};
use fujimod_emulator_devices::{
    Cx, Device,
    graph::{DeviceGraph, Mapping},
    signals::Level,
};
use fujimod_emulator_ip_arm::{gicv2, pl330};
use serde::Deserialize;
use serde_json::Value;
use std::{collections::BTreeMap, sync::Arc};

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Xpp,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Gic {
    pub distributor: u64,
    pub cpu_interface: u64,
    pub lines: usize,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    pub kind: Kind,
    pub cpus: Vec<board::armv7a::Config>,
    pub gic: Gic,
    pub nodes: Vec<Node>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Node {
    pub name: String,
    pub model: Model,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Nor {
    pub chip_select: u8,
    pub options: fujimod_emulator_devices::parts::micron_nor::Options,
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Model {
    Gpio {
        base: u64,
        inputs: [u16; 16],
    },
    ClockControl {
        options: clocks::Options,
    },
    Boot {
        options: boot::Options,
        keys: Vec<String>,
    },
    Sd {
        options: sd::Options,
        format: fujimod_emulator_storage::fat::Options,
        files: BTreeMap<String, String>,
        irq: Option<u32>,
    },
    Timer {
        base: u64,
        routes: Vec<(usize, usize)>,
    },
    SerialConfiguration {
        base: u64,
    },
    Flash {
        controller: u64,
        aperture: u64,
        size: u64,
        asset: String,
        dma: u64,
        irq_routes: Vec<u32>,
        setup_ticks: u64,
        bytes_per_tick: u64,
        transfer_ticks: u64,
        nor: Vec<Nor>,
    },
}
pub struct Built {
    pub board: Board,
    pub nodes: BTreeMap<String, usize>,
    pub inputs: Vec<(String, Level, usize)>,
    pub contacts: BTreeMap<String, fujimod_emulator_devices::parts::card_contacts::Contacts>,
}
struct Source(Arc<flash::Aperture>);
impl pl330::Source for Source {
    fn contains(&self, address: u64, len: usize) -> bool {
        self.0.contains(address, len)
    }
    fn read(&self, address: u64, len: usize) -> Result<Vec<u8>, String> {
        self.0.read(address, len)
    }
}
struct Timers {
    base: u64,
    model: timer::TickTimers,
}
impl Timers {
    fn schedule(&self, cx: &mut Cx<'_>) {
        if let Some(t) = self.model.next_event_tick(cx.now()) {
            cx.schedule(t, 0);
        }
    }
}
impl Device for Timers {
    fn name(&self) -> &'static str {
        "xpp-timers"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        self.advance(cx);
        match self.model.access(
            cx.now(),
            (address - self.base) as u32,
            size.into(),
            false,
            0,
        ) {
            Ok(v) => v.into(),
            Err(e) => {
                cx.fail(e);
                0
            }
        }
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        self.advance(cx);
        if let Err(e) = self.model.access(
            cx.now(),
            (address - self.base) as u32,
            size.into(),
            true,
            value as u32,
        ) {
            cx.fail(e);
        }
        self.schedule(cx);
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        for irq in self.model.tick(cx.now()) {
            cx.set_irq(irq as u32, true);
            cx.set_irq(irq as u32, false);
        }
        self.schedule(cx);
    }
    fn event(&mut self, cx: &mut Cx<'_>, _: u64) {
        self.advance(cx);
    }
    fn next_deadline(&self, now: u64) -> Option<u64> {
        self.model.next_event_tick(now)
    }
    fn save(&self) -> Value {
        self.model.save()
    }
    fn restore(&mut self, v: &Value) -> Result<(), String> {
        self.model.restore(v)
    }
}
pub fn build(
    graph: factory::Graph,
    spec: Spec,
    assets: &BTreeMap<String, Vec<u8>>,
    mode: Mode,
) -> Result<Built, String> {
    let cores = graph.processors.len();
    if spec.cpus.len() != cores
        || !(1..=4).contains(&cores)
        || !(32..=1020).contains(&spec.gic.lines)
    {
        return Err("X-Processor Pro processor/GIC wiring differs".into());
    }
    let mut devices = DeviceGraph::new();
    let mut nodes = BTreeMap::new();
    let mut inputs = vec![];
    let mut card_contacts = BTreeMap::new();
    for node in spec.nodes {
        if nodes.contains_key(&node.name) {
            return Err("Repeated X-Processor Pro node".into());
        }
        let id = match node.model {
            Model::Gpio { base, inputs } => devices.register(
                base,
                0x400,
                Box::new(fujimod_emulator_ip::socionext::gpio::Gpio::new(
                    base,
                    fujimod_emulator_ip::socionext::gpio::Pins::new(inputs),
                )?),
            )?,
            Model::ClockControl { options } => {
                let base = options.base;
                devices.register(base, 0x1000, Box::new(clocks::ClockControl::new(options)?))?
            }
            Model::Boot { options, keys } => {
                let contacts: Vec<_> = keys
                    .into_iter()
                    .map(|name| (name, Level::new(false)))
                    .collect();
                let windows = [
                    (options.selector_base, 0x1000),
                    (options.clock_base, 0x1000),
                    (options.ddr_base, 0x1000),
                    (options.remap_base, 0x1000),
                ]
                .into_iter()
                .chain(
                    options
                        .bus_bases
                        .iter()
                        .chain(&options.memory_port_bases)
                        .map(|&base| (base, 0x1000)),
                )
                .map(|(base, size)| Mapping { base, size })
                .collect();
                let model = boot::BootController::new(
                    options,
                    contacts.iter().map(|(_, l)| l.clone()).collect(),
                )?;
                let id = devices.register_windows(windows, Box::new(model))?;
                for (name, level) in contacts {
                    inputs.push((name, level, id));
                }
                id
            }
            Model::Sd {
                options,
                format,
                files,
                irq,
            } => {
                let files = files
                    .into_iter()
                    .map(|(name, asset)| {
                        Ok((
                            name,
                            assets.get(&asset).ok_or("Missing SD file asset")?.clone(),
                        ))
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                let card = fujimod_emulator_storage::fat::format(&format, &files)?;
                let contacts =
                    fujimod_emulator_devices::parts::card_contacts::Contacts::new(true, false);
                let base = options.base;
                card_contacts.insert(node.name.clone(), contacts.clone());
                let model = sd::Controller::new(
                    options,
                    card,
                    contacts,
                    irq.map(fujimod_emulator_devices::bus::InterruptLine::Shared),
                )?;
                devices.register(base, 0x1000, Box::new(model))?
            }
            Model::Timer { base, routes } => devices.register(
                base,
                timer::WINDOW.into(),
                Box::new(Timers {
                    base,
                    model: timer::TickTimers::new(&routes)?,
                }),
            )?,
            Model::SerialConfiguration { base } => devices.register(
                base,
                serial::REGISTERS,
                Box::new(serial::ConfigurationPort::new(base)?),
            )?,
            Model::Flash {
                controller,
                aperture,
                size,
                asset,
                dma,
                irq_routes,
                setup_ticks,
                bytes_per_tick,
                transfer_ticks,
                nor,
            } => {
                let bytes = assets
                    .get(&asset)
                    .ok_or("Missing X-Processor Pro flash asset")?
                    .clone();
                let source = Arc::new(flash::Aperture::new(aperture, size, bytes)?);
                let mut ports = BTreeMap::new();
                for attachment in nor {
                    let registers = fujimod_emulator_devices::parts::micron_nor::Registers::new(
                        attachment.options,
                    )?;
                    if ports.insert(attachment.chip_select, registers).is_some() {
                        return Err("Repeated serial-flash register device chip select".into());
                    }
                }
                let flash = flash::FlashController::new(
                    flash::Options {
                        base: controller,
                        transfer_ticks,
                        irq: None,
                        array: None,
                    },
                    source.clone(),
                    ports,
                )?;
                let id = devices.register_windows(
                    vec![
                        Mapping {
                            base: controller,
                            size: flash::REGISTERS,
                        },
                        Mapping {
                            base: aperture,
                            size,
                        },
                    ],
                    Box::new(flash),
                )?;
                let engine = pl330::Pl330::new(
                    dma,
                    irq_routes,
                    vec![Arc::new(Source(source))],
                    pl330::Throughput {
                        setup_ticks,
                        bytes_per_tick,
                    },
                )?;
                devices.register(dma, 0x1000, Box::new(engine))?;
                id
            }
        };
        nodes.insert(node.name, id);
    }
    let interrupts = InterruptMap {
        gic: Box::new(gicv2::Gic::new(cores, spec.gic.lines)),
        cores,
        registers: InterruptRegisters::GicV2 {
            distributor: spec.gic.distributor,
            cpu_interface: spec.gic.cpu_interface,
        },
    };
    let mut board = graph.build(
        factory::Options::new(factory::Engine::Unicorn),
        Bus::from_graph(mode, devices)?,
        Some(interrupts),
    )?;
    for (core, config) in spec.cpus.into_iter().enumerate() {
        board.configure_armv7a(core, config)?;
    }
    Ok(Built {
        board,
        nodes,
        inputs,
        contacts: card_contacts,
    })
}
