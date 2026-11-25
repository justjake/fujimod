//! XP4 on the shared board: the caller's execution graph (processors, RAM,
//! memory windows and maps), a GICv2 at the processors' configuration base,
//! and every device the definition selects.
use super::{definition::TimerLayout, device_build, devices::Connected, memory, Definition};
use fujimod_emulator_xp4_storage::storage::SerialFlashController;
use fujimod_catalog_configuration::RunEngine;
use fujimod_emulator::board::{InterruptMap, InterruptRegisters, armv7a, bus::{Bus, Mode}, factory};
use fujimod_emulator_devices::{Cx, Device, checkpoint::Checkpoint};
use fujimod_emulator_ip_arm::gicv2;
use fujimod_emulator_xp4_io::interval_timers::IntervalTimers;

/// GIC lines the XP4 board wires.
const GIC_LINES: usize = 512;
/// Performance-counter cycles per board tick of each application core.
const CYCLES_PER_TICK: u64 = 64;
/// Register window of the XP4 interval timer.
const TIMER_WINDOW: u64 = 0x50;

/// Build the XP4 board as a shared session. `graph` holds the processors
/// and memory the run selected, its instruction rate per core; the
/// definition's devices join it on the board bus. `record` is the
/// definition the session reports.
pub fn build(
    mut graph: factory::Graph,
    mut definition: Definition,
    storage_image: Option<Vec<u8>>,
    card_files: Vec<(String, Vec<u8>)>,
    card_media: Vec<(usize, fujimod_emulator_devices::parts::sd_card::Card)>,
    mode: Mode,
    engine: RunEngine,
    record: serde_json::Value,
) -> Result<(fujimod_emulator_session::Session, Inputs), String> {
    let cores = graph.processors.len();
    if cores != definition.cores || !(1..=4).contains(&cores) {
        return Err("XP4 processor wiring differs from its definition".into());
    }
    let cbar = definition.cbar.ok_or("XP4 requires an explicit configuration base")?;
    let memory = memory::allocate(std::mem::take(&mut definition.memory_regions), definition.ram_size)?;
    let timer = match (definition.timer_base, definition.timer_layout.take()) {
        (Some(base), Some(TimerLayout::Xp4)) => {
            if definition.timer_routes.iter().any(|&(channel, _)| channel >= 8) {
                return Err("Invalid XP4 interval-timer route".into());
            }
            Some((u64::from(base), IntervalTimers::new(&definition.timer_routes)))
        }
        (None, None) => None,
        _ => return Err("the XP4 board needs an XP4 interval timer with its window".into()),
    };
    let cache_sizes = definition.cache_sizes.map(|(d, i, l2)| (d, i, l2));
    let dials = definition.countdown_dials.iter().map(|dial| (dial.name.clone(), dial.channel)).collect();
    let built = device_build::build(definition, &memory, storage_image, card_files, card_media)?;
    // Serial-flash execution is served by the controller's execute-in-place
    // pages, so the views the old engine mapped are not needed here.
    // The XP4 physical map is the identity on every core: each device
    // window, the timer and the GIC's distributor and CPU interface.
    let mut windows = built.devices.as_ref().map(|devices| devices.guard_windows()).unwrap_or_default();
    let mut fixed = vec![(u64::from(cbar) + 0x1000, u64::from(cbar) + 0x3000)];
    if let Some((base, _)) = &timer {
        fixed.push((*base, base + TIMER_WINDOW));
    }
    // A memory region a device window covers is that device's (the input
    // buffer SRAM behind the imaging model); any other overlap with memory,
    // the GIC or the interval timer refuses.
    graph.ram.retain(|ram| {
        let end = ram.base + ram.size as u64;
        !windows.iter().any(|&(lo, hi)| lo <= ram.base && end <= hi)
    });
    let memory_ranges = graph.ram.iter().map(|ram| (ram.base, ram.base + ram.size as u64));
    let taken: Vec<(u64, u64)> = memory_ranges.chain(fixed.iter().copied()).collect();
    if let Some((lo, hi, a, b)) = windows.iter().find_map(|&(lo, hi)| {
        taken.iter().find(|&&(a, b)| lo < b && a < hi).map(|&(a, b)| (lo, hi, a, b))
    }) {
        return Err(format!(
            "Overlapping XP4 device window {lo:#x}..{hi:#x} and memory or interrupt controller {a:#x}..{b:#x}"
        ));
    }
    windows.extend(fixed);
    for core in 0..cores {
        for &(lo, hi) in &windows {
            let size = usize::try_from(hi - lo).map_err(|_| "XP4 device window exceeds the host")?;
            graph.peripherals.push(factory::Peripheral { core, address: lo, physical: lo, size });
        }
    }
    let (mut devices, mut nodes) = match built.devices {
        Some(devices) => devices.into_graph()?,
        None => Default::default(),
    };
    if let Some((base, model)) = timer {
        let id = devices.register(base, TIMER_WINDOW, Box::new(Timers { base, model, scheduled: None }))?;
        nodes.insert("xp4-interval-timers".into(), id);
    }
    // The session's GPIO bank, which pin inputs address by this name.
    if let Some(&gpio) = nodes.get("masked-gpio") {
        nodes.insert("gpio".into(), gpio);
    }
    let interrupts = InterruptMap {
        gic: Box::new(gicv2::Gic::new(cores, GIC_LINES)),
        cores,
        registers: InterruptRegisters::GicV2 {
            distributor: u64::from(cbar) + 0x1000,
            cpu_interface: u64::from(cbar) + 0x2000,
        },
    };
    let mut board = graph.build(factory::Options::new(engine), Bus::from_graph(mode, devices)?, Some(interrupts))?;
    for core in 0..cores {
        board.configure_armv7a(core, armv7a::Config {
            mpidr: 0x8000_0000 | core as u32,
            configuration_base: cbar,
            cycles_per_tick: CYCLES_PER_TICK,
            cache_sizes,
        })?;
    }
    let session = fujimod_emulator_session::Session {
        definition: record,
        board,
        controls: None,
        contact_driver: None,
        resources: fujimod_emulator_exr2_resources::Resources::default(),
        nodes,
        contacts: Default::default(),
        power_cycle: None,
        window_action: None,
        recording_frames: Default::default(),
    };
    Ok((session, Inputs { dials }))
}

/// Build the XP4 board from its definition alone: its cores start from
/// `initial_registers`, DRAM from address 0 and each declared memory region
/// are mapped identically on every core, and each core runs the definition's
/// instruction rate. A discovery definition builds a discovery-mode bus.
pub fn from_definition(
    definition: Definition,
    storage_image: Option<Vec<u8>>,
    card_files: Vec<(String, Vec<u8>)>,
    card_media: Vec<(usize, fujimod_emulator_devices::parts::sd_card::Card)>,
    engine: RunEngine,
) -> Result<(fujimod_emulator_session::Session, Inputs), String> {
    use fujimod_emulator::board::memory::BusMapping;
    let cores = definition.cores;
    if definition.initial_registers.len() > cores {
        return Err("XP4 initial registers name more cores than the definition".into());
    }
    let mut memory = vec![(0, definition.ram_size, 7)];
    memory.extend(definition.memory_regions.iter().map(|r| (r.base, r.size, r.permissions)));
    let graph = factory::Graph {
        processors: (0..cores)
            .map(|core| factory::Processor {
                model: fujimod_emulator_cpu::Model::CortexA7,
                registers: definition.initial_registers.get(core).cloned().unwrap_or_default(),
                released: true,
                parked_until_sgi: false,
                mprofile: None,
                rprofile: None,
            })
            .collect(),
        ram: memory.iter().map(|&(base, size, _)| factory::Ram { base, size }).collect(),
        dma_mappings: vec![BusMapping::ram(0, definition.ram_size)],
        decoders: Vec::new(),
        mappings: (0..cores)
            .flat_map(|core| {
                memory.iter().map(move |&(address, size, permissions)| factory::MemoryMap {
                    core,
                    address,
                    physical: address,
                    size,
                    permissions,
                })
            })
            .collect(),
        peripherals: Vec::new(),
        instructions_per_tick: fujimod_emulator_cpu::timeline::Ratio::new(
            definition.instructions_per_tick.0,
            definition.instructions_per_tick.1,
        )?,
    };
    let mode = if definition.discovery { Mode::Discovery } else { Mode::Strict };
    let record = serde_json::json!({"cores": cores, "ram_size": definition.ram_size, "mode": mode.name()});
    build(graph, definition, storage_image, card_files, card_media, mode, engine, record)
}

/// The XP4 interval timer: eight channels, each pulsing its routed line.
struct Timers {
    base: u64,
    model: IntervalTimers,
    // Earliest pending event; later expiries wait for it to re-arm.
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
    fn access(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, write: bool, value: u64) -> u64 {
        self.advance(cx);
        let result = self.model.access(cx.now(), (address - self.base) as u32, size.into(), write, value as u32);
        if write {
            self.schedule(cx);
        }
        result.map(u64::from).unwrap_or_else(|error| {
            cx.fail(error);
            0
        })
    }
}
impl Device for Timers {
    fn name(&self) -> &'static str {
        "xp4-interval-timers"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        self.access(cx, address, size, false, 0)
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        self.access(cx, address, size, true, value);
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        if self.scheduled.is_some_and(|at| cx.now() < at) {
            return;
        }
        for irq in self.model.tick(cx.now()) {
            cx.pulse_irq(irq as u32);
        }
        self.schedule(cx);
    }
    fn event(&mut self, cx: &mut Cx<'_>, _: u64) {
        self.advance(cx);
    }
    fn next_deadline(&self, now: u64) -> Option<u64> {
        self.model.next_event_tick(now)
    }
    fn save(&self) -> serde_json::Value {
        self.model.save()
    }
    fn restore(&mut self, value: &serde_json::Value) -> Result<(), String> {
        self.scheduled = None;
        self.model.restore(value)
    }
    fn checkpoint(&self) -> Result<Checkpoint, String> {
        Checkpoint::encode(self.name(), &(&self.model, self.scheduled))
    }
    fn validate_checkpoint(&self, saved: &Checkpoint) -> Result<(), String> {
        if !saved.children().is_empty() || !saved.blobs().is_empty() {
            return Err("Timer checkpoint has unexpected parts".into());
        }
        let (model, _): (IntervalTimers, Option<u64>) = saved.decode(self.name())?;
        self.model.clone().restore(&model.save())
    }
    fn restore_checkpoint(&mut self, saved: &Checkpoint) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        (self.model, self.scheduled) = saved.decode(self.name())?;
        Ok(())
    }
    fn fingerprint(&self) -> Result<serde_json::Value, String> {
        Ok(self.checkpoint()?.state())
    }
}

/// The node of the board's serial-flash controller.
fn serial_flash(session: &fujimod_emulator_session::Session) -> Result<usize, String> {
    session.node("xp4-serial-flash").map_err(|_| "Board has no XP4 NOR".to_string())
}

/// Completed NOR contents, without controller or processor state.
pub fn nor_medium(session: &fujimod_emulator_session::Session) -> Result<Vec<u8>, String> {
    session.board.with_device::<Connected<SerialFlashController>, _>(serial_flash(session)?, |flash| {
        flash.0.lock().map_err(|_| "NOR controller lock poisoned")?.nor_medium()
    })?
}

/// Reset NOR program observation, refusing unsettled commands.
pub fn reset_nor_program_coverage(session: &mut fujimod_emulator_session::Session) -> Result<(), String> {
    let node = serial_flash(session)?;
    session.board.edit_device::<Connected<SerialFlashController>, _>(node, |flash| {
        flash.0.lock().map_err(|_| "NOR controller lock poisoned")?.reset_nor_program_coverage()
    })
}

/// A copy of the medium in the `host`th SD card slot, in node order.
pub fn card_medium(
    session: &fujimod_emulator_session::Session,
    host: usize,
) -> Result<fujimod_emulator_devices::parts::sd_card::Card, String> {
    let mut nodes: Vec<(&String, &usize)> = session.nodes.iter().collect();
    nodes.sort_by_key(|&(_, id)| *id);
    nodes.dedup_by_key(|(_, id)| **id);
    nodes
        .into_iter()
        .filter_map(|(name, _)| session.card_medium(name).ok())
        .nth(host)
        .ok_or_else(|| "No SD card slot with that index".into())
}

/// External XP4 inputs the shared session's generic inputs do not name: the
/// command dials of the countdown timers, the USB cable with its condition
/// strap and the DWC3 host transactions, single ADC channels, GPIO pins and
/// the touch panel.
pub struct Inputs {
    /// Countdown-timer channel of each named command dial.
    dials: std::collections::BTreeMap<String, usize>,
}

type Dwc3 = Connected<fujimod_emulator_ip::usb::dwc3::Dwc3>;
type Session = fujimod_emulator_session::Session;

impl Inputs {
    /// Queue `edges` detent edges on a named command dial.
    pub fn turn_dial(&self, session: &mut Session, name: &str, edges: i32) -> Result<(), String> {
        use fujimod_emulator_xp4_io::countdown_timers::CountdownTimers;
        let channel = *self.dials.get(name).ok_or_else(|| format!("No XP4 command dial named {name}"))?;
        let node = session.node("xp4-countdown-timers")?;
        session.board.with_device_inputs::<CountdownTimers, _>(node, |timers, now| {
            timers.dial_input(channel).ok_or("Unwired XP4 countdown dial")?.queue_edges(now, edges)
        })
    }
    /// Set one external ADC channel (XP4 ADC or X-Processor Pro converter).
    pub fn set_adc_input(&self, session: &mut Session, channel: usize, sample: u16) -> Result<(), String> {
        use fujimod_emulator_devices::capability::AnalogInputs;
        let node = session.node("xp4-adc").or_else(|_| session.node("xpp-adc")).map_err(|_| "XP4 ADC inputs are not wired")?;
        session.board.edit_capability::<dyn AnalogInputs, _>(node, |adc| adc.set_sample(channel, sample))
    }
    /// Drive one external GPIO pin.
    pub fn set_gpio_input(&self, session: &mut Session, port: usize, pin: usize, high: bool) -> Result<(), String> {
        let (port, pin) = (u8::try_from(port), u8::try_from(pin));
        let (Ok(port), Ok(pin)) = (port, pin) else { return Err("Invalid XP4 external GPIO pin".into()) };
        session.gpio("gpio", port, pin, high)
    }
    /// Plug or unplug the USB cable: the condition strap the stock USB task
    /// reads and the DWC3 data lines change together.
    pub fn set_usb_connected(&self, session: &mut Session, connected: bool) -> Result<(), String> {
        use fujimod_emulator_xp4_io::usb_condition::UsbCondition;
        if let Ok(node) = session.node("xp4-usb-condition") {
            session.board.with_device_inputs::<UsbCondition, _>(node, |strap, _| {
                strap.presence().store(connected, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            })?;
        }
        self.with_usb(session, |usb, now| usb.link(now, connected))
    }
    pub fn usb_bus_reset(&self, session: &mut Session) -> Result<(), String> {
        self.with_usb(session, |usb, now| usb.bus_reset(now))
    }
    /// Queue one host transaction on a DWC3 physical endpoint; returns its id.
    pub fn usb_transaction(
        &self,
        session: &mut Session,
        endpoint: usize,
        setup: bool,
        out: Option<Vec<u8>>,
    ) -> Result<u64, String> {
        self.with_usb(session, |usb, now| usb.transaction(now, endpoint, setup, out))
    }
    pub fn usb_reply(&self, session: &Session, id: u64) -> Result<Option<fujimod_emulator_ip::usb::dwc3::Reply>, String> {
        let node = session.node("dwc3").map_err(|_| "DWC3 is not wired")?;
        session.board.with_device::<Dwc3, _>(node, |usb| usb.0.lock().unwrap().reply(id).cloned())
    }
    pub fn usb_consume_reply(&self, session: &mut Session, id: u64) -> Result<(), String> {
        self.with_usb(session, |usb, _| usb.consume_reply(id))
    }
    fn with_usb<R>(
        &self,
        session: &mut Session,
        f: impl FnOnce(&mut fujimod_emulator_ip::usb::dwc3::Dwc3, u64) -> Result<R, String>,
    ) -> Result<R, String> {
        let node = session.node("dwc3").map_err(|_| "DWC3 is not wired")?;
        session.board.with_device_inputs::<Dwc3, _>(node, |usb, now| f(&mut usb.0.lock().unwrap(), now))
    }
}
