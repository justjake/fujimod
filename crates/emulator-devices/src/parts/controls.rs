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
}

/// A physical input queue is a non-MMIO source on the same device timeline.
/// Its host API enqueues edges; only a due guest event applies those edges.
pub struct Source(std::sync::Arc<dyn Controls>);
impl Source {
    pub fn new(controls: std::sync::Arc<dyn Controls>) -> Self {
        Self(controls)
    }
}
impl crate::Device for Source {
    fn name(&self) -> &'static str {
        "physical-controls"
    }
    fn read(&mut self, cx: &mut crate::Cx<'_>, _: u64, _: u8) -> u64 {
        cx.unmodeled();
        0
    }
    fn write(&mut self, cx: &mut crate::Cx<'_>, _: u64, _: u8, _: u64) {
        cx.unmodeled();
    }
    fn next_deadline(&self, _: u64) -> Option<u64> {
        self.0.next_deadline()
    }
    fn advance(&mut self, cx: &mut crate::Cx<'_>) {
        for line in self.0.advance(cx.now()) {
            cx.log(&line);
        }
    }
    fn save(&self) -> Value {
        self.0.save()
    }
    fn restore(&mut self, state: &Value) -> Result<(), String> {
        self.0.restore(state)
    }
}
