//! Bus scheduling and discovery data, with device images retained as children
//! so archive blob sharing covers the complete device graph.
use super::*;
use fujimod_emulator_devices::checkpoint::Checkpoint as Encoded;
const KIND: &str = "board-bus-v1";

impl Checkpoint {
    pub(in crate::board) fn export(&self) -> Result<Encoded, String> {
        Ok(Encoded::encode(KIND, self)?.with_children(self.devices.clone()))
    }
}
impl Bus {
    pub(in crate::board) fn import_checkpoint(
        &self,
        encoded: &Encoded,
    ) -> Result<Checkpoint, String> {
        let mut saved: Checkpoint = encoded.decode(KIND)?;
        if saved.mode == Mode::Strict
            && (saved.continue_refusals
                || !saved.shadow.is_empty()
                || !saved.skipped_deadlines.is_empty())
        {
            return Err("strict snapshot carries discovery continuation state".into());
        }
        if !encoded.blobs().is_empty() {
            return Err("bus snapshot has unexpected blobs".into());
        }
        saved.devices = encoded.children().to_vec();
        self.validate_checkpoint(&saved)?;
        let count = self.len();
        if saved.dirty.len() != count
            || saved.visited.len() != count
            || saved
                .touched
                .iter()
                .chain(&saved.wake_queue)
                .any(|id| *id >= count)
            || saved.skipped_deadlines.keys().any(|id| *id >= count)
            || saved
                .unknown
                .keys()
                .any(|site| site.device.is_some_and(|id| id >= count))
            || saved.transactions.as_ref().is_some_and(|t| {
                t.rows.len() > t.limit
                    || t.ranges
                        .as_ref()
                        .is_some_and(|ranges| ranges.iter().any(|r| r.begin >= r.end))
            })
        {
            return Err("bus snapshot worklist or observer state differs".into());
        }
        Ok(saved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strict_archives_cannot_enable_discovery_continuation_or_shadow_hardware() {
        let strict = Bus::new(Mode::Strict);
        for field in 0..3 {
            let mut saved = strict.checkpoint(None).unwrap();
            match field {
                0 => saved.continue_refusals = true,
                1 => {
                    saved.shadow.insert(0x1234, 42);
                }
                _ => {
                    saved.skipped_deadlines.insert(0, 1);
                }
            }
            assert!(
                strict
                    .import_checkpoint(&saved.export().unwrap())
                    .err()
                    .unwrap()
                    .contains("discovery continuation")
            );
        }
        let mut discovery = Bus::new(Mode::Discovery);
        discovery.continue_refusals(true).unwrap();
        let saved = discovery.checkpoint(None).unwrap().export().unwrap();
        assert!(strict.import_checkpoint(&saved).is_err());
        assert!(Bus::new(Mode::Discovery).import_checkpoint(&saved).is_ok());
    }
}
