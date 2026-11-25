//! The XP4 inputs the emulator bindings reach beside a shared session.
//! `fujimod-python-xp4` implements them and registers the factory, so the
//! emulator bindings depend on no XP4 crate.
use fujimod_emulator_devices::parts::sd_card::Card;
use fujimod_emulator_session::Session;
use pyo3::prelude::*;
use std::sync::OnceLock;

/// External XP4 inputs and media the session's generic inputs do not name.
pub trait Xp4Inputs {
    fn turn_dial(&self, session: &mut Session, name: &str, edges: i32) -> PyResult<()>;
    fn set_adc_input(&self, session: &mut Session, channel: usize, sample: u16) -> PyResult<()>;
    fn set_gpio_input(&self, session: &mut Session, port: usize, pin: usize, high: bool) -> PyResult<()>;
    fn set_usb_connected(&self, session: &mut Session, connected: bool) -> PyResult<()>;
    fn usb_bus_reset(&self, session: &mut Session) -> PyResult<()>;
    fn usb_transaction(
        &self,
        session: &mut Session,
        physical_endpoint: usize,
        setup: bool,
        out: Option<Vec<u8>>,
    ) -> PyResult<u64>;
    fn usb_reply_json(&self, session: &Session, id: u64) -> PyResult<Option<String>>;
    fn usb_consume_reply(&self, session: &mut Session, id: u64) -> PyResult<()>;
    /// A copy of the medium in the `host`th SD card slot.
    fn card_medium(&self, session: &Session, host: usize) -> PyResult<Card>;
    /// Completed NOR contents, without controller or processor state.
    fn nor_medium(&self, session: &Session) -> PyResult<Vec<u8>>;
    /// Reset NOR program observation, refusing unsettled commands.
    fn reset_nor_program_coverage(&self, session: &mut Session) -> PyResult<()>;
}

/// An XP4 session, its inputs and the bytes of RAM host reads may span.
pub struct Opened {
    pub session: Session,
    pub inputs: Box<dyn Xp4Inputs>,
    pub read_limit: usize,
}

/// Builds an XP4 session from its definition JSON, optional storage image,
/// SD card files and existing SD card media (SD host index, medium) that
/// replace the formatted SD card of those hosts.
pub type Xp4Factory = fn(
    &str,
    Option<Vec<u8>>,
    Vec<(String, Vec<u8>)>,
    Vec<(usize, Card)>,
) -> PyResult<Opened>;
static FACTORY: OnceLock<Xp4Factory> = OnceLock::new();

/// Set the factory `Board.from_xp4` uses; later calls are ignored.
pub fn set_xp4_factory(factory: Xp4Factory) {
    FACTORY.get_or_init(|| factory);
}

pub fn open(
    definition: &str,
    storage_image: Option<Vec<u8>>,
    card_files: Vec<(String, Vec<u8>)>,
    card_media: Vec<(usize, Card)>,
) -> PyResult<Opened> {
    let open = FACTORY.get().ok_or_else(|| {
        pyo3::exceptions::PyRuntimeError::new_err("XP4 boards need the fujimod-python-xp4 bindings")
    })?;
    open(definition, storage_image, card_files, card_media)
}
