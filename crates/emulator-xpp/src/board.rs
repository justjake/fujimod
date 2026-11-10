//! X-Processor Pro topology on the shared native execution graph.
//! The caller supplies checked wiring, assets, and physical contact inputs.
use crate::{
    adc, blocks, boot, clocks, display, dram, encoder, face, flash, h264, i2c, imaging, jpeg, keys,
    lens, power, resets, sd, serial, shutter, statistics, sync, timer,
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

/// The X-Processor Pro audio interface's DMA window. Its samples read silent
/// until a codec is modeled; the words follow the register block at the same
/// address.
const AUDIO_SOURCE: (u64, u64) = (0xfff1_b000, 0x100);

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
/// A serial port's peer: none, an idle receive line (nothing attached), a
/// register file (the X-T20 sub-CPU) or a byte-frame device with 16-bit
/// registers (the battery gauge).
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Peer {
    None,
    /// Nothing attached and the receive line idle high: every byte sent
    /// clocks in 0xff.
    Idle,
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
#[serde(deny_unknown_fields)]
pub struct LiveViewImage {
    pub lines: u32,
    pub ycbcr: [u8; 3],
    /// Profile of a declared scene that replaces the uniform image.
    #[serde(default)]
    pub scene: Option<String>,
    /// Host RGB image that replaces the uniform image.
    #[serde(default)]
    pub host_image: Option<HostImage>,
    /// Uniform scene developed with the color unit's film law and white
    /// balance, in place of the fixed `ycbcr` pair (which stays as the image
    /// until the firmware programs the unit).
    #[serde(default)]
    pub film: Option<LiveViewFilm>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveViewFilm {
    /// 14-bit linear light of the scene at the reference white balance.
    pub linear_rgb: [u16; 3],
    /// (R, G, B) gains at which the scene renders neutral.
    pub white_balance_reference: [u32; 3],
    /// The sensor's host picture, developed pixel by pixel in place of the
    /// uniform `linear_rgb` light (each sRGB byte read as linear light on the
    /// sensor's 16383 scale, as the zone statistics read it).
    #[serde(default)]
    pub picture: Option<HostImage>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostImage {
    pub width: u64,
    pub height: u64,
    /// Rows of red, green and blue bytes, in hexadecimal.
    pub rgb: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlockJob {
    pub start: (u64, u32),
    /// Absent: `stop` names the bit a write clears to acknowledge.
    #[serde(default)]
    pub status: Option<(u64, u32)>,
    #[serde(default)]
    pub stop: Option<(u64, u32)>,
    pub irq: u32,
    pub ticks: u64,
}
/// Data a register block's job moves when it finishes, with the declared
/// approximation [`crate::development::PROFILE`], or
/// [`crate::development::FILM_PROFILE`] with the color unit's register block.
#[derive(Deserialize)]
#[serde(tag = "law", rename_all = "kebab-case", deny_unknown_fields)]
pub enum BlockWork {
    /// The development engine; the X-Trans site (line, sample) of its first
    /// source sample.
    Develop {
        profile: String,
        phase: (u8, u8),
        #[serde(default)]
        color: Option<u64>,
        /// Base of the still tone unit's register block: its matrix and curves
        /// are the still development's, the color unit's the rest.
        #[serde(default)]
        tone: Option<u64>,
        /// Reference (R, G, B) gains at which the declared scene renders neutral.
        #[serde(default)]
        white_balance: Option<[u32; 3]>,
    },
    /// The resizer.
    Resize { profile: String },
}
/// Writing `value` to `word` sets the bits `mask` of `status` to `state`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlockCommand {
    pub word: u64,
    pub value: u32,
    pub status: u64,
    pub mask: u32,
    pub state: u32,
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
// Deserialized once per board description, so the variant size difference costs nothing.
#[allow(clippy::large_enum_variant)]
pub enum Model {
    Gpio {
        base: u64,
        inputs: [u16; 16],
    },
    /// A command dial on declared GPIO pads, its phases emitted at declared
    /// edge times. The X-Processor Pro board has no pulse unit, so a counter
    /// wiring is refused.
    Encoder {
        options: fujimod_emulator_parts_controls::encoder::Options,
        #[serde(default)]
        counter: Option<String>,
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
        #[serde(default)]
        job: Option<BlockJob>,
        /// Data the job moves when it finishes.
        #[serde(default)]
        work: Option<BlockWork>,
        /// A uniform live-view image written before each frame interrupt.
        #[serde(default)]
        live_view_image: Option<LiveViewImage>,
        /// (offset, value) of words that read a fixed value.
        #[serde(default)]
        fixed: Vec<(u64, u32)>,
        /// Commands that set a field of a status word.
        #[serde(default)]
        commands: Vec<BlockCommand>,
    },
    Statistics {
        base: u64,
        irq: u32,
        period_ticks: u64,
        /// Uniform scene: red, green and blue zone sums.
        #[serde(default)]
        scene: Option<[u32; 3]>,
        /// Measured buffers written each frame: output register offset and words.
        #[serde(default)]
        buffers: Vec<(u64, Vec<u32>)>,
        /// Words of a declared buffer that are linear light sums: output
        /// register offset and `u32` indices into that buffer's words. They
        /// follow the scene light like the zone sums; the other words are
        /// written as declared.
        #[serde(default)]
        buffer_light_words: Vec<(u64, Vec<u32>)>,
        /// The zone sums scale by the imaging chip's integration value over
        /// this reference (declared with the imaging control's `integration`).
        #[serde(default)]
        exposure_reference: Option<u32>,
    },
    /// GPIO keys whose inputs also drive external interrupt lines; follows
    /// the GPIO bank and the reset/gate bank.
    KeyLines {
        lines: Vec<(usize, usize, usize)>,
        poll_ticks: u64,
    },
    Face {
        base: u64,
        irq: u32,
        run_ticks: u64,
    },
    Hdmi {
        base: u64,
    },
    Jpeg {
        base: u64,
        irq: u32,
        run_ticks: u64,
    },
    JpegDecoder {
        base: u64,
        irq: u32,
        run_ticks: u64,
    },
    H264 {
        base: u64,
        irq: u32,
        run_ticks: u64,
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
        #[serde(default)]
        handshake_ticks: Option<u64>,
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
        /// A sensor readout unit at one of the channel offsets.
        #[serde(default)]
        readout: Option<imaging::Readout>,
        /// Profile of the declared sensor frame the readout writes.
        #[serde(default)]
        sensor_frame: Option<String>,
        /// Caller-owned host RGB image the sensor frame writes instead of
        /// the virtual scene.
        #[serde(default)]
        sensor_image: Option<HostImage>,
        /// Opt-in register-bus frame log, read back through the device's
        /// diagnostic state (never part of a checkpoint).
        #[serde(default)]
        write_log: bool,
        /// Chip register pair (select, low, high) published as the
        /// integration value the statistics engine's zone sums follow.
        #[serde(default)]
        integration: Option<(u8, u16, u16)>,
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
    pub encoders: BTreeMap<String, fujimod_emulator_devices::parts::encoder_input::EncoderInput>,
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
    /// The channels and the earliest queued event. The event itself is in the
    /// board timeline, which restores with this state, so the cache must come
    /// back with it: re-arming would queue an extra event and change when
    /// the device is advanced.
    fn checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        fujimod_emulator_devices::checkpoint::Checkpoint::encode(
            self.name(),
            &(&self.model, self.scheduled),
        )
    }
    fn validate_checkpoint(
        &self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<(), String> {
        if !saved.children().is_empty() || !saved.blobs().is_empty() {
            return Err("Timer checkpoint has unexpected parts".into());
        }
        let (model, scheduled): (timer::TickTimers, Option<u64>) = saved.decode(self.name())?;
        self.model.check(&model)?;
        // A queued event is never later than the next channel expiry.
        if let (Some(at), Some(next)) = (scheduled, model.next_event_tick(0))
            && at > next
        {
            return Err("Timer checkpoint event cache is later than its next expiry".into());
        }
        Ok(())
    }
    fn restore_checkpoint(
        &mut self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        (self.model, self.scheduled) = saved.decode(self.name())?;
        Ok(())
    }
    fn fingerprint(&self) -> Result<Value, String> {
        Ok(self.checkpoint()?.state())
    }
}
/// Bytes of a hexadecimal node field.
fn hex(text: &str) -> Result<Vec<u8>, String> {
    if !text.len().is_multiple_of(2) {
        return Err("live-view host image is not whole bytes of hexadecimal".into());
    }
    (0..text.len())
        .step_by(2)
        .map(|i| {
            text.get(i..i + 2)
                .and_then(|pair| u8::from_str_radix(pair, 16).ok())
                .ok_or_else(|| "live-view host image is not hexadecimal".to_string())
        })
        .collect()
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
    let mut encoders = BTreeMap::new();
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
    // The audio block's words are also a read-only PL330 source, so the movie
    // channel's DMALDP from 0xfff1b020 reads zeros instead of refusing.
    let audio_words = spec
        .nodes
        .iter()
        .any(|n| matches!(&n.model, Model::RegisterBlock { base, .. } if *base == AUDIO_SOURCE.0))
        .then(blocks::Published::default);
    // The color unit and the still tone unit a development engine reads,
    // published by their blocks.
    let color = spec.nodes.iter().find_map(|n| match &n.model {
        Model::RegisterBlock {
            work:
                Some(BlockWork::Develop {
                    color: Some(base),
                    white_balance,
                    tone,
                    ..
                }),
            ..
        } => Some((
            *base,
            blocks::Published::default(),
            *white_balance,
            *tone,
            blocks::Published::default(),
        )),
        _ => None,
    });
    // A scene picture on the sensor and the live-view film's declared radiance
    // give the zone sums their per-zone content; without them the statistics
    // scene stays uniform.
    let film_radiance = spec.nodes.iter().find_map(|n| match &n.model {
        Model::RegisterBlock {
            live_view_image: Some(image),
            ..
        } => image.film.as_ref().map(|film| film.linear_rgb),
        _ => None,
    });
    let mut zone_source = None;
    for node in &spec.nodes {
        if let Model::ImagingControl {
            sensor_image: Some(image),
            ..
        } = &node.model
            && let Some(radiance) = film_radiance
        {
            zone_source = Some((image.width, image.height, hex(&image.rgb)?, radiance));
        }
    }
    // The imaging chip's integration value, shared with the statistics engine.
    let integration_value = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let integration_declared = spec.nodes.iter().any(|n| {
        matches!(
            n.model,
            Model::ImagingControl {
                integration: Some(_),
                ..
            }
        )
    });
    // The statistics scene-light scale (`statistics::Engine::set_scene_light`),
    // shared with the live-view film that draws the same light.
    let scene_light =
        std::sync::Arc::new(std::sync::atomic::AtomicU32::new(statistics::LIGHT_UNIT));
    let exposure_reference = spec.nodes.iter().find_map(|n| match &n.model {
        Model::Statistics {
            scene: Some(_),
            exposure_reference,
            ..
        } => *exposure_reference,
        _ => None,
    });
    let mut color_published = false;
    let mut tone_published = false;
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
            Model::Encoder { options, counter } => {
                if counter.is_some() {
                    return Err(
                        "the X-Processor Pro board has no command-dial counter wiring".into(),
                    );
                }
                let wiring = encoder::BankPins::new(
                    pins.clone()
                        .ok_or("the command-dial encoder follows the GPIO bank")?,
                );
                let model = fujimod_emulator_parts_controls::encoder::Encoder::new(
                    options,
                    fujimod_emulator_parts_controls::encoder::Wiring {
                        outputs: fujimod_emulator_parts_controls::encoder::OutputWiring::Gpio(
                            Box::new(wiring),
                        ),
                        counter: None,
                    },
                )?;
                if encoders.insert(node.name.clone(), model.input()).is_some() {
                    return Err("repeated X-Processor Pro command-dial encoder".into());
                }
                devices.attach(Box::new(model))
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
                    Peer::Idle => serial::Peer::Idle,
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
                job,
                work,
                live_view_image,
                fixed,
                commands,
            } => {
                let frames = frames.map(|f| blocks::Frames {
                    start: f.start,
                    stopped: f.stopped,
                    irq: f.irq,
                    period: f.period_ticks,
                });
                let mut model = blocks::Block::new(base, size, idle, frames)?;
                if base == AUDIO_SOURCE.0
                    && let Some(words) = &audio_words
                {
                    model = model.with_published(words.clone());
                }
                if let Some(j) = job {
                    model = model.with_job(blocks::Job {
                        start: j.start,
                        status: j.status,
                        stop: j.stop,
                        irq: j.irq,
                        ticks: j.ticks,
                    })?;
                }
                if base == blocks::MOVIE_OUTPUT_BASE {
                    model = model.with_movie_output(blocks::MOVIE_OUTPUT)?;
                }
                if let Some(work) = work {
                    let film = color.is_some();
                    let (profile, law, reads_color) = match work {
                        BlockWork::Develop {
                            profile,
                            phase,
                            color,
                            ..
                        } => (profile, blocks::Work::Develop { phase }, color.is_some()),
                        BlockWork::Resize { profile } => (profile, blocks::Work::Resize, film),
                    };
                    let expected = if reads_color {
                        crate::development::FILM_PROFILE
                    } else {
                        crate::development::PROFILE
                    };
                    if profile != expected {
                        return Err(format!("unknown still development profile {profile}"));
                    }
                    model = model.with_work(law)?;
                    if let (
                        blocks::Work::Develop { .. },
                        Some((_, words, reference, tone, tone_words)),
                    ) = (law, &color)
                    {
                        let reference = reference
                            .ok_or("a still color unit needs a reference white balance")?;
                        model = model.with_color(words.clone(), reference)?;
                        if tone.is_some() {
                            model = model.with_tone(tone_words.clone())?;
                        }
                    }
                }
                if let Some((unit, words, _, tone, tone_words)) = color.as_ref() {
                    if *unit == base {
                        model = model.with_published(words.clone());
                        color_published = true;
                    }
                    if *tone == Some(base) {
                        model = model.with_published(tone_words.clone());
                        tone_published = true;
                    }
                }
                if let Some(image) = live_view_image {
                    model = match (image.scene.as_deref(), image.host_image) {
                        (None, None) => match image.film {
                            None => model.with_live_view_image(image.lines, image.ycbcr)?,
                            Some(film) => {
                                let (_, words, ..) = color
                                    .as_ref()
                                    .ok_or("a live-view film needs the still color unit")?;
                                model = model.with_live_view_film(
                                    image.lines,
                                    film.linear_rgb,
                                    image.ycbcr,
                                    words.clone(),
                                    film.white_balance_reference,
                                )?;
                                if let Some(reference) = exposure_reference {
                                    model = model.with_film_exposure(statistics::Exposure {
                                        value: integration_value.clone(),
                                        reference,
                                    })?;
                                }
                                model = model.with_film_scene_light(statistics::SceneLight {
                                    value: scene_light.clone(),
                                })?;
                                match film.picture {
                                    Some(picture) => model.with_film_picture(
                                        picture.width,
                                        picture.height,
                                        hex(&picture.rgb)?,
                                    )?,
                                    None => model,
                                }
                            }
                        },
                        (Some(_), _) | (_, Some(_)) if image.film.is_some() => {
                            return Err("a live-view film has no scene or host image".into());
                        }
                        (Some(fujimod_virtual_scene::PROFILE), None) => {
                            model.with_live_view_scene(image.lines)?
                        }
                        (None, Some(host)) => model.with_live_view_host_image(
                            image.lines,
                            host.width,
                            host.height,
                            hex(&host.rgb)?,
                        )?,
                        (Some(_), Some(_)) => {
                            return Err("a live-view image has one source".into());
                        }
                        (Some(other), None) => {
                            return Err(format!("unknown live-view scene {other}"));
                        }
                    };
                }
                if !fixed.is_empty() {
                    model = model.with_fixed(fixed)?;
                }
                if !commands.is_empty() {
                    model = model.with_commands(
                        commands
                            .into_iter()
                            .map(|c| blocks::Command {
                                word: c.word,
                                value: c.value,
                                status: c.status,
                                mask: c.mask,
                                state: c.state,
                            })
                            .collect(),
                    )?;
                }
                devices.register(base, model.window(), Box::new(model))?
            }
            Model::Statistics {
                base,
                irq,
                period_ticks,
                scene,
                buffers,
                buffer_light_words,
                exposure_reference,
            } => {
                let mut model = statistics::Engine::new(base, irq, period_ticks)?;
                if let Some(scene) = scene {
                    model = model.with_scene(scene);
                    if let Some((width, height, rgb, radiance)) = &zone_source {
                        model = model.with_zone_scene(statistics::ZoneScene {
                            width: *width,
                            height: *height,
                            rgb: rgb.clone(),
                            sums: scene,
                            radiance: *radiance,
                        })?;
                    }
                    model = model.with_scene_light(statistics::SceneLight {
                        value: scene_light.clone(),
                    })?;
                }
                if let Some(reference) = exposure_reference {
                    if !integration_declared {
                        return Err(
                            "A statistics exposure requires a declared imaging integration".into(),
                        );
                    }
                    model = model.with_exposure(statistics::Exposure {
                        value: integration_value.clone(),
                        reference,
                    })?;
                }
                for (output, words) in buffers {
                    model = model.with_buffer(output, &words)?;
                }
                for (output, words) in buffer_light_words {
                    model = model.with_buffer_light_words(output, &words)?;
                }
                devices.register(base, statistics::WINDOW, Box::new(model))?
            }
            Model::KeyLines { lines, poll_ticks } => {
                let (_, external) = gates.as_ref().ok_or("Key lines follow the reset gates")?;
                devices.attach(Box::new(keys::KeyLines::new(
                    &lines,
                    poll_ticks,
                    pins.clone().ok_or("Key lines follow the GPIO bank")?,
                    external.clone(),
                )?))
            }
            Model::Face {
                base,
                irq,
                run_ticks,
            } => devices.register(
                base,
                face::WINDOW,
                Box::new(face::Engine::new(base, irq, run_ticks)?),
            )?,
            Model::Jpeg {
                base,
                irq,
                run_ticks,
            } => devices.register(
                base,
                jpeg::WINDOW,
                Box::new(jpeg::Encoder::new(base, irq, run_ticks)?),
            )?,
            Model::JpegDecoder {
                base,
                irq,
                run_ticks,
            } => devices.register(
                base,
                jpeg::WINDOW,
                Box::new(jpeg::Decoder::new(base, irq, run_ticks)?),
            )?,
            Model::H264 {
                base,
                irq,
                run_ticks,
            } => devices.register(
                base,
                h264::WINDOW,
                Box::new(h264::Encoder::new(base, irq, run_ticks)?),
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
                handshake_ticks,
                base,
                size,
                set_bits,
                clear_bits,
                scanout,
            } => {
                let output = display::Output::new(
                    base,
                    size,
                    &set_bits,
                    &clear_bits,
                    scanout.map(|(output, period)| display::Scanout { output, period }),
                )?;
                let output = match handshake_ticks {
                    Some(ticks) => output.with_handshake(ticks)?,
                    None => output,
                };
                devices.register(base, size, Box::new(output))?
            }
            Model::DisplayFrames { irqs, period_ticks } => {
                devices.attach(Box::new(display::FrameClock::new(irqs, period_ticks)?))
            }
            Model::ImagingControl {
                base,
                hole,
                readout,
                sensor_frame,
                sensor_image,
                write_log,
                integration,
            } => {
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
                    Box::new({
                        let mut control = imaging::Control::new(base)?;
                        if let Some(r) = readout {
                            control = control.with_readout(r)?;
                        }
                        let control = match sensor_frame {
                            Some(profile) => control.with_sensor_frame(&profile)?,
                            None => control,
                        };
                        let control = match sensor_image {
                            Some(image) => control.with_sensor_image(
                                image.width,
                                image.height,
                                hex(&image.rgb)?,
                            )?,
                            None => control,
                        };
                        let control = match integration {
                            Some((select, low, high)) => {
                                control.with_integration(imaging::Integration {
                                    select,
                                    low,
                                    high,
                                    value: integration_value.clone(),
                                })?
                            }
                            None => control,
                        };
                        if write_log {
                            control.with_write_log()
                        } else {
                            control
                        }
                    }),
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
                let mut sources: Vec<Arc<dyn pl330::Source>> = vec![Arc::new(Source(source))];
                if let Some(words) = &audio_words {
                    sources.push(Arc::new(blocks::SourceWindow::new_writable(
                        AUDIO_SOURCE.0,
                        AUDIO_SOURCE.1,
                        words.clone(),
                    )?));
                }
                sources.extend(
                    lens_link
                        .iter()
                        .map(|link| Arc::new(link.clone()) as Arc<dyn pl330::Source>),
                );
                let engine = pl330::Pl330::new(
                    dma,
                    irq_routes,
                    sources,
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
    if color.is_some() && !color_published {
        return Err("the still color unit must be a register block".into());
    }
    if color.as_ref().is_some_and(|c| c.3.is_some()) && !tone_published {
        return Err("the still tone unit must be a register block".into());
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
        encoders,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn graph_at(pc: u64) -> factory::Graph {
        serde_json::from_value(json!({
            "processors": [{"model": "cortex-a7", "released": true, "mprofile": null,
                            "rprofile": null, "registers": [["Pc", pc], ["Pstate", 0xd3]]}],
            "ram": [{"base": 0, "size": 0x10000}],
            "mappings": [{"core": 0, "address": 0, "physical": 0, "size": 0x10000,
                          "permissions": 7}],
            "dma_mappings": [{"address": 0, "physical": 0, "size": 0x10000, "permissions": 3}],
            "peripherals": [{"core": 0, "address": 0xf0000000_u64, "physical": 0xf0000000_u64,
                             "size": 0x10000000}],
            "decoders": [],
            "instructions_per_tick": [1, 1]
        }))
        .unwrap()
    }
    fn graph() -> factory::Graph {
        graph_at(0)
    }
    fn build_nodes(nodes: Value) -> Result<Built, String> {
        let spec = serde_json::from_value(json!({
            "kind": "xpp",
            "cpus": [{"mpidr": 0x80000000_u32, "configuration_base": 0xffff0000_u32,
                      "cycles_per_tick": 64}],
            "gic": {"distributor": 0xffff1000_u64, "cpu_interface": 0xffff2000_u64, "lines": 512},
            "nodes": nodes
        }))
        .map_err(|e| e.to_string())?;
        build(graph(), spec, &BTreeMap::new(), Mode::Strict)
    }

    #[test]
    fn the_audio_register_block_is_a_pl330_source() {
        let nodes = json!([
            {"name": "audio", "model": {"kind": "register-block", "base": 0xfff1b000_u64,
                                        "size": 0x1000, "idle": []}},
            {"name": "flash", "model": {
                "kind": "flash", "controller": 0xfff14000_u64, "aperture": 0xf0000000_u64,
                "size": 0x1000, "asset": "flash", "irq": 35, "dma": 0xfff18000_u64,
                "irq_routes": [207, 208, 209, 210, 211, 212, 213, 214],
                "setup_ticks": 16, "bytes_per_tick": 16, "transfer_ticks": 1, "nor": []}}
        ]);
        let spec = serde_json::from_value(json!({
            "kind": "xpp",
            "cpus": [{"mpidr": 0x80000000_u32, "configuration_base": 0xffff0000_u32,
                      "cycles_per_tick": 64}],
            "gic": {"distributor": 0xffff1000_u64, "cpu_interface": 0xffff2000_u64, "lines": 512},
            "nodes": nodes
        }))
        .unwrap();
        let assets = BTreeMap::from([("flash".to_string(), vec![0xff; 0x1000])]);
        let mut built = build(graph_at(0x200), spec, &assets, Mode::Strict).unwrap();
        // CPU stores: audio +0x20 = 0x11223344, then DMAGO channel 0 with the
        // program at 0x100, then spin.
        let stub: [u8; 0x60] = [
            0x3c, 0x00, 0x9f, 0xe5, 0x3c, 0x10, 0x9f, 0xe5, 0x00, 0x10, 0x80, 0xe5, 0x38, 0x00,
            0x9f, 0xe5, 0x38, 0x10, 0x9f, 0xe5, 0x00, 0x10, 0x80, 0xe5, 0x34, 0x00, 0x9f, 0xe5,
            0x34, 0x10, 0x9f, 0xe5, 0x00, 0x10, 0x80, 0xe5, 0x30, 0x00, 0x9f, 0xe5, 0x00, 0x10,
            0xa0, 0xe3, 0x00, 0x10, 0x80, 0xe5, 0xfe, 0xff, 0xff, 0xea, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0x20, 0xb0, 0xf1, 0xff, 0x44, 0x33, 0x22, 0x11, 0x0c, 0x8d, 0xf1,
            0xff, 0x00, 0x01, 0x00, 0x00, 0x08, 0x8d, 0xf1, 0xff, 0x00, 0x00, 0xa2, 0x00, 0x04,
            0x8d, 0xf1, 0xff,
        ];
        // DMAMOV SAR, 0xfff1b020; DAR, 0x4000; CCR 16-byte bursts;
        // DMALDP 0; DMAST; DMAEND.
        let program = [
            0xbc, 0, 0x20, 0xb0, 0xf1, 0xff, 0xbc, 2, 0x00, 0x40, 0x00, 0x00, 0xbc, 1, 0x17, 0xc0,
            0x05, 0x00, 0x25, 0x00, 0x08, 0x00,
        ];
        built.board.write_physical(0x100, &program).unwrap();
        built.board.write_physical(0x200, &stub).unwrap();
        built.board.run_until(40, 1).unwrap();
        let mut bytes = [0xff; 16];
        built.board.read_physical(0x4000, &mut bytes).unwrap();
        assert_eq!(bytes[..4], 0x1122_3344_u32.to_le_bytes());
        assert_eq!(bytes[4..], [0; 12]);
    }

    #[test]
    fn strict_board_encodes_an_h264_frame_and_routes_line_110() {
        // The X-T20 movie block at 0xfff5c000; line 110 is row 0x31 of 0xed4720.
        let spec = serde_json::from_value(json!({
            "kind": "xpp",
            "cpus": [{"mpidr": 0x80000000_u32, "configuration_base": 0xffff0000_u32,
                      "cycles_per_tick": 64}],
            "gic": {"distributor": 0xffff1000_u64, "cpu_interface": 0xffff2000_u64, "lines": 512},
            "nodes": [
                {"name": "h264", "model": {
                    "kind": "h264", "base": 0xfff5c000_u64, "irq": 110, "run_ticks": 10}},
                {"name": "movie-registers", "model": {
                    "kind": "register-block", "base": 0xfff5b000_u64,
                    "size": 0x1000, "idle": []}}
            ]
        }))
        .unwrap();
        let mut built = build(graph_at(0x200), spec, &BTreeMap::new(), Mode::Strict).unwrap();
        let id = built.nodes["h264"];
        let registers = built.nodes["movie-registers"];
        let mut packed = Vec::with_capacity(16 * 16 * 2);
        for _ in 0..16 {
            for column in 0..16 {
                let pair = if column < 8 { 40 } else { 200 };
                packed.extend_from_slice(&[pair, pair, 96, 160]);
            }
        }
        built.board.write_physical(0x2000, &packed).unwrap();
        // Allow the frame to finish, then save the status word, GIC ISPENDR3
        // (line 110 is bit 14) and the adjacent 0xfff5b000 block. The pool
        // holds 0x5000, 0xfff5c204, 0xffff120c and 0xfff5b000.
        let stub: [u8; 144] = [
            0x78, 0x40, 0x9f, 0xe5, 0x78, 0x00, 0x9f, 0xe5, 0x78, 0x20, 0x9f, 0xe5, 0x78, 0x50,
            0x9f, 0xe5, 0x00, 0x00, 0xa0, 0xe1, 0x00, 0x00, 0xa0, 0xe1, 0x00, 0x00, 0xa0, 0xe1,
            0x00, 0x00, 0xa0, 0xe1, 0x00, 0x00, 0xa0, 0xe1, 0x00, 0x00, 0xa0, 0xe1, 0x00, 0x00,
            0xa0, 0xe1, 0x00, 0x00, 0xa0, 0xe1, 0x00, 0x00, 0xa0, 0xe1, 0x00, 0x00, 0xa0, 0xe1,
            0x00, 0x00, 0xa0, 0xe1, 0x00, 0x00, 0xa0, 0xe1, 0x00, 0x00, 0xa0, 0xe1, 0x00, 0x00,
            0xa0, 0xe1, 0x00, 0x00, 0xa0, 0xe1, 0x00, 0x00, 0xa0, 0xe1, 0x00, 0x00, 0xa0, 0xe1,
            0x00, 0x00, 0xa0, 0xe1, 0x00, 0x00, 0xa0, 0xe1, 0x00, 0x00, 0xa0, 0xe1, 0x00, 0x00,
            0xa0, 0xe1, 0x00, 0x10, 0x90, 0xe5, 0x00, 0x10, 0x84, 0xe5, 0x00, 0x30, 0x92, 0xe5,
            0x04, 0x30, 0x84, 0xe5, 0x00, 0x30, 0x95, 0xe5, 0x08, 0x30, 0x84, 0xe5, 0xfe, 0xff,
            0xff, 0xea, 0x00, 0x50, 0x00, 0x00, 0x04, 0xc2, 0xf5, 0xff, 0x0c, 0x12, 0xff, 0xff,
            0x00, 0xb0, 0xf5, 0xff,
        ];
        built.board.write_physical(0x200, &stub).unwrap();
        built
            .board
            .device_input::<blocks::Block, _>(registers, |block, cx| {
                block.write(cx, 0xfff5b000, 4, 0x5a5a_5a5a);
                Ok(())
            })
            .unwrap();
        built
            .board
            .device_input::<h264::Encoder, _>(id, |device, cx| {
                for (offset, value) in [
                    (0x000, 0),
                    (0x008, 0x21),
                    (0x018, 16 << 16 | 16),
                    (0x208, 0x0400_0400),
                    (0x01C, 0x8000),
                    (0x028, 0x10000),
                    (0x02C, 0x9000),
                    (0x100, 0x2000),
                    (0x104, 0x3000),
                    (0x108, 0x4000),
                    (0x200, 0x10001),
                ] {
                    device.write(cx, 0xfff5c000 + offset, 4, value);
                }
                Ok(())
            })
            .unwrap();
        built.board.run_until(100, 1).unwrap();
        let saved = built.board.device_save(id).unwrap();
        assert_eq!(saved["frames"], 1);
        assert_eq!(saved["irq"], true);
        let mut saved_words = [0u8; 12];
        built.board.read_physical(0x5000, &mut saved_words).unwrap();
        assert_eq!(
            u32::from_le_bytes(saved_words[..4].try_into().unwrap()) & 0x10000,
            0x10000,
            "status reads ready"
        );
        assert_eq!(
            u32::from_le_bytes(saved_words[4..8].try_into().unwrap()),
            1 << 14,
            "pending line 110"
        );
        assert_eq!(
            u32::from_le_bytes(saved_words[8..].try_into().unwrap()),
            0x5a5a_5a5a,
            "the adjacent block keeps its own window"
        );
        let mut frame = [0u8; 80];
        built.board.read_physical(0x8000, &mut frame).unwrap();
        assert_eq!(&frame[..5], [0, 0, 0, 1, 0x67], "SPS at the output base");

        // The stock movie path leaves +0x01c/+0x028/+0x02c zero because the
        // only context writer (0x184c9d0) clears them. A second start with the
        // stock descriptor must complete without a zero-span refusal.
        built
            .board
            .device_input::<h264::Encoder, _>(id, |device, cx| {
                for (offset, value) in [(0x01C, 0), (0x028, 0), (0x02C, 0), (0x200, 0x10001)] {
                    device.write(cx, 0xfff5c000 + offset, 4, value);
                }
                Ok(())
            })
            .unwrap();
        built.board.run_until(200, 1).unwrap();
        let saved = built.board.device_save(id).unwrap();
        assert_eq!(saved["frames"], 2, "zero-span frame completes");
        assert_eq!(saved["irq"], true);
    }

    #[test]
    fn command_dial_phases_follow_the_gray_order_at_their_declared_pace() {
        // A dial on GPIO bank 1 pads 2 and 3, resting at phase 2 (A high, B low),
        // with a 10-tick edge period so the timeline stays short.
        let built = build_nodes(json!([
            {"name": "gpio", "model": {"kind": "gpio", "base": 0xfff10000_u64,
                                       "inputs": [0, 4, 0, 0, 0, 0, 0, 0,
                                                  0, 0, 0, 0, 0, 0, 0, 0]}},
            {"name": "dial", "model": {"kind": "encoder", "options": {
                "pins": [{"port": 1, "pin": 2}, {"port": 1, "pin": 3}],
                "phase_ticks": 10, "rest_phase": 2}, "counter": null}}
        ]))
        .unwrap();
        let Built {
            mut board,
            nodes,
            encoders,
            ..
        } = built;
        let gpio = nodes["gpio"];
        let level = |board: &Board| {
            let state = board.device_save(gpio).unwrap();
            state["inputs"][1].as_u64().unwrap() as u32 & 0xf
        };
        assert_eq!(level(&board), 0x4);
        let dial = encoders["dial"].clone();
        let dial_node = nodes["dial"];
        // One increasing step: 2 -> 0 -> 1 -> 3 -> 2, one edge per declared period.
        dial.queue_cycles(0, 1).unwrap();
        board.inputs_changed(dial_node).unwrap();
        for (edge, pins) in [(10, 0x0), (20, 0x8), (30, 0xc), (40, 0x4)] {
            board.run_until(edge + 1, 1).unwrap();
            assert_eq!(level(&board), pins, "up at {edge}");
        }
        // One decreasing step runs the same Gray order backwards.
        dial.queue_cycles(45, -1).unwrap();
        board.inputs_changed(dial_node).unwrap();
        for (edge, pins) in [(55, 0xc), (65, 0x8), (75, 0x0), (85, 0x4)] {
            board.run_until(edge + 1, 1).unwrap();
            assert_eq!(level(&board), pins, "down at {edge}");
        }
    }

    #[test]
    fn graph_nodes_take_the_x_t20_imaging_and_key_inputs() {
        // X-T20 2.11 values: the idle serial channel 3, the statistics scene
        // and one measured buffer, live-view output unit 1 and the S1/S2 lines.
        let gpio = json!({"name": "gpio", "model": {"kind": "gpio", "base": 0xfff10000_u64,
                                                    "inputs": ([0_u16; 16])}});
        let gates = json!({"name": "resets", "model": {"kind": "reset-gates",
                                                       "base": 0xfff0a000_u64}});
        let keys = json!({"name": "keys", "model": {"kind": "key-lines",
                                                    "lines": [[0, 0, 0], [1, 0, 1]],
                                                    "poll_ticks": 48000}});
        let unit = |frames: Value| {
            json!({"name": "live-view", "model": {
                "kind": "register-block", "base": 0xfff56000_u64, "size": 0x1000,
                "idle": [[0, 1]], "frames": frames,
                "live_view_image": {"lines": 480, "ycbcr": [73, 114, 138]}}})
        };
        let frames = json!({"start": [0xc8, 0x10000000], "stopped": [0xd8, 0x100],
                            "irq": 108, "period_ticks": 880000});
        let built = build_nodes(json!([
            gpio,
            gates,
            {"name": "serial3", "model": {"kind": "serial-port", "base": 0xfff2b000_u64,
                                          "peer": {"kind": "idle"}}},
            {"name": "statistics", "model": {"kind": "statistics", "base": 0xfff54000_u64,
                                             "irq": 106, "period_ticks": 880000,
                                             "scene": [144736, 383321, 33695],
                                             "buffers": [[0x1d0, [1, 2, 3]]]}},
            unit(frames),
            keys
        ]))
        .unwrap();
        assert_eq!(
            built.nodes.keys().map(String::as_str).collect::<Vec<_>>(),
            [
                "gpio",
                "keys",
                "live-view",
                "resets",
                "serial3",
                "statistics"
            ]
        );
        // Key lines take the GPIO pins and the reset/gate bank's lines.
        let error = |nodes| build_nodes(nodes).err().unwrap();
        assert_eq!(
            error(json!([gpio, keys])),
            "Key lines follow the reset gates"
        );
        assert_eq!(
            error(json!([gates, keys])),
            "Key lines follow the GPIO bank"
        );
        // A live-view image needs the unit's frame law.
        assert_eq!(
            error(json!([unit(Value::Null)])),
            "Invalid X-Processor Pro live-view image"
        );
    }

    #[test]
    fn timer_checkpoint_keeps_the_event_cache_that_matches_the_board_timeline() {
        use fujimod_emulator_devices::test_host::{TestHost, assert_exact_restore, write};
        let base = 0xfff30000;
        let mut timers = Timers {
            base,
            model: timer::TickTimers::new(&[(0, 48), (1, 49)]).unwrap(),
            scheduled: None,
        };
        let mut host = TestHost::new();
        write(&mut timers, &mut host, base + 0x100, 40);
        write(&mut timers, &mut host, base + 0x104, 40);
        write(&mut timers, &mut host, base, 0xfffe_0001);
        let expiry = timers.model.next_event_tick(0).unwrap();
        assert_eq!(timers.scheduled, Some(expiry));
        let saved = assert_exact_restore(&mut timers, |d| {
            let mut host = TestHost::new();
            host.now = expiry + 1;
            let mut cx = host.cx();
            d.advance(&mut cx);
        });
        // The cache is part of the state: a restored device does not queue the
        // same event a second time.
        assert_eq!(timers.scheduled, Some(expiry));
        let mut fresh = Timers {
            base,
            model: timer::TickTimers::new(&[(0, 48), (1, 49)]).unwrap(),
            scheduled: None,
        };
        fresh.restore_checkpoint(&saved).unwrap();
        assert_eq!(fresh.scheduled, Some(expiry));
        let mut miswired = Timers {
            base,
            model: timer::TickTimers::new(&[(0, 50)]).unwrap(),
            scheduled: None,
        };
        assert!(miswired.restore_checkpoint(&saved).is_err());
    }
}
