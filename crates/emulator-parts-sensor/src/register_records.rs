//! Packed three-byte sensor register records: register high byte, register
//! low byte and value, at eight-byte stride; bytes 3 to 7 of each record are
//! not part of the record. The X30 sends them through 0x124b300 and queues
//! them through 0x1345978. The controller keeps the last value written to each
//! register. No drive mode, standby, exposure or read reply is mapped, so
//! every readout is valid and carries the sensor's declared linear response.
use crate::sensor::Controls;
use fujimod_emulator_devices::checkpoint::Checkpoint;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const NAME: &str = "register-records-controls";

#[derive(Default)]
pub struct Sensor {
    registers: BTreeMap<u16, u8>,
}

impl Controls for Sensor {
    fn checkpoint(&self) -> Result<Checkpoint, String> {
        Checkpoint::encode(NAME, &self.registers)
    }
    fn validate_checkpoint(&self, saved: &Checkpoint) -> Result<(), String> {
        let _: BTreeMap<u16, u8> = saved.decode(NAME)?;
        if !saved.blobs().is_empty() || !saved.children().is_empty() {
            return Err("sensor controller checkpoint configuration differs".into());
        }
        Ok(())
    }
    fn restore_checkpoint(&mut self, saved: &Checkpoint) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        self.registers = saved.decode(NAME)?;
        Ok(())
    }
    fn fingerprint(&self) -> serde_json::Value {
        serde_json::json!({ "registers": self.registers })
    }
    fn command(&mut self, bytes: &[u8]) -> Result<(), String> {
        let (records, rest) = bytes.as_chunks::<8>();
        if !rest.is_empty() {
            return Err("sensor register records need eight-byte slots".into());
        }
        for record in records {
            self.registers
                .insert(u16::from_be_bytes([record[0], record[1]]), record[2]);
        }
        Ok(())
    }
    fn frame(&mut self) -> Result<bool, String> {
        Ok(true)
    }
    fn preview_view(
        &self,
        _horizontal: [usize; 2],
    ) -> Result<Option<fujimod_emulator_imaging::scene::view::View>, String> {
        Ok(None)
    }
    fn timing(&mut self, _horizontal: u32, _vertical: u32, _shutter_drive: Option<u64>) -> Result<(), String> {
        Ok(())
    }
    fn save(&self) -> serde_json::Value {
        serde_json::to_value(Saved { registers: self.registers.clone() }).unwrap()
    }
    fn restore(&mut self, state: &serde_json::Value) -> Result<(), String> {
        let saved: Saved = serde_json::from_value(state.clone()).map_err(|e| e.to_string())?;
        self.registers = saved.registers;
        Ok(())
    }
    /// The X30 1.03 capture machine expects the still frame's edge after its
    /// RAW copy. The exposure-end message 0x69 (posted at that frame edge by
    /// 0x13449e4 -> 0x1341b3c) reaches action 0x12697dc, which sends job
    /// event 9 through 0x1293f04 only once the RAW copy (0x65, 0x1292d18)
    /// has set its count. The other order ends in action 0x1269a58, which
    /// has no event-9 retry (X70's counterpart 0x122f7f4 calls 0x125c8b8),
    /// and 0x1293f04 is the firmware's only event-9 sender. Without event 9
    /// the record task never queues develop, so the edge follows readout.
    fn edge_at_raw_readout_end(&self) -> bool {
        true
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    registers: BTreeMap<u16, u8>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_the_last_value_of_each_sixteen_bit_register() {
        let mut sensor = Sensor::default();
        sensor
            .command(&[0x34, 0x07, 2, 0, 0, 0, 0, 0, 0x00, 0x23, 0xcf, 0, 0, 0, 0, 0, 0x34, 0x07, 3, 9, 9, 9, 9, 9])
            .unwrap();
        assert_eq!(sensor.registers, BTreeMap::from([(0x0023, 0xcf), (0x3407, 3)]));
        assert!(sensor.command(&[0; 7]).is_err());
        let saved = sensor.save();
        let mut restored = Sensor::default();
        restored.restore(&saved).unwrap();
        assert_eq!(restored.registers, sensor.registers);
        let checkpoint = sensor.checkpoint().unwrap();
        let mut thawed = Sensor::default();
        thawed.restore_checkpoint(&checkpoint).unwrap();
        assert_eq!(thawed.registers, sensor.registers);
    }
}
