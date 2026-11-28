//! Run an X-Processor 5 camera to REC from the firmware's located REC state.
//!
//! The firmware enters REC before it draws the shooting screen, and live view
//! keeps redrawing it, so a frame sequence never settles. The camera is at REC
//! once the located UI mode and system mode hold their REC values with no
//! pending mode change and no open menu, the stock code is intact, and a drawn
//! screen (more than one color) has been scanned out while that has held for
//! the firmware's settle time. The host reads these words and frames; it
//! writes and substitutes nothing.
use crate::RecReport;
use fujimod_catalog::rec_readiness::{RecReadiness, StateWord};
use fujimod_emulator_session::Session;
use sha2::{Digest, Sha256};

/// Guest time between two observations.
const OBSERVE_EVERY_MILLISECONDS: u64 = 10;

fn read(session: &Session, word: StateWord) -> Result<u32, String> {
    let mut bytes = [0; 4];
    let width = usize::from(word.bytes);
    session
        .board
        .read_physical(word.address, &mut bytes[..width])?;
    Ok(u32::from_le_bytes(bytes))
}

fn intact(session: &Session, readiness: &RecReadiness) -> Result<bool, String> {
    for check in &readiness.identity_checks {
        let size = usize::try_from(check.size).map_err(|_| "identity check exceeds memory")?;
        let mut bytes = vec![0; size];
        session.board.read_physical(check.address, &mut bytes)?;
        let digest: String = Sha256::digest(&bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        if digest != check.sha256 {
            return Ok(false);
        }
    }
    Ok(true)
}

/// A frame drawn by the firmware: more than one color.
fn drawn(rgba: &[u8]) -> bool {
    let (pixels, _) = rgba.as_chunks::<4>();
    pixels.iter().any(|pixel| Some(pixel) != pixels.first())
}

fn report(
    session: &Session,
    readiness: &RecReadiness,
    status: &str,
    scanout: Option<usize>,
) -> Result<RecReport, String> {
    let pending_mode = read(session, readiness.pending_mode)?;
    let menu_active = read(session, readiness.menu_active)?;
    Ok(RecReport {
        status: status.into(),
        instructions: session.board.instruction_counts().iter().sum(),
        virtual_ticks: session.board.now(),
        ui_mode: read(session, readiness.ui_mode)?,
        system_mode: read(session, readiness.system_mode)?,
        menu_active,
        menu_node: 0,
        pending_mode,
        pending: pending_mode != 0 || menu_active != 0,
        setup_presses: 0,
        translation_cache_size_bytes: session.board.translation_cache_size_bytes(),
        scanout_instance: scanout,
        failure: None,
        discovery_callee_stubs: session.board.discovery_callee_stubs(),
    })
}

/// Run the session until the firmware's REC state holds with a drawn screen,
/// or the instruction budget is spent.
pub fn to_rec(
    session: &mut Session,
    readiness: &RecReadiness,
    quantum: usize,
    ticks_per_second: u64,
    instruction_limit: u64,
) -> Result<RecReport, String> {
    let limits =
        crate::host::Limits::new(session, Some(instruction_limit), None, ticks_per_second)?;
    let window = ticks_per_second * OBSERVE_EVERY_MILLISECONDS / 1000;
    let settle = ticks_per_second.saturating_mul(readiness.settle_milliseconds) / 1000;
    let mut held_since = None;
    let mut drawn_output = None;
    loop {
        let now = session.board.now();
        let at_rec = read(session, readiness.ui_mode)? == readiness.rec_ui_mode
            && read(session, readiness.system_mode)? == readiness.rec_system_mode
            && read(session, readiness.pending_mode)? == 0
            && read(session, readiness.menu_active)? == 0
            && intact(session, readiness)?;
        if !at_rec {
            held_since = None;
            drawn_output = None;
        } else if held_since.is_none() {
            held_since = Some(now);
        }
        if let (Some(since), Some(output)) = (held_since, drawn_output)
            && now.saturating_sub(since) >= settle
        {
            return report(session, readiness, "rec", Some(output));
        }
        if limits.reached(session) {
            return report(session, readiness, "instruction-limit", None);
        }
        let advanced = limits.step(session, quantum, None, Some(now.saturating_add(window)))?;
        for frame in session.board.take_frames() {
            if held_since.is_some() && drawn(&frame.rgba) {
                drawn_output = Some(
                    drawn_output.map_or(frame.output, |output: usize| output.min(frame.output)),
                );
            }
        }
        if !advanced {
            let mut stopped = report(session, readiness, "quiescent", None)?;
            stopped.failure =
                Some("the board is quiescent without an input or device deadline".into());
            return Ok(stopped);
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_frame_is_drawn_when_it_holds_more_than_one_color() {
        assert!(!super::drawn(&[0, 0, 0, 255, 0, 0, 0, 255]));
        assert!(super::drawn(&[0, 0, 0, 255, 9, 0, 0, 255]));
        assert!(!super::drawn(&[]));
    }
}
