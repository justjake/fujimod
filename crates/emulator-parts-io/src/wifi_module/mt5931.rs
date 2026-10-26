//! MT5931 HIF command/event ABI. Protocol references are in README.md.
//! This endpoint has no RF peers. Identity is an explicit board input.
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub mac_address: [u8; 6],
}
#[derive(Default, Serialize, Deserialize)]
pub struct Endpoint {
    control: u32,
    interrupt_enable: u32,
    interrupt_status: u32,
    interrupt_output: bool,
    tx_released: [u8; 8],
    rx1: VecDeque<Vec<u8>>,
    commands: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Change {
    Control(u32),
    InterruptEnable(u32),
    LowPower(u32),
    ClearInterrupt(u32),
    ClearTx(usize),
    Receive,
    Command { event: Option<Vec<u8>> },
}
impl Endpoint {
    pub fn fingerprint(&self) -> serde_json::Value {
        serde_json::json!({"control":self.control,"interrupt_enable":self.interrupt_enable,
            "interrupt_status":self.interrupt_status,"interrupt_output":self.interrupt_output,
            "tx_released":self.tx_released,
            "rx1":self.rx1.iter().map(|packet| fujimod_emulator_devices::fingerprint::bytes(packet)).collect::<Vec<_>>(),"commands":self.commands})
    }
    pub fn interrupt(&self) -> bool {
        self.interrupt_output && self.interrupt_status & self.interrupt_enable != 0
    }
    pub fn low_power(&self, value: u32) -> Result<Change, String> {
        if value & 0x300 != 0 {
            return Err("MT5931 ownership handoff/wakeup timing not modeled".into());
        }
        if value & !3 != 0 {
            return Err(format!("MT5931 unsupported WHLPCR bits {value:#x}"));
        }
        Ok(Change::LowPower(value))
    }
    pub fn transfer(
        &self,
        options: &Options,
        address: u32,
        write: bool,
        payload: &[u8],
        quantity: usize,
    ) -> Result<(Vec<u8>, Option<Change>), String> {
        if address == 0x2c && write {
            return self.command(options, payload, quantity);
        }
        if address == 0x34 && !write {
            let packet = self.rx1.front().ok_or("MT5931 RX1 FIFO underflow")?;
            if quantity != packet.len().div_ceil(4) * 4 {
                return Err(format!(
                    "MT5931 RX1 extent {quantity} does not match packet {}",
                    packet.len()
                ));
            }
            let mut data = packet.clone();
            data.resize(quantity, 0);
            return Ok((data, Some(Change::Receive)));
        }
        if quantity != 4 || write && payload.len() != 4 {
            return Err(format!(
                "MT5931 unsupported register/FIFO extent address={address:#x} write={write} bytes={quantity}"
            ));
        }
        if write {
            let value = u32::from_le_bytes(payload.try_into().unwrap());
            let change = match address {
                4 => self.low_power(value)?,
                0xc if value & !6 == 0 => Change::Control(value),
                0x14 => Change::InterruptEnable(value),
                0x10 if self.control & 2 != 0 => Change::ClearInterrupt(value),
                _ => {
                    return Err(format!(
                        "MT5931 unsupported register write {address:#x}={value:#x}"
                    ));
                }
            };
            return Ok((Vec::new(), Some(change)));
        }
        let (value, change) = match address {
            0 => (0x205931, None),
            4 => (0x100 | u32::from(self.interrupt_output), None),
            0xc => (self.control, None),
            0x14 => (self.interrupt_enable, None),
            0x10 => (
                self.interrupt_status,
                (self.control & 2 == 0).then_some(Change::ClearInterrupt(self.interrupt_status)),
            ),
            0x20 | 0x24 => {
                let index = (address - 0x20) as usize;
                (
                    u32::from_le_bytes(self.tx_released[index..index + 4].try_into().unwrap()),
                    Some(Change::ClearTx(index)),
                )
            }
            0x50 => ((self.rx1.front().map_or(0, Vec::len) as u32) << 16, None),
            _ => return Err(format!("MT5931 unsupported register read {address:#x}")),
        };
        Ok((value.to_le_bytes().to_vec(), change))
    }
    fn event(id: u8, sequence: u8, body: &[u8]) -> Vec<u8> {
        let mut result = Vec::with_capacity(8 + body.len());
        result.extend_from_slice(&((8 + body.len()) as u16).to_le_bytes());
        result.extend_from_slice(&[1, 0, id, sequence, 0, 0]);
        result.extend_from_slice(body);
        result
    }
    fn command(
        &self,
        options: &Options,
        packet: &[u8],
        quantity: usize,
    ) -> Result<(Vec<u8>, Option<Change>), String> {
        if packet.len() != quantity || quantity < 8 {
            return Err("MT5931 truncated HIF command header".into());
        }
        let length = (u16::from_le_bytes(packet[..2].try_into().unwrap()) & 0xfff) as usize;
        if length < 8
            || length > quantity
            || quantity != length.div_ceil(4) * 4
            || packet[3] != 0x50
            || packet[5] > 1
        {
            return Err(format!(
                "MT5931 unsupported HIF command framing {:02x?}",
                packet
            ));
        }
        if self.rx1.len() >= 16 || self.tx_released[4] == u8::MAX {
            return Err("MT5931 command/RX queue capacity exceeded".into());
        }
        let body = &packet[8..length];
        let event = match (packet[4], packet[5], body.len()) {
            (0xf2, 1, 8) => None, // CMD_MCU_LP_PARAM: oscillator stable time, reserved bytes ignored.
            (0xc1, 0, 12) => {
                let mut config = [0; 12];
                config[..6].copy_from_slice(&options.mac_address);
                Some(Self::event(9, packet[6], &config))
            }
            (0x80, 0, _) => {
                return Err(format!(
                    "MT5931 NIC capability reply unqualified: CID=0x80 sequence={} query_body_bytes={}; documented EVENT_NIC_CAPABILITY has 12/16 body bytes; extended capability words require protocol evidence",
                    packet[6],
                    body.len()
                ));
            }
            _ => {
                return Err(format!(
                    "MT5931 command not modeled CID={:#04x} set={} sequence={} body={:02x?}",
                    packet[4], packet[5], packet[6], body
                ));
            }
        };
        Ok((Vec::new(), Some(Change::Command { event })))
    }
    pub fn commit(&mut self, change: Change) {
        match change {
            Change::Control(value) => self.control = value,
            Change::InterruptEnable(value) => self.interrupt_enable = value,
            Change::LowPower(value) => {
                if value & 1 != 0 {
                    self.interrupt_output = true;
                }
                if value & 2 != 0 {
                    self.interrupt_output = false;
                }
            }
            Change::ClearInterrupt(mask) => self.interrupt_status &= !mask,
            Change::ClearTx(index) => self.tx_released[index..index + 4].fill(0),
            Change::Receive => {
                self.rx1.pop_front();
            }
            Change::Command { event } => {
                self.commands += 1;
                self.tx_released[4] += 1;
                self.interrupt_status |= 1;
                if let Some(event) = event {
                    self.rx1.push_back(event);
                    self.interrupt_status |= 4;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn options() -> Options {
        Options {
            mac_address: [2, 3, 4, 5, 6, 7],
        }
    }
    fn command(id: u8, set: bool, sequence: u8, body: &[u8]) -> Vec<u8> {
        let mut packet = ((8 + body.len()) as u16).to_le_bytes().to_vec();
        packet.extend_from_slice(&[0, 0x50, id, u8::from(set), sequence, 0xa5]);
        packet.extend_from_slice(body);
        packet.resize(packet.len().div_ceil(4) * 4, 0xa5);
        packet
    }
    fn transfer(
        e: &mut Endpoint,
        address: u32,
        write: bool,
        payload: &[u8],
        quantity: usize,
    ) -> Vec<u8> {
        let (data, change) = e
            .transfer(&options(), address, write, payload, quantity)
            .unwrap();
        if let Some(change) = change {
            e.commit(change);
        }
        data
    }
    fn read(e: &mut Endpoint, address: u32) -> u32 {
        u32::from_le_bytes(transfer(e, address, false, &[], 4).try_into().unwrap())
    }
    #[test]
    fn oscillator_and_basic_config_use_wire_sequence_and_opaque_reserved_bytes() {
        let mut e = Endpoint::default();
        let packet = command(0xf2, true, 0xff, &[0xa5; 8]);
        let (_, effect) = e
            .transfer(&options(), 0x2c, true, &packet, packet.len())
            .unwrap();
        assert_eq!(e.commands, 0); // Planning is observational; transport owns the deadline.
        assert_eq!(read(&mut e, 0x50), 0);
        e.commit(effect.unwrap());
        assert_eq!(read(&mut e, 0x50), 0); // F2 has no reply.
        let packet = command(0xc1, false, 0x72, &[0xa5; 12]);
        transfer(&mut e, 0x2c, true, &packet, 20);
        assert_eq!(read(&mut e, 0x50), 20 << 16);
        let response = transfer(&mut e, 0x34, false, &[], 20);
        assert_eq!(&response[..8], &[20, 0, 1, 0, 9, 0x72, 0, 0]);
        assert_eq!(&response[8..14], &options().mac_address);
        assert_eq!(&response[14..], &[0; 6]);
        assert_eq!(read(&mut e, 0x50), 0);
        assert!(
            e.transfer(&options(), 0x34, false, &[], 20)
                .unwrap_err()
                .contains("underflow")
        );
        assert_eq!(read(&mut e, 0x24), 2); // TC4 credit returned for each actual TX.
        assert_eq!(read(&mut e, 0x24), 0);
    }
    #[test]
    fn fifo_order_masks_read_clear_and_write_one_clear() {
        let mut e = Endpoint::default();
        for seq in [9, 2] {
            let packet = command(0xc1, false, seq, &[0; 12]);
            transfer(&mut e, 0x2c, true, &packet, packet.len());
        }
        assert!(!e.interrupt());
        transfer(&mut e, 0x14, true, &5u32.to_le_bytes(), 4);
        transfer(&mut e, 4, true, &1u32.to_le_bytes(), 4);
        assert!(e.interrupt());
        transfer(&mut e, 0xc, true, &2u32.to_le_bytes(), 4);
        assert_eq!(read(&mut e, 0x10), 5);
        assert_eq!(read(&mut e, 0x10), 5);
        transfer(&mut e, 0x10, true, &1u32.to_le_bytes(), 4);
        assert_eq!(read(&mut e, 0x10), 4);
        transfer(&mut e, 0xc, true, &0u32.to_le_bytes(), 4);
        assert_eq!(read(&mut e, 0x10), 4);
        assert!(!e.interrupt());
        assert_eq!(read(&mut e, 0x10), 0);
        assert_eq!(transfer(&mut e, 0x34, false, &[], 20)[5], 9);
        assert_eq!(transfer(&mut e, 0x34, false, &[], 20)[5], 2);
        assert_eq!(read(&mut e, 0x50), 0);
    }
    #[test]
    fn extended_capability_unknown_commands_and_malformed_packets_refuse_without_effects() {
        let mut e = Endpoint::default();
        assert!(e.low_power(0x100).is_err());
        assert!(e.low_power(0x200).is_err());
        for (id, set, body_length, expected) in [
            (0x80, false, 20, "extended capability words"),
            (0xee, true, 4, "CID=0xee"),
        ] {
            let before = serde_json::to_vec(&e).unwrap();
            let packet = command(id, set, 3, &vec![0; body_length]);
            assert!(
                e.transfer(&options(), 0x2c, true, &packet, packet.len())
                    .unwrap_err()
                    .contains(expected)
            );
            assert_eq!(serde_json::to_vec(&e).unwrap(), before);
        }
        let packet = command(0xc1, false, 5, &[0; 12]);
        for (index, value) in [(0, 0), (3, 0x80), (5, 2)] {
            let mut bad = packet.clone();
            bad[index] = value;
            assert!(e.transfer(&options(), 0x2c, true, &bad, bad.len()).is_err());
        }
        transfer(&mut e, 0x2c, true, &packet, 20);
        let before = serde_json::to_vec(&e).unwrap();
        assert!(e.transfer(&options(), 0x34, false, &[], 16).is_err());
        assert_eq!(serde_json::to_vec(&e).unwrap(), before);
    }
    #[test]
    fn binary_checkpoint_preserves_pending_fifo_and_inspection_is_observational() {
        let mut e = Endpoint::default();
        let packet = command(0xc1, false, 11, &[0; 12]);
        transfer(&mut e, 0x2c, true, &packet, 20);
        let before = serde_json::to_vec(&e).unwrap();
        assert_eq!(e.fingerprint(), e.fingerprint());
        assert_eq!(before, serde_json::to_vec(&e).unwrap());
        let mut encoded = Vec::new();
        ciborium::into_writer(&e, &mut encoded).unwrap();
        let mut restored: Endpoint = ciborium::from_reader(encoded.as_slice()).unwrap();
        assert_eq!(e.fingerprint(), restored.fingerprint());
        assert_eq!(
            transfer(&mut e, 0x34, false, &[], 20),
            transfer(&mut restored, 0x34, false, &[], 20)
        );
    }
}
