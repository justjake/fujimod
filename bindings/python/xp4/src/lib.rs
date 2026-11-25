//! Python bindings for XP4 boards: `Board.from_xp4` opens a shared session
//! from an XP4 definition with the XP4 inputs beside it.
use fujimod_emulator_devices::parts::sd_card::Card;
use fujimod_emulator_session::Session;
use fujimod_emulator_xp4_board::{Definition, shared};
use fujimod_python_base::{error, xp4_backend::{Opened, Xp4Inputs, set_xp4_factory}};
use pyo3::{exceptions::PyValueError, prelude::*};

struct Inputs(shared::Inputs);

impl Xp4Inputs for Inputs {
    fn turn_dial(&self, session: &mut Session, name: &str, edges: i32) -> PyResult<()> {
        self.0.turn_dial(session, name, edges).map_err(error)
    }
    fn set_adc_input(&self, session: &mut Session, channel: usize, sample: u16) -> PyResult<()> {
        self.0.set_adc_input(session, channel, sample).map_err(error)
    }
    fn set_gpio_input(&self, session: &mut Session, port: usize, pin: usize, high: bool) -> PyResult<()> {
        self.0.set_gpio_input(session, port, pin, high).map_err(error)
    }
    fn set_usb_connected(&self, session: &mut Session, connected: bool) -> PyResult<()> {
        self.0.set_usb_connected(session, connected).map_err(error)
    }
    fn usb_bus_reset(&self, session: &mut Session) -> PyResult<()> {
        self.0.usb_bus_reset(session).map_err(error)
    }
    fn usb_transaction(
        &self,
        session: &mut Session,
        physical_endpoint: usize,
        setup: bool,
        out: Option<Vec<u8>>,
    ) -> PyResult<u64> {
        self.0.usb_transaction(session, physical_endpoint, setup, out).map_err(error)
    }
    fn usb_reply_json(&self, session: &Session, id: u64) -> PyResult<Option<String>> {
        let reply = self.0.usb_reply(session, id).map_err(error)?;
        reply
            .map(|reply| serde_json::to_string(&reply).map_err(|e| PyValueError::new_err(e.to_string())))
            .transpose()
    }
    fn usb_consume_reply(&self, session: &mut Session, id: u64) -> PyResult<()> {
        self.0.usb_consume_reply(session, id).map_err(error)
    }
    fn card_medium(&self, session: &Session, host: usize) -> PyResult<Card> {
        shared::card_medium(session, host).map_err(error)
    }
    fn nor_medium(&self, session: &Session) -> PyResult<Vec<u8>> {
        shared::nor_medium(session).map_err(error)
    }
    fn reset_nor_program_coverage(&self, session: &mut Session) -> PyResult<()> {
        shared::reset_nor_program_coverage(session).map_err(error)
    }
}

fn open(
    definition: &str,
    storage_image: Option<Vec<u8>>,
    card_files: Vec<(String, Vec<u8>)>,
    card_media: Vec<(usize, Card)>,
) -> PyResult<Opened> {
    let definition: Definition =
        serde_json::from_str(definition).map_err(|e| PyValueError::new_err(e.to_string()))?;
    let read_limit = definition.ram_size;
    let (session, inputs) = shared::from_definition(
        definition,
        storage_image,
        card_files,
        card_media,
        fujimod_catalog_configuration::RunEngine::Unicorn,
    )
    .map_err(error)?;
    Ok(Opened { session, inputs: Box::new(Inputs(inputs)), read_limit })
}

/// Let `Board.from_xp4` build XP4 sessions.
pub fn register() {
    set_xp4_factory(open);
}
