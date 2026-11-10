//! X-Processor Pro imaging statistics engine at 0xfff54000.
//!
//! X-T20 2.11 section 5 programs the configuration and output buffer words
//! +0x04..+0x1e4, enables interrupts with the low ten bits of +0x04 and
//! starts the engine with bit 28 of +0x1e8. The level handler 0xc36280 reads
//! the status +0x08 (bits 0..9) and writes it back to clear it. The stop
//! sequence 0xc365a8 clears +0x00 bit 0, waits until +0x00 reads exactly 1,
//! then until +0x1ec bit 16 (idle) reads set, and clears the start bit.
//!
//! While started, the engine finishes a frame at every frame tick, counted
//! from tick zero with the frame sync's declared period, and sets every
//! enabled status bit; the interrupt line is high while an enabled status bit
//! is set. No frame is ever in flight, so +0x1ec reads only the idle bit and
//! +0x00 bit 0 reads set. Other words read back their last write.
//!
//! The twelve output buffers are the words +0x1b8..+0x1e4; each holds the
//! address of the half it writes, and +0x10 is the distance between the two
//! halves (0x3690). The firmware alternates those addresses itself, frame by
//! frame: the section 6 statistics consumer 0x16963dc reads the half a
//! completion filled, then hands the other half's addresses to the section 5
//! setters 0xc38d5c..0xc38fc0 (through 0x156a470/0x1568ad8), which reprogram
//! the words. Measured on X-T20 2.11 from 253M instructions on: +0x1cc
//! alternates 0x3f480360/0x3f4839f0 on every completion while CONTROL stays
//! at START (no per-frame restart), so both halves hold data one frame old.
//! Each completion therefore writes its buffers at exactly the programmed
//! address, in the half the firmware chose; the engine does not add +0x10
//! itself (doing so would double the firmware's offset and leave the first
//! half stale). With a declared scene, each frame writes the zone sums buffer
//! (+0x1cc) of the half it completes: three planes of 16x16 u32 zone sums
//! (red, green, blue; green is about 2.6 times red, probably both green
//! sites) and a fourth plane of zeros, as in the camera's captures. The
//! scene is uniform, so every zone of a plane holds the declared value.
//! A declared picture ([`ZoneScene`]) replaces the uniform value with the
//! mean linear radiance of each zone, sampled once when the picture is
//! declared; the sums keep the declared scale.
//! With a picture, three more buffers follow it. The first window record
//! (+0x1d0 words 0, 2 and 4) scales the declared wall words by the mean
//! radiance of the centre 2x2 zones over the wall's declared radiance. The
//! coarse planes (+0x1d4, three 8x8 planes) hold the sum of each cell's 2x2
//! zones times the camera's measured 2.2%. The reference planes (+0x1d8,
//! three 8x8 planes) hold the constants 5084, 12710 and 5084, unscaled. The
//! 256-bin histogram (+0x1e4) bins the display-coded luma of the picture,
//! half a code per bin, on a regular grid, with its counts scaled to the
//! camera's measured total of 725454; it does not take the exposure or
//! scene-light scale. All of these are written at the address the buffer's
//! output word holds; a word that holds no half (0) writes nothing, as the
//! firmware has not programmed that buffer. Without a picture, none of the
//! three is written, and the window keeps its declared words.
//! Declared buffers name one of the twelve output words and the u32 words
//! the engine writes there each frame, in the half it completes (measured
//! buffers of the same scene). Declared light words of a buffer are linear
//! light sums and follow the same scale as the zone sums; the other words are
//! written as declared. The other buffers are not written.
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Register window size in bytes.
pub const WINDOW: u64 = 0x200;
const RUN: u32 = 1;
const ENABLE: u64 = 0x04;
const STATUS: u64 = 0x08;
const CONTROL: u64 = 0x1e8;
const STATE: u64 = 0x1ec;
const SOURCES: u32 = 0x3ff;
const START: u32 = 1 << 28;
const IDLE: u32 = 1 << 16;
const ZONE_SUMS: u64 = 0x1cc;
const ZONES: usize = 256;
const OUTPUTS: std::ops::RangeInclusive<u64> = 0x1b8..=0x1e4;
/// The window buffer: 80 u32 in records of eight, red, green and blue at
/// +0/+2/+4 of each record.
const WINDOW_OUTPUT: u64 = 0x1d0;
/// The words of the window's first record: the centre window's channels.
const WINDOW_CENTRE: [u32; 3] = [0, 2, 4];
/// The coarse planes: three 8x8 planes of 2x2 zone sums.
const COARSE_OUTPUT: u64 = 0x1d4;
/// The reference planes: three 8x8 planes of constants.
const REFERENCE_OUTPUT: u64 = 0x1d8;
/// The 256-bin luma histogram.
const HISTOGRAM_OUTPUT: u64 = 0x1e4;
/// The camera's coarse planes read 2.2% above the sum of their zones
/// (measured on one uniform scene).
const COARSE_GAIN: f64 = 1.022;
/// The camera's histogram counts sum to this in every scene (measured on one
/// uniform scene).
const HISTOGRAM_TOTAL: u32 = 725_454;
/// Samples per axis of the picture grid the histogram counts.
const HISTOGRAM_GRID: u64 = 64;
/// Each channel's reference-plane level: red, green, blue (the same in every
/// camera capture).
const REFERENCE_LEVELS: [u32; 3] = [5_084, 12_710, 5_084];

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    words: BTreeMap<u64, u32>,
    status: u32,
    due: Option<u64>,
    line: bool,
    frames: u64,
    /// Scene-light scale in thousandths of the factor (`LIGHT_UNIT` = 1.0).
    #[serde(default = "unit_light", skip_serializing_if = "is_unit_light")]
    light: u32,
}
impl Default for State {
    fn default() -> Self {
        Self {
            words: BTreeMap::new(),
            status: 0,
            due: None,
            line: false,
            frames: 0,
            light: LIGHT_UNIT,
        }
    }
}
fn unit_light() -> u32 {
    LIGHT_UNIT
}
fn is_unit_light(light: &u32) -> bool {
    *light == LIGHT_UNIT
}
/// Thousandths of the scene-light factor: a value of [`LIGHT_UNIT`] leaves the
/// declared scene unchanged.
pub const LIGHT_UNIT: u32 = 1000;
/// Largest scene-light factor the input accepts (1000).
const LIGHT_MAX: f64 = 1000.;
pub struct Engine {
    base: u64,
    irq: u32,
    period: u64,
    scene: Option<[u32; 3]>,
    picture: Option<Picture>,
    exposure: Option<Exposure>,
    light: Option<SceneLight>,
    buffers: Vec<Buffer>,
    state: State,
}
/// What a declared picture contributes to the buffers, computed once when it
/// is declared: the mean radiance of every zone, the centre window's mean,
/// the wall reference and the luma histogram.
struct Picture {
    sums: [u32; 3],
    zones: Vec<[f64; 3]>,
    centre: [f64; 3],
    reference: [f64; 3],
    histogram: [u32; 256],
}
/// One declared buffer: the output word (+0x1b8..+0x1e4), the u32 words the
/// engine writes there each frame, and the word indices that are linear light
/// sums.
struct Buffer {
    output: u64,
    words: Vec<u32>,
    light: Vec<u32>,
}
/// The integration value another device publishes (0 = not yet set) and the
/// value at which the declared zone sums hold: the sums scale by
/// value / reference, as a sensor's signal scales with its integration time.
pub struct Exposure {
    pub value: std::sync::Arc<std::sync::atomic::AtomicU32>,
    pub reference: u32,
}
impl Exposure {
    /// The published scale (1 while the value is 0, i.e. not yet published).
    pub fn factor(&self) -> f64 {
        match self.value.load(std::sync::atomic::Ordering::Relaxed) {
            0 => 1.,
            value => f64::from(value) / f64::from(self.reference),
        }
    }
}
/// A scene-light scale published by a runtime input
/// ([`Engine::set_scene_light`]) and shared with the devices that draw the same
/// light, in thousandths of the factor (0 is dark, [`LIGHT_UNIT`] unchanged).
pub struct SceneLight {
    pub value: std::sync::Arc<std::sync::atomic::AtomicU32>,
}
impl SceneLight {
    /// The current scale; a fresh input reads [`LIGHT_UNIT`].
    pub fn factor(&self) -> f64 {
        f64::from(self.value.load(std::sync::atomic::Ordering::Relaxed)) / f64::from(LIGHT_UNIT)
    }
}
/// A declared scene image for the zone sums and the measured wall it is
/// calibrated against: 8-bit sRGB rows, the wall's measured zone sums and the
/// wall's declared linear radiance (the same units as the sensor's 16383
/// full scale). Zone sums are linear in the scene's linearised radiance.
pub struct ZoneScene {
    pub width: u64,
    pub height: u64,
    pub rgb: Vec<u8>,
    pub sums: [u32; 3],
    pub radiance: [u16; 3],
}
impl ZoneScene {
    /// Linearised sample of the declared wall's radiance, 0..1.
    fn reference(&self, c: usize) -> f64 {
        f64::from(self.radiance[c]) / 16383.
    }
    /// The mean linear radiance of one zone, sampled on a 4x4 grid.
    fn zone(&self, zx: usize, zy: usize) -> [f64; 3] {
        const GRID: usize = 16;
        const STEPS: u64 = 4;
        let mut sum = [0.; 3];
        for sy in 0..STEPS {
            for sx in 0..STEPS {
                let x = (2 * (zx as u64 * STEPS + sx) + 1) * self.width / (2 * GRID as u64 * STEPS);
                let y =
                    (2 * (zy as u64 * STEPS + sy) + 1) * self.height / (2 * GRID as u64 * STEPS);
                let at =
                    ((y.min(self.height - 1) * self.width + x.min(self.width - 1)) * 3) as usize;
                for c in 0..3 {
                    sum[c] += linear(self.rgb[at + c]);
                }
            }
        }
        sum.map(|v| v / (STEPS * STEPS) as f64)
    }
}
/// sRGB byte to linear light, 0..1.
pub(crate) fn linear(sample: u8) -> f64 {
    let s = f64::from(sample) / 255.;
    if s <= 0.04045 {
        s / 12.92
    } else {
        ((s + 0.055) / 1.055).powf(2.4)
    }
}
/// Display-coded luma of one pixel: BT.601 weights, the matrix the declared
/// still profile encodes its Y'CbCr with.
fn luma(rgb: [u8; 3]) -> u8 {
    let y = 0.299 * f64::from(rgb[0]) + 0.587 * f64::from(rgb[1]) + 0.114 * f64::from(rgb[2]);
    y.round_ties_even().clamp(0., 255.) as u8
}
/// The picture's 256-bin luma histogram, half a code per bin, sampled on a
/// regular grid and scaled so the counts total [`HISTOGRAM_TOTAL`] exactly.
/// The exposure and scene-light scales do not enter it.
fn histogram(scene: &ZoneScene) -> [u32; 256] {
    let mut counts = [0u32; 256];
    for sy in 0..HISTOGRAM_GRID {
        for sx in 0..HISTOGRAM_GRID {
            let x = ((2 * sx + 1) * scene.width / (2 * HISTOGRAM_GRID)).min(scene.width - 1);
            let y = ((2 * sy + 1) * scene.height / (2 * HISTOGRAM_GRID)).min(scene.height - 1);
            let at = ((y * scene.width + x) * 3) as usize;
            let bin = usize::from(luma([scene.rgb[at], scene.rgb[at + 1], scene.rgb[at + 2]]) / 2);
            counts[bin] += 1;
        }
    }
    let samples: u64 = counts.iter().map(|count| u64::from(*count)).sum();
    let mut scaled = [0u32; 256];
    let mut before = 0u64;
    let mut counted = 0u64;
    for (bin, count) in counts.iter().enumerate() {
        counted += u64::from(*count);
        let upto = (counted * u64::from(HISTOGRAM_TOTAL) + samples / 2) / samples;
        scaled[bin] = (upto - before) as u32;
        before = upto;
    }
    scaled
}
/// The coarse planes: three 8x8 planes, each cell the sum of its 2x2 zones
/// times the measured gain.
fn coarse(zones: &[[u32; 3]]) -> Vec<u8> {
    (0..3)
        .flat_map(|channel| {
            (0..64).map(move |cell| {
                let (x, y) = (2 * (cell % 8), 2 * (cell / 8));
                let sum: u64 = [0, 1, 2, 3]
                    .map(|n| u64::from(zones[(y + n / 2) * 16 + x + n % 2][channel]))
                    .into_iter()
                    .sum();
                (sum as f64 * COARSE_GAIN)
                    .round()
                    .clamp(0., f64::from(u32::MAX)) as u32
            })
        })
        .flat_map(u32::to_le_bytes)
        .collect()
}
/// The reference planes: three 8x8 planes of each channel's constant, not
/// scaled by exposure or scene light.
fn reference_planes() -> Vec<u8> {
    REFERENCE_LEVELS
        .iter()
        .flat_map(|level| std::iter::repeat_n(level.to_le_bytes(), 64))
        .flatten()
        .collect()
}
impl Engine {
    pub fn new(base: u64, irq: u32, period: u64) -> Result<Self, String> {
        if base & 0xfff != 0 || !(32..512).contains(&irq) || period == 0 {
            return Err("Invalid X-Processor Pro statistics engine".into());
        }
        Ok(Self {
            base,
            irq,
            period,
            scene: None,
            picture: None,
            exposure: None,
            light: None,
            buffers: Vec::new(),
            state: State::default(),
        })
    }
    /// A uniform scene: the red, green and blue sum of every zone.
    pub fn with_scene(mut self, zone_sums: [u32; 3]) -> Self {
        self.scene = Some(zone_sums);
        self
    }
    /// The zone sums follow a declared scene image instead of one uniform
    /// value per plane; requires the measured wall sums (`with_scene`). The
    /// picture's zone means and histogram are computed here, once.
    pub fn with_zone_scene(mut self, scene: ZoneScene) -> Result<Self, String> {
        if self.scene.is_none()
            || !(1..=8192).contains(&scene.width)
            || !(1..=8192).contains(&scene.height)
            || scene.rgb.len() as u64 != scene.width * scene.height * 3
            || scene.sums.contains(&0)
            || scene.radiance.contains(&0)
        {
            return Err("Invalid X-Processor Pro zone scene".into());
        }
        let zones: Vec<[f64; 3]> = (0..ZONES)
            .map(|zone| scene.zone(zone % 16, zone / 16))
            .collect();
        let mut centre = [0.; 3];
        for zy in 7..9 {
            for zx in 7..9 {
                let zone = zones[zy * 16 + zx];
                for c in 0..3 {
                    centre[c] += zone[c] / 4.;
                }
            }
        }
        self.picture = Some(Picture {
            sums: scene.sums,
            reference: [scene.reference(0), scene.reference(1), scene.reference(2)],
            zones,
            centre,
            histogram: histogram(&scene),
        });
        Ok(self)
    }
    /// The zone sums follow a published integration value; requires the
    /// declared sums (`with_scene`).
    pub fn with_exposure(mut self, exposure: Exposure) -> Result<Self, String> {
        if self.scene.is_none() || exposure.reference == 0 {
            return Err("Invalid X-Processor Pro statistics exposure".into());
        }
        self.exposure = Some(exposure);
        Ok(self)
    }
    /// The declared scene's light follows a published scale
    /// ([`Engine::set_scene_light`]) as well; requires the declared sums.
    pub fn with_scene_light(mut self, light: SceneLight) -> Result<Self, String> {
        if self.scene.is_none() {
            return Err("Invalid X-Processor Pro statistics scene light".into());
        }
        light
            .value
            .store(self.state.light, std::sync::atomic::Ordering::Relaxed);
        self.light = Some(light);
        Ok(self)
    }
    /// Scale the declared scene's light by `factor` (1 leaves it unchanged);
    /// published to the devices that share the scale.
    pub fn set_scene_light(&mut self, factor: f64) -> Result<(), String> {
        if !factor.is_finite() || !(0. ..=LIGHT_MAX).contains(&factor) {
            return Err("Invalid X-Processor Pro scene light".into());
        }
        self.state.light = (factor * f64::from(LIGHT_UNIT)).round() as u32;
        self.publish_light();
        Ok(())
    }
    fn publish_light(&self) {
        if let Some(light) = &self.light {
            light
                .value
                .store(self.state.light, std::sync::atomic::Ordering::Relaxed);
        }
    }
    /// Scale of the declared scene light: the published integration value
    /// (1 until one is published) times the published scene-light scale.
    fn scene_factor(&self) -> f64 {
        let exposure = self.exposure.as_ref().map_or(1., Exposure::factor);
        exposure * f64::from(self.state.light) / f64::from(LIGHT_UNIT)
    }
    /// A declared buffer: the output word (+0x1b8..+0x1e4) and its contents.
    pub fn with_buffer(mut self, output: u64, words: &[u32]) -> Result<Self, String> {
        if !OUTPUTS.contains(&output)
            || output & 3 != 0
            || words.is_empty()
            || (output == ZONE_SUMS && self.scene.is_some())
            || self.buffers.iter().any(|buffer| buffer.output == output)
        {
            return Err("Invalid X-Processor Pro statistics buffer".into());
        }
        self.buffers.push(Buffer {
            output,
            words: words.to_vec(),
            light: Vec::new(),
        });
        Ok(self)
    }
    /// The words of a declared buffer that are linear light sums: `words` are
    /// `u32` indices into the buffer [`Engine::with_buffer`] declared at
    /// `output`. They follow the scene light and the published exposure like
    /// the zone sums; the other words are written as declared.
    pub fn with_buffer_light_words(mut self, output: u64, words: &[u32]) -> Result<Self, String> {
        let buffer = self
            .buffers
            .iter_mut()
            .find(|buffer| buffer.output == output)
            .ok_or("Invalid X-Processor Pro statistics buffer light words")?;
        if words.is_empty()
            || !buffer.light.is_empty()
            || words
                .iter()
                .any(|word| *word as usize >= buffer.words.len())
        {
            return Err("Invalid X-Processor Pro statistics buffer light words".into());
        }
        buffer.light = words.to_vec();
        Ok(self)
    }
    /// Write one output buffer at the address its word holds: the firmware
    /// programs the half it wants filled (see the module notes).
    fn write_output(&self, cx: &mut Cx<'_>, output: u64, data: &[u8]) {
        let address = u64::from(self.word(output));
        if cx.ram_write(address, data).is_err() {
            cx.fail("Statistics buffer outside RAM");
        }
    }
    /// Write a buffer the picture fills at the half its output word names; a
    /// word that holds no half (0) writes nothing, as the firmware has not
    /// programmed that buffer.
    fn write_scene_output(&self, cx: &mut Cx<'_>, output: u64, data: &[u8]) {
        if self.word(output) != 0 {
            self.write_output(cx, output, data);
        }
    }
    fn write_buffers(&self, cx: &mut Cx<'_>) {
        let factor = self.scene_factor();
        let scaled = |value: f64| (value * factor).round().clamp(0., f64::from(u32::MAX)) as u32;
        let values = if let Some(picture) = &self.picture {
            let mut values = vec![[0u32; 3]; ZONES];
            let mut planes = Vec::with_capacity(4 * ZONES * 4);
            for plane in 0..4 {
                for (zone, value) in values.iter_mut().enumerate() {
                    if plane < 3 {
                        value[plane] = scaled(
                            f64::from(picture.sums[plane]) * picture.zones[zone][plane]
                                / picture.reference[plane],
                        );
                    }
                    let word = if plane < 3 { value[plane] } else { 0 };
                    planes.extend_from_slice(&word.to_le_bytes());
                }
            }
            self.write_output(cx, ZONE_SUMS, &planes);
            Some(values)
        } else if let Some(scene) = self.scene {
            let mut planes = Vec::with_capacity(4 * ZONES * 4);
            for value in scene.map(|v| scaled(f64::from(v))).into_iter().chain([0]) {
                for _ in 0..ZONES {
                    planes.extend_from_slice(&value.to_le_bytes());
                }
            }
            self.write_output(cx, ZONE_SUMS, &planes);
            None
        } else {
            None
        };
        for buffer in &self.buffers {
            let data: Vec<u8> = buffer
                .words
                .iter()
                .enumerate()
                .flat_map(|(index, word)| {
                    let index = index as u32;
                    let word = match &self.picture {
                        Some(picture)
                            if buffer.output == WINDOW_OUTPUT && WINDOW_CENTRE.contains(&index) =>
                        {
                            let c = (index / 2) as usize;
                            scaled(f64::from(*word) * picture.centre[c] / picture.reference[c])
                        }
                        _ if buffer.light.contains(&index) => scaled(f64::from(*word)),
                        _ => *word,
                    };
                    word.to_le_bytes()
                })
                .collect();
            self.write_output(cx, buffer.output, &data);
        }
        if let (Some(picture), Some(values)) = (&self.picture, &values) {
            // These follow the picture, so they replace a declared buffer at
            // the same output word.
            self.write_scene_output(cx, COARSE_OUTPUT, &coarse(values));
            self.write_scene_output(cx, REFERENCE_OUTPUT, &reference_planes());
            let histogram: Vec<u8> = picture
                .histogram
                .iter()
                .flat_map(|bin| bin.to_le_bytes())
                .collect();
            self.write_scene_output(cx, HISTOGRAM_OUTPUT, &histogram);
        }
    }
    fn word(&self, offset: u64) -> u32 {
        self.state.words.get(&offset).copied().unwrap_or(0)
    }
    fn update_line(&mut self, cx: &mut Cx<'_>) {
        let level = self.state.status & self.word(ENABLE) & SOURCES != 0;
        if level != self.state.line {
            self.state.line = level;
            cx.set_irq(self.irq, level);
        }
    }
    fn offset(&self, address: u64, size: u8) -> Option<u64> {
        let offset = address.checked_sub(self.base)?;
        (size == 4 && offset & 3 == 0 && offset < WINDOW).then_some(offset)
    }
}
impl Device for Engine {
    fn name(&self) -> &'static str {
        "xpp-imaging-statistics"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        u64::from(match self.offset(address, size) {
            Some(0) => self.word(0) | RUN,
            Some(STATUS) => self.state.status,
            Some(STATE) => IDLE,
            Some(o) if o < STATE => self.word(o),
            _ => {
                cx.unmodeled();
                0
            }
        })
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let Ok(value) = u32::try_from(value) else {
            return cx.unmodeled();
        };
        match self.offset(address, size) {
            Some(STATUS) => self.state.status &= !value,
            Some(CONTROL) => {
                self.state.due = match (value & START != 0, self.state.due) {
                    (false, _) => None,
                    (true, Some(at)) => Some(at),
                    (true, None) => {
                        let now = cx.now();
                        (now / self.period)
                            .checked_add(1)
                            .and_then(|n| n.checked_mul(self.period))
                    }
                };
                self.state.words.insert(CONTROL, value);
            }
            Some(o) if o < STATE => {
                self.state.words.insert(o, value);
            }
            _ => return cx.unmodeled(),
        }
        self.update_line(cx);
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        let Some(due) = self.state.due.filter(|at| *at <= cx.now()) else {
            return;
        };
        self.state.status |= self.word(ENABLE) & SOURCES;
        self.state.frames += 1;
        self.write_buffers(cx);
        let frames = (cx.now() - due) / self.period + 1;
        self.state.due = frames
            .checked_mul(self.period)
            .and_then(|step| due.checked_add(step));
        if self.state.due.is_none() {
            cx.fail("Statistics engine deadline overflow");
        }
        self.update_line(cx);
    }
    fn next_deadline(&self, _now: u64) -> Option<u64> {
        self.state.due
    }
    fn save(&self) -> Value {
        serde_json::to_value(&self.state).unwrap_or(Value::Null)
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let state: State = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        self.check(&state)?;
        self.state = state;
        self.publish_light();
        Ok(())
    }
    fujimod_emulator_devices::checkpoint_state!(state: State, Self::check);
}
impl Engine {
    fn check(&self, state: &State) -> Result<(), String> {
        let started = state.words.get(&CONTROL).is_some_and(|w| w & START != 0);
        if started != state.due.is_some()
            || state.due.is_some_and(|at| at % self.period != 0)
            || state.words.keys().any(|o| o & 3 != 0 || *o >= STATE)
            || state.light > (LIGHT_MAX * f64::from(LIGHT_UNIT)) as u32
        {
            return Err("Invalid statistics engine checkpoint".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::test_host::{TestHost, advance, read, write};

    const BASE: u64 = 0xfff54000;

    /// The measured wall: its zone sums, the radiance they hold and the sRGB
    /// bytes whose linear light is that radiance (chosen to round-trip).
    const WALL_SUMS: [u32; 3] = [144_736, 383_321, 33_695];
    const WALL_RADIANCE: [u16; 3] = [2_001, 8_337, 15_238];
    const WALL_BYTES: [u8; 3] = [98, 189, 247];
    /// The first two records of the measured window buffer: red, green and
    /// blue at +0/+2/+4 of each record of eight.
    const WALL_WINDOW: [u32; 16] = [
        641_739, 0, 1_700_693, 0, 150_556, 0, 0, 0, 590, 0, 1_676, 0, 158, 0, 0, 0,
    ];

    /// sRGB byte whose linear light is `radiance` on the declared 16383 scale.
    fn srgb(radiance: u16) -> u8 {
        let linear = f64::from(radiance) / 16383.;
        let s = if linear <= 0.0031308 {
            linear * 12.92
        } else {
            1.055 * linear.powf(1. / 2.4) - 0.055
        };
        (s * 255.).round() as u8
    }

    fn picture(width: u64, height: u64, pixel: impl Fn(u64, u64) -> [u8; 3]) -> Vec<u8> {
        let mut rgb = Vec::with_capacity((width * height * 3) as usize);
        for y in 0..height {
            for x in 0..width {
                rgb.extend_from_slice(&pixel(x, y));
            }
        }
        rgb
    }

    fn zone_scene(rgb: Vec<u8>) -> ZoneScene {
        ZoneScene {
            width: 16,
            height: 16,
            rgb,
            sums: WALL_SUMS,
            radiance: WALL_RADIANCE,
        }
    }

    fn word(h: &TestHost, at: usize) -> u32 {
        u32::from_le_bytes(h.ram[at..at + 4].try_into().unwrap())
    }

    /// The declared wall radiance's linear value, as the wall reference.
    fn wall_reference(c: usize) -> f64 {
        f64::from(WALL_RADIANCE[c]) / 16383.
    }

    #[test]
    fn a_picture_at_the_wall_radiance_reproduces_the_declared_wall_buffers() {
        let mut stats = Engine::new(BASE, 106, 1000)
            .unwrap()
            .with_scene(WALL_SUMS)
            .with_zone_scene(zone_scene(picture(16, 16, |_, _| WALL_BYTES)))
            .unwrap()
            .with_buffer(WINDOW_OUTPUT, &WALL_WINDOW)
            .unwrap()
            .with_buffer_light_words(WINDOW_OUTPUT, &[0, 2, 4, 8, 10, 12])
            .unwrap();
        let mut h = TestHost::new();
        write(&mut stats, &mut h, BASE + ZONE_SUMS, 0x1000);
        write(&mut stats, &mut h, BASE + WINDOW_OUTPUT, 0x2000);
        write(&mut stats, &mut h, BASE + CONTROL, START);
        advance(&mut stats, &mut h, 1000);
        // The centre window's record holds the declared wall words (to the
        // unit the 8-bit wall bytes round by).
        let centre = [WALL_WINDOW[0], WALL_WINDOW[2], WALL_WINDOW[4]];
        for (word, declared) in [0, 8, 16]
            .map(|o| word(&h, 0x2000 + o))
            .into_iter()
            .zip(centre)
        {
            assert!(word.abs_diff(declared) <= 1, "{word} vs {declared}");
        }
        // The second record stays a declared light sum at unit light.
        assert_eq!(
            [32, 40, 48].map(|o| word(&h, 0x2000 + o)),
            [WALL_WINDOW[8], WALL_WINDOW[10], WALL_WINDOW[12]]
        );
        // Every zone of a plane holds the declared sum.
        for (c, &sum) in WALL_SUMS.iter().enumerate() {
            for zone in [0, 255] {
                assert_eq!(word(&h, 0x1000 + 0x400 * c + 4 * zone), sum);
            }
            assert_eq!(word(&h, 0x1000 + 0x400 * 3), 0);
        }
    }

    #[test]
    fn a_picture_with_a_bright_centre_raises_the_centre_window_and_its_zones() {
        let bright = srgb(4_002);
        let rgb = picture(16, 16, |x, y| {
            if (7..9).contains(&x) && (7..9).contains(&y) {
                [bright; 3]
            } else {
                WALL_BYTES
            }
        });
        let mut stats = Engine::new(BASE, 106, 1000)
            .unwrap()
            .with_scene(WALL_SUMS)
            .with_zone_scene(zone_scene(rgb))
            .unwrap()
            .with_buffer(WINDOW_OUTPUT, &WALL_WINDOW)
            .unwrap()
            .with_buffer_light_words(WINDOW_OUTPUT, &[0, 2, 4, 8, 10, 12])
            .unwrap();
        let mut h = TestHost::new();
        write(&mut stats, &mut h, BASE + ZONE_SUMS, 0x1000);
        write(&mut stats, &mut h, BASE + WINDOW_OUTPUT, 0x2000);
        write(&mut stats, &mut h, BASE + CONTROL, START);
        advance(&mut stats, &mut h, 1000);
        let ratio = linear(bright) / wall_reference(0);
        let expected =
            (f64::from(WALL_WINDOW[0]) * linear(bright) / wall_reference(0)).round() as u32;
        assert!(word(&h, 0x2000).abs_diff(expected) <= 1);
        // The centre zones rise with the window; a corner zone keeps the wall.
        let centre = word(&h, 0x1000 + 4 * (7 * 16 + 7));
        assert!((f64::from(centre) / f64::from(WALL_SUMS[0]) - ratio).abs() < 1e-3);
        assert_eq!(word(&h, 0x1000), WALL_SUMS[0]);
        assert!(word(&h, 0x2000) > WALL_WINDOW[0] * 3 / 2);
        // A red-heavy centre changes the window's red-to-green ratio.
        let red = picture(16, 16, |x, y| {
            if (7..9).contains(&x) && (7..9).contains(&y) {
                [255, WALL_BYTES[1], WALL_BYTES[2]]
            } else {
                WALL_BYTES
            }
        });
        let mut stats = Engine::new(BASE, 106, 1000)
            .unwrap()
            .with_scene(WALL_SUMS)
            .with_zone_scene(zone_scene(red))
            .unwrap()
            .with_buffer(WINDOW_OUTPUT, &WALL_WINDOW)
            .unwrap()
            .with_buffer_light_words(WINDOW_OUTPUT, &[0, 2, 4, 8, 10, 12])
            .unwrap();
        write(&mut stats, &mut h, BASE + ZONE_SUMS, 0x1000);
        write(&mut stats, &mut h, BASE + WINDOW_OUTPUT, 0x2000);
        write(&mut stats, &mut h, BASE + CONTROL, START);
        advance(&mut stats, &mut h, 2000);
        let green = word(&h, 0x2000 + 8);
        assert!(green.abs_diff(WALL_WINDOW[2]) <= 1);
        let declared = f64::from(WALL_WINDOW[0]) / f64::from(WALL_WINDOW[2]);
        let red_head = f64::from(word(&h, 0x2000)) / f64::from(green);
        assert!(red_head > 3. * declared, "{red_head} vs {declared}");
    }

    #[test]
    fn the_coarse_planes_hold_their_zones_summed_times_the_measured_gain() {
        let rgb = picture(16, 16, |x, y| {
            [
                (x * 16 + y) as u8,
                (x * 5 + y * 11) as u8,
                (x * 3 + y * 7 + 40) as u8,
            ]
        });
        let mut stats = Engine::new(BASE, 106, 1000)
            .unwrap()
            .with_scene(WALL_SUMS)
            .with_zone_scene(zone_scene(rgb))
            .unwrap();
        let mut h = TestHost::new();
        write(&mut stats, &mut h, BASE + ZONE_SUMS, 0x1000);
        write(&mut stats, &mut h, BASE + COARSE_OUTPUT, 0x3000);
        write(&mut stats, &mut h, BASE + CONTROL, START);
        advance(&mut stats, &mut h, 1000);
        let zones = |c: usize, zone: usize| u64::from(word(&h, 0x1000 + 0x400 * c + 4 * zone));
        for c in 0..3 {
            for cell in 0..64 {
                let (x, y) = (2 * (cell % 8), 2 * (cell / 8));
                let sum: u64 = [0, 1, 2, 3]
                    .map(|n| zones(c, (y + n / 2) * 16 + x + n % 2))
                    .into_iter()
                    .sum();
                let expected = (sum as f64 * 1.022).round() as u32;
                assert_eq!(
                    word(&h, 0x3000 + 0x100 * c + 4 * cell),
                    expected,
                    "c{c} cell{cell}"
                );
            }
        }
    }

    #[test]
    fn the_histogram_counts_the_pictures_luma_in_proportion_to_its_area() {
        let rgb = picture(
            16,
            16,
            |x, _| {
                if x < 12 { [0, 0, 0] } else { [255, 255, 255] }
            },
        );
        let mut stats = Engine::new(BASE, 106, 1000)
            .unwrap()
            .with_scene(WALL_SUMS)
            .with_zone_scene(zone_scene(rgb))
            .unwrap();
        let mut h = TestHost::new();
        write(&mut stats, &mut h, BASE + HISTOGRAM_OUTPUT, 0x5000);
        write(&mut stats, &mut h, BASE + CONTROL, START);
        advance(&mut stats, &mut h, 1000);
        let bins: Vec<u32> = (0..256).map(|bin| word(&h, 0x5000 + 4 * bin)).collect();
        let total: u32 = bins.iter().sum();
        assert_eq!(total, 725_454);
        // Display-coded black and white: bins 0 and 127, in the 3:1 area ratio.
        assert_eq!(bins[0], 544_091);
        assert_eq!(bins[127], 181_363);
        assert!(
            bins.iter()
                .enumerate()
                .all(|(bin, count)| { *count == 0 || bin == 0 || bin == 127 })
        );
    }

    #[test]
    fn the_reference_planes_hold_their_constants_whatever_the_light() {
        use std::sync::atomic::{AtomicU32, Ordering};
        let integration = std::sync::Arc::new(AtomicU32::new(0));
        let mut stats = Engine::new(BASE, 106, 1000)
            .unwrap()
            .with_scene(WALL_SUMS)
            .with_zone_scene(zone_scene(picture(16, 16, |_, _| WALL_BYTES)))
            .unwrap()
            .with_exposure(Exposure {
                value: integration.clone(),
                reference: 1566,
            })
            .unwrap()
            .with_scene_light(SceneLight {
                value: std::sync::Arc::new(AtomicU32::new(LIGHT_UNIT)),
            })
            .unwrap();
        let mut h = TestHost::new();
        write(&mut stats, &mut h, BASE + REFERENCE_OUTPUT, 0x4000);
        write(&mut stats, &mut h, BASE + CONTROL, START);
        advance(&mut stats, &mut h, 1000);
        let levels = || [5_084, 12_710, 5_084];
        for c in 0..3 {
            for cell in 0..64 {
                assert_eq!(word(&h, 0x4000 + 0x100 * c + 4 * cell), levels()[c]);
            }
        }
        // The scales move the zone sums but not the reference planes.
        stats.set_scene_light(0.5).unwrap();
        integration.store(2 * 1566, Ordering::Relaxed);
        advance(&mut stats, &mut h, 2000);
        for c in 0..3 {
            for cell in 0..64 {
                assert_eq!(word(&h, 0x4000 + 0x100 * c + 4 * cell), levels()[c]);
            }
        }
    }

    #[test]
    fn without_a_picture_the_scene_buffers_stay_unwritten() {
        let mut stats = Engine::new(BASE, 106, 1000)
            .unwrap()
            .with_scene([1000, 2000, 300])
            .with_buffer(WINDOW_OUTPUT, &WALL_WINDOW)
            .unwrap()
            .with_buffer_light_words(WINDOW_OUTPUT, &[0, 2, 4])
            .unwrap();
        let mut h = TestHost::new();
        write(&mut stats, &mut h, BASE + ZONE_SUMS, 0x1000);
        write(&mut stats, &mut h, BASE + WINDOW_OUTPUT, 0x2000);
        write(&mut stats, &mut h, BASE + COARSE_OUTPUT, 0x3000);
        write(&mut stats, &mut h, BASE + REFERENCE_OUTPUT, 0x4000);
        write(&mut stats, &mut h, BASE + HISTOGRAM_OUTPUT, 0x5000);
        write(&mut stats, &mut h, BASE + CONTROL, START);
        advance(&mut stats, &mut h, 1000);
        assert_eq!(
            [0, 8, 16].map(|o| word(&h, 0x2000 + o)),
            [WALL_WINDOW[0], WALL_WINDOW[2], WALL_WINDOW[4]]
        );
        assert_eq!(word(&h, 0x2000 + 32), WALL_WINDOW[8]);
        assert_eq!(
            [0x1000, 0x1400, 0x1800].map(|at| word(&h, at)),
            [1000, 2000, 300]
        );
        assert!(h.ram[0x3000..0x6000].iter().all(|byte| *byte == 0));
    }

    #[test]
    fn a_zone_scene_image_scales_the_measured_wall_sums_by_radiance() {
        let sums = [145_000, 383_000, 34_000];
        let radiance = [1015_u16, 764, 577];
        let wall = [srgb(1015), srgb(764), srgb(577)];
        let levels: [u8; 3] = [wall[0], wall[1], wall[2]];
        let wall_pixels = || {
            std::iter::repeat_n(levels, 16)
                .flatten()
                .collect::<Vec<u8>>()
        };
        let engine = || {
            Engine::new(BASE, 106, 1000)
                .unwrap()
                .with_scene(sums)
                .with_zone_scene(ZoneScene {
                    width: 4,
                    height: 4,
                    rgb: wall_pixels(),
                    sums,
                    radiance,
                })
                .unwrap()
        };
        let mut h = TestHost::new();
        let word = |h: &TestHost, address: usize| {
            u32::from_le_bytes(h.ram[address..address + 4].try_into().unwrap())
        };
        let mut stats = engine();
        write(&mut stats, &mut h, BASE + ZONE_SUMS, 0x1000);
        write(&mut stats, &mut h, BASE + CONTROL, START);
        advance(&mut stats, &mut h, 1000);
        for (c, &sum) in sums.iter().enumerate().take(3) {
            for zone in [0, 255] {
                let value = word(&h, 0x1000 + 0x400 * c + 4 * zone);
                let expected = f64::from(sum);
                assert!(
                    (f64::from(value) - expected).abs() <= expected * 0.02,
                    "c{c} zone{zone}: {value} vs {expected}"
                );
            }
        }
        assert_eq!(word(&h, 0x1000 + 0x400 * 3), 0);
        // A black scene has zero sums; a scene twice as bright is twice the sums.
        let mut black = Engine::new(BASE, 106, 1000)
            .unwrap()
            .with_scene(sums)
            .with_zone_scene(ZoneScene {
                width: 4,
                height: 4,
                rgb: vec![0; 48],
                sums,
                radiance,
            })
            .unwrap();
        write(&mut black, &mut h, BASE + ZONE_SUMS, 0x2000);
        write(&mut black, &mut h, BASE + CONTROL, START);
        advance(&mut black, &mut h, 1000);
        assert_eq!(word(&h, 0x2000), 0);
        // A missing wall scene is refused.
        assert!(
            Engine::new(BASE, 106, 1000)
                .unwrap()
                .with_zone_scene(ZoneScene {
                    width: 4,
                    height: 4,
                    rgb: vec![0; 48],
                    sums,
                    radiance,
                })
                .is_err()
        );
    }

    #[test]
    fn a_published_integration_value_scales_the_zone_sums() {
        use std::sync::atomic::{AtomicU32, Ordering};
        let value = std::sync::Arc::new(AtomicU32::new(0));
        let frame = |value: &std::sync::Arc<AtomicU32>| {
            let mut stats = Engine::new(BASE, 106, 1000)
                .unwrap()
                .with_scene([1000, 2000, 300])
                .with_exposure(Exposure {
                    value: value.clone(),
                    reference: 1566,
                })
                .unwrap();
            let mut h = TestHost::new();
            write(&mut stats, &mut h, BASE + ZONE_SUMS, 0x1000);
            write(&mut stats, &mut h, BASE + CONTROL, START);
            advance(&mut stats, &mut h, 1000);
            let word = |address: usize| {
                u32::from_le_bytes(h.ram[address..address + 4].try_into().unwrap())
            };
            [word(0x1000), word(0x1400), word(0x1800)]
        };
        // Not yet published: the declared sums.
        assert_eq!(frame(&value), [1000, 2000, 300]);
        value.store(2 * 1566, Ordering::Relaxed);
        assert_eq!(frame(&value), [2000, 4000, 600]);
        assert!(
            Engine::new(BASE, 106, 1000)
                .unwrap()
                .with_exposure(Exposure {
                    value,
                    reference: 1566
                })
                .is_err()
        );
    }

    #[test]
    fn a_published_scene_light_scales_the_zone_sums_and_is_checkpointed() {
        use std::sync::atomic::{AtomicU32, Ordering};
        let light = std::sync::Arc::new(AtomicU32::new(LIGHT_UNIT));
        let mut stats = Engine::new(BASE, 106, 1000)
            .unwrap()
            .with_scene([1000, 2000, 300])
            .with_scene_light(SceneLight {
                value: light.clone(),
            })
            .unwrap();
        let mut h = TestHost::new();
        let planes = |h: &TestHost| {
            [0, 1, 2].map(|c| {
                let at = 0x1000 + 0x400 * c;
                u32::from_le_bytes(h.ram[at..at + 4].try_into().unwrap())
            })
        };
        write(&mut stats, &mut h, BASE + ZONE_SUMS, 0x1000);
        write(&mut stats, &mut h, BASE + CONTROL, START);
        advance(&mut stats, &mut h, 1000);
        assert_eq!(planes(&h), [1000, 2000, 300]);
        // The input halves the declared light and publishes the scale.
        stats.set_scene_light(0.5).unwrap();
        assert_eq!(light.load(Ordering::Relaxed), 500);
        advance(&mut stats, &mut h, 2000);
        assert_eq!(planes(&h), [500, 1000, 150]);
        // A checkpoint keeps the input and republishes it on restore.
        let saved = stats.save();
        stats.set_scene_light(2.).unwrap();
        stats.restore(&saved).unwrap();
        assert_eq!(light.load(Ordering::Relaxed), 500);
        advance(&mut stats, &mut h, 3000);
        assert_eq!(planes(&h), [500, 1000, 150]);
        // Only a finite factor in range is an input.
        assert!(stats.set_scene_light(f64::NAN).is_err());
        assert!(stats.set_scene_light(-1.).is_err());
        assert!(stats.set_scene_light(1001.).is_err());
        assert!(
            Engine::new(BASE, 106, 1000)
                .unwrap()
                .with_scene_light(SceneLight { value: light })
                .is_err()
        );
    }

    #[test]
    fn started_engine_completes_at_frame_ticks_and_stops_at_once() {
        let mut stats = Engine::new(BASE, 106, 1000).unwrap();
        let mut h = TestHost::new();
        write(&mut stats, &mut h, BASE + 0x1b8, 0x3f48_0000);
        write(&mut stats, &mut h, BASE + ENABLE, 0x3ff_03ff);
        h.now = 1500;
        write(&mut stats, &mut h, BASE + CONTROL, START);
        assert_eq!(stats.next_deadline(0), Some(2000));
        advance(&mut stats, &mut h, 1999);
        assert_eq!(read(&mut stats, &mut h, BASE + STATUS), 0);
        advance(&mut stats, &mut h, 2000);
        assert_eq!(read(&mut stats, &mut h, BASE + STATUS), 0x3ff);
        assert!(stats.state.line);
        // 0xc36280: write the status back to clear it.
        write(&mut stats, &mut h, BASE + STATUS, 0x3ff);
        assert_eq!(read(&mut stats, &mut h, BASE + STATUS), 0);
        assert!(!stats.state.line);
        let saved = stats.save();
        advance(&mut stats, &mut h, 4500);
        assert_eq!(stats.state.frames, 2);
        stats.restore(&saved).unwrap();
        assert_eq!(stats.next_deadline(0), Some(3000));
        // 0xc365a8: stop, then the handshakes.
        write(&mut stats, &mut h, BASE, 0);
        assert_eq!(read(&mut stats, &mut h, BASE), 1);
        assert_eq!(read(&mut stats, &mut h, BASE + STATE) & 0x10000, 0x10000);
        write(&mut stats, &mut h, BASE + CONTROL, 0);
        assert_eq!(stats.next_deadline(0), None);
        assert_eq!(read(&mut stats, &mut h, BASE + 0x1b8), 0x3f48_0000);
        let mut cx = h.cx();
        stats.write(&mut cx, BASE + STATE, 4, 0);
        assert!(cx.is_unmodeled());
    }

    #[test]
    fn the_firmware_alternates_the_zone_sums_output_word_between_the_halves() {
        let mut stats = Engine::new(BASE, 106, 1000)
            .unwrap()
            .with_scene([145_000, 383_000, 34_000]);
        let mut h = TestHost::new();
        let word = |h: &TestHost, address: usize| {
            u32::from_le_bytes(h.ram[address..address + 4].try_into().unwrap())
        };
        let plane =
            |h: &TestHost, address: usize| [0, 1, 2, 3].map(|n| word(h, address + 0x400 * n));
        // The firmware programs the first half, then starts the engine.
        write(&mut stats, &mut h, BASE + ZONE_SUMS, 0x1000);
        write(&mut stats, &mut h, BASE + CONTROL, START);
        advance(&mut stats, &mut h, 1000);
        assert_eq!(plane(&h, 0x1000), [145_000, 383_000, 34_000, 0]);
        assert_eq!(word(&h, 0x4690), 0);
        // Measured order on X-T20 2.11: the section 6 statistics consumer hands
        // the other half's address to the section 5 setters, so the next frame
        // completes into the second half.
        write(&mut stats, &mut h, BASE + ZONE_SUMS, 0x4690);
        advance(&mut stats, &mut h, 2000);
        assert_eq!(plane(&h, 0x4690), [145_000, 383_000, 34_000, 0]);
        // The +0x10 distance is the firmware's, not an offset the engine adds.
        assert_eq!(word(&h, 0x4690 + 0x3690), 0);
        // A stop/start between frames keeps writing the programmed half.
        write(&mut stats, &mut h, BASE + CONTROL, 0);
        h.ram[0x4690..0x4694].fill(0);
        write(&mut stats, &mut h, BASE + CONTROL, START);
        advance(&mut stats, &mut h, 3000);
        assert_eq!(word(&h, 0x4690), 145_000);
        // Back to the first half, still alternating across the restart.
        write(&mut stats, &mut h, BASE + ZONE_SUMS, 0x1000);
        h.ram[0x1000..0x1004].fill(0);
        write(&mut stats, &mut h, BASE + CONTROL, 0);
        write(&mut stats, &mut h, BASE + CONTROL, START);
        advance(&mut stats, &mut h, 4000);
        assert_eq!(word(&h, 0x1000), 145_000);
    }

    #[test]
    fn declared_buffers_are_written_to_their_output_each_frame() {
        assert!(
            Engine::new(BASE, 106, 1000)
                .unwrap()
                .with_buffer(0x1b4, &[1])
                .is_err()
        );
        assert!(
            Engine::new(BASE, 106, 1000)
                .unwrap()
                .with_scene([1, 2, 3])
                .with_buffer(ZONE_SUMS, &[1])
                .is_err()
        );
        let mut stats = Engine::new(BASE, 106, 1000)
            .unwrap()
            .with_buffer(0x1d0, &[7, 0, 9])
            .unwrap();
        let mut h = TestHost::new();
        let word = |h: &TestHost, address: usize| {
            u32::from_le_bytes(h.ram[address..address + 4].try_into().unwrap())
        };
        write(&mut stats, &mut h, BASE + 0x1d0, 0x2000);
        write(&mut stats, &mut h, BASE + CONTROL, START);
        advance(&mut stats, &mut h, 1000);
        assert_eq!([0, 4, 8].map(|o| word(&h, 0x2000 + o)), [7, 0, 9]);
        // The firmware programs the other half for the next frame.
        write(&mut stats, &mut h, BASE + 0x1d0, 0x5690);
        advance(&mut stats, &mut h, 2000);
        assert_eq!(word(&h, 0x5690), 7);
        assert_eq!(word(&h, 0x5690 + 0x3690), 0);
        assert_eq!(word(&h, 0x2000), 7);
    }

    #[test]
    fn declared_light_words_of_a_buffer_follow_the_scene_light() {
        use std::sync::atomic::{AtomicU32, Ordering};
        let scene = std::sync::Arc::new(AtomicU32::new(LIGHT_UNIT));
        let integration = std::sync::Arc::new(AtomicU32::new(0));
        let mut stats = Engine::new(BASE, 106, 1000)
            .unwrap()
            .with_scene([1000, 2000, 300])
            .with_scene_light(SceneLight {
                value: scene.clone(),
            })
            .unwrap()
            .with_exposure(Exposure {
                value: integration.clone(),
                reference: 1566,
            })
            .unwrap()
            .with_buffer(0x1d0, &[10, 20, 30, 40])
            .unwrap()
            .with_buffer_light_words(0x1d0, &[0, 2])
            .unwrap();
        let mut h = TestHost::new();
        let words = |h: &TestHost, at: usize| {
            [0, 4, 8, 12].map(|o| u32::from_le_bytes(h.ram[at + o..at + 4 + o].try_into().unwrap()))
        };
        write(&mut stats, &mut h, BASE + 0x1d0, 0x2000);
        write(&mut stats, &mut h, BASE + CONTROL, START);
        // Frame 1 writes the first half.
        advance(&mut stats, &mut h, 1000);
        assert_eq!(words(&h, 0x2000), [10, 20, 30, 40]);
        // Only the declared light words follow the scene light; the firmware
        // programs the other half for the next frame.
        stats.set_scene_light(0.5).unwrap();
        write(&mut stats, &mut h, BASE + 0x1d0, 0x5690);
        advance(&mut stats, &mut h, 2000);
        assert_eq!(words(&h, 0x5690), [5, 20, 15, 40]);
        // The published exposure scales them as it scales the zone sums, in the
        // half programmed for this frame.
        integration.store(2 * 1566, Ordering::Relaxed);
        write(&mut stats, &mut h, BASE + 0x1d0, 0x2000);
        advance(&mut stats, &mut h, 3000);
        assert_eq!(words(&h, 0x2000), [10, 20, 30, 40]);
        // A buffer must be declared, and every index inside it.
        assert!(
            Engine::new(BASE, 106, 1000)
                .unwrap()
                .with_buffer_light_words(0x1d0, &[0])
                .is_err()
        );
        assert!(
            Engine::new(BASE, 106, 1000)
                .unwrap()
                .with_buffer(0x1d0, &[1, 2, 3])
                .unwrap()
                .with_buffer_light_words(0x1d0, &[3])
                .is_err()
        );
    }
}
