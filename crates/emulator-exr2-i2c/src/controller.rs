//! Register-level transfers through a controller with an unbound external part.
//!
//! These transitions consume controller registers and physical IRQ routes only.
//! They never infer reply lengths, endpoint policy or software intent. Unknown
//! receive requests each produce one zero byte until the guest issues STOP.
use super::transfer::{Finish, Pending};
use super::{Operation as Op, Phase, Registers, Routes};
use fujimod_emulator_devices::record::Result;
use fujimod_emulator_devices::{Action, Record, Value};

impl Registers {
    pub fn transport_access(
        &mut self,
        write: bool,
        address: u64,
        size: u32,
        value: u64,
        routes: &Routes,
        actions: &mut Vec<Action>,
    ) -> Result<()> {
        let Ok(offset) = self.offset(address, size) else {
            return Ok(()); // Unknown registers have no controller behavior; the framework owns fallback.
        };
        let value = value as u32;
        self.unmodeled_transaction = true;
        if !write {
            if offset == 4 {
                self.regs.insert(4, 0);
                self.rx_ready = false;
                self.rx_index += 1;
                self.received_bytes += 1;
                self.rx_consumed = true;
            }
            return Ok(());
        }
        // Configuration has no transfer completion or IRQ on real supported
        // accesses. Retain the guest's unsupported mode without widening the
        // strict controller's supported-mode set.
        if offset == 0x10 || offset == 0 {
            self.regs.insert(offset, value);
            return Ok(());
        }
        if offset == 0x18 {
            return Ok(()); // Status is read-only.
        }
        let (op, status, phase, finish) = if offset == 8 {
            match value {
                0x19 | 0x1b => {
                    self.parts.payload.clear();
                    self.parts.target = None;
                    self.rx_index = 0;
                    self.rx_ready = false;
                    self.rx_configured = false;
                    self.receive_configured = false;
                    self.rx_bytes.clear();
                    self.rx_kind = None;
                    self.regs.insert(8, 8);
                    (Op::Start, 1, Phase::AwaitingAddress, Finish::None)
                }
                0xa | 0xe => {
                    self.regs.insert(8, value);
                    if self.phase == Phase::WritingData {
                        self.phase = Phase::AwaitingReadAddress;
                    }
                    self.rx_configured = true;
                    self.receive_configured = true;
                    return Ok(());
                }
                0xb => {
                    self.regs.insert(8, 0xa);
                    (Op::Stop, 2, Phase::Idle, Finish::None)
                }
                v if v & 0x20 != 0 => {
                    self.regs.insert(8, v & !0x20);
                    (Op::ReceiveByte, 4, Phase::ReadingData, Finish::ZeroReceive)
                }
                _ => {
                    self.regs.insert(8, value);
                    (Op::Stop, 0xf, Phase::Idle, Finish::None)
                }
            }
        } else if matches!(
            self.phase,
            Phase::AwaitingAddress | Phase::AwaitingReadAddress
        ) {
            let target = (value as u8) >> 1;
            let read = value & 1 != 0;
            self.regs.insert(4, value);
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
                Finish::UnboundAddress { target, read },
            )
        } else {
            self.regs.insert(4, value);
            self.parts.payload.push(value as u8);
            (Op::WriteByte, 0xc, Phase::WritingData, Finish::None)
        };
        let target = self.irq_target(routes)?;
        self.start();
        self.next_token = self.next_token.wrapping_add(1);
        let token = self.next_token;
        self.pending.insert(
            token,
            Pending {
                generation: self.generation,
                unbound_reply: true,
                profile: self.profile.clone(),
                latched: false,
                binding: [[None; 6]; 2],
                target,
                op,
                status,
                phase,
                finish,
            },
        );
        actions.push(Action::Log(Record::new(
            "Discovery control-bus transfer",
            [
                ("address", Value::hex(address.into())),
                ("value", Value::hex(value.into())),
                ("status", Value::hex(status.into())),
                ("irq", Value::int(self.irq)),
                ("target_cpu", Value::int(target)),
                ("zero_data", Value::Bool(true)),
            ],
        )));
        actions.push(Action::Schedule {
            delay: self.transfer_ticks,
            token,
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Channel, RouteBank};
    use super::*;

    fn routes() -> Routes {
        Routes {
            banks: [
                RouteBank {
                    enabled: true,
                    ..Default::default()
                },
                RouteBank::default(),
            ],
        }
    }

    fn finish(m: &mut Registers, actions: Vec<Action>, status: u32) {
        let token = actions
            .iter()
            .find_map(|a| match a {
                Action::Schedule { delay: 4, token } => Some(*token),
                _ => None,
            })
            .unwrap();
        let mut completion = Vec::new();
        assert!(
            m.complete_native(token, &Routes::default(), None, &mut completion)
                .unwrap()
        );
        assert!(
            completion
                .iter()
                .any(|a| matches!(a, Action::QueueIrq(0, 0x65)))
        );
        assert_eq!(m.regs.get_or_zero(0x18), status);
        assert!(!m.busy);
        m.irq_completed(token, &routes(), &mut completion).unwrap();
    }

    #[test]
    fn register_engine_starts_without_body_part_defaults() {
        let model = Registers::new(Channel::Primary, 0xfff85000, vec![9], 0x65, 4);

        assert_eq!(model.irq, 0x65);
        assert_eq!(model.transfer_ticks, 4);
        assert_eq!(model.parts.sensor_address, None);
        assert_eq!(model.parts.prefix_address, None);
        assert!(model.parts.sensor_fields.is_empty());
        assert_eq!(model.parts.prefix_high, 0);
        assert_eq!(model.parts.heartbeat_register, 0);
        assert_eq!(model.parts.topology, Some(Vec::new()));
    }

    #[test]
    fn unknown_mode_and_zero_stream_use_real_status_and_irq_without_changing_strict_laws() {
        let mut m = Registers::new(Channel::Primary, 0xfff85000, vec![9], 0x65, 4);
        let mut a = Vec::new();
        assert!(
            m.write_primary(0xfff85010, 4, 1, &routes(), &mut a)
                .is_err()
        );
        m.transport_access(true, 0xfff85010, 4, 1, &routes(), &mut a)
            .unwrap();
        m.transport_access(true, 0xfff85000, 4, 1, &routes(), &mut a)
            .unwrap();
        assert_eq!(m.modes, vec![9]);
        assert!(a.is_empty());
        assert!(
            m.write_primary(0xfff85008, 4, 0x19, &routes(), &mut a)
                .is_err()
        );
        m.transport_access(true, 0xfff85008, 4, 0x19, &routes(), &mut a)
            .unwrap();
        assert_eq!(m.regs.get_or_zero(0x18), 0);
        finish(&mut m, std::mem::take(&mut a), 1);
        m.transport_access(true, 0xfff85008, 4, 0xa, &routes(), &mut a)
            .unwrap();
        assert!(
            m.write_primary(0xfff85004, 4, 0x6b, &routes(), &mut a)
                .is_err()
        );
        m.transport_access(true, 0xfff85004, 4, 0x6b, &routes(), &mut a)
            .unwrap();
        finish(&mut m, std::mem::take(&mut a), 4);
        // The byte count comes only from guest requests, never a driver buffer.
        for index in 0..20 {
            m.transport_access(true, 0xfff85008, 4, 0x2e, &routes(), &mut a)
                .unwrap();
            finish(&mut m, std::mem::take(&mut a), 4);
            m.transport_access(false, 0xfff85004, 4, 0, &routes(), &mut a)
                .unwrap();
            assert_eq!(m.regs.get_or_zero(4), 0);
            assert_eq!(m.rx_index, index + 1);
        }
        m.transport_access(true, 0xfff85008, 4, 0xb, &routes(), &mut a)
            .unwrap();
        finish(&mut m, a, 2);
        assert_eq!(m.phase, Phase::Idle);
        assert_eq!(m.operations_started, m.operations_completed);
        assert_eq!(m.operations_completed, m.irqs_raised);
        assert!(
            m.write_primary(0xfff85010, 4, 1, &routes(), &mut Vec::new())
                .is_err()
        );
    }
}
