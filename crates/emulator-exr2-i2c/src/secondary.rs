//! Bus1 request admission against immutable declared endpoint contracts.
//! The native binding supplies the current cable input and IRQ routes at
//! each start/read. Completion checks still sample them at completion time.
use super::{
    Operation as Op, Phase, Profile, ReceiveCommand, Registers, Routes,
    SecondaryFinish as Finish, SecondaryRequest,
};
use fujimod_emulator_devices::record::Result;
use fujimod_emulator_devices::{Action, WordMap};

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Contract {
    pub fixed_modes: bool,
    pub register_address: Option<u8>,
    pub pwic_address: Option<u8>,
    pub register_modes: Vec<u32>,
    pub pwic_modes: Vec<u32>,
    pub read_registers: Vec<u32>,
    pub register_masks: WordMap,
    pub pwic_masks: WordMap,
    pub pwic_read_prefixes: Vec<(u32, Vec<u8>)>,
}

impl Registers {
    pub fn secondary_input(&self, connected: bool) -> Result<()> {
        match self.profile {
            Some(Profile::DeclaredSecondary) if connected != self.parts.secondary_usb_connected => {
                super::fail("Declared secondary bus cable input changed")
            }
            Some(Profile::UsbDisconnected | Profile::Pwic61 | Profile::PwicStartup)
                if connected =>
            {
                super::fail("Connected USB charge state is outside the disconnected input profile")
            }
            Some(
                Profile::DeclaredSecondary
                | Profile::UsbDisconnected
                | Profile::Pwic61
                | Profile::PwicStartup,
            ) => Ok(()),
            _ => super::fail(
                "Bus1 target response requires explicit disconnected usbcharge input profile",
            ),
        }
    }

    fn secondary_begin(
        &mut self,
        request: (Op, u32, Phase, Finish),
        connected: bool,
        routes: &Routes,
        actions: &mut Vec<Action>,
    ) -> Result<()> {
        self.secondary_input(connected)?;
        if self.busy {
            return super::fail("Bus1 operation while prior shift is pending");
        }
        let target = self.irq_target(routes)?;
        self.start_secondary(
            SecondaryRequest {
                op: request.0,
                status: request.1,
                phase: request.2,
                finish: request.3,
                target,
            },
            actions,
        );
        Ok(())
    }

    fn secondary_field(&self, pwic: bool, register: u32, value: u32) -> Result<()> {
        let (registers, masks) = if pwic {
            (&self.parts.pwic_registers, &self.secondary.pwic_masks)
        } else {
            (&self.parts.target_registers, &self.secondary.register_masks)
        };
        if !registers
            .get(register)
            .zip(masks.get(register))
            .is_some_and(|(old, mask)| (value ^ old) & !mask == 0)
        {
            return super::fail(if pwic {
                "PWIC write changes unsupported register or bits outside recovered field mask"
            } else {
                "Bus1 write changes bits outside the selected register contract"
            });
        }
        Ok(())
    }

    fn secondary_receive_input(&self) -> Result<Vec<u8>> {
        let p = &self.parts;
        if !p.admitted(p.target) {
            return super::fail("Secondary bus receive has no admitted target");
        }
        if p.target == self.secondary.pwic_address {
            let register = p
                .pwic_prefix
                .ok_or_else(|| fujimod_emulator_devices::Unsupported("Missing PWIC prefix".into()))?;
            let mut bytes = self
                .secondary
                .pwic_read_prefixes
                .iter()
                .find(|(r, _)| *r == register)
                .map_or_else(Vec::new, |(_, bytes)| bytes.clone());
            let byte = p
                .pwic_registers
                .get(register)
                .ok_or_else(|| fujimod_emulator_devices::Unsupported("Missing PWIC input register".into()))?;
            bytes.push(byte as u8);
            Ok(bytes)
        } else {
            let register = *p
                .payload
                .first()
                .ok_or_else(|| fujimod_emulator_devices::Unsupported("Missing bus1 prefix".into()))?;
            let byte = p
                .target_registers
                .get(register.into())
                .ok_or_else(|| fujimod_emulator_devices::Unsupported("Missing bus1 input register".into()))?;
            Ok(vec![byte as u8])
        }
    }

    pub fn write_secondary(
        &mut self,
        address: u64,
        size: u32,
        value: u64,
        connected: bool,
        routes: &Routes,
        actions: &mut Vec<Action>,
    ) -> Result<()> {
        let offset = self.offset(address, size)?;
        // Legacy modes are recomputed from the mutable PWIC map before every
        // write; explicit contracts keep their fixed modes.
        if !self.secondary.fixed_modes {
            self.modes = if self.parts.pwic_registers.items().next().is_some() {
                vec![1, 9]
            } else {
                vec![9]
            };
        }
        if self.prepare_write(address, size, value)? {
            return Ok(());
        }
        let value = value as u32;
        if offset == 8 {
            match (&self.phase, value) {
                (Phase::Idle, 0x19 | 0x1b) => {
                    self.secondary_begin(
                        (Op::Start, 1, Phase::AwaitingAddress, Finish::None),
                        connected,
                        routes,
                        actions,
                    )?;
                    self.regs.insert(8, 8);
                    self.parts.payload.clear();
                    self.parts.target = None;
                    self.parts.target_present = true;
                    self.rx_ready = false;
                    self.rx_consumed = false;
                    self.receive_configured = false;
                    self.last_byte_nack = false;
                    self.rx_byte = None;
                    self.rx_bytes.clear();
                    self.rx_index = 0;
                }
                (Phase::AwaitingAddress, 0xa) => {
                    self.regs.insert(8, value);
                }
                (Phase::WritingData, 0xa) if self.parts.payload.len() == 1 => {
                    if self.parts.target != self.secondary.register_address {
                        return super::fail(
                            "PWIC uses recovered separate prefix and read transactions",
                        );
                    }
                    if !self
                        .secondary
                        .read_registers
                        .contains(&u32::from(self.parts.payload[0]))
                    {
                        return super::fail("Unknown bus1 target read register");
                    }
                    self.phase = Phase::AwaitingReadAddress;
                    self.regs.insert(8, value);
                }
                (Phase::WritingData, 0xb)
                    if self.parts.payload.len() > 2
                        && self.profile == Some(Profile::DeclaredSecondary)
                        && self.parts.target == self.secondary.register_address =>
                {
                    let register = u32::from(self.parts.payload[0]);
                    let bytes = self.parts.payload[1..].to_vec();
                    for (offset, byte) in bytes.iter().enumerate() {
                        self.secondary_field(false, register + offset as u32, (*byte).into())?;
                    }
                    self.secondary_begin(
                        (
                            Op::StopWrite,
                            2,
                            Phase::Idle,
                            Finish::WriteBurst { register, bytes },
                        ),
                        connected,
                        routes,
                        actions,
                    )?;
                    self.regs.insert(8, 0xa);
                }
                (Phase::WritingData, 0xb) if self.parts.payload.len() == 2 => {
                    let register = u32::from(self.parts.payload[0]);
                    let byte = self.parts.payload[1];
                    let pwic = self.parts.target == self.secondary.pwic_address;
                    if pwic {
                        self.secondary_field(true, register, byte.into())?;
                    }
                    if !pwic && self.secondary.register_masks.get(register).is_none() {
                        return super::fail("Bus1 only supports recovered0C/11 RMW writes");
                    }
                    if !pwic {
                        self.secondary_field(false, register, byte.into())?;
                    }
                    self.secondary_begin(
                        (
                            Op::StopWrite,
                            2,
                            Phase::Idle,
                            Finish::Write {
                                pwic,
                                register,
                                byte,
                            },
                        ),
                        connected,
                        routes,
                        actions,
                    )?;
                    self.regs.insert(8, 0xa);
                }
                (Phase::WritingData, 0xb)
                    if self.parts.target == self.secondary.pwic_address
                        && self.parts.payload.len() == 1
                        && self
                            .parts
                            .pwic_registers
                            .get(self.parts.payload[0].into())
                            .is_some() =>
                {
                    self.secondary_begin(
                        (Op::StopPrefix, 2, Phase::Idle, Finish::Prefix),
                        connected,
                        routes,
                        actions,
                    )?;
                    self.regs.insert(8, 0xa);
                }
                (Phase::ReadingData, _) => match self.receive_command(value)? {
                    ReceiveCommand::Configure | ReceiveCommand::Nack => return Ok(()),
                    ReceiveCommand::Byte(index) => {
                        let byte = self.rx_bytes.get(index).copied().ok_or_else(|| {
                            fujimod_emulator_devices::Unsupported("Missing bus1 receive byte".into())
                        })?;
                        self.rx_consumed = false;
                        self.secondary_begin(
                            (
                                Op::ReceiveByte,
                                4,
                                Phase::ReadingData,
                                Finish::Receive(byte),
                            ),
                            connected,
                            routes,
                            actions,
                        )?;
                        self.regs.insert(8, if value == 0x2a { 0xa } else { 0xe });
                    }
                    ReceiveCommand::Stop => {
                        let register =
                            if self.parts.target == self.secondary.pwic_address {
                                self.parts.pwic_prefix.ok_or_else(|| {
                                    fujimod_emulator_devices::Unsupported("Missing PWIC prefix".into())
                                })?
                            } else {
                                u32::from(*self.parts.payload.first().ok_or_else(|| {
                                    fujimod_emulator_devices::Unsupported("Missing bus1 prefix".into())
                                })?)
                            };
                        self.secondary_begin(
                            (Op::StopRead, 2, Phase::Idle, Finish::Read(register)),
                            connected,
                            routes,
                            actions,
                        )?;
                        self.regs.insert(8, 0xa);
                    }
                },
                _ => {
                    return super::fail(format!(
                        "Unknown bus1 command {value:#x} in phase {}",
                        self.phase.as_str()
                    ));
                }
            }
            return Ok(());
        }
        if value > 0xff {
            return super::fail("Bus1 transmit requires one byte");
        }
        let p = &self.parts;
        let contract = &self.secondary;
        let mode = self.regs.get_or_zero(0x10);
        if self.phase == Phase::AwaitingAddress
            && self.regs.get_or_zero(8) == 0xa
            && p.admitted(contract.register_address)
            && contract
                .register_address
                .is_some_and(|a| value == u32::from(a) << 1)
            && contract.register_modes.contains(&mode)
        {
            self.secondary_begin(
                (Op::AddressWrite, 0xc, Phase::WritingData, Finish::None),
                connected,
                routes,
                actions,
            )?;
            self.parts.target = self.secondary.register_address;
            self.parts.target_present = true;
        } else if self.phase == Phase::AwaitingAddress
            && self.regs.get_or_zero(8) == 0xa
            && p.admitted(contract.pwic_address)
            && contract
                .pwic_address
                .is_some_and(|a| value >> 1 == u32::from(a))
            && contract.pwic_modes.contains(&mode)
            && p.pwic_registers.items().next().is_some()
        {
            let read = value & 1 != 0;
            if read
                && !p
                    .pwic_prefix
                    .is_some_and(|r| p.pwic_registers.get(r).is_some())
            {
                return super::fail(
                    "PWIC direct read requires an actual completed supported prefix transaction",
                );
            }
            self.secondary_begin(
                (
                    if read {
                        Op::AddressRead
                    } else {
                        Op::AddressWrite
                    },
                    if read { 4 } else { 0xc },
                    if read {
                        Phase::ReadingData
                    } else {
                        Phase::WritingData
                    },
                    Finish::None,
                ),
                connected,
                routes,
                actions,
            )?;
            self.parts.target = self.secondary.pwic_address;
            self.parts.target_present = true;
            if read {
                self.rx_bytes = self.secondary_receive_input()?;
            }
        } else if self.phase == Phase::AwaitingReadAddress
            && p.admitted(contract.register_address)
            && contract
                .register_address
                .is_some_and(|a| value == (u32::from(a) << 1 | 1))
        {
            self.secondary_begin(
                (Op::AddressRead, 4, Phase::ReadingData, Finish::None),
                connected,
                routes,
                actions,
            )?;
            self.rx_bytes = self.secondary_receive_input()?;
        } else if self.phase == Phase::WritingData {
            if !p.admitted(p.target) {
                return super::fail("Secondary bus data has no admitted target");
            }
            if p.target == contract.pwic_address {
                if p.payload.is_empty() {
                    if p.pwic_registers.get(value).is_none() {
                        return super::fail(format!(
                            "PWIC profile rejects unsupported register prefix {value:#x}"
                        ));
                    }
                } else if p.payload.len() == 1 {
                    self.secondary_field(true, p.payload[0].into(), value)?;
                } else {
                    return super::fail(
                        "PWIC profile supports only a prefix or recovered field pair",
                    );
                }
            } else if p.payload.is_empty() {
                if !contract.read_registers.contains(&value) {
                    return super::fail("Unknown bus1 target register prefix");
                }
            } else if self.profile == Some(Profile::DeclaredSecondary) {
                let register = u32::from(p.payload[0]) + p.payload.len() as u32 - 1;
                if register > 255 {
                    return super::fail("Bus1 register burst exceeds byte address space");
                }
                self.secondary_field(false, register, value)?;
            } else if p.payload.len() != 1
                || contract.register_masks.get(p.payload[0].into()).is_none()
            {
                return super::fail(
                    "Bus1 only supports one-byte prefixes or0C/11 register/data pairs",
                );
            } else {
                self.secondary_field(false, p.payload[0].into(), value)?;
            }
            self.secondary_begin(
                (
                    Op::WriteByte,
                    4,
                    Phase::WritingData,
                    Finish::Byte(value as u8),
                ),
                connected,
                routes,
                actions,
            )?;
        } else {
            return super::fail(format!(
                "Unknown bus1 transmit {value:#x} in phase {}",
                self.phase.as_str()
            ));
        }
        self.regs.insert(4, value);
        Ok(())
    }
}
