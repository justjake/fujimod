//! EXR II face detector and face results.
pub mod construct;
pub mod face_detector;
pub mod face_results;
pub use face_detector::FaceDetector;

fn valid_image_source_address(address: u32) -> bool {
    address & 3 == 0 && (0x40000000..0x60000000).contains(&address)
}
