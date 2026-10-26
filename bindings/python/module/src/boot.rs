//! Catalog run preparation and boot for the Python emulator bindings, installed through
//! `fujimod_python_base::boot` when the module initializes.
use fujimod_emulator_boot as boot;
use fujimod_emulator_session::Session;
use fujimod_python_base::boot::{Boot, Inputs};

struct CatalogBoot;

impl Boot for CatalogBoot {
    fn prepare(&self, configuration_id: &str, overlays: &[String]) -> Result<String, String> {
        let prepared = boot::prepare(configuration_id, overlays)?;
        serde_json::to_string(&prepared.to_json()).map_err(|failure| failure.to_string())
    }
    fn open(&self, inputs: Inputs) -> Result<Session, String> {
        boot::open(boot::Options {
            configuration_id: inputs.configuration_id,
            overlays: inputs.overlays,
            update: inputs.update,
            boot_rom: inputs.boot_rom,
            nand: inputs.nand,
            settings: inputs.settings,
            clock_seconds_2000: inputs.clock_seconds_2000,
            preferred_languages: inputs.preferred_languages,
            camera: inputs.camera,
        })
    }
    fn to_rec(&self, session: &mut Session, limit: u64) -> Result<String, String> {
        let report = boot::to_rec(session, limit)?;
        serde_json::to_string(&report).map_err(|failure| failure.to_string())
    }
    fn screenshot(&self, session: &Session) -> Result<Vec<u8>, String> {
        boot::screenshot(session)
    }
}

/// Install catalog boots for `Board.from_inputs`, `Board.boot_to_rec`, `Board.screenshot` and
/// `prepare_configuration`.
pub(crate) fn register() {
    fujimod_python_base::boot::install(&CatalogBoot);
}
