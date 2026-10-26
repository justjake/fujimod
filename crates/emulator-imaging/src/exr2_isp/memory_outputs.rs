//! RAW readback destinations the EXR II image input and output banks share
//! through one board wire.
use crate::exr2_isp::dma::{Raster, Transfer};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

#[derive(Default, Serialize, Deserialize)]
pub struct MemoryOutputs {
    pub destinations: [Option<Raster>; 2],
    pub active_destinations: [Option<Raster>; 2],
    pub transfers: [Transfer; 2],
    pub active: bool,
}
pub type MemoryOutputHandle = Arc<Mutex<MemoryOutputs>>;
impl MemoryOutputs {
    pub fn fingerprint(&self) -> serde_json::Value {
        let Self {
            destinations,
            active_destinations,
            transfers,
            active,
        } = self;
        serde_json::json!({"destinations":destinations,
            "active_destinations":active_destinations,"active":active,
            "transfers":transfers.iter().map(Transfer::fingerprint).collect::<Vec<_>>()})
    }
    pub fn configure(&mut self, channel: usize, raster: Option<Raster>) -> Result<(), String> {
        if channel >= 2 {
            return Err("RAW readback output channel is invalid".into());
        }
        if self.destinations[channel] == raster {
            return Ok(());
        }
        if let Some(r) = raster {
            r.validate(r.width * 2)?;
        }
        self.destinations[channel] = raster;
        Ok(())
    }
    pub fn completed(&self) -> bool {
        self.active && self.transfers.iter().all(|t| t.deadline().is_none())
    }
}
