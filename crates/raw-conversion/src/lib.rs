//! Tethered RAW conversion: a Fujifilm X-Trans IV/V body in USB RAW CONV.
//! mode develops an uploaded RAF with its own processor and returns the JPEG.
pub mod card;
pub mod jpeg;
pub mod profile;
pub mod ptp;
pub mod run;

#[cfg(test)]
mod fake_camera;
