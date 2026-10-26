//! Python bindings for host image algorithms: virtual scenes, film stages, JPEG YYCbCr,
//! UI raster composites, mono rasters, method-4 decoding and RAF radiance. The `_native`
//! module registers them on `fujimod._native.emulator`.
use pyo3::prelude::*;
mod film;
mod jpeg;
mod method4;
mod mono_raster;
mod raf;
mod raster;
mod scene;

/// Add these functions, classes and profiles to the `fujimod._native.emulator` module.
pub fn register(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<film::FilmStage>()?;
    module.add_class::<scene::HostImage>()?;
    module.add_function(wrap_pyfunction!(jpeg::jpeg_decode_yycbcr, module)?)?;
    module.add_function(wrap_pyfunction!(jpeg::jpeg_encode_yycbcr, module)?)?;
    module.add_function(wrap_pyfunction!(method4::method4_decode, module)?)?;
    module.add_function(wrap_pyfunction!(mono_raster::mono12_phase_gain, module)?)?;
    module.add_function(wrap_pyfunction!(mono_raster::zero_row_edges, module)?)?;
    module.add_function(wrap_pyfunction!(raf::raf_radiance_rgb, module)?)?;
    module.add_function(wrap_pyfunction!(raster::ui_composite, module)?)?;
    module.add_function(wrap_pyfunction!(raster::ui_composite_tiled, module)?)?;
    module.add_function(wrap_pyfunction!(raster::ui_fill_counts, module)?)?;
    module.add_function(wrap_pyfunction!(scene::virtual_scene_histogram, module)?)?;
    module.add_function(wrap_pyfunction!(scene::virtual_scene_statistics, module)?)?;
    module.add_function(wrap_pyfunction!(scene::virtual_scene_yycbcr, module)?)?;
    module.add("VIRTUAL_SCENE_PROFILE", fujimod_virtual_scene::PROFILE)?;
    module.add("STATISTICS_PROFILE", scene::STATISTICS_PROFILE)?;
    Ok(())
}
