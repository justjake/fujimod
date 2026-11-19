//! X-mount lens connector of the X-T5 body: mount detect, accessory sense,
//! lens supplies, ready/busy interrupt and the lens-bus serial controller.
//!
//! Pads (X-T5 4.31 lens driver, scratch/lens/REPORT.md 1.3..1.5):
//! - detect (0.11) reads low while a lens is attached;
//! - three sense pads (10.0/10.1/10.2) show the lens pattern 1/0/1 while the
//!   lens is unpowered (the body's detect step requires it before power);
//! - the two supply enables (15.9 and 15.10) are the body's output latches;
//!   the lens powers up when both are driven high;
//! - the ready pad (14.1) carries the lens ready output. While the pad is in
//!   its interrupt function (`ready_mux`), the low level drives one GIC line
//!   and the high level another (INTIDs 131 and 115 on the X-T5).
//!
//! The wrapped serial controller (PL022 at 0xff910000) carries the lens
//! protocol to [`x_mount_model::Port`]. Which lens is attached is a run input:
//! the manifest names a default and `XT5_LENS=none|xf18-55|prime` overrides it.
//! Electrical meanings and timings are inferred, not measured.
use fujimod_emulator_devices::checkpoint::Checkpoint;
use fujimod_emulator_devices::parts::pins::Pin;
use fujimod_emulator_devices::xp5::pins::SharedPins;
use fujimod_emulator_devices::{Cx, Device};
use fujimod_emulator_parts_lens::x_mount_link::FixedReply;
use fujimod_emulator_parts_lens::x_mount_model::{Port, Spec, Timing};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const LENS_ENV: &str = "XT5_LENS";
pub const READY_ENV: &str = "XT5_LENS_READY";

fn default_lens() -> String {
    "xf18-55".into()
}
fn default_operation_us() -> u64 {
    1000
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    /// Default lens: a preset name or "none"; `XT5_LENS` overrides it.
    #[serde(default = "default_lens")]
    pub lens: String,
    pub detect: Pin,
    pub sense: [Pin; 3],
    pub sense_unpowered: [bool; 3],
    pub supplies: [Pin; 2],
    pub ready: Pin,
    /// Pad function that routes the ready pad to its interrupt lines; absent routes it always.
    #[serde(default)]
    pub ready_mux: Option<u32>,
    pub ready_low_line: u32,
    pub ready_high_line: u32,
    /// From both supplies high until the lens pulls ready low.
    pub startup_us: u64,
    /// From then until the lens releases ready high.
    pub busy_us: u64,
    /// Shortest ready-low pulse for a focus or aperture request.
    #[serde(default = "default_operation_us")]
    pub operation_us: u64,
    /// Declared payloads for request families the lens model does not drive.
    #[serde(default)]
    pub replies: Vec<FixedReply>,
}
impl Options {
    fn validate(&self) -> Result<(), String> {
        let pads: Vec<Pin> = [self.detect, self.ready]
            .into_iter()
            .chain(self.sense)
            .chain(self.supplies)
            .collect();
        for (i, pad) in pads.iter().enumerate() {
            pad.validate()?;
            if pads[..i].contains(pad) {
                return Err("X-mount connector requires distinct pads".into());
            }
        }
        if self.ready_mux.is_some_and(|m| m > 3)
            || self.ready_low_line == self.ready_high_line
            || !(32..1020).contains(&self.ready_low_line)
            || !(32..1020).contains(&self.ready_high_line)
        {
            return Err("invalid X-mount ready interrupt wiring".into());
        }
        Ok(())
    }
}

/// The lens selected for this run (`XT5_LENS` before the manifest default).
pub fn lens_choice(options: &Options) -> String {
    std::env::var(LENS_ENV)
        .ok()
        .map(|v| v.trim().to_ascii_lowercase())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| options.lens.clone())
}

/// Build the selected lens, or `None` when no lens is attached.
pub fn build_lens(options: &Options, choice: &str, ticks_per_second: u64) -> Result<Option<Port>, String> {
    options.validate()?;
    if choice == "none" {
        return Ok(None);
    }
    let mut spec = Spec::preset(choice)
        .ok_or_else(|| format!("{LENS_ENV}={choice}: expected none, xf18-55 or prime"))?;
    spec.replies = options.replies.clone();
    let (startup_us, busy_us) = match std::env::var(READY_ENV) {
        Ok(value) => {
            let parse = |s: &str| s.trim().parse::<u64>().map_err(|_| format!("{READY_ENV} needs STARTUP_US,BUSY_US"));
            let (a, b) = value
                .split_once(',')
                .ok_or_else(|| format!("{READY_ENV} needs STARTUP_US,BUSY_US"))?;
            (parse(a)?, parse(b)?)
        }
        Err(_) => (options.startup_us, options.busy_us),
    };
    let ticks = |us: u64| -> Result<u64, String> {
        u64::try_from(us as u128 * ticks_per_second as u128 / 1_000_000)
            .map_err(|_| "X-mount lens timing overflow".to_string())
    };
    if ticks_per_second < 1000 {
        return Err("X-mount lens needs at least 1000 ticks per second".into());
    }
    Ok(Some(Port::new(
        spec,
        Timing {
            ticks_per_ms: ticks_per_second / 1000,
            startup_ticks: ticks(startup_us)?,
            busy_ticks: ticks(busy_us)?,
            min_operation_ticks: ticks(options.operation_us)?,
        },
    )?))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    options: Options,
    choice: String,
    /// Ready-low and ready-high interrupt line levels.
    asserted: [bool; 2],
}

/// The connector owns the lens-bus controller's window and drives the pads.
pub struct Connector {
    options: Options,
    choice: String,
    lens: Option<Port>,
    pins: SharedPins,
    transport: Box<dyn Device>,
    asserted: [bool; 2],
}
struct Outputs {
    detect: bool,
    sense: Option<[bool; 3]>,
    ready: bool,
    lines: [bool; 2],
}
impl Connector {
    /// `transport` is the serial controller whose wire is `lens` (or an idle line).
    pub fn new(
        options: Options,
        choice: String,
        lens: Option<Port>,
        pins: SharedPins,
        transport: Box<dyn Device>,
    ) -> Result<Self, String> {
        options.validate()?;
        let connector = Self { options, choice, lens, pins, transport, asserted: [false; 2] };
        // Attached pads hold their levels from construction on, before any guest read.
        if let Some(outputs) = connector.outputs() {
            connector.apply_pads(&outputs);
        }
        Ok(connector)
    }
    pub fn lens(&self) -> Option<&Port> {
        self.lens.as_ref()
    }
    fn supplies_on(&self) -> bool {
        let pins = self.pins.lock().unwrap();
        self.options
            .supplies
            .iter()
            .all(|pad| pins.output_level(pad.port.into(), pad.pin.into()) == 1)
    }
    fn ready_routed(&self) -> bool {
        let Some(function) = self.options.ready_mux else { return true };
        let pins = self.pins.lock().unwrap();
        let pad = self.options.ready;
        let register = pins.ports[usize::from(pad.port)].registers[3 + usize::from(pad.pin / 8)];
        (register >> ((pad.pin % 8) * 2)) & 3 == function
    }
    fn outputs(&self) -> Option<Outputs> {
        let lens = self.lens.as_ref()?;
        let ready = lens.ready_high();
        let routed = self.ready_routed();
        Some(Outputs {
            detect: false,
            sense: (!lens.powered()).then_some(self.options.sense_unpowered),
            ready,
            lines: [routed && !ready, routed && ready],
        })
    }
    fn pads_differ(&self, outputs: &Outputs) -> bool {
        let pins = self.pins.lock().unwrap();
        let input = |pad: Pin| pins.ports[usize::from(pad.port)].input & (1 << pad.pin) != 0;
        input(self.options.detect) != outputs.detect
            || input(self.options.ready) != outputs.ready
            || outputs.sense.is_some_and(|levels| {
                self.options.sense.iter().zip(levels).any(|(pad, level)| input(*pad) != level)
            })
    }
    fn apply_pads(&self, outputs: &Outputs) {
        let mut pins = self.pins.lock().unwrap();
        let mut set = |pad: Pin, high: bool| pins.set_input(pad.port.into(), pad.pin.into(), high);
        set(self.options.detect, outputs.detect);
        set(self.options.ready, outputs.ready);
        if let Some(levels) = outputs.sense {
            for (pad, level) in self.options.sense.iter().zip(levels) {
                set(*pad, level);
            }
        }
    }
    fn connector_deadline(&self, now: u64) -> Option<u64> {
        let lens = self.lens.as_ref()?;
        let outputs = self.outputs()?;
        if self.pads_differ(&outputs) || outputs.lines != self.asserted {
            return Some(now);
        }
        lens.next_event_tick(now, self.supplies_on())
    }
    fn update(&mut self, cx: &mut Cx<'_>) {
        let Some(lens) = self.lens.clone() else { return };
        lens.tick(cx.now(), self.supplies_on());
        let Some(outputs) = self.outputs() else { return };
        if self.pads_differ(&outputs) {
            self.apply_pads(&outputs);
        }
        let lines = [self.options.ready_low_line, self.options.ready_high_line];
        for (i, line) in lines.into_iter().enumerate() {
            if outputs.lines[i] != self.asserted[i] {
                cx.set_irq(line, outputs.lines[i]);
                self.asserted[i] = outputs.lines[i];
            }
        }
        self.drain_logs(cx);
    }
    fn drain_logs(&self, cx: &mut Cx<'_>) {
        if let Some(lens) = &self.lens {
            for line in lens.take_logs() {
                cx.log(&line);
            }
        }
    }
    fn saved(&self) -> Saved {
        Saved { options: self.options.clone(), choice: self.choice.clone(), asserted: self.asserted }
    }
}
impl Device for Connector {
    fn name(&self) -> &'static str {
        "xp5-x-mount-lens"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        self.update(cx);
        let value = self.transport.read(cx, address, size);
        self.drain_logs(cx);
        value
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        self.update(cx);
        self.transport.write(cx, address, size, value);
        self.drain_logs(cx);
    }
    fn pollable(&self, address: u64, size: u8) -> bool {
        self.transport.pollable(address, size)
    }
    fn next_deadline(&self, now: u64) -> Option<u64> {
        [self.connector_deadline(now), self.transport.next_deadline(now)]
            .into_iter()
            .flatten()
            .min()
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        let now = cx.now();
        let mut cursor = now;
        while let Some(at) = self.next_deadline(cursor).filter(|at| *at <= now) {
            cursor = at;
            cx.at_tick(at, |cx| {
                self.update(cx);
                self.transport.advance(cx);
                self.drain_logs(cx);
            });
            if cx.refusal().is_some() {
                return;
            }
        }
    }
    fn checkpoint(&self) -> Result<Checkpoint, String> {
        Ok(Checkpoint::encode(self.name(), &self.saved())?
            .with_children(vec![self.transport.checkpoint()?]))
    }
    fn validate_checkpoint(&self, saved: &Checkpoint) -> Result<(), String> {
        let candidate: Saved = saved.decode(self.name())?;
        if candidate.options != self.options
            || candidate.choice != self.choice
            || saved.children().len() != 1
            || !saved.blobs().is_empty()
        {
            return Err(format!(
                "X-mount connector checkpoint differs (saved lens {}, this run {})",
                candidate.choice, self.choice
            ));
        }
        self.transport.validate_checkpoint(&saved.children()[0])
    }
    fn restore_checkpoint(&mut self, saved: &Checkpoint) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        self.transport.restore_checkpoint(&saved.children()[0])?;
        self.asserted = saved.decode::<Saved>(self.name())?.asserted;
        Ok(())
    }
    fn save(&self) -> Value {
        json!({"connector": self.saved(), "transport": self.transport.save()})
    }
    fn diagnostic_state(&self) -> Value {
        let lens = self.lens.as_ref().map(|lens| {
            json!({
                "name": lens.name(),
                "powered": lens.powered(),
                "serving": lens.serving(),
                "ready_high": lens.ready_high(),
                "identification_reads": lens.identification_reads(),
            })
        });
        json!({"connector": self.saved(), "lens": lens, "transport": self.transport.save()})
    }
    fn fingerprint(&self) -> Result<Value, String> {
        Ok(self.save())
    }
}

#[cfg(test)]
mod tests;
