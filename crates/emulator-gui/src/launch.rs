//! Launch the desktop application for one camera session.
use fujimod_catalog_configuration::RunEngine;
use std::process::{Child, Command};

/// Arguments that ask the desktop application to open a prepared camera session.
pub fn arguments(
    configuration: &str,
    update: &std::path::Path,
    engine: Option<RunEngine>,
) -> Vec<String> {
    let mut arguments = vec![
        "--configuration".into(),
        configuration.into(),
        "--update".into(),
        update.display().to_string(),
    ];
    if let Some(engine) = engine {
        arguments.extend([
            "--engine".into(),
            match engine {
                RunEngine::Unicorn => "unicorn",
                RunEngine::Hypervisor => "hypervisor",
            }
            .into(),
        ]);
    }
    arguments
}

/// Start the desktop application for a camera session.
pub fn open(
    configuration: &str,
    update: &std::path::Path,
    engine: Option<RunEngine>,
) -> Result<Child, String> {
    let executable = std::env::var_os("FUJIMOD_EMULATOR_GUI")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("fujimod-emulator-gui"));
    Command::new(&executable)
        .args(arguments(configuration, update, engine))
        .spawn()
        .map_err(|error| format!("{}: {error}", executable.display()))
}
