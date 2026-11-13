//! The targets on the wires of one I2C controller, as a controller model sees
//! them: an address byte after a START or repeated START, then bytes written
//! or read until a STOP. Controller register models own timing, interrupts and
//! status; a bus owns which parts answer and what they reply.
use serde_json::Value;

/// Targets on the wires of one controller.
pub trait Bus: Send + 'static {
    /// The address byte of a START or repeated START; `false` is a NACK.
    fn start(&mut self, address: u16, read: bool) -> bool;
    /// A byte the controller transmitted to the addressed target. An error is
    /// a write the attached part does not model.
    fn write(&mut self, byte: u8) -> Result<(), String>;
    fn read(&mut self) -> Result<u8, String>;
    fn stop(&mut self);
    fn save(&self) -> Value;
    fn restore(&mut self, value: &Value) -> Result<(), String>;
    /// Complete typed target state; targets without one refuse.
    fn checkpoint(&self) -> Result<crate::checkpoint::Checkpoint, String> {
        Err("I2C bus has no exact checkpoint implementation".into())
    }
    fn validate_checkpoint(&self, _: &crate::checkpoint::Checkpoint) -> Result<(), String> {
        Err("I2C bus has no exact checkpoint implementation".into())
    }
    fn restore_checkpoint(&mut self, _: &crate::checkpoint::Checkpoint) -> Result<(), String> {
        Err("I2C bus has no exact checkpoint implementation".into())
    }
}

/// Change one declared register value supplied by an attached I2C part.
///
/// Host input APIs use this to model external register changes; firmware still
/// observes the value only through ordinary I2C reads.
pub trait RegisterInput {
    fn set_register_input(&mut self, address: u16, register: u8, value: u16) -> Result<(), String>;
}
