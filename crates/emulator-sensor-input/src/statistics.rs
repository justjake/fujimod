//! Scene-derived IBUF color sums, with the stock 16x16/64-byte-row layout.
//! Unmapped statistic fields remain explore accesses; see ae-pipeline.md.
//!
//! With a sensor exposure ([`Statistics::with_sensor_scene`]) each completion measures the
//! host scene the sensor sees (`XT5_SCENE`, or an 18% grey card when none is set) through the
//! X-T5 sensor response at the exposure read at that completion
//! ([`fujimod_emulator_imaging::xtrans::Response::xt5`]): colour gains, the 1024 pedestal and
//! clipping at 16383, so the firmware's AE and AWB see their own programming. A run-definition
//! statistics scene ([`Device::set_scene`]) takes precedence.
use fujimod_emulator_devices::checkpoint::Checkpoint;
use fujimod_emulator_imaging::exposure::ExposureHandle;
use fujimod_emulator_imaging::xtrans;
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy)]
pub enum Planes {
    Color,
    Rgb,
    Ae,
    /// Focus (contrast AF) cells, 0x2000 bytes: see [`FocusFrame`].
    FocusCells,
    /// Per-cell maximum and minimum level, 0x400 bytes: see [`FocusFrame`].
    FocusLevels,
    /// Per-cell count of clipped pixels, 0x400 bytes: see [`FocusFrame`].
    FocusSums,
}

/// Samples a line and lines of the picture the focus statistics are measured on. The width
/// sets the finest blur circle the sums resolve (1 / 2048 of the picture width).
const FOCUS_WIDTH: usize = 2048;
const FOCUS_LINES: usize = 72;
/// The focus grid: 13 x 9 cells (the 117 AF areas) inside 16-cell hardware rows.
const FOCUS_COLUMNS: usize = 13;
const FOCUS_ROWS: usize = 9;
/// Sample distances of the four band-pass filters, fine to coarse.
const FOCUS_SCALES: [usize; 4] = [1, 2, 4, 8];
/// The measured lines stand for a whole live-view frame: the factor from a sum over
/// [`FOCUS_LINES`] lines to the sum over the lines of the frame (a functional value; the
/// hardware filter gains are not decoded). `XT5_FOCUS_GAIN` replaces it.
const FOCUS_GAIN: f64 = 16.;
/// Pixels a cell stands for in the clipped-pixel count (a 1944 x 1317 preview input over
/// 13 x 9 cells).
const FOCUS_CELL_PIXELS: f64 = 149. * 146.;

/// The focus (contrast AF) statistics of the X-T5 preview input buffer for one frame.
///
/// X-T5 4.31 evidence (static). The collector `0x2fec7d8` (AF statistics service, request
/// `0x2fe53e4` / message 0x15ab from the S1 AF sequence, collect `0x2fe536c`) reads, for a grid
/// of at most 13 columns x 9 rows (the 117 AF areas):
///
/// - statistics plane 0x21 (bank +0x41a0, 0x2000 bytes, output register +0x410): rows of
///   0x200 bytes, cells of 0x20 bytes, eight u32 a cell, copied to eight arrays of 117;
/// - plane 0xe (bank +0x21a0, output +0x418): rows of 0x40 bytes, one u32 a cell, split into
///   its high and low 16 bits;
/// - plane 0xf (bank +0x25a0, output +0x420): rows of 0x40 bytes, one u32 a cell.
///
/// The contrast calculation `0x2fa4e6c` uses the high half of plane 0xe as the peak level of
/// a cell (maximum over the selected areas, `0x2fa5858`) and plane 0xf as a pixel count that
/// it divides by the pixels of a cell (`0x2fa5768`, `0x2fa5998`: a fraction in 1 / 10000,
/// compared with a threshold together with the peak level: bright point sources). What the
/// eight words of a cell and the low half of plane 0xe measure is not decoded.
///
/// This model is functional: words 0..3 and 4..7 of a cell are the band-pass sums of
/// [`fujimod_emulator_imaging::focus::contrast_grid`] at four scales (fine to coarse) of the
/// host scene's linear luma at the sensor exposure, blurred by the blur circle of the virtual
/// focus ([`fujimod_emulator_imaging::focus`]); plane 0xe is the cell's maximum (high half)
/// and minimum level; plane 0xf is the count of clipped pixels of the cell. `XT5_FOCUS_AUX`
/// (bit 0: plane 0xe, bit 1: plane 0xf; default 3) leaves an auxiliary plane zero for
/// experiments. Without a host scene every value is zero (a flat grey card has no contrast).
#[derive(Clone)]
pub struct FocusFrame {
    pub cells: Vec<u8>,
    pub levels: Vec<u8>,
    pub sums: Vec<u8>,
}

fn host_number(name: &str, default: f64) -> f64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v >= 0.)
        .unwrap_or(default)
}

impl FocusFrame {
    /// The frame at board time `now` and relative exposure `exposure`. One frame serves the
    /// three outputs of the same completion, so the newest frame is kept.
    pub fn at(now: u64, exposure: f64) -> Result<Self, String> {
        use fujimod_emulator_imaging::{focus, frame};
        static KEPT: Mutex<Option<(u64, u64, FocusFrame)>> = Mutex::new(None);
        let key = (now, exposure.to_bits());
        if let Some((at, bits, kept)) = KEPT.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            if (*at, *bits) == key {
                return Ok(kept.clone());
            }
        }
        let defocus = focus::sample(now).unwrap_or_default();
        let made = match frame::host_scene() {
            None => Self::measure(&[], exposure, 0.),
            Some(scene) => {
                let mut rgb = Vec::with_capacity(FOCUS_WIDTH * FOCUS_LINES * 3);
                frame::render_sharp_rgb_lines(scene, FOCUS_WIDTH, FOCUS_LINES, |_, line| {
                    rgb.extend_from_slice(line);
                    Ok(())
                })?;
                Self::measure(&rgb, exposure, defocus.blur * FOCUS_WIDTH as f64)
            }
        };
        *KEPT.lock().unwrap_or_else(|e| e.into_inner()) = Some((key.0, key.1, made.clone()));
        Ok(made)
    }

    /// The planes of an sRGB raster of [`FOCUS_WIDTH`] x [`FOCUS_LINES`] (or of no picture:
    /// all zero) at relative exposure `exposure`, blurred by a circle of `diameter` samples.
    fn measure(rgb: &[u8], exposure: f64, diameter: f64) -> Self {
        static OPTIONS: std::sync::OnceLock<(f64, u32)> = std::sync::OnceLock::new();
        let (gain, aux) = *OPTIONS.get_or_init(|| {
            (host_number("XT5_FOCUS_GAIN", FOCUS_GAIN), host_number("XT5_FOCUS_AUX", 3.) as u32)
        });
        let mut frame = Self { cells: vec![0; 0x2000], levels: vec![0; 0x400], sums: vec![0; 0x400] };
        if rgb.len() != FOCUS_WIDTH * FOCUS_LINES * 3 {
            return frame;
        }
        let response = xtrans::Response::xt5(exposure);
        let clip = f64::from(xtrans::WHITE - xtrans::BLACK);
        // Linear luma as the green-referred sensor signal above the pedestal.
        let levels: Vec<f64> = rgb
            .as_chunks::<3>()
            .0
            .iter()
            .map(|p| {
                let luma = 0.30 * xtrans::srgb_to_linear(p[0])
                    + 0.59 * xtrans::srgb_to_linear(p[1])
                    + 0.11 * xtrans::srgb_to_linear(p[2]);
                (luma * response.exposure * response.white).clamp(0., clip)
            })
            .collect();
        let bands = fujimod_emulator_imaging::focus::contrast_grid(
            &levels,
            FOCUS_WIDTH,
            FOCUS_LINES,
            FOCUS_COLUMNS,
            FOCUS_ROWS,
            &FOCUS_SCALES,
            diameter,
        );
        let (cell_lines, cell_samples) = (FOCUS_LINES / FOCUS_ROWS, FOCUS_WIDTH / FOCUS_COLUMNS);
        for row in 0..FOCUS_ROWS {
            for column in 0..FOCUS_COLUMNS {
                let cell = row * FOCUS_COLUMNS + column;
                for (band, sums) in bands.iter().enumerate() {
                    let value = ((sums[cell] * gain).round().min(f64::from(u32::MAX)) as u32).to_le_bytes();
                    for word in [band, band + 4] {
                        let at = row * 0x200 + column * 0x20 + word * 4;
                        frame.cells[at..at + 4].copy_from_slice(&value);
                    }
                }
                let (mut low, mut high, mut clipped) = (f64::MAX, 0f64, 0usize);
                for y in row * cell_lines..(row + 1) * cell_lines {
                    let line = &levels[y * FOCUS_WIDTH..][column * cell_samples..(column + 1) * cell_samples];
                    for level in line {
                        low = low.min(*level);
                        high = high.max(*level);
                        clipped += usize::from(*level >= clip);
                    }
                }
                let at = row * 0x40 + column * 4;
                if aux & 1 != 0 {
                    let word = (high.round() as u32) << 16 | low.round() as u32;
                    frame.levels[at..at + 4].copy_from_slice(&word.to_le_bytes());
                }
                if aux & 2 != 0 {
                    let fraction = clipped as f64 / (cell_lines * cell_samples) as f64;
                    let word = (fraction * FOCUS_CELL_PIXELS).round() as u32;
                    frame.sums[at..at + 4].copy_from_slice(&word.to_le_bytes());
                }
            }
        }
        frame
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct Scene {
    name: String,
    width: usize,
    height: usize,
    rgb: Vec<[u16; 3]>,
    samples_per_cell: u32,
    #[serde(default = "black_level")]
    black_level: u16,
    #[serde(default = "ae_area")]
    ae_cell_area: u32,
    #[serde(default = "signal_shift")]
    signal_shift: u8,
}
fn black_level() -> u16 {
    1024
}
fn ae_area() -> u32 {
    120 * 78
}
fn signal_shift() -> u8 {
    2
}
impl Default for Scene {
    fn default() -> Self {
        Self {
            name: "mid-gray (linear12=2048)".into(),
            width: 1,
            height: 1,
            rgb: vec![[2048; 3]],
            samples_per_cell: 1024,
            black_level: black_level(),
            ae_cell_area: ae_area(),
            signal_shift: signal_shift(),
        }
    }
}
impl Scene {
    fn load(value: &Value) -> Result<Self, String> {
        let object = value.as_object().ok_or("scene must be an object")?;
        if let Some(key) = object.keys().find(|key| {
            !matches!(
                key.as_str(),
                "name"
                    | "width"
                    | "height"
                    | "rgb"
                    | "luma"
                    | "samples_per_cell"
                    | "black_level"
                    | "ae_cell_area"
                    | "signal_shift"
            )
        }) {
            return Err(format!("unknown scene input {key}"));
        }
        let integer = |key: &str, fallback: usize| -> Result<usize, String> {
            value.get(key).map_or(Ok(fallback), |v| {
                v.as_i64()
                    .and_then(|n| usize::try_from(n).ok())
                    .filter(|n| *n > 0)
                    .ok_or_else(|| format!("scene {key} must be positive"))
            })
        };
        let width = integer("width", 1)?;
        let height = integer("height", 1)?;
        let count = width
            .checked_mul(height)
            .filter(|n| *n <= 1_048_576)
            .ok_or("scene dimensions exceed 1M pixels")?;
        let sample = |v: &Value| -> Result<u16, String> {
            v.as_i64()
                .and_then(|n| u16::try_from(n).ok())
                .ok_or("scene samples must be linear radiance (0..65535)".into())
        };
        let rgb = if let Some(values) = value.get("rgb").and_then(Value::as_array) {
            if values.len() != count * 3 {
                return Err("scene rgb requires width*height*3 samples".into());
            }
            values
                .as_chunks::<3>()
                .0
                .iter()
                .map(|p| Ok([sample(&p[0])?, sample(&p[1])?, sample(&p[2])?]))
                .collect::<Result<Vec<_>, String>>()?
        } else if let Some(values) = value.get("luma").and_then(Value::as_array) {
            if values.len() != count {
                return Err("scene luma requires width*height samples".into());
            }
            values
                .iter()
                .map(|v| Ok([sample(v)?; 3]))
                .collect::<Result<Vec<_>, String>>()?
        } else {
            vec![[sample(value.get("luma").ok_or("scene requires luma or rgb")?)?; 3]; count]
        };
        let samples_per_cell = u32::try_from(integer("samples_per_cell", 1024)?)
            .ok()
            .filter(|n| *n <= u32::MAX / 16383)
            .ok_or("scene cell sum overflows u32")?;
        Ok(Self {
            name: value
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("scene")
                .to_owned(),
            width,
            height,
            rgb,
            samples_per_cell,
            black_level: value
                .get("black_level")
                .map(sample)
                .transpose()?
                .unwrap_or_else(black_level),
            ae_cell_area: u32::try_from(integer("ae_cell_area", ae_area() as usize)?)
                .ok()
                .filter(|n| *n <= 65536)
                .ok_or("scene AE cell area exceeds 65536")?,
            signal_shift: value
                .get("signal_shift")
                .map(|v| {
                    v.as_i64()
                        .and_then(|n| u8::try_from(n).ok())
                        .filter(|n| *n <= 2)
                        .ok_or("scene signal_shift must be 0..2".to_string())
                })
                .transpose()?
                .unwrap_or_else(signal_shift),
        })
    }
    /// The sensor's view of `rgb` (`width` x `height` sRGB pixels) through `response`: each
    /// pixel's signal above the pedestal per colour, unclipped (planes clip the raw value).
    fn sensor(width: usize, height: usize, rgb: &[u8], response: xtrans::Response) -> Self {
        Self {
            name: "sensor scene".into(),
            width,
            height,
            rgb: rgb
                .as_chunks::<3>()
                .0
                .iter()
                .map(|p| {
                    std::array::from_fn(|c| {
                        let level = xtrans::srgb_to_linear(p[c]);
                        let signal = level * response.colour_gain[c] * response.exposure * response.white;
                        signal.round().clamp(0., f64::from(u16::MAX)) as u16
                    })
                })
                .collect(),
            signal_shift: 0,
            ..Self::default()
        }
    }
    /// [`Scene::sensor`] of the host scene (`XT5_SCENE`; an 18% grey card, sRGB 118, when none
    /// is set) at `columns * 8` x `rows * 8` pixels, at relative exposure `exposure`.
    fn host(columns: usize, rows: usize, exposure: f64) -> Result<Self, String> {
        let (width, height) = (columns * 8, rows * 8);
        let response = xtrans::Response::xt5(exposure);
        let Some(scene) = fujimod_emulator_imaging::frame::host_scene() else {
            return Ok(Self::sensor(width, height, &vec![118; width * height * 3], response));
        };
        let mut rgb = Vec::with_capacity(width * height * 3);
        fujimod_emulator_imaging::frame::render_rgb_lines(scene, width, height, |_, line| {
            rgb.extend_from_slice(line);
            Ok(())
        })?;
        Ok(Self::sensor(width, height, &rgb, response))
    }
    fn planes(&self, columns: usize, rows: usize, ae: bool) -> Vec<u8> {
        let mut bytes = vec![0; 0x1000];
        // The scene spans the configured grid, not the unused 16x16 padding.
        // Equal green channels are a virtual Bayer sampling choice.
        for y in 0..rows {
            for x in 0..columns {
                // Exact area weights in integer coordinates retain small bright
                // regions that a center-point sampler would miss entirely.
                let (left, right) = (x * self.width, (x + 1) * self.width);
                let (top, bottom) = (y * self.height, (y + 1) * self.height);
                let mut weighted = [0.0; 3];
                for py in top / rows..bottom.div_ceil(rows) {
                    let dy = bottom.min((py + 1) * rows) - top.max(py * rows);
                    for px in left / columns..right.div_ceil(columns) {
                        let dx = right.min((px + 1) * columns) - left.max(px * columns);
                        for channel in 0..3 {
                            let signal = f64::from(self.rgb[py * self.width + px][channel])
                                * f64::from(1u32 << self.signal_shift);
                            let raw = (signal + f64::from(self.black_level)).clamp(0.0, 16383.0);
                            weighted[channel] += raw * (dx * dy) as f64;
                        }
                    }
                }
                for (plane, channel) in [0, 1, 2, 1].into_iter().enumerate() {
                    let samples = if ae {
                        self.ae_cell_area * [2, 5, 2, 5][plane] / 9
                    } else {
                        self.samples_per_cell
                    };
                    let raw = weighted[channel] / (self.width * self.height) as f64;
                    let sum = (raw * f64::from(samples)).round() as u32;
                    let at = plane * 0x400 + y * 64 + x * 4;
                    bytes[at..at + 4].copy_from_slice(&sum.to_le_bytes());
                }
            }
        }
        bytes
    }
}
#[derive(Serialize, Deserialize)]
struct State {
    scene: Scene,
    bytes: Vec<u8>,
    frames: u64,
    last_destination: u64,
    /// A run-definition scene was set ([`Device::set_scene`]); it replaces the sensor scene.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    explicit: bool,
}
#[derive(Clone)]
pub struct Statistics {
    state: Arc<Mutex<State>>,
    base: u64,
    bank_size: usize,
    /// The banks are ordinary guest memory (another device or the CPU may use
    /// the same SRAM for other data), so frames complete by DMA into it.
    in_memory: bool,
    /// The sensor exposure: measure the host scene through the sensor response
    /// ([`Statistics::with_sensor_scene`]).
    exposure: Option<ExposureHandle>,
}
impl Statistics {
    /// Statistics whose SRAM is plain guest memory rather than a device window.
    pub fn in_memory(base: u64, bank_size: usize) -> Result<Self, String> {
        Ok(Self { in_memory: true, ..Self::new(base, bank_size)? })
    }

    pub fn new(base: u64, bank_size: usize) -> Result<Self, String> {
        if base & 31 != 0
            || !(0x1c60..=0x100000).contains(&bank_size)
            || base.checked_add(bank_size as u64 * 2).is_none()
        {
            return Err("invalid statistics SRAM wiring".into());
        }
        Ok(Self {
            state: Arc::new(Mutex::new(State {
                scene: Scene::default(),
                bytes: vec![0; bank_size * 2],
                frames: 0,
                last_destination: 0,
                explicit: false,
            })),
            base,
            bank_size,
            in_memory: false,
            exposure: None,
        })
    }

    /// Measure the host scene through the X-T5 sensor response at the exposure `exposure`
    /// holds at each completion (see the module notes).
    pub fn with_sensor_scene(mut self, exposure: ExposureHandle) -> Self {
        self.exposure = Some(exposure);
        self
    }

    pub fn complete(
        &self,
        cx: &mut Cx<'_>,
        destination: u64,
        columns: usize,
        rows: usize,
        planes: Planes,
    ) -> Result<(), String> {
        if destination == 0 {
            return Ok(());
        }
        let mut s = self.state.lock().unwrap();
        if !(1..=16).contains(&columns) || !(1..=16).contains(&rows) {
            return Err("statistics grid exceeds 16x16 hardware cells".into());
        }
        let ae = matches!(planes, Planes::Ae);
        let relative = match &self.exposure {
            Some(exposure) if !s.explicit => Some(exposure.lock().map_or(1., |e| e.relative())),
            _ => None,
        };
        let focus = |frame: &dyn Fn(FocusFrame) -> Vec<u8>, size: usize| match relative {
            // A declared (uniform) statistics scene has no contrast.
            None => Ok(vec![0; size]),
            Some(relative) => FocusFrame::at(cx.now(), relative).map(frame),
        };
        let mut bytes = match (planes, relative) {
            (Planes::FocusCells, _) => focus(&|f| f.cells, 0x2000)?,
            (Planes::FocusLevels, _) => focus(&|f| f.levels, 0x400)?,
            (Planes::FocusSums, _) => focus(&|f| f.sums, 0x400)?,
            (_, Some(relative)) => Scene::host(columns, rows, relative)?.planes(columns, rows, ae),
            (_, None) => s.scene.planes(columns, rows, ae),
        };
        if matches!(planes, Planes::Rgb | Planes::Ae) {
            bytes.truncate(0xc00);
        }
        if !self.in_memory
            && destination >= self.base
            && destination
                .checked_add(bytes.len() as u64)
                .is_some_and(|end| end <= self.base + (self.bank_size * 2) as u64)
        {
            let at = (destination - self.base) as usize;
            s.bytes[at..at + bytes.len()].copy_from_slice(&bytes);
        } else {
            cx.ram_write(destination, &bytes)?;
        }
        s.frames += 1;
        s.last_destination = destination;
        Ok(())
    }
}
impl Device for Statistics {
    fn name(&self) -> &'static str {
        "imaging-statistics-sram"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let Some(at) = address
            .checked_sub(self.base)
            .and_then(|n| usize::try_from(n).ok())
        else {
            cx.unmodeled();
            return 0;
        };
        if !matches!(size, 1 | 2 | 4 | 8)
            || at
                .checked_add(size as usize)
                .is_none_or(|end| end > self.bank_size * 2)
        {
            cx.unmodeled();
            return 0;
        }
        // Only four color planes are currently decoded. The rest of the bank holds no
        // measurement for the declared uniform scene and reads back its stored bytes (zero
        // unless a frame completion wrote them).
        let s = self.state.lock().unwrap();
        let mut bytes = [0; 8];
        bytes[..size as usize].copy_from_slice(&s.bytes[at..at + size as usize]);
        u64::from_le_bytes(bytes)
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let Some(at) = address
            .checked_sub(self.base)
            .and_then(|n| usize::try_from(n).ok())
        else {
            cx.unmodeled();
            return;
        };
        if !matches!(size, 1 | 2 | 4 | 8)
            || at
                .checked_add(size as usize)
                .is_none_or(|end| end > self.bank_size * 2)
        {
            cx.unmodeled();
            return;
        }
        self.state.lock().unwrap().bytes[at..at + size as usize]
            .copy_from_slice(&value.to_le_bytes()[..size as usize]);
    }
    fn set_scene(&mut self, input: &Value) -> Result<bool, String> {
        let scene = Scene::load(input)?;
        let mut state = self.state.lock().unwrap();
        state.scene = scene;
        state.explicit = true;
        Ok(true)
    }
    fn checkpoint(&self) -> Result<Checkpoint, String> {
        Checkpoint::encode(self.name(), &*self.state.lock().unwrap())
    }
    fn validate_checkpoint(&self, saved: &Checkpoint) -> Result<(), String> {
        let state: State = saved.decode(self.name())?;
        if state.bytes.len() != self.bank_size * 2
            || state.scene.width.checked_mul(state.scene.height) != Some(state.scene.rgb.len())
        {
            return Err("statistics checkpoint dimensions disagree".into());
        }
        Ok(())
    }
    fn restore_checkpoint(&mut self, saved: &Checkpoint) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        *self.state.lock().unwrap() = saved.decode(self.name())?;
        Ok(())
    }
    fn save(&self) -> Value {
        serde_json::to_value(&*self.state.lock().unwrap()).unwrap()
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let state: State = serde_json::from_value(value.clone())
            .map_err(|e| format!("statistics checkpoint: {e}"))?;
        if state.bytes.len() != self.bank_size * 2
            || state.scene.width.checked_mul(state.scene.height) != Some(state.scene.rgb.len())
        {
            return Err("statistics checkpoint dimensions disagree".into());
        }
        *self.state.lock().unwrap() = state;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn word(bytes: &[u8], plane: usize, x: usize, y: usize) -> u32 {
        let at = plane * 0x400 + y * 64 + x * 4;
        u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
    }
    #[test]
    fn area_average_keeps_a_small_bright_region() {
        let scene = Scene {
            width: 20,
            height: 20,
            rgb: (0..400)
                .map(|i| {
                    if i % 20 < 5 && i / 20 < 4 {
                        [4000; 3]
                    } else {
                        [0; 3]
                    }
                })
                .collect(),
            samples_per_cell: 1,
            black_level: 0,
            signal_shift: 0,
            ..Default::default()
        };
        assert_eq!(word(&scene.planes(1, 1, false), 0, 0, 0), 200);
        assert_eq!(word(&scene.planes(7, 7, false), 0, 1, 1), 1200);
        assert_eq!(word(&scene.planes(7, 7, false), 0, 6, 6), 0);
    }
    #[test]
    fn subpixel_cells_preserve_rgb_and_padding() {
        let scene = Scene {
            rgb: vec![[1, 2, 3]],
            samples_per_cell: 1024,
            black_level: 0,
            signal_shift: 0,
            ..Scene::default()
        };
        let bytes = scene.planes(7, 7, false);
        assert_eq!(
            [
                word(&bytes, 0, 6, 6),
                word(&bytes, 1, 6, 6),
                word(&bytes, 2, 6, 6),
                word(&bytes, 3, 6, 6)
            ],
            [1024, 2048, 3072, 2048]
        );
        assert_eq!(word(&bytes, 0, 7, 6), 0);
    }
    #[test]
    fn ae_xtrans_counts_include_black_pedestal() {
        let bytes = Scene::default().planes(16, 16, true);
        assert_eq!(
            [
                word(&bytes, 0, 0, 0),
                word(&bytes, 1, 0, 0),
                word(&bytes, 2, 0, 0)
            ],
            [9216 * 2080, 9216 * 5200, 9216 * 2080]
        );
    }
    /// The sensor scene: doubling the exposure doubles the signal above the pedestal; green
    /// takes the most signal, red and blue their colour gains; bright pixels clip at 16383
    /// before the cells integrate them.
    #[test]
    fn sensor_scene_signal_follows_exposure_and_clips() {
        let grey = vec![118u8; 16 * 16 * 3];
        let cell = |exposure: f64, plane: usize| {
            let scene = Scene::sensor(16, 16, &grey, xtrans::Response::xt5(exposure));
            let samples = f64::from(scene.ae_cell_area * [2, 5, 2, 5][plane] / 9);
            f64::from(word(&scene.planes(16, 16, true), plane, 3, 3)) / samples
        };
        let (one, two) = (cell(1., 1), cell(2., 1));
        assert!(((two - 1024.) / (one - 1024.) - 2.).abs() < 1e-3, "{one} -> {two}");
        // 18% grey (sRGB 118, linear 0.181) at the reference exposure: green 0.181 x 7679.5
        // above black.
        assert!((one - 1024. - xtrans::srgb_to_linear(118) * xtrans::XT5_SCENE_WHITE).abs() < 1., "{one}");
        assert!(cell(1., 0) < one && cell(1., 2) < one && cell(1., 0) < cell(1., 2));
        // Four stops over: green clips; red (gain 0.47) does not yet.
        assert_eq!(cell(16., 1).round(), 16383.);
        assert!(cell(16., 0) < 16383.);
        let white = vec![255u8; 16 * 16 * 3];
        let clipped = Scene::sensor(16, 16, &white, xtrans::Response::xt5(8.));
        let samples = clipped.samples_per_cell;
        assert_eq!(word(&clipped.planes(1, 1, false), 1, 0, 0), 16383 * samples);
    }
    /// A sensor-scene statistics block measures the host scene at the exposure read at each
    /// completion; a run-definition scene replaces it.
    #[test]
    fn completions_read_the_exposure_each_time() {
        use fujimod_emulator_devices::test_host::recording::RecordingHost;
        use fujimod_emulator_imaging::exposure::Exposure;
        let exposure = ExposureHandle::default();
        let statistics = Statistics::in_memory(0x1000, 0x2000).unwrap().with_sensor_scene(exposure.clone());
        let mut host = RecordingHost { ram: vec![0; 0x1000], ram_base: 0x1000, ..Default::default() };
        let mut green = || {
            statistics.complete(&mut Cx::new(&mut host, 0, 0, 0), 0x1000, 16, 16, Planes::Ae).unwrap();
            word(&host.ram, 1, 5, 5)
        };
        let one = green();
        *exposure.lock().unwrap() = Exposure { seconds: 1. / 125., analog_gain: 2. };
        let two = green();
        let pedestal = 1024 * (120 * 78 * 5 / 9);
        assert!(((two - pedestal) as f64 / (one - pedestal) as f64 - 2.).abs() < 1e-3);
        let mut device = statistics.clone();
        device.set_scene(&serde_json::json!({"luma": 2048})).unwrap();
        assert_eq!(green(), 9216 * 5200);
    }
    #[test]
    fn clipping_precedes_area_integration() {
        let scene = Scene {
            width: 2,
            rgb: vec![[0; 3], [65535; 3]],
            samples_per_cell: 2,
            black_level: 0,
            ..Scene::default()
        };
        assert_eq!(word(&scene.planes(1, 1, false), 0, 0, 0), 16383);
    }

    /// Focus frame: vertical bars give contrast in every band and every cell, a larger blur
    /// circle gives less (the fine band falls first), exposure scales the sums, a flat picture
    /// gives none, and a declared statistics scene writes zero planes of the right sizes.
    #[test]
    fn focus_frame_falls_with_the_blur_circle() {
        let bars: Vec<u8> = (0..FOCUS_WIDTH * FOCUS_LINES)
            .flat_map(|i| if (i % FOCUS_WIDTH) / 64 % 2 == 0 { [30u8; 3] } else { [220u8; 3] })
            .collect();
        let cell = |frame: &FocusFrame, row: usize, column: usize, word: usize| {
            let at = row * 0x200 + column * 0x20 + word * 4;
            u64::from(u32::from_le_bytes(frame.cells[at..at + 4].try_into().unwrap()))
        };
        let total = |diameter: f64| -> Vec<u64> {
            let frame = FocusFrame::measure(&bars, 1., diameter);
            (0..4)
                .map(|word| (0..FOCUS_ROWS).map(|r| (0..FOCUS_COLUMNS).map(|c| cell(&frame, r, c, word)).sum::<u64>()).sum())
                .collect()
        };
        let sharp = total(0.);
        assert!(sharp[0] > 100_000 && sharp.windows(2).all(|p| p[0] < p[1]), "{sharp:?}");
        let (soft, softer) = (total(6.), total(40.));
        for band in 0..4 {
            assert!(soft[band] < sharp[band] && softer[band] < soft[band], "band {band}");
        }
        assert!(soft[0] * sharp[3] < soft[3] * sharp[0], "the fine band falls first");
        let frame = FocusFrame::measure(&bars, 1., 0.);
        assert_eq!((frame.cells.len(), frame.levels.len(), frame.sums.len()), (0x2000, 0x400, 0x400));
        for (row, column) in [(0, 0), (4, 6), (8, 12)] {
            assert!(cell(&frame, row, column, 0) > 0, "cell {row},{column}");
            assert_eq!(cell(&frame, row, column, 1), cell(&frame, row, column, 5));
            // Maximum over minimum in plane 0xe; nothing clips, so plane 0xf is zero.
            let at = row * 0x40 + column * 4;
            let levels = u32::from_le_bytes(frame.levels[at..at + 4].try_into().unwrap());
            assert!(levels >> 16 > levels & 0xffff && levels & 0xffff > 0);
            assert_eq!(u32::from_le_bytes(frame.sums[at..at + 4].try_into().unwrap()), 0);
        }
        // Columns 13..15 and rows 9..15 of the hardware grid stay empty.
        assert_eq!(cell(&frame, 0, 13, 0), 0);
        assert!(frame.cells[9 * 0x200..].iter().all(|v| *v == 0));
        // Twice the exposure doubles the sums (no clipping at these levels).
        let bright = FocusFrame::measure(&bars, 2., 0.);
        assert!((cell(&bright, 4, 6, 0) as f64 / cell(&frame, 4, 6, 0) as f64 - 2.).abs() < 0.01);
        // Sixteen times the exposure clips the bright bars: half of each cell's pixels.
        let clipped = FocusFrame::measure(&bars, 16., 0.);
        let count = u32::from_le_bytes(clipped.sums[4 * 0x40 + 6 * 4..][..4].try_into().unwrap());
        assert!((f64::from(count) / FOCUS_CELL_PIXELS - 0.5).abs() < 0.15, "{count}");
        let flat = vec![118u8; FOCUS_WIDTH * FOCUS_LINES * 3];
        assert!(FocusFrame::measure(&flat, 1., 0.).cells.iter().all(|v| *v == 0));

        use fujimod_emulator_devices::test_host::recording::RecordingHost;
        let statistics = Statistics::in_memory(0x1000, 0x2000).unwrap();
        for (planes, size) in [(Planes::FocusCells, 0x2000), (Planes::FocusLevels, 0x400), (Planes::FocusSums, 0x400)] {
            let mut host = RecordingHost { ram: vec![0x55; 0x2100], ram_base: 0x1000, ..Default::default() };
            statistics.complete(&mut Cx::new(&mut host, 0, 0, 0), 0x1000, 13, 9, planes).unwrap();
            assert!(host.ram[..size].iter().all(|v| *v == 0));
            assert!(host.ram[size..].iter().all(|v| *v == 0x55));
        }
    }
}
