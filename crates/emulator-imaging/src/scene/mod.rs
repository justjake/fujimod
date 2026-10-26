//! Declared external inputs. These laws do not inspect firmware state.

mod cadence;
pub mod clocked;
pub mod frames;
pub mod mosaic;
pub mod optics;
mod recorded;
pub mod view;

pub use cadence::{FrameCadence, FrameEdge, FramePhases};
pub use frames::{ImageFrames, StatisticsFrames, StatisticsSpan};
pub use recorded::{Recorded, RecordedSpan};

pub mod sensor_frames;
pub use sensor_frames::{Frame, FramePixels, SensorFrames, SensorFramesHandle};
