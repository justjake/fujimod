//! Physical controls a body has no input model for. Key edges are refused by
//! the session; ladder samples read as the declared idle fallback.
use fujimod_emulator_devices::checkpoint::Checkpoint;
use fujimod_emulator_devices::parts::controls::Controls;
use serde_json::Value;

#[derive(Default)]
pub struct AbsentControls;

impl Controls for AbsentControls {
    fn advance(&self, _: u64) -> Vec<String> {
        Vec::new()
    }
    fn next_deadline(&self) -> Option<u64> {
        None
    }
    fn sample(&self, _: usize, fallback: u16) -> u16 {
        fallback
    }
    fn save(&self) -> Value {
        Value::Null
    }
    fn restore(&self, state: &Value) -> Result<(), String> {
        if state.is_null() {
            Ok(())
        } else {
            Err("absent controls checkpoint holds key state".into())
        }
    }
    fn checkpoint(&self) -> Result<Checkpoint, String> {
        Checkpoint::encode("physical-controls", &())
    }
    fn validate_checkpoint(&self, saved: &Checkpoint) -> Result<(), String> {
        saved.decode::<()>("physical-controls")?;
        Ok(())
    }
    fn restore_checkpoint(&self, saved: &Checkpoint) -> Result<(), String> {
        self.validate_checkpoint(saved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ladders_read_their_idle_fallback_and_no_edges_exist() {
        let controls = AbsentControls;
        assert_eq!(controls.sample(3, 4095), 4095);
        assert_eq!(controls.next_deadline(), None);
        assert!(controls.advance(u64::MAX).is_empty());
        assert!(controls.restore(&serde_json::json!({"held": []})).is_err());
        assert!(controls.restore(&controls.save()).is_ok());
        let checkpoint = controls.checkpoint().unwrap();
        controls.validate_checkpoint(&checkpoint).unwrap();
        controls.restore_checkpoint(&checkpoint).unwrap();
    }
}
