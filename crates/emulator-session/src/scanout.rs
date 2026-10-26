//! Observational scanout through the native register model and physical RAM.
use super::Session;
use fujimod_emulator_devices::exr2::scanout_frame::Frame;
use fujimod_emulator_devices::capability::{ScanoutSource};
impl Session {
    /// Reuse a presentation only after rechecking registers and every source byte.
    /// No event, guest instruction or architectural state participates in this memo.
    pub fn recording_frame(
        &self,
        name: &str,
        instance: usize,
        diagnostics: bool,
    ) -> Result<Option<std::sync::Arc<Frame>>, String> {
        self.board
            .inspect_capability::<dyn ScanoutSource, _>(self.node(name)?, |scanout| {
                let key = serde_json::to_vec(&scanout.metadata()).map_err(|e| e.to_string())?;
                let mut caches = self.recording_frames.borrow_mut();
                let cache = caches
                    .entry((name.to_owned(), instance, diagnostics))
                    .or_default();
                cache.capture(
                    key,
                    |address, count| {
                        let mut bytes = vec![0; count];
                        self.board.read_physical(address, &mut bytes)?;
                        Ok(bytes)
                    },
                    |read| {
                        scanout
                            .frame(
                                instance,
                                true,
                                read,
                                diagnostics,
                            )
                            .map(|frame| frame.map(std::sync::Arc::new))
                    },
                )
            })
    }
    pub fn scanout_state(&self, name: &str) -> Result<serde_json::Value, String> {
        self.board
            .inspect_capability::<dyn ScanoutSource, _>(self.node(name)?, |scanout| Ok(scanout.metadata()))
    }

    pub fn scanout(
        &self,
        name: &str,
        instance: usize,
        pixels: bool,
        diagnostic_hashes: bool,
    ) -> Result<Option<Frame>, String> {
        self.board
            .inspect_capability::<dyn ScanoutSource, _>(self.node(name)?, |scanout| {
                let mut read = |address, count| {
                    let mut bytes = vec![0; count];
                    self.board.read_physical(address, &mut bytes)?;
                    Ok(bytes)
                };
                scanout.frame(
                    instance,
                    pixels,
                    &mut read,
                    diagnostic_hashes,
                )
            })
    }
    pub fn scanout_main(
        &self,
        name: &str,
        instance: usize,
        pixels: bool,
        diagnostic_hashes: bool,
    ) -> Result<Option<Frame>, String> {
        self.board
            .inspect_capability::<dyn ScanoutSource, _>(self.node(name)?, |scanout| {
                let mut read = |address, count| {
                    let mut bytes = vec![0; count];
                    self.board.read_physical(address, &mut bytes)?;
                    Ok(bytes)
                };
                scanout.main_frame(
                    instance,
                    pixels,
                    &mut read,
                    diagnostic_hashes,
                )
            })
    }
}
