//! Shared-runtime connection for the two register-level I2C controllers.
//! Time, attached wire inputs and actual IRQ register outputs are the only
//! capabilities. Discovery continuation is selected by the runtime bus.
use super::{Channel, Registers, RouteBank, Routes, options::Options};
use fujimod_emulator_devices::{Action, Cx, Device};
use std::collections::BTreeMap;

pub struct Wiring {
    pub controllers: Vec<(usize, fujimod_emulator_devices::exr2::irq::Inputs)>,
    pub cable: Option<fujimod_emulator_devices::signals::Level>,
    pub bridge: Option<fujimod_emulator_devices::parts::lens_bridge::Registers>,
}
pub struct I2cController {
    engine: Registers,
    controllers: [fujimod_emulator_devices::exr2::irq::Inputs; 2],
    cable: Option<fujimod_emulator_devices::signals::Level>,
    bridge: Option<fujimod_emulator_devices::parts::lens_bridge::Registers>,
    due: BTreeMap<u64, u64>,
    cable_expected: Option<bool>,
}
impl I2cController {
    pub fn touch_period(&self) -> Result<u64, String> {
        self.engine
            .parts
            .touch
            .as_ref()
            .map(|p| p.controller.period())
            .ok_or("bus has no attached touch input".into())
    }
    pub fn admit_touch(
        &self,
        now: u64,
        script: &[fujimod_emulator_devices::parts::touch_input::ContactInput],
    ) -> Result<(), String> {
        let mut panel = self
            .engine
            .parts
            .touch
            .clone()
            .ok_or("bus has no attached touch input")?;
        panel.inject(now, script)
    }
    pub fn touch(
        &mut self,
        now: u64,
        script: &[fujimod_emulator_devices::parts::touch_input::ContactInput],
    ) -> Result<(), String> {
        self.engine
            .parts
            .touch
            .as_mut()
            .ok_or("bus has no attached touch input")?
            .inject(now, script)
    }
    fn touch_ready(&self) -> bool {
        self.engine.parts.touch.as_ref().is_some_and(|panel| {
            self.controllers[panel.options.core]
                .source(panel.options.irq.into())
                .is_ok_and(|s| s.enabled)
        })
    }
    fn touch_reads(&self) -> u64 {
        self.engine
            .parts
            .touch
            .as_ref()
            .and_then(|p| {
                self.engine
                    .parts
                    .prefixed_registers
                    .iter()
                    .find(|r| r.0 == p.options.register)
            })
            .map_or(0, |r| r.2)
    }
    fn settle_touch(&mut self, cx: &mut Cx<'_>) -> Result<(), String> {
        let (ready, reads) = (self.touch_ready(), self.touch_reads());
        if let Some(panel) = &mut self.engine.parts.touch {
            if let Some(frame) = panel.controller.tick(cx.now(), reads, ready)? {
                let row = self
                    .engine
                    .parts
                    .prefixed_registers
                    .iter_mut()
                    .find(|r| r.0 == panel.options.register)
                    .ok_or("touch report register is absent")?;
                row.1 = frame.to_vec();
                cx.raise_irq(panel.options.core, panel.options.irq.into())?;
            }
        }
        Ok(())
    }
    pub fn new(options: Options, wiring: Wiring) -> Result<Self, String> {
        let cable_expected = match &options.parts {
            super::options::Parts::Secondary(options) => options.connected,
            _ => None,
        };
        let engine = options.build()?;
        let mut controllers: [Option<fujimod_emulator_devices::exr2::irq::Inputs>; 2] = Default::default();
        for (core, input) in wiring.controllers {
            if core >= 2 || controllers[core].replace(input).is_some() {
                return Err("invalid I2C interrupt-controller connection".into());
            }
        }
        let [Some(first), Some(second)] = controllers else {
            return Err("I2C requires both physical interrupt controllers".into());
        };
        if (engine.channel == Channel::Secondary) != wiring.cable.is_some()
            || wiring.bridge.is_some()
                && (engine.channel != Channel::Secondary
                    || engine.secondary.register_address.is_none())
        {
            return Err(
                "I2C cable or lens-bridge connection does not match its attached parts".into(),
            );
        }
        if wiring
            .cable
            .as_ref()
            .is_some_and(|c| cable_expected.is_some_and(|expected| c.high() != expected))
        {
            return Err(
                "I2C reset cable input differs from its declared attached-part input".into(),
            );
        }
        if let Some(bridge) = &wiring.bridge {
            for register in 0..4 {
                bridge.write_register(
                    register as u8,
                    engine.parts.target_registers.get_or_zero(register) as u8,
                )?;
            }
        }
        Ok(Self {
            engine,
            controllers: [first, second],
            cable: wiring.cable,
            bridge: wiring.bridge,
            due: BTreeMap::new(),
            cable_expected,
        })
    }
    fn routes(&self) -> Routes {
        Routes {
            banks: std::array::from_fn(|core| {
                let source = self.controllers[core]
                    .source(self.engine.irq.into())
                    .expect("validated I2C IRQ");
                RouteBank {
                    enabled: source.enabled,
                    configured: true,
                    owner_valid: source.configuration60.is_some()
                        && source.mode140.is_some()
                        && source.mode180.is_some()
                        && source.vector.is_some_and(|v| v != 0 && v & 3 == 0)
                        && source.priority.is_some_and(|p| p <= 15),
                    binding: [
                        source.configuration60.map(i64::from),
                        Some(i64::from(source.source_config)),
                        source.mode140.map(i64::from),
                        source.mode180.map(i64::from),
                        source.vector.map(i64::from),
                        source.priority.map(i64::from),
                    ],
                }
            }),
        }
    }
    fn sample_cable(&mut self) {
        if self.cable_expected.is_none() && self.cable.is_some() {
            self.engine.parts.secondary_usb_connected = self.connected();
        }
    }
    fn connected(&self) -> bool {
        self.cable.as_ref().is_some_and(fujimod_emulator_devices::signals::Level::high)
    }
    fn read_engine(
        engine: &mut Registers,
        address: u64,
        size: u8,
        connected: bool,
    ) -> fujimod_emulator_devices::record::Result<u32> {
        if engine.channel == Channel::Secondary && address == u64::from(engine.base) + 4 {
            engine.secondary_input(connected)?;
        }
        engine.read(address, size.into())
    }
    fn write_engine(
        engine: &mut Registers,
        address: u64,
        size: u8,
        value: u64,
        connected: bool,
        routes: &Routes,
        actions: &mut Vec<Action>,
    ) -> fujimod_emulator_devices::record::Result<()> {
        match engine.channel {
            Channel::Primary => engine.write_primary(address, size.into(), value, routes, actions),
            Channel::Secondary => {
                engine.write_secondary(address, size.into(), value, connected, routes, actions)
            }
        }
    }
    fn actions(&mut self, cx: &mut Cx<'_>, actions: Vec<Action>) -> Result<(), String> {
        for action in actions {
            match action {
                Action::Log(record) | Action::Record(record) => cx.record(record),
                Action::QueueIrq(core, line) => cx.raise_irq(core.into(), line.into())?,
                Action::Schedule { delay, token } => {
                    let at = cx
                        .now()
                        .checked_add(delay)
                        .ok_or("I2C completion tick overflow")?;
                    if delay == 0 || self.due.insert(token, at).is_some() {
                        return Err("invalid I2C completion token".into());
                    }
                }
            }
        }
        Ok(())
    }
    fn settle(&mut self, cx: &mut Cx<'_>) -> Result<(), String> {
        let mut ready = self
            .due
            .iter()
            .filter(|(_, at)| **at <= cx.now())
            .map(|(&token, &at)| (at, token))
            .collect::<Vec<_>>();
        ready.sort_unstable();
        for (_, token) in ready {
            self.sample_cable();
            let routes = self.routes();
            let mut actions = Vec::new();
            let post = self
                .engine
                .complete_native(token, &routes, Some(self.connected()), &mut actions)
                .map_err(|e| e.0)?;
            self.due.remove(&token);
            self.actions(cx, actions)?;
            if post {
                let routes = self.routes();
                let mut actions = Vec::new();
                self.engine
                    .irq_completed(token, &routes, &mut actions)
                    .map_err(|e| e.0)?;
                self.actions(cx, actions)?;
            }
            // The declared bridge consumes register bytes written through its
            // hardware endpoint, after STOP commits them. No firmware pointer
            // or lens task structure participates in this connection.
            if let Some(bridge) = &self.bridge {
                for register in 0..4 {
                    bridge.write_register(
                        register as u8,
                        self.engine.parts.target_registers.get_or_zero(register) as u8,
                    )?;
                }
            }
        }
        self.settle_touch(cx)
    }
}
impl Device for I2cController {
    fn touch_panel(&self) -> Option<&(dyn fujimod_emulator_devices::capability::TouchPanel + 'static)> {
        Some(self)
    }
    fn touch_panel_mut(&mut self) -> Option<&mut (dyn fujimod_emulator_devices::capability::TouchPanel + 'static)> {
        Some(self)
    }
    fn checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        let bridge = self
            .bridge
            .as_ref()
            .map(|v| {
                (0..4)
                    .map(|i| v.read_register(i))
                    .collect::<Result<Vec<_>, _>>()
            })
            .transpose()?;
        fujimod_emulator_devices::checkpoint::Checkpoint::encode(
            self.name(),
            &(
                &self.engine,
                &self.due,
                self.cable.as_ref().map(fujimod_emulator_devices::signals::Level::high),
                bridge,
            ),
        )
    }
    fn validate_checkpoint(&self, saved: &fujimod_emulator_devices::checkpoint::Checkpoint) -> Result<(), String> {
        let (_, _, cable, bridge): (
            Registers,
            BTreeMap<u64, u64>,
            Option<bool>,
            Option<Vec<u8>>,
        ) = saved.decode(self.name())?;
        if cable.is_some() != self.cable.is_some()
            || bridge.as_ref().map(Vec::len) != self.bridge.as_ref().map(|_| 4)
        {
            return Err("I2C snapshot wiring differs".into());
        }
        Ok(())
    }
    fn restore_checkpoint(&mut self, saved: &fujimod_emulator_devices::checkpoint::Checkpoint) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        let (engine, due, cable, bridge): (
            Registers,
            BTreeMap<u64, u64>,
            Option<bool>,
            Option<Vec<u8>>,
        ) = saved.decode(self.name())?;
        self.engine = engine;
        self.due = due;
        if let (Some(wire), Some(high)) = (&self.cable, cable) {
            wire.drive(high);
        }
        if let (Some(wire), Some(values)) = (&self.bridge, bridge) {
            for (i, value) in values.into_iter().enumerate() {
                wire.write_register(i as u8, value)?;
            }
        }
        Ok(())
    }
    fn fingerprint(&self) -> Result<serde_json::Value, String> {
        let bridge = self
            .bridge
            .as_ref()
            .map(|registers| {
                (0..4)
                    .map(|r| registers.read_register(r))
                    .collect::<Result<Vec<_>, _>>()
            })
            .transpose()?;
        let mut state = self.save();
        // Equal present levels do not imply equal future cable admission.
        state["cable_expected"] = serde_json::json!(self.cable_expected);
        state["bridge"] = serde_json::json!(bridge);
        Ok(state)
    }

    fn name(&self) -> &'static str {
        "i2c-controller"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        self.sample_cable();
        let connected = self.connected();
        match Self::read_engine(&mut self.engine, address, size, connected) {
            Ok(word) => word.into(),
            Err(error) => {
                cx.unsupported(error.0);
                0
            }
        }
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        self.sample_cable();
        let routes = self.routes();
        let connected = self.connected();
        let mut actions = Vec::new();
        match Self::write_engine(
            &mut self.engine,
            address,
            size,
            value,
            connected,
            &routes,
            &mut actions,
        ) {
            Ok(()) => {
                if let Err(error) = self.actions(cx, actions) {
                    cx.fail(error);
                }
            }
            Err(error) => cx.unsupported(error.0),
        }
    }
    fn access_contract(&self, address: u64, size: u8, value: Option<u64>) -> Option<String> {
        let mut engine = self.engine.clone();
        if self.cable_expected.is_none() && self.cable.is_some() {
            engine.parts.secondary_usb_connected = self.connected();
        }
        let result = match value {
            Some(value) => Self::write_engine(
                &mut engine,
                address,
                size,
                value,
                self.connected(),
                &self.routes(),
                &mut Vec::new(),
            ),
            None => Self::read_engine(&mut engine, address, size, self.connected()).map(|_| ()),
        };
        result.err().map(|e| e.0)
    }
    fn discovery_access(
        &mut self,
        cx: &mut Cx<'_>,
        address: u64,
        size: u8,
        value: Option<u64>,
    ) -> Option<u64> {
        self.engine.offset(address, size.into()).ok()?;
        let mut actions = Vec::new();
        let routes = self.routes();
        match self.engine.transport_access(
            value.is_some(),
            address,
            size.into(),
            value.unwrap_or(0),
            &routes,
            &mut actions,
        ) {
            Ok(()) => match self.actions(cx, actions) {
                Ok(()) => Some(0),
                Err(error) => {
                    cx.fail(error);
                    None
                }
            },
            Err(error) => {
                cx.unsupported(error.0);
                None
            }
        }
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        if let Err(error) = self.settle(cx) {
            cx.fail(error);
        }
    }
    fn next_deadline(&self, now: u64) -> Option<u64> {
        let touch = self.engine.parts.touch.as_ref().and_then(|p| {
            p.controller
                .next_event_tick(now, self.touch_reads(), self.touch_ready())
        });
        self.due.values().copied().chain(touch).min()
    }
    fn pollable(&self, address: u64, size: u8) -> bool {
        self.engine
            .offset(address, size.into())
            .is_ok_and(|offset| offset != 4)
    }
    fn save(&self) -> serde_json::Value {
        serde_json::json!({"engine": self.engine, "due": self.due,
            "cable": self.cable.as_ref().map(fujimod_emulator_devices::signals::Level::high)})
    }
}

impl fujimod_emulator_devices::capability::TouchPanel for I2cController {
    fn touch_period(&self) -> std::result::Result<u64, String> {
        I2cController::touch_period(self)
    }
    fn admit_touch(&self, now: u64, script: &[fujimod_emulator_devices::parts::touch_input::ContactInput]) -> std::result::Result<(), String> {
        I2cController::admit_touch(self, now, script)
    }
    fn touch(&mut self, now: u64, script: &[fujimod_emulator_devices::parts::touch_input::ContactInput]) -> std::result::Result<(), String> {
        I2cController::touch(self, now, script)
    }
}
