//! Immutable host RGB/RAW/mosaic frames. Updating host input never changes a
//! latched exposure. Only the sensor's physical frame edge advances frame identity.
use super::FrameEdge;
use super::mosaic::{Cfa, MosaicSites, Radiance};
use super::noise::SensorNoise;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex, OnceLock};
#[derive(Serialize, Deserialize)]
pub enum FramePixels {
    Rgb24(Arc<[u8]>),
    Raw16(Vec<u16>),
    /// Per-site linear radiance of a CFA mosaic in a RAF's normalization:
    /// sensor counts = radiance × seconds / f_number² × analog_gain.
    Mosaic {
        cfa: Cfa,
        radiance: Radiance,
    },
}
#[derive(Serialize, Deserialize)]
enum PixelKind {
    Rgb24,
    Raw16,
    Mosaic { cfa: Cfa, norm: f64 },
}
#[derive(Serialize, Deserialize)]
#[serde(try_from = "FrameState")]
pub struct Frame {
    pub width: usize,
    pub height: usize,
    pub pixels: FramePixels,
    /// Scene radiance of an sRGB white pixel, in cd/m².
    #[serde(default = "daylight_white")]
    pub white_luminance: f64,
    #[serde(default)]
    pub noise: Option<SensorNoise>,
    /// Mosaic cell means, derived once from the immutable pixels.
    #[serde(skip)]
    sites: OnceLock<MosaicSites>,
}
/// Decoded input records use the same validation as live sensor installation.
#[derive(Deserialize)]
struct FrameState {
    width: usize,
    height: usize,
    pixels: FramePixels,
    #[serde(default = "daylight_white")]
    white_luminance: f64,
    #[serde(default)]
    noise: Option<SensorNoise>,
}
impl TryFrom<FrameState> for Frame {
    type Error = String;
    fn try_from(state: FrameState) -> Result<Self, Self::Error> {
        if !state.white_luminance.is_finite() || state.white_luminance <= 0. {
            return Err("sensor frame white luminance must be finite and positive".into());
        }
        let mut frame = Self::new(state.width, state.height, state.pixels)?;
        frame.white_luminance = state.white_luminance;
        frame.noise = state.noise.map(SensorNoise::validate).transpose()?;
        Ok(frame)
    }
}
fn daylight_white() -> f64 {
    10000.
}
impl Frame {
    /// Whether two frames hold the same immutable pixels. Optical scenes and
    /// their caches depend only on these, so frames sharing a pixel buffer
    /// share projections.
    pub fn same_pixels(&self, other: &Frame) -> bool {
        self.width == other.width
            && self.height == other.height
            && self.white_luminance.to_bits() == other.white_luminance.to_bits()
            && match (&self.pixels, &other.pixels) {
                (FramePixels::Rgb24(a), FramePixels::Rgb24(b)) => Arc::ptr_eq(a, b),
                (
                    FramePixels::Mosaic {
                        cfa: a,
                        radiance: x,
                    },
                    FramePixels::Mosaic {
                        cfa: b,
                        radiance: y,
                    },
                ) => {
                    a == b
                        && x.norm.to_bits() == y.norm.to_bits()
                        && Arc::ptr_eq(x.bytes(), y.bytes())
                }
                _ => false,
            }
    }
    /// Per-site mosaic radiance for mosaic frames; `None` for RGB and RAW.
    pub fn mosaic(&self) -> Option<&MosaicSites> {
        match &self.pixels {
            FramePixels::Mosaic { cfa, radiance } => {
                Some(self.sites.get_or_init(|| {
                    MosaicSites::new(*cfa, self.width, self.height, radiance.clone())
                }))
            }
            FramePixels::Rgb24(_) | FramePixels::Raw16(_) => None,
        }
    }
    pub fn fingerprint(&self) -> serde_json::Value {
        let pixels = match &self.pixels {
            FramePixels::Rgb24(bytes) => {
                serde_json::json!({"kind":"rgb24","contents":fujimod_emulator_devices::fingerprint::bytes(bytes)})
            }
            FramePixels::Raw16(samples) => {
                serde_json::json!({"kind":"raw16","contents":fujimod_emulator_devices::fingerprint::words16(samples)})
            }
            FramePixels::Mosaic { cfa, radiance } => {
                serde_json::json!({"kind":"mosaic","cfa":cfa,"contents":fujimod_emulator_devices::fingerprint::float32s(radiance.values())})
            }
        };
        let mut result = serde_json::json!({"width":self.width,"height":self.height,"white_luminance":self.white_luminance,"pixels":pixels});
        if let Some(noise) = self.noise {
            result["noise"] = serde_json::json!(noise);
        }
        result
    }
    pub fn checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        let (kind, pixels): (PixelKind, Arc<[u8]>) = match &self.pixels {
            FramePixels::Rgb24(bytes) => (PixelKind::Rgb24, bytes.clone()),
            FramePixels::Raw16(words) => (
                PixelKind::Raw16,
                words
                    .iter()
                    .flat_map(|v| v.to_le_bytes())
                    .collect::<Vec<_>>()
                    .into(),
            ),
            FramePixels::Mosaic { cfa, radiance } => (
                PixelKind::Mosaic {
                    cfa: *cfa,
                    norm: radiance.norm,
                },
                radiance.bytes().clone(),
            ),
        };
        Ok(fujimod_emulator_devices::checkpoint::Checkpoint::encode(
            "sensor-frame",
            &(
                self.width,
                self.height,
                self.white_luminance,
                kind,
                self.noise,
            ),
        )?
        .with_blobs(vec![pixels]))
    }
    pub fn from_checkpoint(
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<Self, String> {
        let (width, height, white, kind, noise): (
            usize,
            usize,
            f64,
            PixelKind,
            Option<SensorNoise>,
        ) = saved.decode("sensor-frame")?;
        let bytes_per_site = match kind {
            PixelKind::Rgb24 => 3,
            PixelKind::Raw16 => 2,
            PixelKind::Mosaic { .. } => 2,
        };
        if width == 0
            || height == 0
            || width > 8192
            || height > 8192
            || width * height > 32_000_000
            || !white.is_finite()
            || white <= 0.
            || saved.blobs().len() != 1
            || !saved.children().is_empty()
            || saved.blobs()[0].len() != width * height * bytes_per_site
        {
            return Err("sensor frame checkpoint raster or radiance differs".into());
        }
        let pixels = match kind {
            PixelKind::Rgb24 => FramePixels::Rgb24(saved.blobs()[0].clone()),
            PixelKind::Raw16 => FramePixels::Raw16(
                saved.blobs()[0]
                    .chunks_exact(2)
                    .map(|b| u16::from_le_bytes([b[0], b[1]]))
                    .collect(),
            ),
            PixelKind::Mosaic { cfa, norm } => FramePixels::Mosaic {
                cfa,
                radiance: Radiance::from_bytes(saved.blobs()[0].clone(), norm)?,
            },
        };
        let mut frame = Self::new(width, height, pixels)?;
        frame.white_luminance = white;
        frame.noise = noise.map(SensorNoise::validate).transpose()?;
        Ok(frame)
    }
    pub fn new(width: usize, height: usize, pixels: FramePixels) -> Result<Self, String> {
        if width == 0 || height == 0 || width > 8192 || height > 8192 || width * height > 32_000_000
        {
            return Err("sensor input dimensions are unbounded".into());
        }
        let len = match &pixels {
            FramePixels::Rgb24(p) => p.len() / 3,
            FramePixels::Raw16(p) => p.len(),
            FramePixels::Mosaic { cfa, radiance } => {
                if cfa.iter().flatten().any(|c| *c > 2) {
                    return Err("mosaic CFA colors must be 0, 1 or 2".into());
                }
                if !radiance.bytes().len().is_multiple_of(2) {
                    return Err("mosaic counts must be whole u16 samples".into());
                }
                if !radiance.norm.is_finite()
                    || radiance.norm <= 0.
                    || radiance.values().any(|v| !v.is_finite())
                {
                    return Err("mosaic radiance must be finite and nonnegative".into());
                }
                radiance.len()
            }
        };
        if len != width * height || matches!(&pixels,FramePixels::Rgb24(p) if p.len()%3!=0) {
            return Err("sensor input has an invalid raster length".into());
        }
        Ok(Self {
            width,
            height,
            pixels,
            white_luminance: daylight_white(),
            noise: None,
            sites: OnceLock::new(),
        })
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub struct SensorFrames {
    pub current: Arc<Frame>,
    pub latched: Arc<Frame>,
    pub edge: Option<FrameEdge>,
    pub view: Option<super::view::View>,
}
pub type SensorFramesHandle = Arc<Mutex<SensorFrames>>;
impl SensorFrames {
    pub fn fingerprint(&self) -> serde_json::Value {
        serde_json::json!({"current":self.current.fingerprint(),"latched":self.latched.fingerprint(),"edge":self.edge,"view":self.view})
    }
    pub fn new(frame: Frame) -> Self {
        let frame = Arc::new(frame);
        Self {
            current: frame.clone(),
            latched: frame,
            edge: None,
            view: None,
        }
    }
    pub fn set_frame(&mut self, frame: Frame) {
        self.current = Arc::new(frame);
    }
    pub fn latch(&mut self, tick: u64) -> Result<FrameEdge, String> {
        if self.edge.is_some_and(|s| tick < s.tick) {
            return Err("sensor frame clock moved backward".into());
        }
        let edge = FrameEdge {
            index: self
                .edge
                .map_or(Some(1), |s| s.index.checked_add(1))
                .ok_or("sensor frame index overflow")?,
            tick,
        };
        self.latched = self.current.clone();
        self.edge = Some(edge);
        Ok(edge)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mosaic_normalization_is_validated_and_changes_pixel_identity() {
        let counts = Radiance::from_counts([0, u16::MAX], 1.);
        for norm in [
            0.,
            -1.,
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::MIN_POSITIVE,
        ] {
            let invalid = Frame::new(
                2,
                1,
                FramePixels::Mosaic {
                    cfa: [[0; 6]; 6],
                    radiance: Radiance::from_bytes(counts.bytes().clone(), norm).unwrap(),
                },
            );
            assert!(invalid.is_err(), "norm {norm}");
        }
        let frame = Frame::new(
            2,
            1,
            FramePixels::Mosaic {
                cfa: [[0; 6]; 6],
                radiance: counts.clone(),
            },
        )
        .unwrap();
        let other = Frame::new(
            2,
            1,
            FramePixels::Mosaic {
                cfa: [[0; 6]; 6],
                radiance: Radiance::from_bytes(counts.bytes().clone(), 2.).unwrap(),
            },
        )
        .unwrap();
        assert!(!frame.same_pixels(&other));
        assert_ne!(frame.fingerprint(), other.fingerprint());
        let mut odd = serde_json::to_value(&frame).unwrap();
        odd["pixels"]["Mosaic"]["radiance"]["counts"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!(0));
        assert!(serde_json::from_value::<Frame>(odd).is_err());
        let mut encoded = serde_json::to_value(&frame).unwrap();
        encoded["pixels"]["Mosaic"]["radiance"]["norm"] = serde_json::json!(0.);
        assert!(serde_json::from_value::<Frame>(encoded).is_err());
        let corrupt = fujimod_emulator_devices::checkpoint::Checkpoint::encode(
            "sensor-frame",
            &(
                2usize,
                1usize,
                10000.,
                PixelKind::Mosaic {
                    cfa: [[0u8; 6]; 6],
                    norm: 0.,
                },
            ),
        )
        .unwrap()
        .with_blobs(vec![counts.bytes().clone()]);
        assert!(Frame::from_checkpoint(&corrupt).is_err());
    }

    #[test]
    fn frame_edge_freezes_external_input() {
        let mut s = SensorFrames::new(Frame::new(2, 1, FramePixels::Raw16(vec![1, 2])).unwrap());
        assert_eq!(s.latch(5).unwrap(), FrameEdge { index: 1, tick: 5 });
        s.set_frame(Frame::new(2, 1, FramePixels::Raw16(vec![3, 4])).unwrap());
        assert!(matches!(&s.latched.pixels,FramePixels::Raw16(p) if p==&[1,2]));
        assert_eq!(s.edge.unwrap().index, 1);
        s.latch(6).unwrap();
        assert!(matches!(&s.latched.pixels,FramePixels::Raw16(p) if p==&[3,4]));
    }
    #[test]
    fn mosaic_frames_checkpoint_and_fingerprint_every_site_and_phase() {
        let cfa = [
            [0, 2, 1, 2, 0, 1],
            [1, 1, 0, 1, 1, 2],
            [1, 1, 2, 1, 1, 0],
            [2, 0, 1, 0, 2, 1],
            [1, 1, 2, 1, 1, 0],
            [1, 1, 0, 1, 1, 2],
        ];
        let radiance = Radiance::from_counts(0..21, 2.);
        let frame = Frame::new(
            7,
            3,
            FramePixels::Mosaic {
                cfa,
                radiance: radiance.clone(),
            },
        )
        .unwrap();
        let saved = frame.checkpoint().unwrap();
        assert_eq!(saved.blobs()[0].len(), 21 * 2);
        assert!(Arc::ptr_eq(&saved.blobs()[0], radiance.bytes()));
        let restored = Frame::from_checkpoint(&saved).unwrap();
        assert_eq!(restored.fingerprint(), frame.fingerprint());
        // The restore shares the radiance buffer, so optical caches still match.
        assert!(restored.same_pixels(&frame));
        let mut shifted = cfa;
        shifted[0].swap(0, 1);
        let other = Frame::new(
            7,
            3,
            FramePixels::Mosaic {
                cfa: shifted,
                radiance,
            },
        )
        .unwrap();
        assert_ne!(other.fingerprint(), frame.fingerprint());
        let corrupt = saved.clone().with_blobs(vec![Arc::from(vec![0u8; 3])]);
        assert!(Frame::from_checkpoint(&corrupt).is_err());
        assert!(
            Frame::new(
                1,
                1,
                FramePixels::Mosaic {
                    cfa: [[3; 6]; 6],
                    radiance: Radiance::from_counts([1], 1.)
                }
            )
            .is_err()
        );
        assert!(
            Frame::new(
                1,
                1,
                FramePixels::Mosaic {
                    cfa: [[0; 6]; 6],
                    radiance: Radiance::from_counts([1], f64::NAN)
                }
            )
            .is_err()
        );
        assert!(
            Frame::new(
                1,
                1,
                FramePixels::Mosaic {
                    cfa: [[0; 6]; 6],
                    radiance: Radiance::from_counts([1], -1.)
                }
            )
            .is_err()
        );
    }
}

#[cfg(test)]
mod fingerprint_tests {
    use super::*;
    #[test]
    fn sensor_identity_hashes_complete_pixels_without_expanding_rasters() {
        let mut source = SensorFrames::new(
            Frame::new(
                1536,
                1024,
                FramePixels::Rgb24(vec![90; 1536 * 1024 * 3].into()),
            )
            .unwrap(),
        );
        source.latch(10).unwrap();
        let saved = source.clone();
        let before = source.fingerprint();
        assert!(serde_json::to_vec(&before).unwrap().len() < 1024);
        source.set_frame(Frame::new(1, 1, FramePixels::Rgb24(vec![4, 5, 6].into())).unwrap());
        assert_ne!(source.fingerprint(), before);
        assert_eq!(saved.fingerprint(), before);
        assert!(Arc::ptr_eq(&saved.latched, &source.latched));
        source.latch(20).unwrap();
        assert!(!Arc::ptr_eq(&saved.latched, &source.latched));
        source = saved;
        assert_eq!(source.fingerprint(), before);
    }
    #[test]
    fn raw_fingerprint_uses_every_word_in_little_endian_order() {
        let words = [0x1234, 0xff00, 0, 0xabcd];
        let identity = fujimod_emulator_devices::fingerprint::words16(&words);
        let bytes: Vec<_> = words.iter().flat_map(|v| v.to_le_bytes()).collect();
        assert_eq!(
            identity["sha256"],
            fujimod_emulator_devices::fingerprint::bytes(&bytes)["sha256"]
        );
        assert_ne!(
            identity,
            fujimod_emulator_devices::fingerprint::words16(&[0x1234, 0xff00, 0, 0xabce])
        );
    }
}
