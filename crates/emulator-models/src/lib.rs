//! Registers the constructors of every model crate. Final crates, and tests that
//! build boards, call [`register`] before building one.
use fujimod_emulator_exr2_plan::construction::{
    Constructors, register_interface_models, set_constructors,
};

/// Register every model crate's constructors; later calls do nothing.
pub fn register() {
    set_constructors(exr2);
}

fn exr2() -> Constructors {
    let mut constructors = Constructors::default();
    register_interface_models(&mut constructors);
    fujimod_emulator_exr2_audio::construct::register(&mut constructors);
    fujimod_emulator_exr2_boot_serial::construct::register(&mut constructors);
    fujimod_emulator_exr2_codecs::construct::register(&mut constructors);
    fujimod_emulator_exr2_companion::construct::register(&mut constructors);
    fujimod_emulator_exr2_display::construct::register(&mut constructors);
    fujimod_emulator_exr2_gpu::construct::register(&mut constructors);
    fujimod_emulator_exr2_i2c::construct::register(&mut constructors);
    fujimod_emulator_exr2_io::construct::register(&mut constructors);
    fujimod_emulator_exr2_isp_face::construct::register(&mut constructors);
    fujimod_emulator_exr2_isp_input::construct::register(&mut constructors);
    fujimod_emulator_exr2_isp_output::construct::register(&mut constructors);
    fujimod_emulator_exr2_isp_statistics::construct::register(&mut constructors);
    fujimod_emulator_exr2_serial::construct::register(&mut constructors);
    fujimod_emulator_exr2_storage::construct::register(&mut constructors);
    fujimod_emulator_exr2_system::construct::register(&mut constructors);
    constructors
}
