fn main() {
    if let Err(error) = fujimod_emulator_gui_app_lib::run() {
        eprintln!("fujimod-emulator-gui: {error}");
        std::process::exit(1);
    }
}
