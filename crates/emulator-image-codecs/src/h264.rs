//! Host compression backends. No guest memory, clocks or device effects.
//! Software uses scalar Rust and libm; VideoToolbox requires hardware encoding.
pub use fujimod_catalog_construction::VideoEncoderBackend as Backend;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub mod decode;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    pub width: usize,
    pub height: usize,
    pub frames_per_second: u32,
    pub bitrate: u32,
    pub max_qp: u8,
    pub gop: usize,
}
impl Config {
    pub fn validate(self) -> Result<(), String> {
        if !(16..=1920).contains(&self.width)
            || !(16..=1088).contains(&self.height)
            || !self.width.is_multiple_of(16)
            || !self.height.is_multiple_of(16)
            || !(1..=60).contains(&self.frames_per_second)
            || !(10_000..=100_000_000).contains(&self.bitrate)
            || !(1..=51).contains(&self.max_qp)
            || !(1..=120).contains(&self.gop)
        {
            return Err(
                "H.264 requires bounded progressive macroblock-aligned 8-bit 4:2:0 configuration"
                    .into(),
            );
        }
        Ok(())
    }
    pub fn frame_bytes(self) -> usize {
        self.width * self.height * 3 / 2
    }
}

pub fn validate_backend(backend: Backend) -> Result<(), String> {
    if backend == Backend::Videotoolbox && !cfg!(target_os = "macos") {
        return Err(
            "VideoToolbox H.264 requires macOS; select software in the run configuration".into(),
        );
    }
    Ok(())
}

pub enum Codec {
    Software(Box<rusty_h264_encoder::Encoder>),
    #[cfg(target_os = "macos")]
    Videotoolbox(videotoolbox::Encoder),
}
#[cfg(target_os = "macos")]
mod videotoolbox;

impl Codec {
    pub fn new(backend: Backend, config: Config, history: &[Arc<[u8]>]) -> Result<Self, String> {
        config.validate()?;
        validate_backend(backend)?;
        let mut codec = match backend {
            Backend::Software => {
                let mut settings =
                    rusty_h264_encoder::EncoderConfig::baseline(config.width, config.height);
                settings.level_idc = 42;
                settings.qp = config.max_qp;
                settings.gop_size = config.gop as u32;
                settings.min_keyint = 0;
                settings.bitrate = config.bitrate;
                settings.framerate = config.frames_per_second as f32;
                settings.aq_strength = 0.0;
                settings.i_qp_offset = 0;
                Self::Software(Box::new(
                    rusty_h264_encoder::Encoder::new(settings)
                        .map_err(|e| format!("software H.264 initialization: {e}"))?,
                ))
            }
            Backend::Videotoolbox => {
                if !history.is_empty() {
                    return Err("VideoToolbox codec state cannot replay identically".into());
                }
                #[cfg(target_os = "macos")]
                {
                    Self::Videotoolbox(videotoolbox::Encoder::new(config)?)
                }
                #[cfg(not(target_os = "macos"))]
                {
                    return Err("VideoToolbox H.264 requires macOS".into());
                }
            }
        };
        for frame in history {
            codec.encode(config, frame)?;
        }
        Ok(codec)
    }
    pub fn encode(&mut self, config: Config, frame: &[u8]) -> Result<Vec<u8>, String> {
        if frame.len() != config.frame_bytes() {
            return Err("H.264 planar input size differs".into());
        }
        let stream = match self {
            Self::Software(encoder) => {
                let y = config.width * config.height;
                let input = rusty_h264_common::YuvPlanes {
                    width: config.width,
                    height: config.height,
                    y: &frame[..y],
                    u: &frame[y..y + y / 4],
                    v: &frame[y + y / 4..],
                    stride_y: config.width,
                    stride_c: config.width / 2,
                };
                encoder
                    .encode_planes(&input)
                    .map_err(|e| format!("software H.264 encode: {e}"))?
            }
            #[cfg(target_os = "macos")]
            Self::Videotoolbox(encoder) => encoder.encode(config, frame)?,
        };
        normalize(&stream)
    }
}

/// Normalize the actual AVC output to one SPS/PPS/slice access unit. Optional
/// SEI, access-unit delimiters and filler carry no image samples and are omitted.
/// Multiple slices cannot be represented by the supported length-prefix writer.
fn normalize(stream: &[u8]) -> Result<Vec<u8>, String> {
    fn prefix(s: &[u8]) -> usize {
        if s.starts_with(&[0, 0, 0, 1]) {
            4
        } else if s.starts_with(&[0, 0, 1]) {
            3
        } else {
            0
        }
    }
    let mut out = Vec::new();
    let mut i = 0;
    let mut slices = 0;
    while i < stream.len() {
        let n = prefix(&stream[i..]);
        if n == 0 {
            return Err("H.264 backend omitted Annex B prefix".into());
        }
        let start = i + n;
        let mut end = start;
        while end < stream.len() && prefix(&stream[end..]) == 0 {
            end += 1;
        }
        let payload = stream
            .get(start..end)
            .filter(|p| !p.is_empty())
            .ok_or("empty H.264 NAL")?;
        match payload[0] & 31 {
            7 if payload.len() <= 64 => (),
            8 if payload.len() <= 16 => (),
            1 | 5 => {
                slices += 1;
            }
            6 | 9 | 12 => {
                i = end;
                continue;
            }
            _ => {
                return Err(
                    "H.264 backend produced unsupported NAL type or parameter-set length".into(),
                );
            }
        }
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(payload);
        i = end;
    }
    if slices != 1 {
        return Err("H.264 backend must emit exactly one coded slice per frame".into());
    }
    Ok(out)
}

/// The register transport publishes SPS/PPS once at sequence start. Later
/// IDRs reuse those parameter-set IDs; identical repeated headers are omitted.
/// A backend changing its parameter sets within a sequence must refuse.
pub fn transport_frame(stream: &[u8], previous: &[u8]) -> Result<(Vec<u8>, Vec<u8>), String> {
    let starts: Vec<usize> = stream
        .windows(4)
        .enumerate()
        .filter_map(|(i, v)| (v == [0, 0, 0, 1]).then_some(i))
        .collect();
    let mut parameters = Vec::new();
    let mut picture = Vec::new();
    for (index, start) in starts.iter().copied().enumerate() {
        let end = starts.get(index + 1).copied().unwrap_or(stream.len());
        let nal = &stream[start..end];
        let kind = *nal.get(4).ok_or("empty AVC transport NAL")? & 31;
        if matches!(kind, 7 | 8) {
            parameters.extend_from_slice(nal);
        } else {
            picture.extend_from_slice(nal);
        }
    }
    if picture.is_empty() || previous.is_empty() && parameters.is_empty() {
        return Err("H.264 transport lacks a picture or initial parameter sets".into());
    }
    if !previous.is_empty() && !parameters.is_empty() && previous != parameters {
        return Err("H.264 backend changed parameter sets within a sequence".into());
    }
    if previous.is_empty() {
        let mut initial = parameters.clone();
        initial.extend(picture);
        Ok((initial, parameters))
    } else {
        Ok((picture, previous.to_vec()))
    }
}

/// Packed Y0,Y1,Cb,Cr 4:2:2 to planar 4:2:0, averaging adjacent chroma rows.
pub fn planar_420(width: usize, height: usize, packed: &[u8]) -> Result<Vec<u8>, String> {
    if width == 0
        || height == 0
        || !width.is_multiple_of(2)
        || !height.is_multiple_of(2)
        || width > 1920
        || height > 1088
        || packed.len() != width * height * 2
    {
        return Err("H.264 packed input geometry differs".into());
    }
    let y = width * height;
    let mut out = vec![0; y * 3 / 2];
    for row in 0..height {
        for col in 0..width / 2 {
            let p = (row * width / 2 + col) * 4;
            out[row * width + col * 2..row * width + col * 2 + 2]
                .copy_from_slice(&packed[p..p + 2]);
            if row % 2 == 0 {
                let q = p + width * 2;
                let c = row / 2 * (width / 2) + col;
                out[y + c] = (packed[p + 2] as u16 + packed[q + 2] as u16).div_ceil(2) as u8;
                out[y + y / 4 + c] =
                    (packed[p + 3] as u16 + packed[q + 3] as u16).div_ceil(2) as u8;
            }
        }
    }
    Ok(out)
}
