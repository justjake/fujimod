//! XP5 ownership on the common native board, with its declared physical inputs.
use fujimod_catalog_configuration::RunEngine;
use super::*;

pub(super) struct Opened {
    pub session: fujimod_emulator_session::Session,
    pub inputs: fujimod_emulator_xp5_board::session::Inputs,
}

impl Board {
    pub(super) fn xp5_graph(
        graph: factory::Graph,
        devices: &str,
        assets: &Bound<'_, PyDict>,
        read_limit: usize,
        mode: Mode,
        continue_refusals: bool,
        engine: RunEngine,
        definition: serde_json::Value,
    ) -> PyResult<Self> {
        if continue_refusals {
            return Err(PyValueError::new_err(
                "XP5 does not continue modeled refusals",
            ));
        }
        let mut loaded = BTreeMap::new();
        for (name, bytes) in assets.iter() {
            let bytes = bytes.cast::<PyBytes>()?;
            if bytes.as_bytes().len() > fujimod_emulator_storage::fat::MAX_PRELOAD_BYTES {
                return Err(PyValueError::new_err("XP5 asset exceeds 512 MiB"));
            }
            loaded.insert(name.extract::<String>()?, bytes.as_bytes().to_vec());
        }
        let spec: fujimod_emulator_xp5_board::definition::Definition = json(devices)?;
        let (session, inputs) = spec
            .open(graph, &loaded, engine, mode, definition)
            .map_err(error)?;
        Ok(Self {
            driver: None,
            memory_watch: None,
            function_watch: None,
            instruction_points: BTreeMap::new(),
            backend: Some(Backend::Xp5(Box::new(Opened { session, inputs }))),
            objects: BTreeMap::new(),
            failures: Failures::default(),
            read_limit,
            last_continuation_stop: None,
        })
    }
}

impl Board {
    fn xp5_opened_mut(&mut self, what: &str) -> PyResult<&mut Opened> {
        match self.backend.as_mut() {
            Some(Backend::Xp5(opened)) => Ok(opened),
            _ => Err(PyValueError::new_err(format!("XP5 {what} requires an XP5 graph"))),
        }
    }
}

#[pymethods]
impl Board {
    fn xp5_gpio_input(&mut self, port: usize, pin: usize, high: bool) -> PyResult<()> {
        let Some(Backend::Xp5(opened)) = self.backend.as_mut() else {
            return Err(PyValueError::new_err(
                "Physical XP5 GPIO input requires an XP5 graph",
            ));
        };
        opened
            .inputs
            .gpio(&mut opened.session, port, pin, high)
            .map_err(error)
    }
    fn xp5_gpio_level(&self, port: usize, pin: usize) -> PyResult<bool> {
        let Some(Backend::Xp5(opened)) = self.backend.as_ref() else {
            return Err(PyValueError::new_err(
                "Physical XP5 GPIO input requires an XP5 graph",
            ));
        };
        opened.inputs.gpio_level(port, pin).map_err(error)
    }
    fn xp5_key(&mut self, at_ns: u64, key: &str, pressed: bool) -> PyResult<()> {
        let Some(Backend::Xp5(opened)) = self.backend.as_mut() else {
            return Err(PyValueError::new_err(
                "Physical XP5 key input requires an XP5 graph",
            ));
        };
        let key = fujimod_emulator_xp5_board::session::Key::parse(key).map_err(error)?;
        opened
            .inputs
            .key(&mut opened.session, at_ns, key, pressed)
            .map_err(error)
    }
    /// Plug (True) or unplug a USB cable from a data host: the connection
    /// status word, the power controller's supply bits and the DWC3 link.
    fn xp5_usb_cable(&mut self, connected: bool) -> PyResult<()> {
        let opened = self.xp5_opened_mut("USB cable input")?;
        opened.inputs.usb_cable(&mut opened.session, connected).map_err(error)
    }
    /// Only the DWC3 data lines (no supply or status change).
    fn xp5_usb_link(&mut self, connected: bool) -> PyResult<()> {
        let opened = self.xp5_opened_mut("USB link input")?;
        opened.inputs.usb_link(&mut opened.session, connected).map_err(error)
    }
    fn xp5_usb_bus_reset(&mut self) -> PyResult<()> {
        let opened = self.xp5_opened_mut("USB bus reset")?;
        opened.inputs.usb_bus_reset(&mut opened.session).map_err(error)
    }
    /// Queue one host transaction on a DWC3 physical endpoint; returns its id.
    #[pyo3(signature=(endpoint, setup, out=None))]
    fn xp5_usb_transaction(&mut self, endpoint: usize, setup: bool, out: Option<Vec<u8>>) -> PyResult<u64> {
        let opened = self.xp5_opened_mut("USB transaction")?;
        opened
            .inputs
            .usb_transaction(&mut opened.session, endpoint, setup, out)
            .map_err(error)
    }
    /// A completed transaction's reply as JSON ("Ack", "Nak", "Stall" or
    /// {"Data": [...]}), or None while it is pending.
    fn xp5_usb_reply(&self, transaction: u64) -> PyResult<Option<String>> {
        let Some(Backend::Xp5(opened)) = self.backend.as_ref() else {
            return Err(PyValueError::new_err("XP5 USB reply requires an XP5 graph"));
        };
        Ok(opened
            .inputs
            .usb_reply(&opened.session, transaction)
            .map_err(error)?
            .map(|reply| serde_json::to_string(&reply).unwrap()))
    }
    fn xp5_usb_consume(&mut self, transaction: u64) -> PyResult<()> {
        let opened = self.xp5_opened_mut("USB reply release")?;
        opened.inputs.usb_consume(&mut opened.session, transaction).map_err(error)
    }
    /// The DWC3 saved state (registers, endpoints, running) as JSON.
    fn xp5_usb_state(&self) -> PyResult<String> {
        let Some(Backend::Xp5(opened)) = self.backend.as_ref() else {
            return Err(PyValueError::new_err("XP5 USB state requires an XP5 graph"));
        };
        Ok(opened.inputs.usb_state(&opened.session).map_err(error)?.to_string())
    }
    /// Queue a latched dial or switch position (`iso`, `ss`, `ev`, `drive`,
    /// `focus`, `mode`, `power` on the X-T5) at an absolute guest nanosecond.
    fn xp5_set_position(&mut self, at_ns: u64, dial: &str, position: &str) -> PyResult<()> {
        let Some(Backend::Xp5(opened)) = self.backend.as_mut() else {
            return Err(PyValueError::new_err(
                "Physical XP5 position input requires an XP5 graph",
            ));
        };
        opened
            .inputs
            .set_position(&mut opened.session, at_ns, dial, position)
            .map_err(error)
    }
}
