use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

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
    Reply {
        serial: u64,
        result: Value,
    },
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_names_one_command_and_its_arguments() {
        let request: Request =
            serde_json::from_str(r#"{"type":"rpc","serial":7,"action":"press","args":{"id":"shutter"}}"#)
                .unwrap();
        let Request::Rpc { serial, action, args } = request;
        assert_eq!((serial, action.as_str()), (7, "press"));
        assert_eq!(args["id"], "shutter");
        assert_eq!(args.clone(), command_args(args).unwrap());
    }

    #[test]
    fn a_reply_and_a_state_event_keep_their_wire_names() {
        let reply: Value = serde_json::from_str(
            &Event::reply(3, json!(null)).json().unwrap(),
        )
        .unwrap();
        assert_eq!(reply["type"], "reply");
        assert_eq!(reply["serial"], 3);
        let state: Value = serde_json::from_str(
            &Event::State {
                inputs: Vec::new(),
                positions: Default::default(),
                powered_on: true,
            }
            .json()
            .unwrap(),
        )
        .unwrap();
        assert_eq!(state["type"], "state");
        assert_eq!(state["poweredOn"], true);
    }
}
