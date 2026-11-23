//! Physical controls attached to a controller. No program-inspection API.
use serde_json::Value;

pub trait Controls: Send + Sync {
    /// Apply explicit physical edges through this guest virtual nanosecond.
    /// Returned lines describe those edges; logging never affects the model.
    fn advance(&self, now: u64) -> Vec<String>;
    /// Absolute guest tick of the next explicit physical edge.
    fn next_deadline(&self) -> Option<u64>;
    /// Observe the current resistor ladder; host inspection never advances it.
    fn sample(&self, channel: usize, fallback: u16) -> u16;
    fn save(&self) -> Value;
    fn restore(&self, state: &Value) -> Result<(), String>;
    /// Exact future-affecting state, including held keys and queued edges.
    /// Diagnostic `save()` is not a restore source.
    fn checkpoint(&self) -> Result<crate::checkpoint::Checkpoint, String> {
        Err("physical controls have no exact checkpoint implementation".into())
    }
    fn validate_checkpoint(
        &self,
        _: &crate::checkpoint::Checkpoint,
    ) -> Result<(), String> {
        Err("physical controls have no exact checkpoint implementation".into())
    }
    fn restore_checkpoint(&self, _: &crate::checkpoint::Checkpoint) -> Result<(), String> {
        Err("physical controls have no exact checkpoint implementation".into())
    }
}
