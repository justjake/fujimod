//! FE80 addressed parameter DMA and a declared fourteen-threshold luma engine.
//! Uploads remain opaque in ProgramUpload. This separately selected datapath
//! consumes only channel descriptors, image bytes, frame edges, and IRQ wiring.
use super::companion;
use fujimod_emulator_imaging::exr2_isp::dma;
use fujimod_emulator_devices::{Cx, Device};
use fujimod_emulator_imaging::scene::ImageFrames;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub irq: u32,
    pub pixels_per_tick: u64,
}
pub struct Wiring {
    pub frames: ImageFrames,
    pub routes: fujimod_emulator_devices::exr2::irq_routes::IrqRoutes,
}
#[derive(Clone, Serialize, Deserialize)]
struct Job {
    packet: Vec<u8>,
    started: u64,
    due: Option<u64>,
    output: Vec<u8>,
    route: fujimod_emulator_devices::exr2::irq_routes::Irq,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    options: Options,
    registers: BTreeMap<u64, u32>,
    job: Option<Job>,
    completed: u64,
    last_frame: u64,
}
pub struct Processing {
    upload: companion::ProgramUpload,
    options: Options,
    wiring: Wiring,
    registers: BTreeMap<u64, u32>,
    job: Option<Job>,
    completed: u64,
    last_frame: u64,
}
fn word(data: &[u8], i: usize) -> u32 {
    u32::from_le_bytes(data[i..i + 4].try_into().unwrap())
}

fn threshold_energies(
    pixels: &[u8],
    width: usize,
    [x, y, w, h]: [usize; 4],
    lut: &[u8],
    thresholds: [u8; 14],
) -> [u64; 14] {
    let mut counts = [0u64; 256];
    for row in y..y + h {
        let row = &pixels[row * width * 2..(row + 1) * width * 2];
        let sample = |col: usize| i32::from(lut[row[(col / 2) * 4 + col % 2] as usize]);
        let mut previous = sample(x);
        for col in x + 1..x + w {
            let current = sample(col);
            counts[(current - previous).unsigned_abs() as usize] += 1;
            previous = current;
        }
    }
    // Each output uses the same gradients with a different strict threshold.
    // A suffix sum replaces fourteen comparisons per source pixel.
    let mut above = [0u64; 256];
    let mut sum = 0;
    for difference in (0..256).rev() {
        above[difference] = sum;
        sum += counts[difference] * (difference * difference) as u64;
    }
    thresholds.map(|threshold| above[threshold as usize])
}
impl Processing {
    pub fn new(
        transport: companion::Options,
        options: Options,
        wiring: Wiring,
    ) -> Result<Self, String> {
        if options.irq >= 128 || options.pixels_per_tick == 0 {
            return Err("FE80 processing requires an IRQ and positive pixel clock".into());
        }
        Ok(Self {
            upload: companion::ProgramUpload::new(transport)?,
            options,
            wiring,
            registers: BTreeMap::new(),
            job: None,
            completed: 0,
            last_frame: 0,
        })
    }
    fn snapshot(&self) -> Snapshot {
        Snapshot {
            options: self.options.clone(),
            registers: self.registers.clone(),
            job: self.job.clone(),
            completed: self.completed,
            last_frame: self.last_frame,
        }
    }
    fn validate_snapshot(&self, saved: Snapshot) -> Result<Snapshot, String> {
        if saved.options != self.options {
            return Err("FE80 processing checkpoint options differ".into());
        }
        if saved
            .registers
            .keys()
            .any(|a| !Self::channel(*a) && *a != 0xfe80c000)
        {
            return Err("invalid FE80 processing checkpoint registers".into());
        }
        if let Some(job) = &saved.job {
            if job.packet.len() != 44
                || job.output.len() != 56
                || job.route.line != self.options.irq
                || job.route.core > 1
            {
                return Err("invalid FE80 processing checkpoint job".into());
            }
            let (width, height, x, y, w, h) = Self::geometry(&job.packet)?;
            dma::physical(word(&job.packet, 0).into(), width * height * 2)?;
            dma::physical(word(&job.packet, 4).into(), 56)?;
            dma::physical(word(&job.packet, 40).into(), 256)?;
            let due = job
                .started
                .checked_add((w * h) as u64 / self.options.pixels_per_tick + 1);
            if x + w > width || y + h > height || job.due.is_none() || job.due != due {
                return Err("invalid FE80 processing checkpoint geometry or deadline".into());
            }
        }
        Ok(saved)
    }
    fn apply_snapshot(&mut self, saved: Snapshot) {
        self.registers = saved.registers;
        self.job = saved.job;
        self.completed = saved.completed;
        self.last_frame = saved.last_frame;
    }
    fn channel(a: u64) -> bool {
        (0..4).any(|i| {
            [
                0xfe80c100 + i * 0x30,
                0xfe80c108 + i * 0x30,
                0xfe80c200 + i * 0x30,
                0xfe80c208 + i * 0x30,
                0xfe80c440 + i * 0x40,
                0xfe80c448 + i * 0x40,
                0xfe80c840 + i * 0x40,
                0xfe80c848 + i * 0x40,
            ]
            .contains(&a)
        }) || [0xfe80d804, 0xfe80c400, 0xfe80c404, 0xfe80c800, 0xfe80c804].contains(&a)
    }
    fn start(&mut self, cx: &Cx<'_>, pointer: u32) -> Result<(), String> {
        if self.job.is_some() {
            return Err("FE80 processing channel already has work".into());
        }
        let desc = cx.ram_bytes(dma::physical(pointer.into(), 16)?, 16)?;
        if word(&desc, 0) != 0x03000207 || word(&desc, 8) != 44 || word(&desc, 12) != 0 {
            return Err("FE80 luma engine requires one 44-byte parameter DMA descriptor".into());
        }
        let packet = cx.ram_bytes(dma::physical(word(&desc, 4).into(), 44)?, 44)?;
        let (width, height, x, y, w, h) = Self::geometry(&packet)?;
        dma::physical(word(&packet, 0).into(), width * height * 2)?;
        dma::physical(word(&packet, 4).into(), 56)?;
        if x + w > width || y + h > height {
            return Err("FE80 luma window escapes input raster".into());
        }
        // The descriptor addresses an already captured memory raster. Latch
        // it at START; waiting for another sensor edge would pair its result
        // with a later exposure and can halve a continuous AF stream.
        let output = Self::calculate(cx, &packet)?;
        let due = cx
            .now()
            .checked_add((w * h) as u64 / self.options.pixels_per_tick + 1)
            .ok_or("FE80 processing deadline overflow")?;
        self.last_frame = self
            .wiring
            .frames
            .0
            .lock()
            .unwrap()
            .edge
            .map_or(0, |e| e.index);
        self.job = Some(Job {
            packet,
            started: cx.now(),
            due: Some(due),
            output,
            route: self.wiring.routes.latch(self.options.irq)?,
        });
        Ok(())
    }
    fn geometry(p: &[u8]) -> Result<(usize, usize, usize, usize, usize, usize), String> {
        let size = word(p, 8);
        let a = word(p, 32);
        let b = word(p, 36);
        let x = (a & 65535) as usize;
        let y = (a >> 16) as usize;
        // Packet coordinates store the far corner (920,202); treating those
        // fields as extents would read beyond the final source row.
        let v = (
            (size >> 16) as usize,
            (size & 65535) as usize,
            x,
            y,
            (b & 65535).saturating_sub(x as u32) as usize,
            (b >> 16).saturating_sub(y as u32) as usize,
        );
        if v.0 == 0 || v.0 > 8192 || v.0 % 2 != 0 || v.1 == 0 || v.1 > 8192 || v.4 == 0 || v.5 == 0
        {
            return Err("FE80 luma raster has unsupported geometry".into());
        }
        Ok(v)
    }
    fn calculate(cx: &Cx<'_>, p: &[u8]) -> Result<Vec<u8>, String> {
        let (width, height, x, y, w, h) = Self::geometry(p)?;
        let src = dma::physical(word(p, 0).into(), width * height * 2)?;
        let bytes = cx.ram_bytes(src, width * height * 2)?;
        let lut = cx.ram_bytes(dma::physical(word(p, 40).into(), 256)?, 256)?;
        // Declared, unmeasured arithmetic: inverse-tone luma differences above
        // each programmed threshold, squared and summed over the selected ROI.
        // This produces numeric measurements, never an AF result or position.
        let thresholds = [22, 21, 20, 19, 18, 17, 16, 30, 29, 28, 27, 26, 25, 24].map(|i| p[i]);
        let sums = threshold_energies(&bytes, width, [x, y, w, h], &lut, thresholds);
        Ok(sums
            .into_iter()
            .flat_map(|n| ((n / 256).min(u32::MAX as u64) as u32).to_le_bytes())
            .collect())
    }
    fn run(&mut self, cx: &mut Cx<'_>) -> Result<(), String> {
        self.upload.advance(cx);
        if let Some(job) = self.job.as_mut() {
            if job.due.is_some_and(|due| due <= cx.now()) {
                self.wiring.routes.validate(job.route)?;
                dma::write(cx, word(&job.packet, 4).into(), &job.output)?;
                self.registers.insert(0xfe80c100, 0x100);
                self.registers.insert(0xfe80c440, 0x40000);
                self.registers.insert(0xfe80d804, 0);
                self.registers.insert(
                    0xfe80c000,
                    self.registers.get(&0xfe80c000).copied().unwrap_or(0) | 1,
                );
                job.route.raise(cx)?;
                self.job = None;
                self.completed += 1;
            }
        }
        Ok(())
    }
    fn put(&mut self, cx: &mut Cx<'_>, a: u64, v: u32) -> Result<(), String> {
        let control = (0..4).any(|i| a == 0xfe80c440 + i * 0x40 || a == 0xfe80c840 + i * 0x40);
        let pointer = (0..4).any(|i| a == 0xfe80c108 + i * 0x30 || a == 0xfe80c208 + i * 0x30);
        let gate = (0..4).any(|i| a == 0xfe80c100 + i * 0x30 || a == 0xfe80c200 + i * 0x30);
        let config = (0..4).any(|i| a == 0xfe80c448 + i * 0x40 || a == 0xfe80c848 + i * 0x40);
        if control
            && !matches!(
                v,
                0 | 1 | 0x100 | 0x101 | 0x40000 | 0x40001 | 0x40100 | 0x40101
            )
            || pointer && (v == 0 || v & 3 != 0)
            || gate && !matches!(v, 0 | 1 | 0x100)
            || config && v != 0
        {
            return Err("unsupported FE80 channel field".into());
        }
        if a == 0xfe80c100 {
            if v == 1 {
                if self.registers.get(&0xfe80c440) != Some(&0x40001) {
                    return Err("FE80 parameter input requires its prepared control word".into());
                }
                let pointer = *self
                    .registers
                    .get(&0xfe80c108)
                    .ok_or("FE80 input channel lacks a descriptor")?;
                self.start(cx, pointer)?;
            } else if matches!(v, 0 | 0x100) {
                self.job = None;
            } else {
                return Err("unsupported FE80 input channel gate".into());
            }
        } else if gate && v == 1 {
            return Err("FE80 luma engine supports parameter input channel zero only".into());
        } else if [0xfe80c000, 0xfe80c400, 0xfe80c404, 0xfe80c800, 0xfe80c804].contains(&a) {
            let old = self.registers.get(&a).copied().unwrap_or(0);
            self.registers.insert(a, old & !v);
            return Ok(());
        } else if a == 0xfe80d804 {
            return Err("FE80 processing status is read-only".into());
        }
        self.registers.insert(a, v);
        if a == 0xfe80c100 && v == 1 {
            self.registers.insert(a, 3);
        }
        Ok(())
    }
}
impl Device for Processing {
    fn checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        Ok(
            fujimod_emulator_devices::checkpoint::Checkpoint::encode(self.name(), &self.snapshot())?
                .with_children(vec![self.upload.checkpoint()?]),
        )
    }
    fn validate_checkpoint(&self, saved: &fujimod_emulator_devices::checkpoint::Checkpoint) -> Result<(), String> {
        if saved.children().len() != 1 || !saved.blobs().is_empty() {
            return Err("FE80 processing checkpoint transport differs".into());
        }
        self.upload.validate_checkpoint(&saved.children()[0])?;
        self.validate_snapshot(saved.decode(self.name())?)
            .map(|_| ())
    }
    fn restore_checkpoint(&mut self, saved: &fujimod_emulator_devices::checkpoint::Checkpoint) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        let state = saved.decode(self.name())?;
        self.upload.restore_checkpoint(&saved.children()[0])?;
        self.apply_snapshot(state);
        Ok(())
    }

    fn name(&self) -> &'static str {
        "fe80-luma-processing"
    }
    fn read(&mut self, cx: &mut Cx<'_>, a: u64, size: u8) -> u64 {
        self.advance(cx);
        if a == 0xfe80c000 && size == 4 {
            return self.upload.read(cx, a, size)
                | u64::from(self.registers.get(&a).copied().unwrap_or(0));
        }
        if Self::channel(a) {
            if size != 4 || a & 3 != 0 {
                cx.fail("FE80 channels require aligned words");
                return 0;
            }
            return self.registers.get(&a).copied().unwrap_or(0).into();
        }
        self.upload.read(cx, a, size)
    }
    fn write(&mut self, cx: &mut Cx<'_>, a: u64, size: u8, v: u64) {
        self.advance(cx);
        if cx.refusal().is_some() {
            return;
        }
        if Self::channel(a) || a == 0xfe80c000 && v != 0x10 && v & !0x0f0f == 0 {
            if size != 4 || a & 3 != 0 || v > u32::MAX as u64 {
                cx.fail("FE80 channels require aligned words");
                return;
            }
            if let Err(e) = self.put(cx, a, v as u32) {
                cx.fail(e);
            }
        } else {
            if a == 0xfe80d840 {
                self.job = None;
                self.registers.clear();
            }
            self.upload.write(cx, a, size, v);
        }
    }
    fn next_deadline(&self, now: u64) -> Option<u64> {
        [
            self.upload.next_deadline(now),
            self.job.as_ref().and_then(|j| j.due),
        ]
        .into_iter()
        .flatten()
        .min()
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        if let Err(e) = self.run(cx) {
            cx.fail(e);
        }
    }
    fn event(&mut self, cx: &mut Cx<'_>, token: u64) {
        self.upload.event(cx, token);
        self.advance(cx);
    }
    fn save(&self) -> Value {
        json!({"transport":self.upload.save(),"options":self.options,
        "registers":self.registers,"job":self.job,"completed":self.completed,"last_frame":self.last_frame})
    }
    fn fingerprint(&self) -> Result<Value, String> {
        Ok(self.save())
    }
    fn restore(&mut self, v: &Value) -> Result<(), String> {
        let mut fields = v
            .as_object()
            .ok_or("FE80 processing state must be an object")?
            .clone();
        let transport = fields
            .remove("transport")
            .ok_or("missing FE80 transport state")?;
        let saved = self.validate_snapshot(
            serde_json::from_value(Value::Object(fields))
                .map_err(|e| format!("invalid FE80 processing state: {e}"))?,
        )?;
        self.upload.restore(&transport)?;
        self.apply_snapshot(saved);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn gradient_histogram_matches_all_scalar_thresholds_and_unaligned_windows() {
        let width = 38;
        let pixels = (0..width * 7 * 2)
            .map(|i| ((i * 113 + i / 19 * 73) % 256) as u8)
            .collect::<Vec<_>>();
        let lut: Vec<_> = (0..256).map(|i| ((i * 73 + 19) % 256) as u8).collect();
        for x in 0..4 {
            for w in [1, 2, 17, 33] {
                for threshold in (0..256).step_by(14) {
                    let thresholds = std::array::from_fn(|i| (threshold + i).min(255) as u8);
                    let mut expected = [0u64; 14];
                    for row in 1..6 {
                        let sample = |col: usize| {
                            i32::from(lut[pixels[row * width * 2 + col / 2 * 4 + col % 2] as usize])
                        };
                        for col in x + 1..x + w {
                            let d = (sample(col) - sample(col - 1)).unsigned_abs();
                            for i in 0..14 {
                                if d > u32::from(thresholds[i]) {
                                    expected[i] += u64::from(d * d);
                                }
                            }
                        }
                    }
                    assert_eq!(
                        super::threshold_energies(&pixels, width, [x, 1, w, 5], &lut, thresholds),
                        expected
                    );
                }
            }
        }
    }
    use super::*;
    use fujimod_emulator_devices::test_host::{TestHost, advance, write};
    use fujimod_emulator_devices::Host;
    use fujimod_emulator_imaging::analysis::ImageLayout;
    fn fixture() -> (Processing, ImageFrames, TestHost) {
        let frames = ImageFrames::new(ImageLayout {
            width: 8,
            height: 2,
            stride: 16,
        })
        .unwrap();
        let routes = fujimod_emulator_devices::exr2::irq_routes::IrqRoutes::default();
        routes.set(88, Some(0)).unwrap();
        let d = Processing::new(
            companion::Options::default(),
            Options {
                irq: 88,
                pixels_per_tick: 4,
            },
            Wiring {
                frames: frames.clone(),
                routes,
            },
        )
        .unwrap();
        let mut h = TestHost::new();
        let descriptor = [0x03000207u32, 0x5000, 44, 0]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect::<Vec<_>>();
        h.ram_write(0x4000, &descriptor).unwrap();
        let mut p = vec![0u8; 44];
        for (off, v) in [
            (0, 0x6000u32),
            (4, 0x8000),
            (8, 8 << 16 | 2),
            (32, 0),
            (36, 2 << 16 | 8),
            (40, 0x7000),
        ] {
            p[off..off + 4].copy_from_slice(&v.to_le_bytes());
        }
        p[24] = 100;
        h.ram_write(0x5000, &p).unwrap();
        h.ram_write(0x7000, &(0..=255).collect::<Vec<u8>>())
            .unwrap();
        let pixels = (0..2)
            .flat_map(|_| (0..4).flat_map(|x| [x * 40, x * 40 + 20, 128, 128]))
            .collect::<Vec<u8>>();
        h.ram_write(0x6000, &pixels).unwrap();
        h.effects.clear();
        (d, frames, h)
    }
    #[test]
    fn addressed_pixels_start_clock_deadline_and_output_precede_irq() {
        let (mut d, _frames, mut h) = fixture();
        write(&mut d, &mut h, 0xfe80c108, 0x4000);
        write(&mut d, &mut h, 0xfe80c440, 0x40001);
        write(&mut d, &mut h, 0xfe80c100, 1);
        assert_eq!(d.next_deadline(0), Some(5));
        let saved = d.save();
        let checkpoint = d.checkpoint().unwrap();
        assert_eq!(d.save(), saved);
        let (mut replay, _, _) = fixture();
        replay.validate_checkpoint(&checkpoint).unwrap();
        replay.restore_checkpoint(&checkpoint).unwrap();
        assert_eq!(replay.fingerprint().unwrap(), d.fingerprint().unwrap());
        h.ram[0x6000..0x6020].fill(0); // In-flight DMA has latched its pixels.
        advance(&mut d, &mut h, 4);
        assert!(h.effects.is_empty());
        assert_eq!(d.save(), saved); // Pure inspection and early ticks.
        advance(&mut d, &mut h, 5);
        assert_eq!(h.effects, ["dma", "irq"]);
        assert_eq!(h.irqs, [(0, 88)]);
        assert_eq!(d.read(&mut h.cx(), 0xfe80c000, 4) & 1, 1);
        assert_eq!(word(&h.ram, 0x8000), 21);
        assert_eq!(word(&h.ram, 0x8034), 0);
        let mut h2 = TestHost::new();
        h2.now = 5;
        replay.advance(&mut h2.cx());
        assert_eq!(&h.ram[0x8000..0x8038], &h2.ram[0x8000..0x8038]);
        assert_eq!(d.save(), replay.save());
    }
    #[test]
    fn binary_checkpoint_keeps_latched_work_deadline_and_upload_transport() {
        let (mut d, frames, mut h) = fixture();
        write(&mut d, &mut h, 0xfe80c108, 0x4000);
        write(&mut d, &mut h, 0xfe80c440, 0x40001);
        write(&mut d, &mut h, 0xfe80c100, 1);
        let before = d.fingerprint().unwrap();
        let saved = d.checkpoint().unwrap();
        let wire = std::sync::Arc::clone(&frames.0);
        let mut binary = Vec::new();
        ciborium::into_writer(&saved, &mut binary).unwrap();
        let saved: fujimod_emulator_devices::checkpoint::Checkpoint = ciborium::from_reader(&*binary).unwrap();
        assert_eq!(d.fingerprint().unwrap(), before);
        assert!(h.effects.is_empty());
        h.ram[0x6000..0x6020].fill(0);
        advance(&mut d, &mut h, 5);
        let after = d.fingerprint().unwrap();
        let output = h.ram[0x8000..0x8038].to_vec();
        d.restore_checkpoint(&saved).unwrap();
        assert!(std::sync::Arc::ptr_eq(&wire, &d.wiring.frames.0));
        assert_eq!(d.fingerprint().unwrap(), before);
        assert_eq!(d.next_deadline(0), Some(5));
        let mut replay = TestHost::new();
        advance(&mut d, &mut replay, 4);
        assert!(replay.effects.is_empty());
        advance(&mut d, &mut replay, 5);
        assert_eq!(replay.effects, ["dma", "irq"]);
        assert_eq!(replay.irqs, [(0, 88)]);
        assert_eq!(&replay.ram[0x8000..0x8038], &output);
        assert_eq!(d.fingerprint().unwrap(), after);

        let (mut d, _, mut h) = fixture();
        write(&mut d, &mut h, 0xfe80d840, 0);
        write(&mut d, &mut h, 0xfe80d844, 1);
        let saved = d.checkpoint().unwrap();
        let before = d.fingerprint().unwrap();
        let deadline = d.next_deadline(0).unwrap();
        advance(&mut d, &mut h, deadline);
        let after = d.fingerprint().unwrap();
        d.restore_checkpoint(&saved).unwrap();
        assert_eq!(d.fingerprint().unwrap(), before);
        assert_eq!(d.next_deadline(0), Some(deadline));
        let mut replay = TestHost::new();
        advance(&mut d, &mut replay, deadline);
        assert_eq!(d.fingerprint().unwrap(), after);
    }
    #[test]
    fn binary_checkpoint_rejects_missing_child_and_invalid_job_without_mutation() {
        let (mut d, _, mut h) = fixture();
        write(&mut d, &mut h, 0xfe80c108, 0x4000);
        write(&mut d, &mut h, 0xfe80c440, 0x40001);
        write(&mut d, &mut h, 0xfe80c100, 1);
        let saved = d.checkpoint().unwrap();
        let before = d.fingerprint().unwrap();
        assert!(
            d.restore_checkpoint(&saved.clone().with_children(Vec::new()))
                .is_err()
        );
        assert_eq!(d.fingerprint().unwrap(), before);
        let mut state: Snapshot = saved.decode(d.name()).unwrap();
        let job = state.job.as_mut().unwrap();
        job.due = Some(job.started);
        let corrupt = fujimod_emulator_devices::checkpoint::Checkpoint::encode(d.name(), &state)
            .unwrap()
            .with_children(saved.children().to_vec());
        assert!(d.restore_checkpoint(&corrupt).is_err());
        assert_eq!(d.fingerprint().unwrap(), before);
        let mut state: Snapshot = saved.decode(d.name()).unwrap();
        state.options.pixels_per_tick += 1;
        let corrupt = fujimod_emulator_devices::checkpoint::Checkpoint::encode(d.name(), &state)
            .unwrap()
            .with_children(saved.children().to_vec());
        assert!(d.restore_checkpoint(&corrupt).is_err());
        assert_eq!(d.fingerprint().unwrap(), before);
    }
    #[test]
    fn unknown_descriptor_and_corrupt_pending_state_refuse() {
        let (mut d, _, mut h) = fixture();
        h.ram[0x4008..0x400c].copy_from_slice(&80u32.to_le_bytes());
        write(&mut d, &mut h, 0xfe80c108, 0x4000);
        write(&mut d, &mut h, 0xfe80c440, 0x40001);
        let before = d.save();
        let mut cx = h.cx();
        d.write(&mut cx, 0xfe80c100, 4, 1);
        assert!(cx.refusal().unwrap().contains("44-byte"));
        assert_eq!(d.save(), before);
        assert!(h.effects.is_empty());
        let mut corrupt = d.save();
        corrupt["job"] = json!({"packet":[0],"started":0,"due":null,
            "output":[],"route":{"core":0,"line":88}});
        assert!(d.restore(&corrupt).is_err());
        assert_eq!(d.save(), before);
        let mut corrupt = before.clone();
        corrupt["completed"] = serde_json::Value::Null;
        corrupt["registers"][format!("{}", 0xfe80c108u64)] = json!(0x9000);
        assert!(d.restore(&corrupt).is_err());
        assert_eq!(d.save(), before);
    }
    #[test]
    fn typed_pending_checkpoint_refuses_invalid_deadlines_and_transport_before_mutation() {
        let (mut d, _, mut h) = fixture();
        write(&mut d, &mut h, 0xfe80c108, 0x4000);
        write(&mut d, &mut h, 0xfe80c440, 0x40001);
        write(&mut d, &mut h, 0xfe80c100, 1);
        let saved = d.checkpoint().unwrap();
        let before = d.fingerprint().unwrap();
        let mut bad: Snapshot = saved.decode(d.name()).unwrap();
        let job = bad.job.as_mut().unwrap();
        job.due = Some(job.started);
        let corrupt = fujimod_emulator_devices::checkpoint::Checkpoint::encode(d.name(), &bad)
            .unwrap()
            .with_children(saved.children().to_vec());
        assert!(d.validate_checkpoint(&corrupt).is_err());
        assert!(d.restore_checkpoint(&corrupt).is_err());
        assert_eq!(d.fingerprint().unwrap(), before);
        let missing_transport = saved.clone().with_children(vec![]);
        assert!(d.restore_checkpoint(&missing_transport).is_err());
        assert_eq!(d.fingerprint().unwrap(), before);
        assert!(h.effects.is_empty());
        advance(&mut d, &mut h, 4);
        assert_eq!(d.fingerprint().unwrap(), before);
        advance(&mut d, &mut h, 5);
        assert_eq!(h.effects, ["dma", "irq"]);
        assert_eq!(h.irqs, [(0, 88)]);
    }
}
