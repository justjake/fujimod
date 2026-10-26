//! Transmit-only command words and prefixed byte packets. This retains wire
//! output, without inferring a recipient's pixels, analog effects or replies.
use super::spi::{Endpoint, Shift};
use fujimod_emulator_devices::Cx;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub command_prefix: u8,
    pub data_prefix: u8,
    pub payload_bytes: usize,
}

pub struct CommandDataOutput {
    options: Options,
    last_command: Option<u8>,
    partial: Option<Vec<u8>>,
    payload: Option<Vec<u8>>,
}
impl CommandDataOutput {
    pub fn new(options: Options) -> Result<Self, String> {
        if !(1..=256).contains(&options.payload_bytes) {
            return Err("serial output requires a bounded payload extent".into());
        }
        Ok(Self {
            options,
            last_command: None,
            partial: None,
            payload: None,
        })
    }
}
impl Endpoint for CommandDataOutput {
    fujimod_emulator_devices::checkpoint_fields!(options:Options,last_command:Option<u8>,partial:Option<Vec<u8>>,payload:Option<Vec<u8>>);

    fn fingerprint(&self) -> Result<serde_json::Value, String> {
        Ok(
            serde_json::json!({"options":self.options,"last_command":self.last_command,
            "partial_data":self.partial,"payload":self.payload}),
        )
    }
    fn validate(&self, shift: Shift) -> Result<(), String> {
        if shift.receive {
            return Err("command/data serial output has no receive wire".into());
        }
        match shift.bits {
            16 if shift.word >> 8 == u16::from(self.options.command_prefix)
                && self.partial.is_none() =>
            {
                Ok(())
            }
            8 if shift.word <= 255
                && (self.partial.is_some()
                    || shift.word == u16::from(self.options.data_prefix)) =>
            {
                Ok(())
            }
            _ => Err("unsupported command/data serial output frame".into()),
        }
    }
    fn transfer(&mut self, _: &mut Cx<'_>, shift: Shift, _: u64) -> Result<u16, String> {
        self.validate(shift)?;
        if shift.bits == 16 {
            self.last_command = Some(shift.word as u8);
        } else if let Some(partial) = &mut self.partial {
            partial.push(shift.word as u8);
            if partial.len() == self.options.payload_bytes {
                self.payload = self.partial.take();
            }
        } else {
            self.partial = Some(Vec::with_capacity(self.options.payload_bytes));
        }
        // Transmit-only controllers discard this value; receive preflight
        // refuses, so it cannot become a synthetic MISO response.
        Ok(0)
    }
    fn state(&self) -> serde_json::Value {
        serde_json::json!({"last_command":self.last_command,
            "partial_data":self.partial,"payload":self.payload})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::test_host::TestHost;

    #[test]
    fn binary_keyframe_resumes_a_partial_physical_packet() {
        let options = || Options {
            command_prefix: 0x82,
            data_prefix: 0xaa,
            payload_bytes: 2,
        };
        let mut part = CommandDataOutput::new(options()).unwrap();
        let mut host = TestHost::new();
        for (bits, word) in [(16, 0x8241), (8, 0xaa), (8, 0x12)] {
            part.transfer(
                &mut host.cx(),
                Shift {
                    word,
                    bits,
                    receive: false,
                },
                0,
            )
            .unwrap();
        }
        let saved = part.checkpoint().unwrap();
        let before = part.fingerprint().unwrap();
        let mut restored = CommandDataOutput::new(options()).unwrap();
        restored.restore_checkpoint(&saved).unwrap();
        assert_eq!(restored.fingerprint().unwrap(), before);
        let last = Shift {
            word: 0x34,
            bits: 8,
            receive: false,
        };
        part.transfer(&mut host.cx(), last, 0).unwrap();
        restored.transfer(&mut host.cx(), last, 0).unwrap();
        assert_eq!(restored.fingerprint().unwrap(), part.fingerprint().unwrap());
        assert_eq!(restored.state()["payload"], serde_json::json!([0x12, 0x34]));
        assert!(host.irqs.is_empty());
    }
}
