//! What the engine's A-profile runner calls on an Arm generic interrupt
//! controller. The controller is a model; board builders construct it.
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The Security states a GIC implements, fixed by the board declaration.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SecurityStates {
    /// A single Security state (GICD_CTLR.DS=1).
    #[default]
    One,
    /// Secure and Non-secure states (GICD_CTLR.DS=0).
    Two,
}

/// The Security state a processor presents to its GIC CPU interface: whether
/// it executes at EL3 and the value of SCR_EL3.NS. The processor framework
/// supplies it from architectural CPU state; the controller never reads CPU
/// registers itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessorSecurity {
    pub el3: bool,
    pub non_secure: bool,
}
impl ProcessorSecurity {
    /// The architectural reset state: EL3 with SCR_EL3.NS clear.
    pub const RESET: Self = Self {
        el3: true,
        non_secure: false,
    };
}

/// An Arm GIC: distributor, redistributors and CPU interfaces.
pub trait InterruptController: Send {
    /// The (IRQ, FIQ) outputs to one core. Inspection never acknowledges.
    fn pins(&self, core: usize) -> Result<(bool, bool), String>;
    /// Whether an SGI is latched for `core`, regardless of enable or priority (a wake
    /// request for a core parked before its own CPU interface runs). Default: never.
    fn sgi_pending(&self, _core: usize) -> Result<bool, String> {
        Ok(false)
    }
    /// The interrupt `core`'s CPU interface would signal now, or None.
    fn eligible(&self, core: usize) -> Result<Option<usize>, String>;
    fn set_spi(&mut self, id: usize, high: bool) -> Result<(), String>;
    /// Latch an edge on shared interrupt `id`: it pends whatever the line's
    /// configuration, and stays pending until acknowledged. Default: the
    /// line rises and falls.
    fn pulse_spi(&mut self, id: usize) -> Result<(), String> {
        self.set_spi(id, true)?;
        self.set_spi(id, false)
    }
    fn set_ppi(&mut self, core: usize, id: usize, high: bool) -> Result<(), String>;
    /// A guest CPU interface system-register access that trapped.
    fn system(&mut self, core: usize, register: u16, value: Option<u64>) -> Result<u64, String>;
    /// Whether the CPU interfaces depend on each processor's Security state,
    /// so the processor framework must deliver it with
    /// `set_processor_security` before pins or system registers are used.
    fn tracks_processor_security(&self) -> bool {
        false
    }
    /// Deliver one processor's current Security state.
    fn set_processor_security(
        &mut self,
        _core: usize,
        _security: ProcessorSecurity,
    ) -> Result<(), String> {
        Err("This interrupt controller has one Security state".into())
    }
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
