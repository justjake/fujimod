//! The camera builder's open, wrapped as the board the bindings already own.
//!
//! Two layers open a camera: `fujimod_camera` for the stock, SD card and
//! autostart presets, and `fujimod_emulator_camera` for explicit firmware
//! files, internal flash and a power-on combination.
use super::*;
use fujimod_camera::{CardSource, Library, Preset, StateDir};
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
    /// Open a camera.
    ///
    /// `boot` is `stock`, `sd-card` or `autostart`; the firmware's update, boot
    /// ROM and reference camera come from the directory named by
    /// `FUJIMOD_FIRMWARE_LIBRARY`. Give `update` and `boot_rom` to name the
    /// two files instead, `flash` to start from a saved internal-flash image,
    /// or `combination` to hold a catalog power-on combination (`sd`,
    /// `firmware_update` or `service`); those select the emulator layer, whose
    /// SD card is the `sd_card` directory's files.
    ///
    /// With `run_to_rec` false the board is returned before its first
    /// instruction, with the boot controls held; running it and calling
    /// `release_holds` finishes the boot. Otherwise the session is already at
    /// REC or its first-start setup.
    #[staticmethod]
    #[pyo3(signature=(*, firmware, boot="stock", sd_card=None, execution_policy="exact", boot_cache=true,
        run_to_rec=true, sd_card_present=true, sd_card_write_protected=false, sd_card_transfer_ticks=None,
        card_layout=None, start="rom", image=None, image_sha256=None, update=None, boot_rom=None,
        flash=None, combination=None))]
    #[allow(clippy::too_many_arguments)]
    fn open_camera(
        firmware: String,
        boot: &str,
        sd_card: Option<PathBuf>,
        execution_policy: &str,
        boot_cache: bool,
        run_to_rec: bool,
        sd_card_present: bool,
        sd_card_write_protected: bool,
        sd_card_transfer_ticks: Option<u64>,
        card_layout: Option<&str>,
        start: &str,
        image: Option<PathBuf>,
        image_sha256: Option<String>,
        update: Option<PathBuf>,
        boot_rom: Option<PathBuf>,
        flash: Option<PathBuf>,
        combination: Option<String>,
    ) -> PyResult<Self> {
        let library = Library::from_env().map_err(|failure| error(failure.to_string()))?;
        let state = StateDir::from_env();
        let policy =
            fujimod_emulator_cpu::ExecutionPolicy::parse(execution_policy).map_err(error)?;
        if let Some(ticks) = sd_card_transfer_ticks
            && ticks == 0
        {
            return Err(PyValueError::new_err(
                "sd_card_transfer_ticks must be positive",
            ));
        }
        let layout = match card_layout {
            None => None,
            Some(text) => {
                let layout: emulator::CardLayout = serde_json::from_str(text)
                    .map_err(|failure| PyValueError::new_err(format!("card_layout: {failure}")))?;
                layout.validate().map_err(PyValueError::new_err)?;
                Some(layout)
            }
        };
        let image = loader_image(start, image, image_sha256)?;
        let cache = if boot_cache {
            fujimod_camera::BootCache::Reuse
        } else {
            fujimod_camera::BootCache::Bypass
        };
        let files = match (update, boot_rom) {
            (None, None) => emulator::FirmwareInput::Library,
            (Some(update), Some(boot_rom)) => emulator::FirmwareInput::Files { update, boot_rom },
            _ => {
                return Err(PyValueError::new_err(
                    "update and boot_rom are named together, or not at all",
                ));
            }
        };
        let emulator_layer = matches!(files, emulator::FirmwareInput::Files { .. })
            || flash.is_some()
            || combination.is_some()
            || image.is_some();
        if emulator_layer {
            if boot != "stock" {
                return Err(PyValueError::new_err(
                    "files, flash and a combination open the emulator layer; boot must be 'stock'",
                ));
            }
            let card_files = match &sd_card {
                Some(directory) => emulator::directory_files(directory)
                    .map_err(|failure| error(failure.to_string()))?,
                None => Vec::new(),
            };
            let flash = match flash {
                Some(path) => emulator::Flash::Image(
                    std::fs::read(&path)
                        .map_err(|failure| error(format!("{}: {failure}", path.display())))?,
                ),
                None => emulator::Flash::Stock,
            };
            let start = match combination {
                Some(name) => {
                    if image.is_some() {
                        return Err(PyValueError::new_err(
                            "a RAM image and a power-on combination are exclusive",
                        ));
                    }
                    emulator::Start::Held { combination: name }
                }
                None => match image {
                    Some(image) => emulator::Start::Loader(image),
                    None => emulator::Start::Rom,
                },
            };
            let opened = emulator::open(
                &library,
                &state,
                emulator::Request {
                    firmware,
                    files,
                    start,
                    flash,
                    card: emulator::CardInputs {
                        files: card_files,
                        present: sd_card_present,
                        protected: sd_card_write_protected,
                        transfer_ticks: sd_card_transfer_ticks,
                        layout,
                    },
                    policy,
                    engine: None,
                    cache,
                    run_to_rec,
                    kept: Default::default(),
                },
            )
            .map_err(|failure| error(failure.to_string()))?;
            return Ok(Self::from_emulator(opened));
        }
        let preset = Preset::parse(boot).ok_or_else(|| {
            PyValueError::new_err(format!(
                "boot must be 'stock', 'sd-card' or 'autostart', not {boot:?}"
            ))
        })?;
        if preset == Preset::Autostart && !run_to_rec {
            return Err(PyValueError::new_err(
                "an autostart open cannot stop before REC",
            ));
        }
        let opened = fujimod_camera::open(
            &library,
            &state,
            fujimod_camera::Request {
                firmware,
                preset,
                sd_card: sd_card.map(CardSource::Directory),
                policy,
                engine: None,
                cache,
                run_to_rec,
                card: fujimod_camera::CardOptions {
                    present: sd_card_present,
                    protected: sd_card_write_protected,
                    transfer_ticks: sd_card_transfer_ticks,
                    layout,
                },
            },
        )
        .map_err(|failure| error(failure.to_string()))?;
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
        let library = Library::from_env().map_err(|failure| error(failure.to_string()))?;
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

/// A caller-verified loader image, from `start` and the image arguments.
fn loader_image(
    start: &str,
    image: Option<PathBuf>,
    image_sha256: Option<String>,
) -> PyResult<Option<emulator::LoaderImage>> {
    match (start, image, image_sha256) {
        ("rom", None, None) => Ok(None),
        ("rom", _, _) => Err(PyValueError::new_err("image requires start='loader'")),
        ("loader", Some(path), Some(sha256)) => {
            if sha256.len() != 64 || !sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(PyValueError::new_err(
                    "image_sha256 must be 64 hexadecimal characters",
                ));
            }
            let bytes = std::fs::read(&path)
                .map_err(|failure| error(format!("{}: {failure}", path.display())))?;
            Ok(Some(emulator::LoaderImage { bytes, sha256 }))
        }
        ("loader", _, _) => Err(PyValueError::new_err(
            "start='loader' requires image and image_sha256",
        )),
        (other, _, _) => Err(PyValueError::new_err(format!(
            "start must be 'rom' or 'loader', not {other:?}"
        ))),
    }
}
