//! Launch the desktop application as the frontend of a native session.
use std::process::{Child, Command};

/// Arguments that connect the desktop application to an existing session.
pub fn arguments(endpoint: &str) -> Vec<String> {
    vec!["--endpoint".into(), endpoint.into()]
}

/// The desktop application showing a camera. The camera runs while it is open;
/// controllers joining and leaving the room do not end the run.
pub struct Window(Child);

impl Window {
    pub fn open(&mut self) -> bool {
        matches!(self.0.try_wait(), Ok(None))
    }
    pub fn close(mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
    pub fn wait(mut self) -> Result<(), String> {
        self.0.wait().map(drop).map_err(|error| error.to_string())
    }
}

/// Start the desktop application for a camera session.
pub fn connect(endpoint: &str) -> Result<Window, String> {
    let executable = std::env::var_os("FUJIMOD_EMULATOR_GUI")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("fujimod-emulator-gui"));
    Command::new(&executable)
        .args(arguments(endpoint))
        .spawn()
        .map(Window)
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
