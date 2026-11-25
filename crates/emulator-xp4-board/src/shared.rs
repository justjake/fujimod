//! XP4 on the shared board: the caller's execution graph (processors, RAM,
//! memory windows and maps), a GICv2 at the processors' configuration base,
//! and every device the definition selects.
use super::{definition::TimerLayout, device_build, memory, Definition};
use fujimod_catalog_configuration::RunEngine;
use fujimod_emulator::board::{Board, InterruptMap, InterruptRegisters, armv7a, bus::{Bus, Mode}, factory};
use fujimod_emulator_devices::{Cx, Device, checkpoint::Checkpoint};
use fujimod_emulator_ip_arm::gicv2;
use fujimod_emulator_xp4_io::interval_timers::IntervalTimers;

/// GIC lines the XP4 board wires.
const GIC_LINES: usize = 512;
/// Performance-counter cycles per board tick of each application core.
const CYCLES_PER_TICK: u64 = 64;
/// Register window of the XP4 interval timer.
const TIMER_WINDOW: u64 = 0x50;

/// Build the XP4 board. `graph` holds the processors and memory the run
/// selected; the definition's devices join it on the board bus.
pub fn build(
    graph: factory::Graph,
    mut definition: Definition,
    storage_image: Option<Vec<u8>>,
    card_files: Vec<(String, Vec<u8>)>,
    card_media: Vec<(usize, fujimod_emulator_devices::parts::sd_card::Card)>,
    mode: Mode,
    engine: RunEngine,
) -> Result<Board, String> {
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
    let built = device_build::build(definition, &memory, storage_image, card_files, card_media)?;
    // Serial-flash execution is served by the controller's execute-in-place
    // pages, so the views the old engine mapped are not needed here.
    let mut devices = built.devices.ok_or("the XP4 definition selects no devices")?.into_graph()?;
    if let Some((base, model)) = timer {
        devices.register(base, TIMER_WINDOW, Box::new(Timers { base, model, scheduled: None }))?;
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
    Ok(board)
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
