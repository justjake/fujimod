//! Image processing over the image interface: integer analysis laws, RAW development,
//! resampling and separable filters, RAF reading, the scene's optical blur and clocked
//! frame source, and the EXR II ISP parameter bank.
pub mod analysis;
pub mod parameters;
pub mod raw;
pub mod resample;
pub mod scene;
pub mod separable;
pub mod yuv;

use fujimod_emulator_image_formation::{exposure::ExposedFrame, radiometric::Image};
use fujimod_emulator_imaging::scene::frames::SensorDecoders;

/// Decoders of checkpointed sensor images and exposures and of RAF files.
pub fn sensor_decoders() -> SensorDecoders {
    SensorDecoders {
        image: Image::decode,
        exposure: ExposedFrame::decode,
        raf: scene::raf::read_frame,
    }
}
