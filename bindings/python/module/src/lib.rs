//! The `fujimod._native` extension module. Each binding crate registers its submodules here,
//! with the EXR II board builder, the model constructors and the catalog boot, so a device
//! model edit rebuilds the model, the models registration crate and this crate only.
use pyo3::prelude::*;

#[cfg(feature = "emulator")]
mod boot;

#[pymodule]
fn _native(py: Python<'_>, module: &Bound<'_, PyModule>) -> PyResult<()> {
    #[cfg(feature = "emulator")]
    fujimod_emulator_exr2_board::register_board_builder();
    #[cfg(feature = "emulator")]
    fujimod_emulator_models::register();
    fujimod_python_recording::register(py, module)?;
    fujimod_python_remote::register(py, module)?;
    fujimod_python_catalog::register(py, module)?;
    fujimod_python_usb::register(py, module)?;
    fujimod_python_app80::register(py, module)?;
    #[cfg(feature = "emulator")]
    {
        boot::register();
        fujimod_python_emulator::register(py, module)?;
        fujimod_python_xp4::register();
        let emulator = module.getattr("emulator")?.cast_into::<PyModule>()?;
        fujimod_python_host_image::register(&emulator)?;
        fujimod_python_graphics::register(&emulator)?;
    }
    Ok(())
}
