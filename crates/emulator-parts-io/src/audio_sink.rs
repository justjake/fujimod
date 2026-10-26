//! Explicit external serial-audio sink for DMA byte streams.
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    #[serde(default)]
    pub connected: bool,
}

#[derive(Clone, Debug, Default, Deserialize, serde::Serialize)]
struct State {
    connected: bool,
    bytes_received: u64,
}

/// A virtual sink that counts accepted bytes without interpreting audio samples.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct AudioSink {
    state: State,
}

impl AudioSink {
    pub fn new(options: Options) -> Self {
        Self {
            state: State {
                connected: options.connected,
                bytes_received: 0,
            },
        }
    }

    pub fn connected(&self) -> bool {
        self.state.connected
    }

    pub fn set_connected(&mut self, connected: bool) {
        self.state.connected = connected;
    }

    pub fn receive_dma(&mut self, bytes: &[u8]) -> Result<(), String> {
        if !self.state.connected {
            return Err("serial-audio sink is disconnected".into());
        }
        self.state.bytes_received = self
            .state
            .bytes_received
            .checked_add(bytes.len() as u64)
            .ok_or("serial-audio byte counter overflow")?;
        Ok(())
    }

    pub fn bytes_received(&self) -> u64 {
        self.state.bytes_received
    }

    pub fn save(&self) -> Value {
        json!(self.state)
    }

    pub fn restore(&mut self, value: &Value) -> Result<(), String> {
        let state: State = serde_json::from_value(value.clone())
            .map_err(|error| format!("invalid serial-audio sink checkpoint: {error}"))?;
        self.state = state;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_opaque_bytes_only_when_explicitly_connected() {
        let mut sink = AudioSink::new(Options { connected: true });
        sink.receive_dma(&[0x80, 0x00, 0x7f]).unwrap();
        assert_eq!(sink.bytes_received(), 3);

        let mut restored = AudioSink::new(Options { connected: true });
        restored.restore(&sink.save()).unwrap();
        assert_eq!(restored.bytes_received(), 3);

        let mut disconnected = AudioSink::default();
        assert!(disconnected.receive_dma(&[1]).is_err());
    }
}
