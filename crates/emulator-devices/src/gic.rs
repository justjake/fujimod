//! What the engine's A-profile runner calls on an Arm generic interrupt
//! controller. The controller is a model; board builders construct it.
use serde_json::Value;

/// An Arm GIC: distributor, redistributors and CPU interfaces.
pub trait InterruptController: Send {
    /// The (IRQ, FIQ) outputs to one core. Inspection never acknowledges.
    fn pins(&self, core: usize) -> Result<(bool, bool), String>;
    fn set_spi(&mut self, id: usize, high: bool) -> Result<(), String>;
    fn set_ppi(&mut self, core: usize, id: usize, high: bool) -> Result<(), String>;
    /// A guest CPU interface system-register access that trapped.
    fn system(&mut self, core: usize, register: u16, value: Option<u64>) -> Result<u64, String>;
    fn distributor(
        &mut self,
        core: usize,
        offset: u64,
        size: u8,
        value: Option<u64>,
    ) -> Result<u64, String>;
    /// GICv2 banked CPU interface. GICv3 instead uses system registers.
    fn cpu_interface(
        &mut self,
        _core: usize,
        _offset: u64,
        _size: u8,
        _value: Option<u64>,
    ) -> Result<u64, String> {
        Err("This interrupt controller has no MMIO CPU interface".into())
    }
    fn redistributor(
        &mut self,
        core: usize,
        offset: u64,
        size: u8,
        value: Option<u64>,
    ) -> Result<u64, String>;
    /// The controller's architectural state, as checkpoints record it.
    fn save(&self) -> Value;
    fn restore(&mut self, state: &Value) -> Result<(), String>;
    fn boxed_clone(&self) -> Box<dyn InterruptController>;
}
