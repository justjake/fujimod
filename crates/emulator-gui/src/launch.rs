//! Launch the desktop window for one native camera session.

use crate::Endpoint;
use std::process::{Child, Command};

/// Arguments a desktop window reads before loading the emulator UI.
pub fn arguments(endpoint: &Endpoint, body: &str, model: &str) -> Vec<String> {
    vec![
        "--connect".into(),
        endpoint.url.clone(),
        "--body".into(),
        body.into(),
        "--model".into(),
        model.into(),
    ]
}

/// Start the desktop application connected to an already-running camera.
pub fn open(endpoint: &Endpoint, body: &str, model: &str) -> Result<Child, String> {
    let executable = std::env::var_os("FUJIMOD_EMULATOR_GUI")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("fujimod-emulator-gui"));
    Command::new(&executable)
        .args(arguments(endpoint, body, model))
        .spawn()
        .map_err(|error| format!("{}: {error}", executable.display()))
}
