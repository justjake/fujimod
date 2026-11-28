//! Inputs a camera starts with: the reset inputs and bus facts a body declares, the start
//! entries a firmware declares, execution options and device profiles, native device inputs and
//! firmware inputs. None of them names a reference catalog record, so they build beside the
//! reference catalog.
pub mod application;
pub mod camera;
pub use camera::{Actions, CameraDescription};
pub mod hardware_inputs;
pub use hardware_inputs::HardwareReset;
mod firmware_inputs;
pub use firmware_inputs::*;
mod native_inputs;
pub use native_inputs::*;
mod options;
pub use options::*;
mod reset;
pub use reset::*;
