//! Python bindings for the graphics data path, the EXR II GPU, display scanout and display
//! color conversion. The
//! `_native` module registers them on `fujimod._native.emulator`.
use pyo3::prelude::*;
mod display_pixels;
mod python_graphics;
mod scanout_observation;

/// Add these functions and classes to the `fujimod._native.emulator` module.
pub fn register(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<python_graphics::UiBatch>()?;
    module.add_function(wrap_pyfunction!(python_graphics::clear_parse, module)?)?;
    module.add_function(wrap_pyfunction!(python_graphics::clear_path_descriptor, module)?)?;
    module.add_function(wrap_pyfunction!(python_graphics::clear_render, module)?)?;
    module.add_function(wrap_pyfunction!(python_graphics::copy_untile, module)?)?;
    module.add_function(wrap_pyfunction!(python_graphics::gpu_ui_render, module)?)?;
    module.add_function(wrap_pyfunction!(python_graphics::math_check, module)?)?;
    module.add_function(wrap_pyfunction!(python_graphics::ui_batch, module)?)?;
    module.add_function(wrap_pyfunction!(python_graphics::ui_initialize, module)?)?;
    module.add_function(wrap_pyfunction!(python_graphics::ui_stream_draws, module)?)?;
    module.add_function(wrap_pyfunction!(python_graphics::ui_stream_parse, module)?)?;
    module.add_function(wrap_pyfunction!(scanout_observation::render_scanout_snapshot, module)?)?;
    module.add_function(wrap_pyfunction!(scanout_observation::scanout_snapshot_state, module)?)?;
    module.add_function(wrap_pyfunction!(display_pixels::decode_display_rgb, module)?)?;
    Ok(())
}
