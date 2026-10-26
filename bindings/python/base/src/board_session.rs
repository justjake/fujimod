//! Borrowing the emulator session of a Python emulator board from other binding crates.
//!
//! The emulator bindings install the borrower when they register, so binding crates such as
//! the remote bindings drive a board's session without depending on them.
use fujimod_emulator_session::Session;
use pyo3::{exceptions::PyRuntimeError, prelude::*};
use std::sync::OnceLock;

/// Calls `call` with the session of a Python `fujimod._native.emulator.Board`.
pub type Borrower =
    fn(&Bound<'_, PyAny>, &mut dyn FnMut(&mut Session) -> PyResult<()>) -> PyResult<()>;
static BORROWER: OnceLock<Borrower> = OnceLock::new();

/// Install the emulator bindings' borrower; later installations are ignored.
pub fn install(borrower: Borrower) {
    let _ = BORROWER.set(borrower);
}

/// Run `call` with the session of a Python emulator board.
pub fn with_session<T>(
    board: &Bound<'_, PyAny>,
    call: impl FnOnce(&mut Session) -> PyResult<T>,
) -> PyResult<T> {
    let borrower = BORROWER
        .get()
        .ok_or_else(|| PyRuntimeError::new_err("emulator bindings are not registered"))?;
    let mut call = Some(call);
    let mut result = None;
    borrower(board, &mut |session| {
        if let Some(call) = call.take() {
            result = Some(call(session)?);
        }
        Ok(())
    })?;
    result.ok_or_else(|| PyRuntimeError::new_err("board session was not borrowed"))
}
