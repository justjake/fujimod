//! X-Processor Pro topology on the shared native execution graph.
//! The caller supplies checked wiring, assets, and physical contact inputs.
use crate::{
    adc, blocks, boot, clocks, display, dram, face, flash, i2c, imaging, lens, power, resets, sd,
    serial, shutter, statistics, sync, timer,
};
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
    /// Array geometry and latencies behind this chip select.
    #[serde(default)]
    pub array: Option<flash::Array>,
}
/// A serial port's peer: none, a register file (the X-T20 sub-CPU) or a
/// byte-frame device with 16-bit registers (the battery gauge).
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Peer {
    None,
    Registers {
        registers: Vec<(u8, u8)>,
        #[serde(default)]
        clock: Option<serial::Clock>,
    },
    Frames {
        registers: Vec<(u8, u16)>,
    },
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlockFrames {
    pub start: (u64, u32),
    pub stopped: (u64, u32),
    pub irq: u32,
    pub period_ticks: u64,
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
        /// Reset-gate channel of this slot; requires a reset-gates node.
        #[serde(default)]
        reset_channel: Option<usize>,
    },
    Timer {
        base: u64,
        routes: Vec<(usize, usize)>,
    },
    SerialConfiguration {
        base: u64,
    },
    /// A serial port with immediate completion and an attached peer.
    SerialPort {
        base: u64,
        peer: Peer,
    },
    Adc {
        base: u64,
        samples: Vec<u16>,
        conversion_ticks: u64,
    },
    /// The reset/gate bank. SD channels and the lens take its lines, so it
    /// precedes them.
    ResetGates {
        base: u64,
    },
    RegisterBlock {
        base: u64,
        size: u64,
        idle: Vec<(u64, u32)>,
        frames: Option<BlockFrames>,
    },
    Statistics {
        base: u64,
        irq: u32,
        period_ticks: u64,
    },
    Face {
        base: u64,
        irq: u32,
        run_ticks: u64,
    },
    Hdmi {
        base: u64,
    },
    Dram {
        base: u64,
        refresh_level: u8,
    },
    Power {
        base: u64,
    },
    Usb {
        config: fujimod_emulator_ip::usb::dwc3::Config,
    },
    /// `parts` is a list of `i2c::Part`, decoded separately: a tagged enum
    /// would buffer their register maps and lose the integer keys.
    I2c {
        options: fujimod_emulator_ip::dw_i2c::Options,
        parts: Value,
    },
    DisplayOutput {
        base: u64,
        size: u64,
        set_bits: Vec<(u64, u32)>,
        clear_bits: Vec<(u64, u32)>,
        scanout: Option<(usize, u64)>,
    },
    DisplayFrames {
        irqs: Vec<u32>,
        period_ticks: u64,
    },
    /// The imaging control page, less the frame interrupt words that a
    /// frame-sync node at `hole` owns.
    ImagingControl {
        base: u64,
        hole: Option<u64>,
    },
    FrameSync {
        base: u64,
        interrupts: u64,
        frame_lines: Vec<(usize, u32)>,
        period_ticks: u64,
    },
    /// X-mount lens on its serial port; follows the GPIO and reset-gate nodes.
    Lens {
        port: u64,
        page: Vec<u8>,
        replies: Vec<(u32, u16)>,
        power: (usize, usize, usize),
        ready: (usize, usize),
        line: usize,
        contacts: Vec<(usize, usize, bool)>,
        ready_ticks: u64,
        release_ticks: u64,
        drive_ticks: u64,
        poll_ticks: u64,
    },
    /// LZ decompressor (no OSD plane codec, as on the XP4 board).
    Decompressor {
        base: u64,
        irq: u32,
    },
    /// DMP 2D renderer.
    Renderer {
        base: u64,
        irq: u32,
    },
    /// Shutter motor and sensors on the GPIO banks; follows the GPIO node.
    Shutter {
        mode: (usize, usize),
        motor: (usize, usize),
        sensors: Vec<(usize, usize)>,
        line: usize,
        positions: Vec<Vec<bool>>,
        step_ticks: u64,
        poll_ticks: u64,
    },
    Flash {
        controller: u64,
        aperture: u64,
        size: u64,
        asset: String,
        /// Interrupt line of the controller status.
        #[serde(default)]
        irq: Option<u32>,
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
    fn writable(&self) -> bool {
        true
    }
    fn write(&self, address: u64, bytes: &[u8]) -> Result<(), String> {
        self.0.store(address, bytes)
    }
}
struct Timers {
    base: u64,
    model: timer::TickTimers,
    // Earliest pending event; later expiries wait for it to re-arm, so
    // register traffic does not queue one event per access.
    scheduled: Option<u64>,
}
impl Timers {
    fn schedule(&mut self, cx: &mut Cx<'_>) {
        let now = cx.now();
        if self.scheduled.is_some_and(|at| at <= now) {
            self.scheduled = None;
        }
        if let Some(t) = self.model.next_event_tick(now)
            && self.scheduled.is_none_or(|at| t < at)
        {
            cx.schedule(t, 0);
            self.scheduled = Some(t);
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
        // The restored timeline holds its own events; re-arm on next use.
        self.scheduled = None;
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
    // Cross-node wiring, in node order: GPIO pins for the lens and shutter,
    // reset/gate lines for SD and the lens, the lens link for the PL330.
    let mut pins = None;
    let mut gates: Option<(Vec<resets::ResetPulses>, resets::ExternalLines)> = None;
    let lens_link = spec
        .nodes
        .iter()
        .find_map(|n| match n.model {
            Model::Lens { port, .. } => Some(lens::LensLink::new(port)),
            _ => None,
        })
        .transpose()?;
    for node in spec.nodes {
        if nodes.contains_key(&node.name) {
            return Err("Repeated X-Processor Pro node".into());
        }
        let id = match node.model {
            Model::Gpio { base, inputs } => {
                if pins.is_some() {
                    return Err("Repeated X-Processor Pro GPIO bank".into());
                }
                let shared = fujimod_emulator_ip::socionext::gpio::Pins::new(inputs);
                pins = Some(shared.clone());
                devices.register(
                    base,
                    0x400,
                    Box::new(fujimod_emulator_ip::socionext::gpio::Gpio::new(
                        base, shared,
                    )?),
                )?
            }
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
                reset_channel,
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
                let mut model = sd::Controller::new(
                    options,
                    card,
                    contacts,
                    irq.map(fujimod_emulator_devices::bus::InterruptLine::Shared),
                )?;
                if let Some(channel) = reset_channel {
                    let (lines, _) = gates
                        .as_ref()
                        .ok_or("X-Processor Pro SD reset line before its reset gates")?;
                    model = model.with_reset(
                        lines
                            .get(channel)
                            .cloned()
                            .ok_or("Invalid X-Processor Pro SD reset channel")?,
                    );
                }
                devices.register(base, 0x1000, Box::new(model))?
            }
            Model::Timer { base, routes } => devices.register(
                base,
                timer::WINDOW.into(),
                Box::new(Timers {
                    base,
                    model: timer::TickTimers::new(&routes)?,
                    scheduled: None,
                }),
            )?,
            Model::SerialConfiguration { base } => devices.register(
                base,
                serial::REGISTERS,
                Box::new(serial::ConfigurationPort::new(base)?),
            )?,
            Model::SerialPort { base, peer } => {
                let mut clock = None;
                let peer = match peer {
                    Peer::None => serial::Peer::None,
                    Peer::Registers {
                        registers,
                        clock: c,
                    } => {
                        clock = c;
                        let mut values = vec![0; serial::PEER_REGISTERS];
                        for (register, value) in registers {
                            *values
                                .get_mut(usize::from(register))
                                .ok_or("X-Processor Pro serial peer register out of range")? =
                                value;
                        }
                        serial::Peer::Registers(values)
                    }
                    Peer::Frames { registers } => {
                        serial::Peer::Frames(registers.into_iter().collect())
                    }
                };
                let mut port = serial::DiscoveryPort::attached(base, peer)?;
                if let Some(clock) = clock {
                    port = port.with_clock(clock)?;
                }
                devices.register(base, serial::REGISTERS, Box::new(port))?
            }
            Model::Adc {
                base,
                samples,
                conversion_ticks,
            } => devices.register(
                base,
                adc::WINDOW,
                Box::new(adc::Converter::new(base, &samples, conversion_ticks)?),
            )?,
            Model::ResetGates { base } => {
                if gates.is_some() {
                    return Err("Repeated X-Processor Pro reset/gate bank".into());
                }
                let model = resets::Gates::new(base)?;
                let lines = (0..3)
                    .map(|channel| model.sd_reset(channel))
                    .collect::<Result<Vec<_>, _>>()?;
                gates = Some((lines, model.external_lines()));
                devices.register(base, resets::WINDOW, Box::new(model))?
            }
            Model::RegisterBlock {
                base,
                size,
                idle,
                frames,
            } => {
                let frames = frames.map(|f| blocks::Frames {
                    start: f.start,
                    stopped: f.stopped,
                    irq: f.irq,
                    period: f.period_ticks,
                });
                let model = blocks::Block::new(base, size, idle, frames)?;
                devices.register(base, model.window(), Box::new(model))?
            }
            Model::Statistics {
                base,
                irq,
                period_ticks,
            } => devices.register(
                base,
                statistics::WINDOW,
                Box::new(statistics::Engine::new(base, irq, period_ticks)?),
            )?,
            Model::Face {
                base,
                irq,
                run_ticks,
            } => devices.register(
                base,
                face::WINDOW,
                Box::new(face::Engine::new(base, irq, run_ticks)?),
            )?,
            Model::Hdmi { base } => devices.register(
                base,
                fujimod_emulator_ip::dw_hdmi::WINDOW,
                Box::new(fujimod_emulator_ip::dw_hdmi::Transmitter::new(base)?),
            )?,
            Model::Dram {
                base,
                refresh_level,
            } => devices.register(
                base,
                dram::WINDOW,
                Box::new(dram::Controller::new(base, refresh_level)?),
            )?,
            Model::Usb { config } => devices.register_windows(
                // The DWC3 global, device and endpoint register groups.
                [
                    (0xc100, 0xc114),
                    (0xc200, 0xc204),
                    (0xc2c0, 0xc2c4),
                    (0xc400, 0xc410),
                    (0xc700, 0xc718),
                    (0xc720, 0xc724),
                    (0xc800, 0xca00),
                ]
                .into_iter()
                .map(|(lo, hi)| Mapping {
                    base: config.base + lo,
                    size: hi - lo,
                })
                .collect(),
                Box::new(fujimod_emulator_ip::usb::dwc3::Dwc3::new(config)?),
            )?,
            Model::Power { base } => {
                devices.register(base, power::WINDOW, Box::new(power::Control::new(base)?))?
            }
            Model::I2c { options, parts } => {
                let base = options.base;
                devices.register(
                    base,
                    fujimod_emulator_ip::dw_i2c::WINDOW,
                    Box::new(fujimod_emulator_ip::dw_i2c::Controller::new(
                        options,
                        i2c::Parts::new(serde_json::from_value(parts).map_err(|e| e.to_string())?)?,
                    )?),
                )?
            }
            Model::DisplayOutput {
                base,
                size,
                set_bits,
                clear_bits,
                scanout,
            } => devices.register(
                base,
                size,
                Box::new(display::Output::new(
                    base,
                    size,
                    &set_bits,
                    &clear_bits,
                    scanout.map(|(output, period)| display::Scanout { output, period }),
                )?),
            )?,
            Model::DisplayFrames { irqs, period_ticks } => {
                devices.attach(Box::new(display::FrameClock::new(irqs, period_ticks)?))
            }
            Model::ImagingControl { base, hole } => {
                let end = base + imaging::WINDOW;
                let windows = match hole {
                    None => vec![(base, end)],
                    Some(at) if at > base && at & 7 == 0 && at + sync::INTERRUPT_WINDOW < end => {
                        vec![(base, at), (at + sync::INTERRUPT_WINDOW, end)]
                    }
                    Some(_) => {
                        return Err("Frame interrupt words outside the imaging control page".into());
                    }
                };
                devices.register_windows(
                    windows
                        .into_iter()
                        .map(|(lo, hi)| Mapping {
                            base: lo,
                            size: hi - lo,
                        })
                        .collect(),
                    Box::new(imaging::Control::new(base)?),
                )?
            }
            Model::FrameSync {
                base,
                interrupts,
                frame_lines,
                period_ticks,
            } => devices.register_windows(
                vec![
                    Mapping {
                        base,
                        size: sync::WINDOW,
                    },
                    Mapping {
                        base: interrupts,
                        size: sync::INTERRUPT_WINDOW,
                    },
                ],
                Box::new(sync::Generator::new(
                    base,
                    interrupts,
                    frame_lines,
                    period_ticks,
                )?),
            )?,
            Model::Lens {
                port,
                page,
                replies,
                power,
                ready,
                line,
                contacts,
                ready_ticks,
                release_ticks,
                drive_ticks,
                poll_ticks,
            } => {
                let (_, lines) = gates.as_ref().ok_or("The lens follows the reset gates")?;
                let model = lens::LensPort::new(
                    lens_link.as_ref().ok_or("Missing lens link")?,
                    lens::Profile {
                        page,
                        replies: replies.into_iter().collect(),
                    },
                    lens::Timing {
                        ready: ready_ticks,
                        release: release_ticks,
                        drive: drive_ticks,
                        poll: poll_ticks,
                    },
                    lens::Wiring {
                        base: port,
                        power,
                        ready,
                        line,
                        contacts,
                    },
                    pins.clone().ok_or("The lens follows the GPIO bank")?,
                    lines.clone(),
                )?;
                devices.register(port, serial::REGISTERS, Box::new(model))?
            }
            Model::Decompressor { base, irq } => devices.register(
                base,
                0x6c,
                Box::new(fujimod_emulator_devices::lz_osd::Decompressor::new(
                    base,
                    irq,
                    Arc::new(std::sync::Mutex::new([0; 2])),
                    None,
                )),
            )?,
            Model::Renderer { base, irq } => devices.register(
                base,
                0x3000,
                Box::new(fujimod_emulator_ip_dmp::smaph::Renderer::new(base, irq)?),
            )?,
            Model::Shutter {
                mode,
                motor,
                sensors,
                line,
                positions,
                step_ticks,
                poll_ticks,
            } => devices.attach(Box::new(shutter::Shutter::new(
                shutter::Wiring {
                    mode,
                    motor,
                    sensors,
                    line,
                },
                shutter::Timing {
                    step: step_ticks,
                    poll: poll_ticks,
                },
                positions,
                pins.clone().ok_or("The shutter follows the GPIO bank")?,
            )?)),
            Model::Flash {
                controller,
                aperture,
                size,
                asset,
                irq,
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
                let mut array = None;
                for attachment in nor {
                    if attachment.array.is_some() {
                        if array.is_some() {
                            return Err("Repeated serial-flash array timing".into());
                        }
                        array = attachment.array;
                    }
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
                        irq,
                        array,
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
                    std::iter::once(Arc::new(Source(source)) as Arc<dyn pl330::Source>)
                        .chain(
                            lens_link
                                .iter()
                                .map(|link| Arc::new(link.clone()) as Arc<dyn pl330::Source>),
                        )
                        .collect(),
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
