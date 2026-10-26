//! Options the EXR II plan names for the models in `fujimod-emulator-exr2-isp`.
//! They live here so a model edit does not rebuild every crate that plans boards.
pub mod block11000;
pub mod combination;
pub mod face_detector;
pub mod face_results;
pub mod input;
pub mod metering;
pub mod output;
pub mod phase_detection;
pub mod producer;
pub mod raw;
pub mod scaler;
pub mod still;
pub mod warp;
pub mod window_analysis;
