//! FFF64000 bounded I420 to packed422 conversion. Registers select all addresses and
//! geometry; source bytes are latched at START and DMA precedes status/IRQ.
use fujimod_emulator_devices::{Cx, Device, checkpoint::Checkpoint, exr2::irq_routes::Irq};
use fujimod_emulator_imaging::exr2_isp::{dma::physical, due, word};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Receipt {
    source: u64,
    destination: u64,
    width: usize,
    height: usize,
    /// Bytes per row. Register 0C expresses this in luma samples.
    pitch: usize,
    bytes: usize,
    source_sha256: String,
    source_packing: String,
    source_pitch: usize,
    source_bytes: usize,
    output_sha256: String,
    packing: String,
    started_tick: u64,
    completed_tick: u64,
}
#[derive(Serialize, Deserialize)]
struct State {
    r: [u32; 8],
    reset: Option<u64>,
    pending: Option<Receipt>,
    last_output: Option<Receipt>,
    transfers: u64,
    irq_level: bool,
}
pub struct ImageTransfer {
    state: State,
    payload: Arc<[u8]>,
    source_payload: Arc<[u8]>,
    irq: Irq,
}
impl ImageTransfer {
    pub fn new(irq: Irq) -> Self {
        Self {
            state: State {
                r: [0x10000, 0, 0, 0, 0, 0, 0, 0],
                reset: None,
                pending: None,
                last_output: None,
                transfers: 0,
                irq_level: false,
            },
            payload: Arc::from([]),
            source_payload: Arc::from([]),
            irq,
        }
    }
    fn publish_irq(&mut self, cx: &mut Cx<'_>) -> Result<(), String> {
        let high = self.state.r[5] & self.state.r[7] != 0;
        if high != self.state.irq_level {
            cx.set_core_irq(self.irq.core, self.irq.line, high)?;
            self.state.irq_level = high;
        }
        Ok(())
    }
    fn start(&mut self, cx: &Cx<'_>, command: u32) -> Result<(), String> {
        let r = &self.state.r;
        let width = (r[4] & 0xfff) as usize;
        let height = (r[4] >> 16) as usize;
        let pitch = r[3] as usize * 2;
        let shift = if width.is_multiple_of(128) {
            3
        } else if width.is_multiple_of(64) {
            2
        } else if width.is_multiple_of(32) {
            1
        } else {
            0
        };
        // Only the declared planar420 input and packed422 output layout is admitted. The alignment
        // field selects transfer grouping; it does not resize the raster.
        if command != 0x10101 | (shift << 4)
            || width == 0
            || height == 0
            || !width.is_multiple_of(2)
            || !height.is_multiple_of(2)
            || pitch != width * 2
        {
            return Err(
                "image transfer requires aligned even I420 and tightly packed422 rows".into(),
            );
        }
        let bytes = pitch
            .checked_mul(height)
            .ok_or("image transfer size overflow")?;
        let source_bytes = width * height * 3 / 2;
        let source = physical(r[1].into(), source_bytes)?;
        let destination = physical(r[2].into(), bytes)?;
        if !source.is_multiple_of(4)
            || !destination.is_multiple_of(4)
            || source < destination + bytes as u64 && destination < source + source_bytes as u64
        {
            return Err("image transfer DMA is unaligned or overlapping".into());
        }
        let source_payload: Arc<[u8]> = cx.ram_bytes(source, source_bytes)?.into();
        let payload: Arc<[u8]> =
            fujimod_emulator_image_processing::yuv::i420_to_yycbcr(width, height, &source_payload)?
                .into();
        cx.ram_bytes(destination, bytes)?;
        let hash = fujimod_emulator_devices::fingerprint::bytes(&payload)["sha256"]
            .as_str()
            .ok_or("image transfer digest absent")?
            .to_owned();
        let source_hash = fujimod_emulator_devices::fingerprint::bytes(&source_payload)["sha256"]
            .as_str()
            .ok_or("image transfer input digest absent")?
            .to_owned();
        let completed_tick = due(cx.now(), (bytes as u64).div_ceil(256).max(2))?;
        let receipt = Receipt {
            source,
            destination,
            width,
            height,
            pitch,
            bytes,
            source_sha256: source_hash,
            source_packing: "i420".into(),
            source_pitch: width,
            source_bytes,
            output_sha256: hash,
            packing: "yycbcr".into(),
            started_tick: cx.now(),
            completed_tick,
        };
        self.payload = payload;
        self.source_payload = source_payload;
        self.state.pending = Some(receipt);
        Ok(())
    }
    fn decoded(&self, saved: &Checkpoint) -> Result<State, String> {
        let state: State = saved.decode(self.name())?;
        if saved.blobs().len() != 2
            || state.irq_level != (state.r[5] & state.r[7] != 0)
            || state.reset.is_some() && state.pending.is_some()
        {
            return Err("image transfer checkpoint state differs".into());
        }
        if let Some(p) = &state.pending {
            physical(p.source, p.source_bytes)?;
            physical(p.destination, p.bytes)?;
            if !p.source.is_multiple_of(4)
                || !p.destination.is_multiple_of(4)
                || p.source < p.destination + p.bytes as u64
                    && p.destination < p.source + p.source_bytes as u64
                || state.r[1] as u64 != p.source
                || state.r[2] as u64 != p.destination
                || state.r[3] as usize * 2 != p.pitch
                || (state.r[4] & 0xfff) as usize != p.width
                || (state.r[4] >> 16) as usize != p.height
                || p.width == 0
                || p.width > 4095
                || p.height == 0
                || p.height > 4095
                || !p.width.is_multiple_of(2)
                || !p.height.is_multiple_of(2)
                || p.source_packing != "i420"
                || p.source_pitch != p.width
                || p.source_bytes != p.width * p.height * 3 / 2
                || p.pitch != p.width * 2
                || p.bytes != p.pitch * p.height
                || p.packing != "yycbcr"
                || p.completed_tick <= p.started_tick
                || saved.blobs()[0].len() != p.bytes
                || saved.blobs()[1].len() != p.source_bytes
                || fujimod_emulator_devices::fingerprint::bytes(&saved.blobs()[1])["sha256"]
                    .as_str()
                    != Some(&p.source_sha256)
                || fujimod_emulator_devices::fingerprint::bytes(&saved.blobs()[0])["sha256"]
                    .as_str()
                    != Some(&p.output_sha256)
            {
                return Err("image transfer checkpoint payload differs".into());
            }
            if fujimod_emulator_image_processing::yuv::i420_to_yycbcr(
                p.width,
                p.height,
                &saved.blobs()[1],
            )?
            .as_slice()
                != saved.blobs()[0].as_ref()
            {
                return Err("image transfer checkpoint conversion differs".into());
            }
        } else if saved.blobs().iter().any(|b| !b.is_empty()) {
            return Err("image transfer checkpoint has inactive payload".into());
        }
        Ok(state)
    }
}
impl Device for ImageTransfer {
    fn checkpoint(&self) -> Result<Checkpoint, String> {
        Ok(Checkpoint::encode(self.name(), &self.state)?
            .with_blobs(vec![self.payload.clone(), self.source_payload.clone()]))
    }
    fn validate_checkpoint(&self, saved: &Checkpoint) -> Result<(), String> {
        self.decoded(saved).map(|_| ())
    }
    fn restore_checkpoint(&mut self, saved: &Checkpoint) -> Result<(), String> {
        self.state = self.decoded(saved)?;
        self.payload = saved.blobs()[0].clone();
        self.source_payload = saved.blobs()[1].clone();
        Ok(())
    }
    fn fingerprint(&self) -> Result<serde_json::Value, String> {
        Ok(
            serde_json::json!({"state":self.state,"payload":fujimod_emulator_devices::fingerprint::bytes(&self.payload),"source_payload":fujimod_emulator_devices::fingerprint::bytes(&self.source_payload)}),
        )
    }
    fn name(&self) -> &'static str {
        "exr2-isp-image-transfer"
    }
    fn read(&mut self, cx: &mut Cx<'_>, a: u64, s: u8) -> u64 {
        self.advance(cx);
        let Some(o) = word(cx, a, s, 0xfff64000, 0x20) else {
            return 0;
        };
        self.state.r[o / 4] as u64
    }
    fn write(&mut self, cx: &mut Cx<'_>, a: u64, s: u8, v: u64) {
        self.advance(cx);
        let Some(o) = word(cx, a, s, 0xfff64000, 0x20) else {
            return;
        };
        if v > u32::MAX as u64 {
            cx.unmodeled();
            return;
        }
        let v = v as u32;
        let mask = match o {
            0 => 0x10131,
            4 | 8 => 0x3fffffff,
            0xc => 0x7fff,
            0x10 => 0x0fff0fff,
            0x18 | 0x1c => 0x31,
            _ => 0,
        };
        if o == 0x14 || v & !mask != 0 {
            cx.fail("unknown image transfer register bits");
            return;
        }
        if o == 0 && v == 0 {
            match due(cx.now(), 2) {
                Ok(t) => {
                    self.state.reset = Some(t);
                    self.state.pending = None;
                    self.payload = Arc::from([]);
                    self.source_payload = Arc::from([]);
                    self.state.r[0] = 0;
                    self.state.r[5] = 0;
                    if let Err(e) = self.publish_irq(cx) {
                        cx.fail(e)
                    }
                }
                Err(e) => cx.fail(e),
            }
            return;
        }
        if self.state.reset.is_some() || self.state.pending.is_some() && !matches!(o, 0x18 | 0x1c) {
            cx.fail("image transfer reconfiguration while busy");
            return;
        }
        if o == 0 {
            if v & 1 != 0 {
                if let Err(e) = self.start(cx, v) {
                    cx.fail(e);
                    return;
                }
                self.state.r[0] = v & !0x10000;
                return;
            }
            if v & 0x10100 != 0x10000 {
                cx.fail("unknown image transfer control command");
                return;
            }
        }
        if o == 0x18 {
            self.state.r[5] &= !v;
        } else {
            self.state.r[o / 4] = v;
        }
        if let Err(e) = self.publish_irq(cx) {
            cx.fail(e)
        }
    }
    fn next_deadline(&self, _: u64) -> Option<u64> {
        self.state
            .reset
            .into_iter()
            .chain(self.state.pending.as_ref().map(|p| p.completed_tick))
            .min()
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        if self.state.reset.is_some_and(|t| cx.now() >= t) {
            self.state.reset = None;
            self.state.r[0] = 0x10000;
        }
        if self
            .state
            .pending
            .as_ref()
            .is_some_and(|p| cx.now() >= p.completed_tick)
        {
            let p = self.state.pending.as_ref().unwrap();
            if let Err(e) = cx.ram_write(p.destination, &self.payload) {
                cx.fail(e);
                return;
            }
            self.state.last_output = self.state.pending.take();
            self.payload = Arc::from([]);
            self.source_payload = Arc::from([]);
            self.state.transfers += 1;
            self.state.r[0] = (self.state.r[0] & 0x30) | 0x10000;
            self.state.r[5] |= 1;
            if let Err(e) = self.publish_irq(cx) {
                cx.fail(e)
            }
        }
    }
    fn event(&mut self, cx: &mut Cx<'_>, _: u64) {
        self.advance(cx)
    }
    fn save(&self) -> serde_json::Value {
        serde_json::json!({"registers":self.state.r,"reset":self.state.reset,"pending":self.state.pending,
            "transfers":self.state.transfers,"last_output":self.state.last_output})
    }
}
