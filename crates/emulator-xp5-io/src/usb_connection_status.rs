//! External USB cable/jig connection status input.
use fujimod_emulator_devices::{Cx, Device, checkpoint::Checkpoint};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct State {
    base: u64,
    connected: bool,
}

pub struct UsbConnectionStatus {
    state: State,
}

impl UsbConnectionStatus {
    pub fn new(base: u64, size: u64) -> Result<Self, String> {
        if base & 3 != 0 || size != 4 {
            return Err("invalid USB connection-status window".into());
        }
        Ok(Self {
            state: State {
                base,
                connected: false,
            },
        })
    }

    pub fn connected(&self) -> bool {
        self.state.connected
    }

    pub fn set_connected(&mut self, connected: bool) {
        self.state.connected = connected;
    }

    fn valid_read(&self, address: u64, size: u8) -> bool {
        address == self.state.base && address & 3 == 0 && size == 4
    }

    fn validate_state(&self, state: &State) -> Result<(), String> {
        if state.base == self.state.base {
            Ok(())
        } else {
            Err("USB connection-status checkpoint window differs".into())
        }
    }
}

impl Device for UsbConnectionStatus {
    fn name(&self) -> &'static str {
        "usb-connection-status"
    }

    fn checkpoint(&self) -> Result<Checkpoint, String> {
        Checkpoint::encode(self.name(), &self.state)
    }

    fn validate_checkpoint(&self, checkpoint: &Checkpoint) -> Result<(), String> {
        let state: State = checkpoint.decode(self.name())?;
        self.validate_state(&state)
    }

    fn restore_checkpoint(&mut self, checkpoint: &Checkpoint) -> Result<(), String> {
        let state: State = checkpoint.decode(self.name())?;
        self.validate_state(&state)?;
        self.state = state;
        Ok(())
    }

    fn fingerprint(&self) -> Result<Value, String> {
        Ok(json!({
            "base": self.state.base,
            "connected": self.state.connected,
        }))
    }

    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        if !self.valid_read(address, size) {
            cx.unmodeled();
            return 0;
        }
        u64::from(self.state.connected)
    }

    fn write(&mut self, cx: &mut Cx<'_>, _: u64, _: u8, _: u64) {
        cx.unmodeled();
    }

    fn pollable(&self, address: u64, size: u8) -> bool {
        self.valid_read(address, size)
    }

    fn save(&self) -> Value {
        serde_json::to_value(&self.state).unwrap()
    }

    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let state: State = serde_json::from_value(value.clone())
            .map_err(|error| format!("invalid USB connection-status state: {error}"))?;
        self.validate_state(&state)?;
        self.state = state;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::test_host::recording::RecordingHost;

    #[test]
    fn external_connection_defaults_disconnected_and_is_checkpointed() {
        let mut host = RecordingHost::default();
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        let mut status = UsbConnectionStatus::new(0xff90e300, 4).unwrap();

        assert_eq!(status.read(&mut cx, 0xff90e300, 4), 0);
        assert!(!status.connected());
        assert_eq!(cx.refusal(), None);

        status.set_connected(true);
        assert_eq!(status.read(&mut cx, 0xff90e300, 4), 1);
        let checkpoint = status.checkpoint().unwrap();

        status.set_connected(false);
        assert_eq!(status.read(&mut cx, 0xff90e300, 4), 0);
        status.restore_checkpoint(&checkpoint).unwrap();
        assert_eq!(status.read(&mut cx, 0xff90e300, 4), 1);
        assert_eq!(cx.refusal(), None);
    }
}
