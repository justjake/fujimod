//! Launch the desktop application as the frontend of a native session.
use std::process::{Child, Command};

/// Arguments that connect the desktop application to an existing session.
pub fn arguments(endpoint: &str) -> Vec<String> {
    vec!["--endpoint".into(), endpoint.into()]
}

/// Start the desktop application for a camera session.
pub fn connect(endpoint: &str) -> Result<Child, String> {
    let executable = std::env::var_os("FUJIMOD_EMULATOR_GUI")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("fujimod-emulator-gui"));
    Command::new(&executable)
        .args(arguments(endpoint))
        .spawn()
        .map_err(|error| format!("{}: {error}", executable.display()))
}

#[cfg(test)]
mod tests {
    use super::arguments;

    #[test]
    fn the_desktop_receives_the_session_endpoint_as_its_own_option() {
        assert_eq!(
            arguments("ws://127.0.0.1:48123/emulator"),
            ["--endpoint", "ws://127.0.0.1:48123/emulator"]
        );
    }
}
