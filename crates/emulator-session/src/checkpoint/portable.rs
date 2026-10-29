//! A durable session point combines board, shared wires and queued host input.
//! Receiving sessions supply the matching constructor and external attachments.
use super::*;
use fujimod_emulator::board::keyframe::Keyframe;
use fujimod_emulator_devices::checkpoint::Checkpoint as Encoded;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
const KIND: &str = "emulator-session-v3";
#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Identities {
    firmware_images: Value,
    device_graph_sha256: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Header<C, D> {
    definition: serde_json::Value,
    identities: Identities,
    nodes: BTreeMap<String, usize>,
    contacts: Vec<String>,
    controls: C,
    contact_driver: D,
}
fn identities(definition: &Value) -> Result<Identities, String> {
    let inputs = definition.get("inputs").unwrap_or(&Value::Null);
    let firmware_images = json!({
        "update": inputs.get("update").cloned().unwrap_or(Value::Null),
        "boot_rom": inputs
            .get("boot_rom")
            .and_then(|image| image.get("content"))
            .cloned()
            .unwrap_or(Value::Null),
    });
    let device_graph = json!({
        "execution": definition.get("execution").cloned().unwrap_or(Value::Null),
        "devices": definition.get("devices").cloned().unwrap_or(Value::Null),
        "scene": definition.get("scene").cloned().unwrap_or(Value::Null),
    });
    let serialized = serde_json::to_vec(&device_graph).map_err(|error| error.to_string())?;
    let device_graph_sha256 = format!("{:x}", Sha256::digest(serialized));
    Ok(Identities {
        firmware_images,
        device_graph_sha256,
    })
}
impl Snapshot {
    /// Validate a portable checkpoint envelope and return its saved counters.
    /// Board and session compatibility is checked when it is imported.
    pub fn inspect(bytes: &[u8]) -> Result<(u64, Vec<u64>), String> {
        let encoded = Encoded::from_bytes(bytes)?;
        let header: Header<Value, Value> = encoded.decode(KIND)?;
        if !encoded.blobs().is_empty() || encoded.children().len() != 2 {
            return Err("snapshot session child/blob count differs".into());
        }
        let expected = identities(&header.definition)?;
        if expected != header.identities {
            return Err("snapshot firmware or device graph identity differs".into());
        }
        Keyframe::portable_metadata(&encoded.children()[0])
    }
    pub fn export(&self) -> Result<Vec<u8>, String> {
        self.export_checkpoint()?.to_bytes()
    }
    pub fn export_checkpoint(&self) -> Result<Encoded, String> {
        let header = Header {
            definition: self.definition.clone(),
            identities: identities(&self.definition)?,
            nodes: self.nodes.clone(),
            contacts: self.contacts.clone(),
            controls: &self.controls,
            contact_driver: &self.contact_driver,
        };
        Ok(Encoded::encode(KIND, &header)?
            .with_children(vec![self.board.export()?, self.resources.export()?]))
    }
}
impl Session {
    pub(super) fn validate_snapshot(&self, saved: &Snapshot) -> Result<(), String> {
        if saved.definition != self.definition
            || saved.nodes != self.nodes
            || saved.contacts != self.contacts.keys().cloned().collect::<Vec<_>>()
        {
            return Err("snapshot session definition or attachments differ".into());
        }
        match (&self.controls, &saved.controls) {
            (Some(current), Some(saved)) if current.spec() == saved.spec() => (),
            (None, None) => (),
            _ => return Err("snapshot physical contact circuits differ".into()),
        }
        match (&self.contact_driver, &saved.contact_driver) {
            (Some(current), Some(driver)) => current.validate_checkpoint(driver, saved.now())?,
            (None, None) => (),
            _ => return Err("snapshot host input driver differs".into()),
        }
        self.resources.validate_checkpoint(&saved.resources)
    }
    /// Decode and validate data for this stopped session. Applying the point is
    /// an explicit `restore` operation; import does not execute or drive inputs.
    pub fn import_snapshot(&self, bytes: &[u8]) -> Result<Snapshot, String> {
        let encoded = Encoded::from_bytes(bytes)?;
        self.import_checkpoint(&encoded)
    }
    pub fn import_checkpoint(&self, encoded: &Encoded) -> Result<Snapshot, String> {
        let header: Header<Option<SwitchPanel>, Option<super::super::contact_driver::Driver>> =
            encoded.decode(KIND)?;
        if !encoded.blobs().is_empty() || encoded.children().len() != 2 {
            return Err("snapshot session child/blob count differs".into());
        }
        if identities(&header.definition)? != header.identities
            || identities(&self.definition)? != header.identities
        {
            return Err("snapshot firmware or device graph identity differs".into());
        }
        let resources = self.resources.import_checkpoint(&encoded.children()[1])?;
        let board = self.board.import_keyframe(&encoded.children()[0])?;
        let saved = Snapshot {
            definition: header.definition,
            nodes: header.nodes,
            contacts: header.contacts,
            board,
            resources,
            controls: header.controls,
            contact_driver: header.contact_driver,
        };
        self.validate_snapshot(&saved)?;
        Ok(saved)
    }
}
