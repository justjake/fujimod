//! Scene-derived IBUF color sums, with the stock 16x16/64-byte-row layout.
//! Unmapped statistic fields remain explore accesses; see ae-pipeline.md.
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy)]
pub enum Planes {
    Color,
    Rgb,
    Ae,
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
                .chunks_exact(3)
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
}
#[derive(Clone)]
pub struct Statistics {
    state: Arc<Mutex<State>>,
    base: u64,
    bank_size: usize,
}
impl Statistics {
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
            })),
            base,
            bank_size,
        })
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
        let mut bytes = s.scene.planes(columns, rows, matches!(planes, Planes::Ae));
        if !matches!(planes, Planes::Color) {
            bytes.truncate(0xc00);
        }
        if destination >= self.base
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
        // Only four color planes are currently decoded. Other fields stay visible.
        let within = at % self.bank_size;
        if !((within + size as usize <= 0x1000)
            || (within >= 0x1060 && within + size as usize <= 0x1c60))
        {
            cx.unmodeled();
            return 0;
        }
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
        self.state.lock().unwrap().scene = scene;
        Ok(true)
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
        let mut scene = Scene::default();
        scene.width = 20;
        scene.height = 20;
        scene.rgb = (0..400)
            .map(|i| {
                if i % 20 < 5 && i / 20 < 4 {
                    [4000; 3]
                } else {
                    [0; 3]
                }
            })
            .collect();
        scene.samples_per_cell = 1;
        scene.black_level = 0;
        scene.signal_shift = 0;
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
}
