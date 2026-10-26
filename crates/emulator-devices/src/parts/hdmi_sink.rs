//! Connected HDMI sink input and its explicitly supplied EDID bytes.
use serde::Deserialize;
use serde_json::{Value, json};
use crate::{Cx, Device};

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    #[serde(default)]
    pub edid: Vec<u8>,
    #[serde(default)]
    pub connected: bool,
}

#[derive(Clone, Debug, Default, Deserialize, serde::Serialize)]
struct State {
    edid: Vec<u8>,
    connected: bool,
}

/// A cable-state input and a bounded source of EDID blocks.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct HdmiSink {
    state: State,
}

impl HdmiSink {
    pub fn new(options: Options) -> Result<Self, String> {
        if !options.edid.is_empty() && (options.edid.len() % 128 != 0 || options.edid.len() > 512) {
            return Err("HDMI EDID must contain one to four whole 128-byte blocks".into());
        }
        if options.connected && options.edid.is_empty() {
            return Err("connected HDMI sink requires explicit EDID bytes".into());
        }
        Ok(Self {
            state: State {
                edid: options.edid,
                connected: options.connected,
            },
        })
    }

    pub fn connected(&self) -> bool {
        self.state.connected
    }

    pub fn set_connected(&mut self, connected: bool) {
        self.state.connected = connected;
    }

    /// Read one 128-byte block using the segment and offset registers.
    pub fn read_block(&self, segment: u32, offset: u32) -> Result<[u8; 128], String> {
        if !self.state.connected {
            return Err("HDMI sink is disconnected".into());
        }
        if segment >= 2 || !matches!(offset, 0 | 0x80) {
            return Err("unsupported HDMI EDID segment or offset".into());
        }
        let start = (segment * 256 + offset) as usize;
        let Some(bytes) = self.state.edid.get(start..start + 128) else {
            return Err("HDMI EDID block lies outside supplied data".into());
        };
        let mut block = [0; 128];
        block.copy_from_slice(bytes);
        Ok(block)
    }

    pub fn save(&self) -> Value {
        json!({"edid": self.state.edid, "connected": self.state.connected})
    }

    pub fn restore(&mut self, value: &Value) -> Result<(), String> {
        let state: State = serde_json::from_value(value.clone())
            .map_err(|error| format!("invalid HDMI sink state: {error}"))?;
        if state.edid != self.state.edid
            || !state.edid.is_empty() && (state.edid.len() % 128 != 0 || state.edid.len() > 512)
        {
            return Err("HDMI sink EDID differs from checkpoint".into());
        }
        if state.connected && state.edid.is_empty() {
            return Err("connected HDMI sink has no EDID".into());
        }
        self.state.connected = state.connected;
        Ok(())
    }
}

/// Explicit external sink input node. Its options describe cable presence and
/// EDID bytes; the transmitter receives the same immutable input through the
/// board graph.
pub struct Input {
    sink: HdmiSink,
}

impl Input {
    pub fn new(options: Options) -> Result<Self, String> {
        Ok(Self {
            sink: HdmiSink::new(options)?,
        })
    }

    pub fn sink(&self) -> HdmiSink {
        self.sink.clone()
    }

    pub fn set_connected(&mut self, connected: bool) {
        self.sink.set_connected(connected);
    }
}

impl Device for Input {
    crate::checkpoint_fields!(sink: HdmiSink);

    fn name(&self) -> &'static str {
        "hdmi-sink-input"
    }

    fn fingerprint(&self) -> Result<Value, String> {
        Ok(self.sink.save())
    }

    fn read(&mut self, cx: &mut Cx<'_>, _: u64, _: u8) -> u64 {
        cx.unmodeled();
        0
    }

    fn write(&mut self, cx: &mut Cx<'_>, _: u64, _: u8, _: u64) {
        cx.unmodeled();
    }

    fn save(&self) -> Value {
        self.sink.save()
    }

    fn restore(&mut self, value: &Value) -> Result<(), String> {
        self.sink.restore(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_only_supplied_connected_edid_blocks() {
        let edid = (0..256).map(|value| value as u8).collect::<Vec<_>>();
        let sink = HdmiSink::new(Options {
            edid: edid.clone(),
            connected: true,
        })
        .unwrap();
        assert_eq!(sink.read_block(0, 0x80).unwrap()[0], 128);
        assert!(sink.read_block(2, 0).is_err());
        assert!(sink.read_block(0, 0x40).is_err());

        let mut disconnected = sink.clone();
        disconnected.set_connected(false);
        assert!(disconnected.read_block(0, 0).is_err());
        let mut restored = HdmiSink::new(Options {
            edid,
            connected: false,
        })
        .unwrap();
        restored.restore(&sink.save()).unwrap();
        assert!(restored.connected());
    }

    #[test]
    fn rejects_missing_or_partial_edid_for_connected_sink() {
        assert!(
            HdmiSink::new(Options {
                edid: Vec::new(),
                connected: true,
            })
            .is_err()
        );
        assert!(
            HdmiSink::new(Options {
                edid: vec![0; 127],
                connected: false,
            })
            .is_err()
        );
    }
}
