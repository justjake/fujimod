//! Python bindings for XP4 board sessions. The `_native` module registers them on the shared
//! `Board`, which reaches a session through `Xp4Backend`.
mod session;
use fujimod_python_base::xp4_backend::{Xp4Backend, set_xp4_factory};
use pyo3::prelude::*;

fn open(definition: &str, storage_image: Option<Vec<u8>>) -> PyResult<Box<dyn Xp4Backend>> {
    Ok(Box::new(session::Xp4Session::from_definition(
        definition,
        storage_image,
    )?))
}

/// Let `Board.from_xp4` build XP4 sessions.
pub fn register() {
    set_xp4_factory(open);
}
