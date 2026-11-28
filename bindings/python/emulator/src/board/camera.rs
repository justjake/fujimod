//! The camera builder's open, wrapped as the board the bindings already own.
//!
//! Two layers open a camera: `fujimod_camera` for the stock, SD card and
//! autostart presets, and `fujimod_emulator_camera` for explicit firmware
//! files, internal flash and a power-on combination.
use super::*;
use fujimod_camera::{Library, Preset, StateDir};
use fujimod_emulator_camera as emulator;

/// A camera one of the two layers opened.
pub(super) enum CameraBoard {
    FujiMod(Box<fujimod_camera::Opened>),
    Emulator(Box<emulator::Opened>),
}

impl CameraBoard {
    pub(super) fn session(&self) -> &fujimod_emulator_session::Session {
        match self {
            Self::FujiMod(opened) => opened.session(),
            Self::Emulator(opened) => opened.session(),
        }
    }

    pub(super) fn session_mut(&mut self) -> &mut fujimod_emulator_session::Session {
        match self {
            Self::FujiMod(opened) => opened.session_mut(),
            Self::Emulator(opened) => opened.session_mut(),
        }
    }

    fn read_limit(&self) -> usize {
        match self {
            Self::FujiMod(opened) => opened.camera.camera.read_limit,
            Self::Emulator(opened) => opened.camera.read_limit,
        }
    }
}

#[pymethods]
impl Board {
    /// Open a camera from its description: JSON text with the same fields as
    /// the MCP server's `camera_open` and the command line's `--camera`. `body` is required. Relative paths resolve from the
    /// current directory.
    ///
    /// A request that holds a power-on combination, starts from a RAM image or
    /// a flash image opens the emulator layer; the others open the FujiMod
    /// layer, whose `boot` is `stock`, `sd-card` or `autostart`. With
    /// `run_to_rec` false, or for a firmware without REC readiness facts, the
    /// board is returned before its first instruction with the boot controls
    /// held; running it and calling `release_holds` finishes the boot.
    #[staticmethod]
    fn open_camera(camera: &str) -> PyResult<Self> {
        let document: fujimod_camera::CameraDescription = serde_json::from_str(camera)
            .map_err(|failure| PyValueError::new_err(format!("camera: {failure}")))?;
        let base = std::env::current_dir().map_err(|failure| error(failure.to_string()))?;
        let request = fujimod_camera::Request::from_description(&document, &base)
            .map_err(|failure| PyValueError::new_err(failure.to_string()))?;
        let library = match &request.camera.files {
            emulator::FirmwareInput::Files { .. } => Library::without_directory(),
            emulator::FirmwareInput::Library => Library::from_env(),
        }
        .map_err(|failure| error(failure.to_string()))?;
        let state = StateDir::from_env();
        if request.preset == Preset::Autostart && !request.camera.run_to_rec {
            return Err(PyValueError::new_err(
                "an autostart open cannot stop before REC",
            ));
        }
        let snapshot = document
            .snapshot
            .as_deref()
            .map(|path| emulator::description::resolve(&base, path));
        let emulator_layer = !matches!(request.camera.start, emulator::Start::Rom)
            || !matches!(request.camera.flash, emulator::Flash::Stock);
        if emulator_layer {
            if request.preset != Preset::Stock {
                return Err(PyValueError::new_err(
                    "hold, loader_image and flash_image open the emulator layer; boot must be 'stock'",
                ));
            }
            let mut opened = match &snapshot {
                Some(_) => emulator::open_unbooted(&library, request.camera),
                None => emulator::open(&library, &state, request.camera),
            }
            .map_err(|failure| error(failure.to_string()))?;
            if let Some(path) = &snapshot {
                fujimod_emulator_boot::load_snapshot(opened.session_mut(), path).map_err(error)?;
            }
            return Ok(Self::from_emulator(opened));
        }
        let mut opened = match &snapshot {
            Some(_) => fujimod_camera::open_unbooted(&library, &state, request),
            None => fujimod_camera::open(&library, &state, request),
        }
        .map_err(|failure| error(failure.to_string()))?;
        if let Some(path) = &snapshot {
            fujimod_emulator_boot::load_snapshot(opened.session_mut(), path).map_err(error)?;
        }
        Ok(Self::from_fujimod(opened))
    }

    /// Release the boot controls a `run_to_rec=false` open holds, and connect
    /// the controls the running firmware loads. Does nothing once released.
    fn release_holds(&mut self) -> PyResult<()> {
        let Some(Backend::Camera(camera)) = self.backend.as_mut() else {
            return Ok(());
        };
        match camera.as_mut() {
            CameraBoard::FujiMod(opened) => opened
                .camera
                .release_holds()
                .map_err(|failure| error(failure.to_string())),
            CameraBoard::Emulator(opened) => opened
                .release_holds()
                .map_err(|failure| error(failure.to_string())),
        }
    }

    /// Power the camera off and on again, keeping its internal flash and the
    /// SD card it last had.
    #[pyo3(signature=(*, hold_controls=false, fresh_card=false, run_to_rec=true))]
    fn power_on(
        &mut self,
        hold_controls: bool,
        fresh_card: bool,
        run_to_rec: bool,
    ) -> PyResult<()> {
        let state = StateDir::from_env();
        let Some(backend) = self.backend.take() else {
            return Err(error("runtime is closed".into()));
        };
        let Backend::Camera(camera) = backend else {
            self.backend = Some(backend);
            return Err(PyValueError::new_err(
                "power_on requires a board created by open_camera",
            ));
        };
        let input = match camera.as_ref() {
            CameraBoard::FujiMod(opened) => opened.camera.firmware_input(),
            CameraBoard::Emulator(opened) => opened.firmware_input(),
        };
        let library = match input {
            emulator::FirmwareInput::Files { .. } => Library::without_directory(),
            emulator::FirmwareInput::Library => Library::from_env(),
        };
        let library = match library {
            Ok(library) => library,
            Err(failure) => {
                self.backend = Some(Backend::Camera(camera));
                return Err(error(failure.to_string()));
            }
        };
        let next = match camera.as_ref() {
            CameraBoard::FujiMod(opened) => opened
                .power_on(
                    &library,
                    &state,
                    fujimod_camera::PowerOn {
                        hold_controls,
                        fresh_card,
                        run_to_rec,
                    },
                )
                .map_err(|failure| failure.to_string())
                .map(|next| CameraBoard::FujiMod(Box::new(next))),
            CameraBoard::Emulator(opened) => opened
                .power_on(
                    &library,
                    &state,
                    emulator::PowerOn {
                        hold: hold_controls.then(|| "sd".to_owned()),
                        fresh_card,
                        run_to_rec,
                    },
                )
                .map_err(|failure| failure.to_string())
                .map(|next| CameraBoard::Emulator(Box::new(next))),
        };
        match next {
            Ok(next) => {
                self.read_limit = next.read_limit();
                self.backend = Some(Backend::Camera(Box::new(next)));
                Ok(())
            }
            Err(failure) => {
                self.backend = Some(Backend::Camera(camera));
                Err(error(failure))
            }
        }
    }

    /// The builder's observation of the running session: the boot stage
    /// ("rec", "running" or "stopped") and its UI and system modes.
    fn status(&self) -> PyResult<String> {
        let session = self.session()?;
        let report = fujimod_emulator_boot::status(session).map_err(error)?;
        serde_json::to_string(&report).map_err(|failure| error(failure.to_string()))
    }
}
