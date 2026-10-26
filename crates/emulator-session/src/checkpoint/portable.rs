//! A durable session point combines board, shared wires and queued host input.
//! Receiving sessions supply the matching constructor and external attachments.
use super::*;
use fujimod_emulator_devices::checkpoint::Checkpoint as Encoded;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
const KIND: &str = "emulator-session-v2";
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Header<C, D> {
    definition: serde_json::Value,
    nodes: BTreeMap<String, usize>,
    contacts: Vec<String>,
    controls: C,
    contact_driver: D,
}
impl Snapshot {
    pub fn export(&self) -> Result<Vec<u8>, String> {
        self.export_checkpoint()?.to_bytes()
    }
    pub fn export_checkpoint(&self) -> Result<Encoded, String> {
        let header = Header {
            definition: self.definition.clone(),
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
