//! Bus0 wire operations and deadline completions. No Python or task-RAM inputs.
use super::{Channel, Phase, Profile, ReceiveCommand, Registers, RxKind};
use fujimod_emulator_devices::record::Result;
use fujimod_emulator_devices::{Action, Record, Value};
use fujimod_emulator_parts_controls::i2c as parts;

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct RouteBank {
    pub enabled: bool,
    pub configured: bool,
    pub owner_valid: bool,
    pub binding: [Option<i64>; 6],
}
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Routes {
    pub banks: [RouteBank; 2],
}
#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
pub enum Op {
    Start,
    AddressWrite,
    AddressRead,
    WriteByte,
    Stop,
    ReceiveByte,
    StopRead,
    StopWrite,
    StopPrefix,
}
impl Op {
    fn name(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::AddressWrite => "address/write",
            Self::AddressRead => "address/read",
            Self::WriteByte => "write-byte",
            Self::Stop => "stop",
            Self::ReceiveByte => "receive-byte",
            Self::StopRead => "stop/read",
            Self::StopWrite => "stop/write",
            Self::StopPrefix => "stop/prefix",
        }
    }
}
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub enum SecondaryFinish {
    None,
    Byte(u8),
    Write { pwic: bool, register: u32, byte: u8 },
    WriteBurst { register: u32, bytes: Vec<u8> },
    Prefix,
    Receive(u8),
    Read(u32),
}
pub struct SecondaryRequest {
    pub op: Op,
    pub status: u32,
    pub phase: Phase,
    pub finish: SecondaryFinish,
    pub target: u8,
}
#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
pub(super) enum WritePart {
    CommandRegisters(usize),
    Status(usize),
    Prefix,
    Sensor,
}
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(super) enum Finish {
    UnboundAddress {
        target: u8,
        read: bool,
    },
    ZeroReceive,
    Secondary(SecondaryFinish),
    None,
    Target(u8),
    Byte(u8),
    FrameAddress {
        target: u8,
        sample: Vec<u8>,
        kind: RxKind,
    },
    HeartbeatAddress {
        target: u8,
        byte: u8,
        metadata: Value,
    },
    PrefixedAddress {
        target: u8,
        register: u16,
        bytes: Vec<u8>,
    },
    SensorAddress(Vec<u8>),
    Receive(usize),
    Write {
        target: u8,
        payload: Vec<u8>,
        part: WritePart,
    },
    Read {
        target: Option<u8>,
        bytes: Vec<u8>,
        kind: RxKind,
        metadata: Value,
    },
}
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(super) struct Pending {
    pub generation: i64,
    pub unbound_reply: bool,
    pub profile: Option<Profile>,
    pub latched: bool,
    pub binding: [[Option<i64>; 6]; 2],
    pub target: u8,
    pub op: Op,
    pub status: u32,
    pub phase: Phase,
    pub finish: Finish,
}
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(super) struct IrqFinish {
    op: Op,
    status: u32,
    target: u8,
    latched: bool,
    binding: [[Option<i64>; 6]; 2],
}
fn binding_value(binding: &[[Option<i64>; 6]; 2]) -> Value {
    Value::Tuple(
        binding
            .iter()
            .map(|row| Value::Tuple(row.iter().map(|value| Value::opt(*value)).collect()))
            .collect(),
    )
}
fn log(name: &'static str, fields: Vec<(&'static str, Value)>) -> Action {
    Action::Log(Record::new(name, fields))
}
fn profile_value(profile: &Option<Profile>) -> Value {
    profile
        .as_ref()
        .map_or(Value::None, |p| Value::str(p.as_str()))
}
impl Registers {
    /// Python's bus1 admission precedes this explicit controller request.
    pub fn start_secondary(&mut self, request: SecondaryRequest, actions: &mut Vec<Action>) {
        self.start();
        self.next_token = self.next_token.wrapping_add(1);
        let token = self.next_token;
        self.pending.insert(
            token,
            Pending {
                generation: self.generation,
                unbound_reply: false,
                profile: self.profile.clone(),
                latched: false,
                binding: [[None; 6]; 2],
                target: request.target,
                op: request.op,
                status: request.status,
                phase: request.phase,
                finish: Finish::Secondary(request.finish),
            },
        );
        actions.push(log(
            "Secondary control-bus operation start",
            vec![
                ("operation", Value::str(request.op.name())),
                ("profile", profile_value(&self.profile)),
                (
                    "transfer_ticks",
                    if self.transfer_ticks_bool {
                        Value::Bool(self.transfer_ticks != 0)
                    } else {
                        Value::int(self.transfer_ticks)
                    },
                ),
                (
                    "provenance",
                    Value::str(
                        "explicit selected virtual target input and unmeasured controller latency",
                    ),
                ),
            ],
        ));
        actions.push(Action::Schedule {
            delay: self.transfer_ticks,
            token,
        });
    }
    pub fn completion_needs_usb(&self, token: u64) -> bool {
        self.channel == Channel::Secondary
            && matches!(
                self.profile,
                Some(
                    Profile::UsbDisconnected
                        | Profile::Pwic61
                        | Profile::PwicStartup
                        | Profile::DeclaredSecondary
                )
            )
            && self
                .pending
                .get(&token)
                .is_some_and(|p| p.generation == self.generation)
    }
    pub(super) fn irq_target(&self, routes: &Routes) -> Result<u8> {
        match (routes.banks[0].enabled, routes.banks[1].enabled) {
            (true, false) => Ok(0),
            (false, true) => Ok(1),
            _ => super::fail(format!(
                "{} physical IRQ{:#x} requires one enabled CPU route",
                if self.channel == Channel::Secondary {
                    "Bus1"
                } else {
                    "Control-bus"
                },
                self.irq
            )),
        }
    }
    fn bindings(&self, routes: &Routes, target: u8) -> Result<[[Option<i64>; 6]; 2]> {
        for (cpu, bank) in routes.banks.iter().enumerate() {
            if !bank.configured {
                return super::fail(
                    "Latched control-bus IRQ requires modeled controller source configuration",
                );
            }
            if cpu == target as usize && !bank.owner_valid {
                return super::fail(
                    "Latched control-bus IRQ owner lacks complete vector/priority/mode programming",
                );
            }
        }
        Ok([routes.banks[0].binding, routes.banks[1].binding])
    }
    /// Bind an explicitly selected completion policy to one started transfer.
    /// Enable masking may change after this call; the owner and programming may not.
    pub fn latch_irq_owner(&mut self, token: u64, routes: &Routes) -> Result<u8> {
        let target = self.irq_target(routes)?;
        let binding = self.bindings(routes, target)?;
        let generation = self.generation;
        let pending = self
            .pending
            .get_mut(&token)
            .ok_or_else(|| fujimod_emulator_devices::Unsupported("Unknown control-bus completion token".into()))?;
        if pending.generation != generation || pending.target != target || !self.busy {
            return super::fail(
                "Cannot latch an inactive or differently routed control-bus transfer",
            );
        }
        pending.latched = true;
        pending.binding = binding;
        Ok(target)
    }
    fn begin_shift(
        &mut self,
        op: Op,
        status: u32,
        phase: Phase,
        finish: Finish,
        routes: &Routes,
        actions: &mut Vec<Action>,
    ) -> Result<()> {
        if matches!(op, Op::Start) {
            self.unmodeled_transaction = false;
        }
        if self.profile.is_none() && self.parts.touch.is_none() {
            return super::fail(
                "Control-bus external ACK behavior requires an explicit input profile",
            );
        }
        if self.busy {
            return super::fail("Control-bus operation while a prior shift is pending");
        }
        let target = self.irq_target(routes)?;
        let latched = matches!(
            self.profile,
            Some(Profile::LatchedHeartbeat | Profile::RegisterStatus)
        );
        let binding = if latched {
            self.bindings(routes, target)?
        } else {
            [[None; 6]; 2]
        };
        self.start();
        self.next_token = self.next_token.wrapping_add(1);
        let token = self.next_token;
        self.pending.insert(
            token,
            Pending {
                generation: self.generation,
                unbound_reply: false,
                profile: self.profile.clone(),
                latched,
                binding,
                target,
                op,
                status,
                phase,
                finish,
            },
        );
        let mut fields = vec![
            ("operation", Value::str(op.name())),
            ("profile", profile_value(&self.profile)),
            (
                "transfer_ticks",
                if self.transfer_ticks_bool {
                    Value::Bool(self.transfer_ticks != 0)
                } else {
                    Value::int(self.transfer_ticks)
                },
            ),
        ];
        if latched {
            fields.extend([
                ("irq_owner", Value::int(target)),
                ("irq_binding", binding_value(&binding)),
            ]);
        }
        fields.push((
            "provenance",
            Value::str("explicit unmeasured virtual target ACK and controller latency"),
        ));
        actions.push(log("Control-bus operation start", fields));
        actions.push(Action::Schedule {
            delay: self.transfer_ticks,
            token,
        });
        Ok(())
    }
    pub fn complete_native(
        &mut self,
        token: u64,
        routes: &Routes,
        usb_connected: Option<bool>,
        actions: &mut Vec<Action>,
    ) -> Result<bool> {
        let Some(pending) = self.pending.remove(&token) else {
            return super::fail("Unknown control-bus completion token");
        };
        if pending.generation != self.generation {
            if pending.latched {
                return super::fail(
                    "Latched control-bus operation generation changed during transfer",
                );
            }
            return Ok(false);
        }
        if self.channel == Channel::Secondary && !pending.unbound_reply {
            match self.profile {
                Some(Profile::DeclaredSecondary) => {
                    if usb_connected != Some(self.parts.secondary_usb_connected) {
                        return super::fail("Declared secondary bus cable input changed");
                    }
                }
                Some(Profile::UsbDisconnected | Profile::Pwic61 | Profile::PwicStartup) => {
                    if usb_connected == Some(true) {
                        return super::fail(
                            "Connected USB charge state is outside the disconnected input profile",
                        );
                    }
                }
                _ => {
                    return super::fail(
                        "Bus1 target response requires explicit disconnected usbcharge input profile",
                    );
                }
            }
        }
        if pending.unbound_reply {
            // Discovery transports use the request-time physical IRQ route,
            // including a temporarily masked source at completion.
        } else if pending.latched {
            if self.profile != pending.profile
                || self.bindings(routes, pending.target)? != pending.binding
            {
                return super::fail(
                    "Latched control-bus IRQ profile/source/vector/priority binding changed",
                );
            }
            if routes
                .banks
                .iter()
                .enumerate()
                .any(|(cpu, bank)| cpu != pending.target as usize && bank.enabled)
            {
                return super::fail("Latched control-bus IRQ gained a competing enabled owner");
            }
        } else if self.irq_target(routes)? != pending.target {
            return super::fail(if self.channel == Channel::Secondary {
                "Bus1 IRQ route changed during transfer"
            } else {
                "Control-bus IRQ route changed during transfer"
            });
        }
        self.busy = false;
        self.regs.insert(0x18, pending.status);
        self.phase = pending.phase;
        self.finish_part(pending.finish)?;
        if matches!(pending.op, Op::AddressRead) {
            actions.push(log(
                "Wire reply begin",
                vec![
                    ("bus_kind", Value::str("i2c")),
                    ("bus", Value::hex(self.base.into())),
                    ("chip", self.parts.target.map_or(Value::None, Value::int)),
                    ("port", Value::hex((self.base + 4).into())),
                    ("unit", Value::int(1)),
                    ("cpu", Value::int(pending.target)),
                    ("tx", Value::Bytes(self.parts.payload.clone())),
                ],
            ));
        }
        self.operations_completed += 1;
        self.post_irq.insert(
            token,
            IrqFinish {
                op: pending.op,
                target: pending.target,
                status: pending.status,
                latched: pending.latched,
                binding: pending.binding,
            },
        );
        actions.push(Action::QueueIrq(pending.target, self.irq));
        Ok(true)
    }
    /// After queued IRQ delivery succeeds, this records the completion. A
    /// refused interrupt preserves the pending state.
    pub fn irq_completed(
        &mut self,
        token: u64,
        routes: &Routes,
        actions: &mut Vec<Action>,
    ) -> Result<()> {
        let Some(done) = self.post_irq.remove(&token) else {
            return super::fail("Unknown control-bus IRQ completion token");
        };
        self.irqs_raised += 1;
        if done.op.name().starts_with("stop") {
            actions.push(log(
                "Wire transaction",
                vec![
                    ("bus_kind", Value::str("i2c")),
                    ("bus", Value::hex(self.base.into())),
                    ("chip", self.parts.target.map_or(Value::None, Value::int)),
                    ("tx", Value::Bytes(self.parts.payload.clone())),
                    ("rx", Value::Bytes(self.rx_bytes.clone())),
                    ("unsupported", Value::Bool(self.unmodeled_transaction)),
                ],
            ));
            self.unmodeled_transaction = false;
        }
        let mut fields = vec![
            ("operation", Value::str(done.op.name())),
            ("status", Value::hex(done.status.into())),
            ("cpu", Value::int(done.target)),
            ("physical_irq", Value::hex(self.irq.into())),
            ("profile", profile_value(&self.profile)),
        ];
        if done.latched {
            fields.extend([
                (
                    "irq_owner_enabled",
                    Value::Bool(routes.banks[done.target as usize].enabled),
                ),
                ("irq_binding", binding_value(&done.binding)),
            ]);
        }
        actions.push(log(
            if self.channel == Channel::Secondary {
                "Secondary control-bus operation complete"
            } else {
                "Control-bus operation complete"
            },
            fields,
        ));
        Ok(())
    }
    fn finish_part(&mut self, finish: Finish) -> Result<()> {
        let p = &mut self.parts;
        match finish {
            Finish::UnboundAddress { target, read } => {
                p.target = Some(target);
                p.target_present = true;
                if read {
                    self.rx_bytes.clear();
                    self.rx_index = 0;
                    self.rx_ready = false;
                    self.rx_kind = Some(RxKind::Other("discovery-zero".into()));
                }
            }
            Finish::ZeroReceive => {
                self.regs.insert(4, 0);
                self.rx_byte = Some(0);
                self.rx_ready = true;
                self.rx_consumed = false;
            }
            Finish::Secondary(finish) => match finish {
                SecondaryFinish::None => (),
                SecondaryFinish::Byte(byte) => p.payload.push(byte),
                SecondaryFinish::Receive(byte) => {
                    self.rx_byte = Some(byte);
                    self.rx_ready = true;
                    self.regs.insert(4, byte.into());
                }
                SecondaryFinish::Write {
                    pwic,
                    register,
                    byte,
                } => {
                    let registers = if pwic {
                        &mut p.pwic_registers
                    } else {
                        &mut p.target_registers
                    };
                    registers.insert(register, byte.into());
                    p.remember(parts::dict(vec![
                        (
                            "address",
                            Value::hex(
                                p.target
                                    .ok_or_else(|| {
                                        fujimod_emulator_devices::Unsupported("Bus1 completion has no target".into())
                                    })?
                                    .into(),
                            ),
                        ),
                        ("register", Value::hex(register.into())),
                        ("write_hex", Value::Str(parts::hex(&[register as u8, byte]))),
                        ("data_byte", Value::int(byte)),
                    ]));
                }
                SecondaryFinish::WriteBurst { register, bytes } => {
                    for (offset, byte) in bytes.iter().enumerate() {
                        p.target_registers
                            .insert(register + offset as u32, (*byte).into());
                    }
                    let mut payload = vec![register as u8];
                    payload.extend_from_slice(&bytes);
                    p.remember(parts::dict(vec![
                        (
                            "address",
                            Value::hex(
                                p.target
                                    .ok_or_else(|| {
                                        fujimod_emulator_devices::Unsupported("Bus1 completion has no target".into())
                                    })?
                                    .into(),
                            ),
                        ),
                        ("register", Value::hex(register.into())),
                        ("write_hex", Value::Str(parts::hex(&payload))),
                    ]));
                }
                SecondaryFinish::Prefix => {
                    let register = *p.payload.first().ok_or_else(|| {
                        fujimod_emulator_devices::Unsupported("Bus1 completion has no prefix".into())
                    })?;
                    p.pwic_prefix = Some(register.into());
                    p.remember(parts::dict(vec![
                        (
                            "address",
                            Value::hex(
                                p.target
                                    .ok_or_else(|| {
                                        fujimod_emulator_devices::Unsupported("Bus1 completion has no target".into())
                                    })?
                                    .into(),
                            ),
                        ),
                        ("register", Value::hex(register.into())),
                        ("prefix_hex", Value::Str(parts::hex(&p.payload))),
                    ]));
                }
                SecondaryFinish::Read(register) => {
                    p.remember(parts::dict(vec![
                        (
                            "address",
                            Value::hex(
                                p.target
                                    .ok_or_else(|| {
                                        fujimod_emulator_devices::Unsupported("Bus1 completion has no target".into())
                                    })?
                                    .into(),
                            ),
                        ),
                        ("register", Value::hex(register.into())),
                        ("read_hex", Value::Str(parts::hex(&self.rx_bytes))),
                    ]));
                }
            },
            Finish::None => (),
            Finish::Target(target) => {
                p.target = Some(target);
                p.target_present = true;
            }
            Finish::Byte(byte) => p.payload.push(byte),
            Finish::FrameAddress {
                target,
                sample,
                kind,
            } => {
                p.target = Some(target);
                p.target_present = true;
                p.rx_metadata = parts::dict(vec![
                    ("frame_hex", Value::Str(parts::hex(&sample))),
                    (
                        "provenance",
                        Value::str(
                            "explicit external input; no chip identity or autonomous measurement",
                        ),
                    ),
                ]);
                self.rx_bytes = sample;
                self.rx_index = 0;
                self.rx_ready = false;
                self.rx_kind = Some(kind);
            }
            Finish::HeartbeatAddress {
                target,
                byte,
                metadata,
            } => {
                p.target = Some(target);
                p.target_present = true;
                p.endpoint9_prefix = None;
                self.rx_bytes = vec![byte];
                self.rx_index = 0;
                self.rx_ready = false;
                self.rx_kind = Some(RxKind::TouchHeartbeat);
                p.rx_metadata = metadata;
            }
            Finish::SensorAddress(sample) => {
                self.rx_bytes = sample;
                self.rx_index = 0;
                self.rx_ready = false;
                self.rx_kind = Some(RxKind::StationaryXyz);
                p.rx_metadata = p.sensor_sample.clone();
            }
            Finish::PrefixedAddress {
                target,
                register,
                bytes,
            } => {
                p.target = Some(target);
                p.target_present = true;
                p.endpoint9_prefix = None;
                self.rx_bytes = bytes;
                self.rx_index = 0;
                self.rx_ready = false;
                self.rx_kind = Some(RxKind::PrefixedRegister);
                p.rx_metadata = parts::dict(vec![("register", Value::int(register))]);
            }
            Finish::Receive(index) => {
                let Some(byte) = self.rx_bytes.get(index) else {
                    return super::fail("Control-bus receive index outside its input");
                };
                self.regs.insert(4, *byte as u32);
                self.rx_ready = true;
            }
            Finish::Write {
                target,
                payload,
                part,
            } => {
                let mut item = parts::dict(vec![
                    ("address", Value::hex(target.into())),
                    ("write_hex", Value::Str(parts::hex(&payload))),
                ]);
                match part {
                    WritePart::CommandRegisters(index) => {
                        p.command_registers[index].complete_write(&payload)?;
                        parts::add(
                            &mut item,
                            "input_kind",
                            Value::str("declared command-register target"),
                        );
                    }
                    WritePart::Status(index) => {
                        let row = &mut p.status[index];
                        let transition = row.complete_write(&payload)?;
                        parts::add(&mut item, "register", Value::hex(payload[0].into()));
                        parts::add(&mut item, "data_byte", Value::int(payload[1]));
                        parts::add(
                            &mut item,
                            "input_kind",
                            Value::str("declared register/status target"),
                        );
                        if let Some(frame) = transition {
                            parts::add(
                                &mut item,
                                "status_transition_hex",
                                Value::Str(parts::hex(&frame)),
                            );
                            parts::add(
                                &mut item,
                                "basis",
                                Value::str(
                                    "explicit completed wire-command/status rule; no guest task RAM",
                                ),
                            );
                        }
                    }
                    WritePart::Prefix => {
                        let prefix = u16::from_be_bytes([payload[0], payload[1]]);
                        parts::add(&mut item, "register_prefix", Value::hex(prefix.into()));
                        parts::add(
                            &mut item,
                            "data_byte",
                            payload.get(2).map_or(Value::None, |v| Value::int(*v)),
                        );
                        p.endpoint9_prefix = if payload.len() == 2 {
                            Some(prefix)
                        } else {
                            None
                        };
                    }
                    WritePart::Sensor => {
                        parts::add(&mut item, "register", Value::hex(payload[0].into()));
                        parts::add(&mut item, "data_byte", Value::int(payload[1]));
                        if p.stationary
                            || matches!(
                                self.profile,
                                Some(
                                    Profile::Stationary
                                        | Profile::Heartbeat
                                        | Profile::LatchedHeartbeat
                                )
                            )
                        {
                            p.sensor_registers
                                .insert(payload[0].into(), payload[1].into());
                        }
                    }
                }
                p.remember(item);
            }
            Finish::Read {
                target,
                bytes,
                kind,
                metadata,
            } => {
                let Some(target) = target else {
                    return super::fail("Control-bus read stop has no target");
                };
                let mut item = parts::dict(vec![
                    ("address", Value::hex(target.into())),
                    ("read_hex", Value::Str(parts::hex(&bytes))),
                    ("input_kind", Value::str(kind.as_str())),
                ]);
                match kind {
                    RxKind::StationaryXyz => {
                        parts::add(&mut item, "register", Value::str("0x1"));
                        parts::add(&mut item, "sample", metadata);
                    }
                    RxKind::TouchHeartbeat => {
                        let counter = match parts::get(&metadata, "counter_value") {
                            Some(Value::Int(v)) => *v as u32,
                            _ => 0,
                        };
                        parts::add(
                            &mut item,
                            "register",
                            Value::hex(p.heartbeat_register.into()),
                        );
                        parts::add(&mut item, "heartbeat", metadata);
                        p.heartbeat_reads += 1;
                        p.heartbeat_counter = (counter + 1) & 0xff;
                    }
                    RxKind::PrefixedRegister => {
                        let register = match parts::get(&metadata, "register") {
                            Some(Value::Int(value)) => *value as u16,
                            _ => return super::fail("Prefixed read lacks its latched register"),
                        };
                        let Some(slot) = p
                            .prefixed_registers
                            .iter_mut()
                            .find(|row| row.0 == register)
                        else {
                            return super::fail("Prefixed read target disappeared during transfer");
                        };
                        slot.2 += 1;
                        parts::add(&mut item, "register", Value::hex(register.into()));
                        parts::add(&mut item, "completed_read_index", Value::int(slot.2));
                    }
                    _ => parts::add(&mut item, "status_input", metadata),
                }
                p.remember(item);
            }
        }
        Ok(())
    }
    fn stationary_sample(&mut self) -> Result<Vec<u8>> {
        let p = &mut self.parts;
        if p.sensor_registers.len() != p.sensor_fields.len()
            || !p
                .sensor_fields
                .iter()
                .all(|key| p.sensor_registers.get(*key).is_some())
        {
            return super::fail(format!(
                "Stationary endpoint7 read requires all{} recovered initialization writes",
                p.sensor_fields.len()
            ));
        }
        let (data_cfg, control1, control2) = (
            p.sensor_registers.get_or_zero(0x0e),
            p.sensor_registers.get_or_zero(0x2a),
            p.sensor_registers.get_or_zero(0x2b),
        );
        if data_cfg > 2 {
            return super::fail("Stationary endpoint7 range/filter configuration is unsupported");
        }
        if control1 & 1 == 0 || control1 & 2 != 0 {
            return super::fail("Stationary endpoint7 requires active six-byte12-bit mode");
        }
        if control2 & 0xc0 != 0
            || [0x2f, 0x30, 0x31]
                .iter()
                .any(|key| p.sensor_registers.get_or_zero(*key) != 0)
        {
            return super::fail(
                "Stationary endpoint7 self-test/reset/nonzero offsets are unsupported",
            );
        }
        let counts = [1024_u16, 512, 256][data_cfg as usize];
        p.sensor_sample = parts::dict(vec![
            (
                "axes_g",
                Value::List(vec![Value::int(0), Value::int(0), Value::int(1)]),
            ),
            (
                "counts",
                Value::List(vec![Value::int(0), Value::int(0), Value::int(counts)]),
            ),
            ("full_scale_g", Value::int([2, 4, 8][data_cfg as usize])),
            ("resolution_bits", Value::int(12)),
            (
                "interpretation",
                Value::str("inferred MMA8452Q-compatible registers; unmeasured +Z pose"),
            ),
        ]);
        let last = (counts << 4).to_be_bytes();
        Ok(vec![0, 0, 0, 0, last[0], last[1]])
    }
    pub fn write_primary(
        &mut self,
        address: u64,
        size: u32,
        value: u64,
        routes: &Routes,
        actions: &mut Vec<Action>,
    ) -> Result<()> {
        let offset = self.offset(address, size)?;
        if self.prepare_write(address, size, value)? {
            return Ok(());
        }
        let value = value as u32;
        if offset == 8 {
            if self.phase == Phase::Idle && matches!(value, 0x19 | 0x1b) {
                self.begin_shift(
                    Op::Start,
                    1,
                    Phase::AwaitingAddress,
                    Finish::None,
                    routes,
                    actions,
                )?;
                self.regs.insert(8, 8); // Start bits0/4 complete; bit3 retained.
                let p = &mut self.parts;
                p.payload.clear();
                p.target = None;
                p.target_present = true;
                self.rx_bytes.clear();
                self.rx_index = 0;
                self.rx_ready = false;
                self.rx_configured = false;
                self.rx_kind = None;
                p.rx_metadata = Value::None;
            } else if self.phase == Phase::AwaitingAddress && value == 0xa {
                self.regs.insert(8, value);
            } else if self.phase == Phase::WritingData
                && value == 0xa
                && let Some(index) = self.parts.command_index(self.parts.target)
            {
                self.parts.command_registers[index].read(&self.parts.payload)?;
                self.phase = Phase::AwaitingReadAddress;
                self.regs.insert(8, value);
            } else if (self.sensor_profile() || self.parts.stationary)
                && self.phase == Phase::WritingData
                && self.parts.admitted(self.parts.target)
                && self.parts.target == self.parts.sensor_address
                && self.parts.payload == [1]
                && value == 0xa
            {
                self.phase = Phase::AwaitingReadAddress;
                self.regs.insert(8, value);
            } else if self.receive_profile() && self.phase == Phase::ReadingData {
                match self.receive_command(value)? {
                    ReceiveCommand::Configure | ReceiveCommand::Nack => (),
                    ReceiveCommand::Byte(index) => {
                        self.begin_shift(
                            Op::ReceiveByte,
                            4,
                            Phase::ReadingData,
                            Finish::Receive(index),
                            routes,
                            actions,
                        )?;
                        self.regs.insert(8, value & !0x20);
                    }
                    ReceiveCommand::Stop => {
                        let finish = Finish::Read {
                            target: self.parts.target,
                            bytes: self.rx_bytes.clone(),
                            kind: self.rx_kind.clone().expect("validated receive kind"),
                            metadata: self.parts.rx_metadata.clone(),
                        };
                        self.begin_shift(Op::StopRead, 2, Phase::Idle, finish, routes, actions)?;
                        self.regs.insert(8, 0xa);
                    }
                }
            } else if self.phase == Phase::WritingData
                && value == 0xb
                && (matches!(self.parts.payload.len(), 2 | 3)
                    || self.parts.command_index(self.parts.target).is_some())
            {
                let p = &self.parts;
                if p.target == p.sensor_address && p.payload.len() != 2 {
                    return super::fail("Endpoint7 stop requires exactly one register/data pair");
                }
                let status = p.status_index(p.target);
                let command = p.command_index(p.target);
                if let Some(index) = command {
                    p.command_registers[index].validate_write(&p.payload)?;
                }
                if !p.admitted(p.target)
                    || (status.is_none()
                        && command.is_none()
                        && p.target != p.prefix_address
                        && p.target != p.sensor_address)
                {
                    return super::fail("Control-bus stop has no supported target address");
                }
                if status.is_some() && p.payload.len() != 2 {
                    return super::fail("Register/status stop requires one register/data pair");
                }
                let part = command.map_or_else(
                    || {
                        status.map_or(
                            if p.target == p.prefix_address {
                                WritePart::Prefix
                            } else {
                                WritePart::Sensor
                            },
                            WritePart::Status,
                        )
                    },
                    WritePart::CommandRegisters,
                );
                let finish = Finish::Write {
                    target: p.target.expect("admitted target"),
                    payload: p.payload.clone(),
                    part,
                };
                self.begin_shift(Op::Stop, 2, Phase::Idle, finish, routes, actions)?;
                self.regs.insert(8, 0xa);
            } else {
                return super::fail(format!(
                    "Unsupported control-bus command {value:#x} in phase {}",
                    self.phase.as_str()
                ));
            }
            return Ok(());
        }
        if value > 0xff {
            return super::fail("Control-bus transmit requires one byte");
        }
        let byte = value as u8;
        if self.phase == Phase::AwaitingAddress && self.regs.get_or_zero(8) == 0xa {
            let p = &self.parts;
            if let Some(index) = p.status_index(Some(byte >> 1))
                && byte & 1 != 0
                && p.admitted(Some(p.status[index].address))
            {
                let finish = Finish::FrameAddress {
                    target: p.status[index].address,
                    sample: p.status[index].frame.clone(),
                    kind: RxKind::RegisterStatus,
                };
                self.begin_shift(
                    Op::AddressRead,
                    4,
                    Phase::ReadingData,
                    finish,
                    routes,
                    actions,
                )?;
                self.regs.insert(4, value);
                return Ok(());
            }
            if p.admitted(p.prefix_address)
                && p.prefix_address.is_some_and(|a| byte == a << 1 | 1)
                && (p.touch.is_some() || matches!(
                    self.profile,
                    Some(Profile::Heartbeat | Profile::LatchedHeartbeat)
                ))
            {
                if let Some((register, bytes, _)) = p
                    .prefixed_registers
                    .iter()
                    .find(|row| Some(row.0) == p.endpoint9_prefix)
                {
                    let finish = Finish::PrefixedAddress {
                        target: p.prefix_address.expect("admitted prefix"),
                        register: *register,
                        bytes: bytes.clone(),
                    };
                    self.begin_shift(
                        Op::AddressRead,
                        4,
                        Phase::ReadingData,
                        finish,
                        routes,
                        actions,
                    )?;
                    self.regs.insert(4, value);
                    return Ok(());
                }
                if p.endpoint9_prefix != Some(p.heartbeat_register) {
                    return super::fail(format!(
                        "Endpoint9 heartbeat requires completed register{:04X} prefix",
                        p.heartbeat_register
                    ));
                }
                let metadata = parts::dict(vec![
                    ("counter_value", Value::int(p.heartbeat_counter)),
                    ("completed_read_index", Value::int(p.heartbeat_reads)),
                    (
                        "progression",
                        Value::str("advance by1 modulo256 only after completed one-byte read stop"),
                    ),
                    (
                        "provenance",
                        Value::str(
                            "explicit unmeasured external liveness input; physical rate/reset/identity unknown",
                        ),
                    ),
                ]);
                let finish = Finish::HeartbeatAddress {
                    target: p.prefix_address.expect("admitted prefix"),
                    byte: p.heartbeat_counter as u8,
                    metadata,
                };
                self.begin_shift(
                    Op::AddressRead,
                    4,
                    Phase::ReadingData,
                    finish,
                    routes,
                    actions,
                )?;
                self.regs.insert(4, value);
                return Ok(());
            }
            let sensor = matches!(
                self.profile,
                Some(
                    Profile::SensorWrite
                        | Profile::Stationary
                        | Profile::Heartbeat
                        | Profile::LatchedHeartbeat
                        | Profile::RegisterStatus
                )
            );
            let admitted_write = byte & 1 == 0
                && p.admitted(Some(byte >> 1))
                && (p.prefix_address == Some(byte >> 1)
                    || (sensor && p.sensor_address == Some(byte >> 1))
                    || p.status_index(Some(byte >> 1)).is_some()
                    || p.command_index(Some(byte >> 1)).is_some());
            if !admitted_write {
                return super::fail(
                    "Control-bus target outside explicit write profile; target reads are unresolved",
                );
            }
            self.begin_shift(
                Op::AddressWrite,
                0xc,
                Phase::WritingData,
                Finish::Target(byte >> 1),
                routes,
                actions,
            )?;
        } else if self.phase == Phase::AwaitingReadAddress
            && self.regs.get_or_zero(8) == 0xa
            && byte & 1 != 0
            && self.parts.admitted(Some(byte >> 1))
            && let Some(index) = self.parts.command_index(Some(byte >> 1))
        {
            if self.parts.target != Some(byte >> 1) {
                return super::fail("Command-register repeated start changed its target");
            }
            let sample = self.parts.command_registers[index].read(&self.parts.payload)?;
            self.begin_shift(
                Op::AddressRead,
                4,
                Phase::ReadingData,
                Finish::FrameAddress {
                    target: byte >> 1,
                    sample,
                    kind: RxKind::CommandRegisters,
                },
                routes,
                actions,
            )?;
        } else if (self.sensor_profile() || self.parts.stationary)
            && self.phase == Phase::AwaitingReadAddress
            && self.regs.get_or_zero(8) == 0xa
            && self.parts.admitted(self.parts.sensor_address)
            && self
                .parts
                .sensor_address
                .is_some_and(|a| byte == a << 1 | 1)
        {
            let sample = self.stationary_sample()?;
            self.begin_shift(
                Op::AddressRead,
                4,
                Phase::ReadingData,
                Finish::SensorAddress(sample),
                routes,
                actions,
            )?;
        } else if self.phase == Phase::WritingData {
            let p = &self.parts;
            if !p.admitted(p.target) {
                return super::fail("Control-bus data has no supported target address");
            }
            if let Some(index) = p.command_index(p.target) {
                p.command_registers[index].validate_byte(&p.payload, byte)?;
            } else if let Some(index) = p.status_index(p.target) {
                p.status[index].validate_byte(&p.payload, byte)?;
            } else if p.target == p.prefix_address {
                let registered = match p.payload.as_slice() {
                    [] => p
                        .prefixed_registers
                        .iter()
                        .any(|row| row.0 >> 8 == byte as u16),
                    [high] => p
                        .prefixed_registers
                        .iter()
                        .any(|row| row.0 == u16::from_be_bytes([*high, byte])),
                    _ => false,
                };
                let ordinary = p.payload.first().copied().unwrap_or(byte) == p.prefix_high;
                if p.payload.len() >= 3 || (!ordinary && !registered) {
                    return super::fail(format!(
                        "Control-bus supports only two-byte{:02X}xx prefixes and an optional one-byte write",
                        p.prefix_high
                    ));
                }
            } else if p.target == p.sensor_address {
                let prefix_read =
                    (self.sensor_profile() || p.stationary) && p.payload.is_empty() && byte == 1;
                if p.payload.len() >= 2
                    || p.payload == [1]
                    || (p.payload.is_empty() && !p.sensor_fields.contains(&value) && !prefix_read)
                {
                    return super::fail("Endpoint7 accepts only recovered register/data pairs");
                }
            } else {
                return super::fail("Control-bus data has no supported target address");
            }
            self.begin_shift(
                Op::WriteByte,
                4,
                Phase::WritingData,
                Finish::Byte(byte),
                routes,
                actions,
            )?;
        } else {
            return super::fail(format!(
                "Control-bus transmit in unsupported phase {}",
                self.phase.as_str()
            ));
        }
        self.regs.insert(4, value);
        Ok(())
    }
}
