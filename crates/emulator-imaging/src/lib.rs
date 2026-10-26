//! The image interface: scene, sensor and image frames and their pixels, the handles and
//! option and spec types plans name, image layout, exposure and optics declarations, film,
//! and the EXR II ISP interface the image models share.
pub mod analysis;
pub mod exposure;
pub mod exr2_isp;
pub mod film;
pub mod frame;
pub mod optics;
pub mod packed_color;
pub mod radiometric;
pub mod scene;
pub mod timing;
