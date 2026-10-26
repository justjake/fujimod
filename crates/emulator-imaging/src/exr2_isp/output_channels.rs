//! DMA state the two EXR II output banks share through one board wire.
use crate::exr2_isp::dma::{self, Transfer};
use fujimod_emulator_devices::exr2::irq_routes::Irq;
use std::sync::{Arc, Mutex};

/// Both banks share this physical output interface. Reservations are current
/// frame spans, never a history; alias overlap is refused before DMA begins.
#[derive(Clone)]
pub struct OutputChannels(pub Arc<Mutex<Channels>>);

#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct Channels {
    pub transfers: [Option<FrameTransfer>; 2],
    pub dma: [Transfer; 2],
    pub clock: Option<dma::Clock>,
    pub irqs: [Option<u32>; 2],
    pub armed: [bool; 2],
    pub readback_routes: Option<[Option<Irq>; 2]>,
    pub readback_deadline: Option<u64>,
}

impl Default for OutputChannels {
    fn default() -> Self {
        let result = Self(Arc::new(Mutex::new(Channels::default())));
        drop(result.0.lock().unwrap());
        result
    }
}

impl OutputChannels {
    pub fn checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        fujimod_emulator_devices::checkpoint::Checkpoint::encode(
            "output-channels",
            &*self.0.lock().map_err(|_| "output-channels poisoned")?,
        )
    }
    pub fn validate_checkpoint(
        &self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<(), String> {
        let _: Channels = saved.decode("output-channels")?;
        Ok(())
    }
    pub fn restore_checkpoint(
        &self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<(), String> {
        *self.0.lock().map_err(|_| "output-channels poisoned")? =
            saved.decode::<Channels>("output-channels")?;
        Ok(())
    }

    pub fn fingerprint(&self) -> serde_json::Value {
        let channels = self.0.lock().unwrap();
        let Channels {
            transfers,
            dma,
            clock,
            irqs,
            armed,
            readback_routes,
            readback_deadline,
        } = &*channels;
        serde_json::json!({"transfers":transfers,"dma":dma.iter().map(Transfer::fingerprint).collect::<Vec<_>>(),
            "clock":clock,"irqs":irqs,"armed":armed,"readback_routes":readback_routes,"readback_deadline":readback_deadline})
    }
}

#[derive(Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Source {
    Frame(u64),
    Memory,
}

#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct FrameTransfer {
    pub source: Source,
    pub span: (u64, u64),
    pub completed: bool,
    pub route: Irq,
}
