use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The part of a native boot that a GUI host must reproduce after it opens.
#[derive(Clone, Debug, Default)]
pub struct GuiOptions {
    pub load_snapshot: Option<std::path::PathBuf>,
    pub discovery: bool,
    pub execution_policy: fujimod_emulator_cpu::ExecutionPolicy,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
pub(crate) enum Request {
    Rpc {
        serial: u64,
        action: String,
        #[serde(default)]
        args: Value,
    },
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub(crate) enum Event {
    Reply { serial: u64, result: Value },
    Error {
        serial: Option<u64>,
        error: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        fault: Option<Value>,
    },
    Catalog {
        configuration: String,
        body: String,
        model: String,
        firmware: String,
        /// Catalog control document used to lay out the body's physical controls.
        controls: Value,
    },
    State {
        inputs: Vec<fujimod_emulator_session::contact_driver::ControlInput>,
        positions: std::collections::BTreeMap<String, String>,
        #[serde(rename = "poweredOn")]
        powered_on: bool,
    },
}

impl Event {
    pub(crate) fn reply(serial: u64, result: Value) -> Self {
        Self::Reply { serial, result }
    }

    pub(crate) fn error(serial: Option<u64>, error: impl Into<String>) -> Self {
        Self::Error {
            serial,
            error: error.into(),
            fault: None,
        }
    }

    pub(crate) fn json(&self) -> Result<String, String> {
        serde_json::to_string(self).map_err(|error| error.to_string())
    }
}

pub(crate) fn command_args(args: Value) -> Result<Value, String> {
    if args.is_null() {
        Ok(json!({}))
    } else if args.is_object() {
        Ok(args)
    } else {
        Err("command arguments must be an object".into())
    }
}
