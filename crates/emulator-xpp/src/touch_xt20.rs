//! X-T20 touch input on the X-Processor Pro I2C bus.
//!
//! Scripted contacts update the controller report and assert its wired
//! input-ready interrupt. Firmware must read the report over I2C to consume it.

use crate::i2c::Parts;
use fujimod_emulator_devices::capability::TouchPanel;
use fujimod_emulator_devices::checkpoint::Checkpoint;
use fujimod_emulator_devices::parts::i2c::{Bus, RegisterInput};
use fujimod_emulator_devices::parts::touch_input::{ContactInput, Phase};
use fujimod_emulator_devices::{Cx, Device};
use fujimod_emulator_ip::dw_i2c::{self, Options as I2cOptions};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};

const REPORT_BYTES: usize = 10;
const PAGE: u8 = 0x3f;
const REPORT_LIMIT: usize = 4096;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub address: u16,
    pub report_register: u8,
    pub input_ready_irq: u32,
    pub ticks_per_second: u64,
}

type SharedPanel = Arc<Mutex<PanelState>>;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PanelState {
    inputs: VecDeque<ContactInput>,
    planned_down: bool,
    contact_down: bool,
    x: u16,
    y: u16,
    reports: VecDeque<[u8; REPORT_BYTES]>,
    report_offset: usize,
    report_ack_pending: bool,
    irq_high: bool,
}

impl PanelState {
    fn validate_script(&self, now: u64, script: &[ContactInput]) -> Result<bool, String> {
        if self.inputs.len() + script.len() > REPORT_LIMIT {
            return Err("X-T20 touch input queue is full".into());
        }
        let mut down = self.planned_down;
        let mut last = self.inputs.back().map_or(now, |input| input.tick.max(now));
        for input in script {
            if input.tick < last || input.tick < now || input.tick.checked_add(1).is_none()
                || input.contact != 0 || input.x > 4095 || input.y > 4095
            {
                return Err("X-T20 touch input requires ordered contact-0 coordinates".into());
            }
            match input.phase {
                Phase::Down if down => {
                    return Err("X-T20 touch contact is already down".into());
                }
                Phase::Move | Phase::Up if !down => {
                    return Err("X-T20 touch move or lift has no contact".into());
                }
                Phase::Down => down = true,
                Phase::Up => down = false,
                Phase::Move => {}
            }
            last = input.tick;
        }
        Ok(down)
    }

    fn apply(&mut self, input: ContactInput) -> Result<[u8; REPORT_BYTES], String> {
        match input.phase {
            Phase::Down if !self.contact_down => self.contact_down = true,
            Phase::Move | Phase::Up if self.contact_down => {}
            Phase::Down => return Err("X-T20 touch contact began twice".into()),
            Phase::Move | Phase::Up => return Err("X-T20 touch contact ended before input".into()),
        }
        self.x = input.x;
        self.y = input.y;
        let mut report = [0; REPORT_BYTES];
        if input.phase != Phase::Up {
            // One active contact in slot 1, with two packed 12-bit coordinates.
            report[0] = 0x09;
            report[1] = (self.x >> 4) as u8;
            report[2] = (self.y >> 4) as u8;
            report[3] = ((self.x as u8 & 0x0f) << 4) | (self.y as u8 & 0x0f);
        } else {
            self.contact_down = false;
        }
        Ok(report)
    }

    fn read_report_byte(&mut self) -> Result<u8, String> {
        let report = self
            .reports
            .front()
            .ok_or_else(|| "X-T20 touch report read without an input-ready report".to_string())?;
        if self.report_ack_pending || self.report_offset >= REPORT_BYTES {
            return Err("X-T20 touch report was read past its declared length".into());
        }
        let byte = report[self.report_offset];
        self.report_offset += 1;
        self.report_ack_pending = self.report_offset == REPORT_BYTES;
        Ok(byte)
    }

    fn check(&self) -> Result<(), String> {
        if self.inputs.len() > REPORT_LIMIT
            || self.reports.len() > REPORT_LIMIT
            || self.report_offset > REPORT_BYTES
            || self.report_ack_pending != (self.report_offset == REPORT_BYTES)
            || self.reports.is_empty() && (self.report_offset != 0 || self.report_ack_pending)
            || self.irq_high != !self.reports.is_empty()
        {
            return Err("X-T20 touch checkpoint state is inconsistent".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Selection {
    active_touch: bool,
    page: u8,
    register: Option<u8>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedBus {
    selection: Selection,
    panel: PanelState,
    registers: BTreeMap<u16, u8>,
    parts: Value,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BusSnapshot {
    options: Options,
    selection: Selection,
    panel: PanelState,
    registers: BTreeMap<u16, u8>,
}

struct TouchBus {
    parts: Parts,
    options: Options,
    selection: Selection,
    registers: BTreeMap<u16, u8>,
    panel: SharedPanel,
}

impl TouchBus {
    fn new(parts: Vec<crate::i2c::Part>, options: Options, panel: SharedPanel) -> Result<Self, String> {
        if options.address > 0x7f
            || options.input_ready_irq >= 1020
            || options.ticks_per_second < 100
            || !options.ticks_per_second.is_multiple_of(100)
        {
            return Err("Invalid X-T20 touch address, interrupt or clock".into());
        }
        if parts.iter().any(|part| part.address == options.address) {
            return Err("X-T20 touch address is also declared as a register-file part".into());
        }
        Ok(Self {
            parts: Parts::new(parts)?,
            options,
            selection: Selection::default(),
            registers: BTreeMap::new(),
            panel,
        })
    }

    fn panel_state(&self) -> Result<std::sync::MutexGuard<'_, PanelState>, String> {
        self.panel.lock().map_err(|_| "X-T20 touch state is poisoned".into())
    }

    fn encode_state(&self) -> Result<SavedBus, String> {
        Ok(SavedBus {
            selection: self.selection.clone(),
            panel: self.panel_state()?.clone(),
            registers: self.registers.clone(),
            parts: self.parts.save(),
        })
    }

    fn restore_state(&mut self, saved: SavedBus) -> Result<(), String> {
        saved.panel.check()?;
        self.parts.restore(&saved.parts)?;
        *self.panel_state()? = saved.panel;
        self.selection = saved.selection;
        self.registers = saved.registers;
        Ok(())
    }
}

impl Bus for TouchBus {
    fn start(&mut self, address: u16, read: bool) -> bool {
        if address == self.options.address {
            self.selection.active_touch = true;
            if !read {
                self.selection.register = None;
            }
            true
        } else {
            self.selection.active_touch = false;
            self.parts.start(address, read)
        }
    }

    fn write(&mut self, byte: u8) -> Result<(), String> {
        if !self.selection.active_touch {
            return self.parts.write(byte);
        }
        let Some(register) = self.selection.register else {
            self.selection.register = Some(byte);
            return Ok(());
        };
        if register == 0xff {
            self.selection.page = byte;
        } else {
            let key = u16::from(self.selection.page) << 8 | u16::from(register);
            self.registers.insert(key, byte);
        }
        self.selection.register = Some(register.wrapping_add(1));
        Ok(())
    }

    fn read(&mut self) -> Result<u8, String> {
        if !self.selection.active_touch {
            return self.parts.read();
        }
        let register = self
            .selection
            .register
            .ok_or_else(|| "X-T20 touch read has no selected register".to_string())?;
        let report_read = self.selection.page == PAGE
            && (register == self.options.report_register
                || self.panel_state()?.report_offset > 0);
        let byte = if report_read {
            self.panel_state()?.read_report_byte()?
        } else {
            let key = u16::from(self.selection.page) << 8 | u16::from(register);
            *self
                .registers
                .get(&key)
                .ok_or_else(|| format!("X-T20 touch register {key:#06x} is not declared"))?
        };
        self.selection.register = Some(register.wrapping_add(1));
        Ok(byte)
    }

    fn stop(&mut self) {
        if !self.selection.active_touch {
            self.parts.stop();
        }
        self.selection.active_touch = false;
    }

    fn save(&self) -> Value {
        self.encode_state()
            .and_then(|state| serde_json::to_value(state).map_err(|e| e.to_string()))
            .unwrap_or(Value::Null)
    }

    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let saved: SavedBus = serde_json::from_value(value.clone())
            .map_err(|e| format!("invalid X-T20 touch I2C state: {e}"))?;
        self.restore_state(saved)
    }

    fn checkpoint(&self) -> Result<Checkpoint, String> {
        let panel = self.panel_state()?.clone();
        panel.check()?;
        let state = BusSnapshot {
            options: self.options.clone(),
            selection: self.selection.clone(),
            panel,
            registers: self.registers.clone(),
        };
        Ok(Checkpoint::encode("xpp-xt20-touch-i2c-bus", &state)?
            .with_children(vec![self.parts.checkpoint()?]))
    }

    fn validate_checkpoint(&self, saved: &Checkpoint) -> Result<(), String> {
        if saved.children().len() != 1 || !saved.blobs().is_empty() {
            return Err("X-T20 touch I2C checkpoint parts differ".into());
        }
        let state: BusSnapshot = saved.decode("xpp-xt20-touch-i2c-bus")?;
        if state.options != self.options {
            return Err("X-T20 touch I2C checkpoint wiring differs".into());
        }
        state.panel.check()?;
        self.parts.validate_checkpoint(&saved.children()[0])
    }

    fn restore_checkpoint(&mut self, saved: &Checkpoint) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        let state: BusSnapshot = saved.decode("xpp-xt20-touch-i2c-bus")?;
        self.parts.restore_checkpoint(&saved.children()[0])?;
        *self.panel_state()? = state.panel;
        self.selection = state.selection;
        self.registers = state.registers;
        Ok(())
    }
}

impl RegisterInput for TouchBus {
    fn set_register_input(&mut self, address: u16, register: u8, value: u16) -> Result<(), String> {
        self.parts.set_register_input(address, register, value)
    }
}

pub struct Xt20TouchI2c {
    controller: dw_i2c::Controller<TouchBus>,
    options: Options,
    panel: SharedPanel,
}

impl Xt20TouchI2c {
    pub fn new(
        i2c_options: I2cOptions,
        parts: Vec<crate::i2c::Part>,
        options: Options,
    ) -> Result<Self, String> {
        let panel = Arc::new(Mutex::new(PanelState::default()));
        let bus = TouchBus::new(parts, options.clone(), panel.clone())?;
        let controller = dw_i2c::Controller::new(i2c_options, bus)?;
        Ok(Self { controller, options, panel })
    }

    /// Set a declared register value supplied by a non-touch part on this
    /// bus. Touch reports and register accesses keep their modeled behavior.
    pub fn set_register_input(
        &mut self,
        address: u16,
        register: u8,
        value: u16,
    ) -> Result<(), String> {
        self.controller.set_register_input(address, register, value)
    }

    fn panel_state(&self) -> Result<std::sync::MutexGuard<'_, PanelState>, String> {
        self.panel.lock().map_err(|_| "X-T20 touch state is poisoned".into())
    }

    fn advance_panel(&mut self, cx: &mut Cx<'_>, i2c_idle: bool) {
        let now = cx.now();
        let Ok(mut panel) = self.panel.lock() else {
            cx.fail("X-T20 touch state is poisoned");
            return;
        };
        while panel.inputs.front().is_some_and(|input| input.tick <= now) {
            let input = panel.inputs.pop_front().unwrap();
            let report = match panel.apply(input) {
                Ok(report) => report,
                Err(error) => {
                    cx.fail(error);
                    return;
                }
            };
            if panel.reports.len() >= REPORT_LIMIT {
                cx.fail("X-T20 touch report queue is full");
                return;
            }
            panel.reports.push_back(report);
            if !panel.irq_high {
                panel.irq_high = true;
                cx.set_irq(self.options.input_ready_irq, true);
            }
        }
        if panel.report_ack_pending && i2c_idle {
            panel.reports.pop_front();
            panel.report_offset = 0;
            panel.report_ack_pending = false;
            if panel.irq_high {
                panel.irq_high = false;
                cx.set_irq(self.options.input_ready_irq, false);
            }
            if !panel.reports.is_empty() {
                panel.irq_high = true;
                cx.set_irq(self.options.input_ready_irq, true);
            }
        }
    }
}

impl TouchPanel for Xt20TouchI2c {
    fn touch_period(&self) -> Result<u64, String> {
        if self.options.ticks_per_second < 100
            || !self.options.ticks_per_second.is_multiple_of(100)
        {
            return Err("X-T20 touch clock must be a positive multiple of 100 Hz".into());
        }
        Ok(self.options.ticks_per_second / 100)
    }

    fn admit_touch(&self, now: u64, script: &[ContactInput]) -> Result<(), String> {
        self.panel_state()?.validate_script(now, script).map(|_| ())
    }

    fn touch(&mut self, now: u64, script: &[ContactInput]) -> Result<(), String> {
        let mut panel = self.panel_state()?;
        panel.planned_down = panel.validate_script(now, script)?;
        panel.inputs.extend(script.iter().copied());
        Ok(())
    }
}

impl Device for Xt20TouchI2c {
    fn name(&self) -> &'static str {
        "designware-i2c"
    }

    fn touch_panel(&self) -> Option<&(dyn TouchPanel + 'static)> {
        Some(self)
    }

    fn touch_panel_mut(&mut self) -> Option<&mut (dyn TouchPanel + 'static)> {
        Some(self)
    }

    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        Device::read(&mut self.controller, cx, address, size)
    }

    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        Device::write(&mut self.controller, cx, address, size, value)
    }

    fn advance(&mut self, cx: &mut Cx<'_>) {
        self.controller.advance(cx);
        let idle = self.controller.next_deadline(cx.now()).is_none();
        self.advance_panel(cx, idle);
    }

    fn next_deadline(&self, now: u64) -> Option<u64> {
        let bus = self.controller.next_deadline(now);
        let panel = self.panel.lock().ok().and_then(|state| {
            state
                .inputs
                .front()
                .map(|input| input.tick.max(now))
                .or_else(|| {
                    (state.report_ack_pending && bus.is_none()).then_some(now)
                })
        });
        match (bus, panel) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) | (None, Some(a)) => Some(a),
            (None, None) => None,
        }
    }

    fn save(&self) -> Value {
        self.controller.save()
    }

    fn restore(&mut self, state: &Value) -> Result<(), String> {
        self.controller.restore(state)
    }

    fn checkpoint(&self) -> Result<Checkpoint, String> {
        self.controller.checkpoint()
    }

    fn validate_checkpoint(&self, state: &Checkpoint) -> Result<(), String> {
        self.controller.validate_checkpoint(state)
    }

    fn restore_checkpoint(&mut self, state: &Checkpoint) -> Result<(), String> {
        self.controller.restore_checkpoint(state)
    }

    fn fingerprint(&self) -> Result<Value, String> {
        self.controller.fingerprint()
    }

    fn pollable(&self, address: u64, size: u8) -> bool {
        self.controller.pollable(address, size)
    }
}
