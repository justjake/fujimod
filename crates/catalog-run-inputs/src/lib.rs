//! Run inputs a run configuration selects: execution options and device profiles, native
//! device inputs, firmware inputs and the bundled hardware-input profiles. None of them names a
//! reference catalog record, so they build beside the reference catalog.
pub mod hardware_inputs;
pub use hardware_inputs::{HARDWARE_INPUTS_SCHEMA_URI, HardwareInputs, HardwareReset};
mod firmware_inputs;
pub use firmware_inputs::*;
mod native_inputs;
pub use native_inputs::*;
mod options;
pub use options::*;

include!(concat!(env!("OUT_DIR"), "/bundled_hardware_inputs.rs"));
