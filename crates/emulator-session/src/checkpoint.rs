//! A session keyframe owns the stopped machine and its external connections.
//! Restoring positions does not drive contacts again or emit duplicate edges.
use super::Session;
use fujimod_emulator::board::keyframe::Keyframe;
use fujimod_emulator_exr2_resources::ResourceSnapshot;
use fujimod_emulator_devices::parts::switch_panel::SwitchPanel;
mod portable;

pub struct Snapshot {
    definition: serde_json::Value,
    nodes: std::collections::BTreeMap<String, usize>,
    contacts: Vec<String>,
    pub(crate) board: Keyframe,
    resources: ResourceSnapshot,
    controls: Option<SwitchPanel>,
    contact_driver: Option<super::contact_driver::Driver>,
}
impl Snapshot {
    pub fn execution_policy(&self) -> fujimod_emulator_cpu::ExecutionPolicy {
        self.board.execution_policy()
    }
    pub fn observations(&self) -> &[(usize, Vec<u64>)] {
        self.board.observations()
    }
    pub fn failure(&self) -> Option<&str> {
        self.board.failure()
    }
    /// Conservative encoded-device cost, including shared blobs at every retention.
    pub fn encoded_device_bytes(&self) -> usize {
        self.board.encoded_device_bytes() + self.resources.encoded_bytes()
    }
    pub fn now(&self) -> u64 {
        self.board.now()
    }
    pub fn instruction_counts(&self) -> &[u64] {
        self.board.instruction_counts()
    }
}
impl Session {
    pub fn checkpoint(&self, previous: Option<&Snapshot>) -> Result<Snapshot, String> {
        let resources = self.resources.checkpoint()?;
        let board = self.board.checkpoint(previous.map(|p| &p.board))?;
        Ok(Snapshot {
            definition: self.definition.clone(),
            nodes: self.nodes.clone(),
            contacts: self.contacts.keys().cloned().collect(),
            board,
            resources,
            controls: self.controls.clone(),
            contact_driver: self.contact_driver.clone(),
        })
    }
    pub fn restore(&mut self, saved: &Snapshot) -> Result<(), String> {
        self.validate_snapshot(saved)?;
        self.board.restore_keyframe(&saved.board)?;
        self.resources.restore_checkpoint(&saved.resources)?;
        self.controls = saved.controls.clone();
        self.contact_driver = saved.contact_driver.clone();
        self.recording_frames.borrow_mut().clear();
        Ok(())
    }
}
