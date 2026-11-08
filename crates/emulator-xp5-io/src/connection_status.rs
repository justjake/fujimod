//! External cable connection level read as bit 0 of one status word.
//!
//! The mechanism is the same for every connector: the guest reads a word whose
//! bit 0 follows one external line, and the run decides whether a cable is
//! attached. Which level that line takes while a cable is attached is a board
//! fact, so the caller declares it with the window.
use fujimod_emulator_devices::{Cx, Device, checkpoint::Checkpoint};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct State {
    base: u64,
    connected: bool,
}

pub struct ConnectionStatus {
    name: &'static str,
    connected_level: bool,
    state: State,
}

impl ConnectionStatus {
    /// A status word that starts disconnected. `connected_level` is the value of
    /// bit 0 while a cable is attached; the other level is read without one.
    pub fn new(
        name: &'static str,
        base: u64,
        size: u64,
        connected_level: bool,
    ) -> Result<Self, String> {
        if base & 3 != 0 || size != 4 {
            return Err(format!("invalid {name} window"));
        }
        Ok(Self {
            name,
            connected_level,
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

    fn level(&self) -> bool {
        self.state.connected == self.connected_level
    }

    fn valid_read(&self, address: u64, size: u8) -> bool {
        address == self.state.base && address & 3 == 0 && size == 4
    }

    fn validate_state(&self, state: &State) -> Result<(), String> {
        if state.base == self.state.base {
            Ok(())
        } else {
            Err(format!("{} checkpoint window differs", self.name))
        }
    }
}

impl Device for ConnectionStatus {
    fn name(&self) -> &'static str {
        self.name
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
            "connected_level": self.connected_level,
        }))
    }

    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        if !self.valid_read(address, size) {
            cx.unmodeled();
            return 0;
        }
        u64::from(self.level())
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
            .map_err(|error| format!("invalid {} state: {error}", self.name))?;
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
    fn active_high_line_defaults_disconnected_and_is_checkpointed() {
        let mut host = RecordingHost::default();
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        let mut status =
            ConnectionStatus::new("usb-connection-status", 0xff90e300, 4, true).unwrap();

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

    #[test]
    fn active_low_line_reads_one_without_a_cable() {
        let mut host = RecordingHost::default();
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        let mut status =
            ConnectionStatus::new("hdmi-hot-plug-status", 0xff90e200, 4, false).unwrap();

        assert_eq!(status.name(), "hdmi-hot-plug-status");
        assert_eq!(status.read(&mut cx, 0xff90e200, 4), 1);
        status.set_connected(true);
        assert_eq!(status.read(&mut cx, 0xff90e200, 4), 0);
        assert_eq!(cx.refusal(), None);

        let checkpoint = status.checkpoint().unwrap();
        let usb = ConnectionStatus::new("usb-connection-status", 0xff90e200, 4, true).unwrap();
        assert!(usb.validate_checkpoint(&checkpoint).is_err());
    }

    #[test]
    fn only_aligned_word_reads_of_the_status_word_are_modeled() {
        let mut host = RecordingHost::default();
        let mut status =
            ConnectionStatus::new("hdmi-hot-plug-status", 0xff90e200, 4, false).unwrap();
        assert!(ConnectionStatus::new("hdmi-hot-plug-status", 0xff90e202, 4, false).is_err());
        assert!(ConnectionStatus::new("hdmi-hot-plug-status", 0xff90e200, 8, false).is_err());

        let mut cx = Cx::new(&mut host, 0, 0, 0);
        status.read(&mut cx, 0xff90e200, 2);
        assert!(cx.is_unmodeled());
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        status.write(&mut cx, 0xff90e200, 4, 0);
        assert!(cx.is_unmodeled());
        assert!(status.pollable(0xff90e200, 4));
        assert!(!status.pollable(0xff90e200, 1));
    }
}
