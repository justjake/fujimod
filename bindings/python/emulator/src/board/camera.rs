//! The camera builder's open, wrapped as the board the bindings already own.
use super::*;
use fujimod_emulator_camera::{Boot, BootCache, CardSource, Library, Request};

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
    /// directory named by `FUJIMOD_FIRMWARE_LIBRARY`. Returns a board whose
    /// session is already at REC or its first-start setup.
    #[staticmethod]
    #[pyo3(signature=(*, firmware, boot="stock", sd_card=None, execution_policy="exact", boot_cache=true))]
    fn open_camera(
        firmware: String,
        boot: &str,
        sd_card: Option<PathBuf>,
        execution_policy: &str,
        boot_cache: bool,
    ) -> PyResult<Self> {
        let library = Library::from_env().map_err(|failure| error(failure.to_string()))?;
        let policy =
            fujimod_emulator_cpu::ExecutionPolicy::parse(execution_policy).map_err(error)?;
        let (boot, card_files) = catalog_boot(boot, sd_card.as_ref())?;
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
            },
        )
        .map_err(|failure| error(failure.to_string()))?;
        let camera = opened.camera;
        Ok(Self::from_graph_session(camera.session, camera.read_limit))
    }

    /// The builder's observation of the running session: the boot stage
    /// ("rec", "running" or "stopped") and its UI and system modes.
    fn status(&self) -> PyResult<String> {
        let session = self.session()?;
        let report = fujimod_emulator_boot::status(session).map_err(error)?;
        serde_json::to_string(&report).map_err(|failure| error(failure.to_string()))
    }
}
