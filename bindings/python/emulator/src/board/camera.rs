//! The camera builder's open, wrapped as the board the bindings already own.
use super::*;
use fujimod_emulator_camera::{Boot, BootCache, CardModel, CardSource, Library, Request};

/// The catalog boot a `boot` argument names, with the card files a stock
/// boot adds from a directory.
fn catalog_boot(boot: &str, sd_card: Option<&PathBuf>) -> PyResult<(Boot, Vec<(String, PathBuf)>)> {
    let source = || match sd_card {
        Some(directory) => CardSource::Directory(directory.clone()),
        None => CardSource::Release,
    };
    match boot {
        "stock" => {
            let files = match sd_card {
                Some(directory) => fujimod_emulator_camera::directory_files(directory)
                    .map_err(|failure| error(failure.to_string()))?,
                None => Vec::new(),
            };
            Ok((Boot::Stock, files))
        }
        "sd-card" => Ok((Boot::SdCard(source()), Vec::new())),
        "automatic" | "automatic-boot" => Ok((Boot::AutomaticBoot(source()), Vec::new())),
        other => Err(PyValueError::new_err(format!(
            "boot must be 'stock', 'sd-card' or 'automatic', not {other:?}"
        ))),
    }
}

#[pymethods]
impl Board {
    /// Open a camera through the shared builder.
    ///
    /// The firmware's update, boot ROM and reference camera come from the
    /// directory named by `FUJIMOD_FIRMWARE_LIBRARY`. With `run_to_rec` false
    /// the board is returned before its first instruction, with the boot
    /// controls held; running it and calling `release_holds` finishes the boot.
    /// Otherwise the session is already at REC or its first-start setup.
    #[staticmethod]
    #[pyo3(signature=(*, firmware, boot="stock", sd_card=None, execution_policy="exact", boot_cache=true,
        run_to_rec=true, sd_card_present=true, sd_card_write_protected=false, sd_card_transfer_ticks=None))]
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
    ) -> PyResult<Self> {
        let library = Library::from_env().map_err(|failure| error(failure.to_string()))?;
        let policy =
            fujimod_emulator_cpu::ExecutionPolicy::parse(execution_policy).map_err(error)?;
        let (boot, card_files) = catalog_boot(boot, sd_card.as_ref())?;
        if let Some(ticks) = sd_card_transfer_ticks
            && ticks == 0
        {
            return Err(PyValueError::new_err(
                "sd_card_transfer_ticks must be positive",
            ));
        }
        let opened = fujimod_emulator_camera::open(
            &library,
            Request {
                firmware,
                boot,
                policy,
                engine: None,
                card_files,
                cache: if boot_cache {
                    BootCache::Reuse
                } else {
                    BootCache::Bypass
                },
                run_to_rec,
                card: CardModel {
                    present: sd_card_present,
                    protected: sd_card_write_protected,
                    transfer_ticks: sd_card_transfer_ticks,
                },
                kept: Default::default(),
                card_source: None,
            },
        )
        .map_err(|failure| error(failure.to_string()))?;
        Ok(Self::from_camera(opened))
    }

    /// Release the boot controls a `run_to_rec=false` open holds, and connect
    /// the controls the running firmware loads. Does nothing once released.
    fn release_holds(&mut self) -> PyResult<()> {
        let Some(Backend::Camera(opened)) = self.backend.as_mut() else {
            return Ok(());
        };
        opened
            .release_holds()
            .map_err(|failure| error(failure.to_string()))
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
        let Some(backend) = self.backend.take() else {
            return Err(error("runtime is closed".into()));
        };
        let Backend::Camera(opened) = backend else {
            self.backend = Some(backend);
            return Err(PyValueError::new_err(
                "power_on requires a board created by open_camera",
            ));
        };
        match opened.power_on(
            &library,
            fujimod_emulator_camera::PowerOn {
                hold_controls,
                fresh_card,
                run_to_rec,
            },
        ) {
            Ok(next) => {
                self.read_limit = next.camera.read_limit;
                self.backend = Some(Backend::Camera(Box::new(next)));
                Ok(())
            }
            Err(failure) => {
                self.backend = Some(Backend::Camera(opened));
                Err(error(failure.to_string()))
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
