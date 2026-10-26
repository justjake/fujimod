//! Transport details below reference resolution. These are mechanisms, not actions.
use crate::{Capture, Input, LocalFuture, ReadSet, Result};
use fujimod_catalog_remote::Facts;

pub trait Backend {
    fn now(&self) -> u64;
    fn memory<'a>(
        &'a mut self,
        address: u64,
        size: usize,
        reads: &'a mut ReadSet,
    ) -> LocalFuture<'a, Vec<u8>>;
    fn input<'a>(&'a mut self, input: &'a Input, facts: &'a Facts) -> LocalFuture<'a, ()>;
    fn accepted(&mut self, input: &Input, facts: &Facts, reads: &mut ReadSet) -> Result<bool>;
    fn frame<'a>(
        &'a mut self,
        facts: &'a Facts,
        reads: &'a mut ReadSet,
    ) -> LocalFuture<'a, Capture>;
    /// One bounded continuation, returning at an observed change or the deadline.
    fn wait<'a>(&'a mut self, reads: &'a ReadSet, deadline: u64) -> LocalFuture<'a, ()>;
    fn control_position(&mut self, id: &str) -> Result<Option<String>>;
    /// The contact a relative turn pulses when the control's motion is two
    /// contacts (one per direction) rather than an encoder; None when the
    /// backend applies the turn as one input.
    fn turn_contact(&mut self, _input: &Input) -> Result<Option<String>> {
        Ok(None)
    }
    /// Press or release a contact named by `turn_contact`.
    fn contact(&mut self, actuator: &str, _pressed: bool) -> Result<()> {
        Err(format!("This transport has no contact {actuator}"))
    }
    /// Whether the firmware's scanner has accepted that contact edge.
    fn contact_accepted(
        &mut self,
        actuator: &str,
        _pressed: bool,
        _reads: &mut ReadSet,
    ) -> Result<bool> {
        Err(format!("This transport has no contact {actuator}"))
    }
}
