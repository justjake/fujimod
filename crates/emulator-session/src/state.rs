//! Complete stopped-session state for direct comparison. Host observations and
//! rendering caches are excluded; definition, controls, contact automation,
//! board and external resources are retained verbatim.
use super::Session;
use fujimod_emulator::board::BoardState;
use fujimod_emulator_exr2_resources::ResourceSnapshot;
use serde_json::Value;

#[derive(Clone, PartialEq)]
pub struct SessionState {
    pub definition: Value,
    pub controls: Option<Value>,
    pub contact_driver: Option<Value>,
    pub board: BoardState,
    pub resources: ResourceSnapshot,
}

impl std::fmt::Debug for SessionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("session state")
    }
}

impl SessionState {
    /// First structural difference between two complete session states.
    pub fn diff(&self, other: &Self) -> Option<String> {
        if let Some(message) = diff_value(&self.definition, &other.definition, "definition") {
            return Some(message);
        }
        if let Some(message) = diff_optional(&self.controls, &other.controls, "controls") {
            return Some(message);
        }
        if let Some(message) = diff_optional(
            &self.contact_driver,
            &other.contact_driver,
            "contact_driver",
        ) {
            return Some(message);
        }
        if let Some(message) = self.board.diff(&other.board) {
            return Some(format!("board/{message}"));
        }
        self.resources.diff(&other.resources)
    }

    /// Canonical typed encoding for clients that compare states as bytes.
    pub fn to_checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        use fujimod_emulator_devices::checkpoint::Checkpoint;
        let mut children = vec![
            Checkpoint::encode("definition", &self.definition)?,
            self.board.to_checkpoint()?,
            self.resources.to_checkpoint()?,
        ];
        if let Some(controls) = &self.controls {
            children.push(Checkpoint::encode("controls", controls)?);
        }
        if let Some(contact) = &self.contact_driver {
            children.push(Checkpoint::encode("contact-driver", contact)?);
        }
        Ok(Checkpoint::encode("session-state", &())?.with_children(children))
    }

    /// Canonical bytes for a client-side direct equality check.
    pub fn to_bytes(&self) -> Result<Vec<u8>, String> {
        let mut bytes = Vec::new();
        ciborium::into_writer(&self.to_checkpoint()?, &mut bytes)
            .map_err(|error| error.to_string())?;
        Ok(bytes)
    }

    /// Readable direct-state JSON with base64 byte leaves.
    pub fn to_json(&self) -> Result<Value, String> {
        Ok(serde_json::json!({
            "schema":2,
            "definition":self.definition,
            "controls":self.controls,
            "contact_driver":self.contact_driver,
            "board":self.board.to_json(),
            "resources":self.resources.to_json()?,
        }))
    }
}

impl Session {
    /// Complete future-affecting state while every processor is stopped.
    pub fn complete_state(&self) -> Result<SessionState, String> {
        Ok(SessionState {
            definition: self.definition.clone(),
            controls: self.controls.as_ref().map(|panel| {
                serde_json::json!({"spec":panel.spec(),"positions":panel.positions()})
            }),
            contact_driver: self
                .contact_driver
                .as_ref()
                .map(|driver| serde_json::to_value(driver).map_err(|error| error.to_string()))
                .transpose()?,
            board: self.board.complete_state()?,
            resources: self.resources.checkpoint()?,
        })
    }
}

fn diff_optional(left: &Option<Value>, right: &Option<Value>, path: &str) -> Option<String> {
    match (left, right) {
        (Some(a), Some(b)) => diff_value(a, b, path),
        (None, None) => None,
        (Some(_), None) => Some(format!("{path}: only in left state")),
        (None, Some(_)) => Some(format!("{path}: only in right state")),
    }
}

fn diff_value(left: &Value, right: &Value, path: &str) -> Option<String> {
    match (left, right) {
        (Value::Object(a), Value::Object(b)) => {
            for key in a.keys().chain(b.keys()) {
                match (a.get(key), b.get(key)) {
                    (Some(x), Some(y)) => {
                        if let Some(message) = diff_value(x, y, &format!("{path}.{key}")) {
                            return Some(message);
                        }
                    }
                    (Some(_), None) => return Some(format!("{path}.{key}: only in left state")),
                    (None, Some(_)) => return Some(format!("{path}.{key}: only in right state")),
                    (None, None) => {}
                }
            }
            None
        }
        (Value::Array(a), Value::Array(b)) => {
            if a.len() != b.len() {
                return Some(format!("{path}: length {} differs from {}", a.len(), b.len()));
            }
            for (index, (x, y)) in a.iter().zip(b).enumerate() {
                if let Some(message) = diff_value(x, y, &format!("{path}[{index}]")) {
                    return Some(message);
                }
            }
            None
        }
        _ if left == right => None,
        _ => Some(format!("{path}: {left} differs from {right}")),
    }
}
