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
    /// H.264 `level_idc` written to the sequence parameter set.
    pub level_idc: u8,
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
            || ![9, 10, 11, 12, 13, 20, 21, 22, 30, 31, 32, 40, 41, 42, 50, 51, 52]
                .contains(&self.level_idc)
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
                settings.level_idc = config.level_idc;
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

/// Settings of a [`Direct`] software encoder.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DirectConfig {
    pub width: usize,
    pub height: usize,
    pub frames_per_second: u32,
    /// Target bits per second; 0 keeps constant QP.
    pub bitrate: u32,
    /// IDR interval in frames; 1 codes every picture as an IDR.
    pub gop: u32,
    /// H.264 `level_idc` written to the sequence parameter set.
    pub level_idc: u8,
}

/// A Constrained Baseline software encoder configured from a hardware register
/// model's parameters rather than [`Config`]. It takes any even size up to
/// 4096x2304, so a 1920x1080 picture codes as 1920x1088 with the SPS frame
/// crop (`frame_crop_bottom_offset` 4) that [`Config::validate`] does not admit.
/// Each IDR carries its SPS and PPS. The encoder holds reference pictures and
/// rate-control history that cannot be serialized.
pub struct Direct {
    config: DirectConfig,
    encoder: Box<rusty_h264_encoder::Encoder>,
}
impl Direct {
    pub fn new(config: DirectConfig) -> Result<Self, String> {
        let DirectConfig { width, height, frames_per_second, bitrate, gop, level_idc } = config;
        if !(16..=4096).contains(&width)
            || !(16..=2304).contains(&height)
            || !width.is_multiple_of(2)
            || !height.is_multiple_of(2)
            || !(1..=240).contains(&frames_per_second)
            || !(1..=1023).contains(&gop)
            || level_idc == 0
        {
            return Err(format!("H.264 direct encoder settings out of range: {config:?}"));
        }
        let mut settings = rusty_h264_encoder::EncoderConfig::baseline(width, height);
        settings.level_idc = level_idc;
        settings.gop_size = gop;
        settings.min_keyint = 0;
        settings.bitrate = bitrate;
        settings.framerate = frames_per_second as f32;
        settings.aq_strength = 0.0;
        settings.i_qp_offset = 0;
        Ok(Self {
            config,
            encoder: Box::new(
                rusty_h264_encoder::Encoder::new(settings)
                    .map_err(|e| format!("software H.264 initialization: {e}"))?,
            ),
        })
    }
    pub fn config(&self) -> DirectConfig {
        self.config
    }
    /// Make the next picture an IDR.
    pub fn request_keyframe(&mut self) {
        self.encoder.request_keyframe();
    }
    /// Encode one tightly packed I420 picture (Y, then Cb, then Cr) to one
    /// Annex B access unit.
    pub fn encode_i420(&mut self, frame: &[u8]) -> Result<Vec<u8>, String> {
        let (width, height) = (self.config.width, self.config.height);
        let y = width * height;
        if frame.len() != y * 3 / 2 {
            return Err("H.264 planar input size differs".into());
        }
        let input = rusty_h264_common::YuvPlanes {
            width,
            height,
            y: &frame[..y],
            u: &frame[y..y + y / 4],
            v: &frame[y + y / 4..],
            stride_y: width,
            stride_c: width / 2,
        };
        self.encoder
            .encode_planes(&input)
            .map_err(|e| format!("software H.264 encode: {e}"))
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    /// A moving textured picture: bars, a drifting gradient and noise.
    fn textured(width: usize, height: usize, n: usize) -> Vec<u8> {
        let mut frame = vec![0u8; width * height * 3 / 2];
        let mut seed = 0x2545_f491u32;
        for y in 0..height {
            for x in 0..width {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let bar = ((x + 5 * n) * 8 / width % 8) as u32 * 24;
                let ramp = ((x + 3 * n + 2 * y) % 97) as u32;
                let noise = if (x / 64 + y / 64) % 3 == 0 { seed >> 28 } else { 0 };
                frame[y * width + x] = (16 + bar + ramp / 2 + noise).min(235) as u8;
            }
        }
        let (cw, ch) = (width / 2, height / 2);
        for y in 0..ch {
            for x in 0..cw {
                frame[width * height + y * cw + x] = (64 + (x + 2 * n) * 128 / cw) as u8;
                frame[width * height + cw * ch + y * cw + x] = (64 + (y + n) * 128 / ch) as u8;
            }
        }
        frame
    }
    fn fnv(hash: &mut u64, bytes: &[u8]) {
        for &b in bytes {
            *hash = (*hash ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    /// The software encoder's output bytes are pinned: an intra picture and
    /// inter pictures with motion, under rate control. Any change of the
    /// encoder build (features, version, kernels) must leave them alone.
    #[test]
    fn the_direct_encoder_output_is_pinned() {
        let (width, height, frames) = match std::env::var("FUJIMOD_H264_PIN").as_deref() {
            Ok("4k") => (3840, 2160, 6),
            _ => (640, 368, 8),
        };
        let mut encoder = super::Direct::new(super::DirectConfig {
            width,
            height,
            frames_per_second: 30,
            bitrate: if width > 1920 { 96_999_424 } else { 6_000_000 },
            gop: 15,
            level_idc: 51,
        })
        .unwrap();
        encoder.request_keyframe();
        let (mut hash, mut total) = (0xcbf2_9ce4_8422_2325u64, 0usize);
        for n in 0..frames {
            let unit = encoder.encode_i420(&textured(width, height, n)).unwrap();
            total += unit.len();
            fnv(&mut hash, &unit);
        }
        println!("pinned {width}x{height}: {total} bytes, hash {hash:#018x}");
        if width == 640 {
            assert_eq!((total, hash), (171_529, 0xe397_217f_5b69_cbdf));
        }
    }
    /// 1920x1080 codes as 1920x1088 with the SPS crop and decodes at 1080.
    #[test]
    fn the_direct_encoder_crops_1080() {
        let config = super::DirectConfig {
            width: 1920,
            height: 1080,
            frames_per_second: 60,
            bitrate: 50_000_000,
            gop: 30,
            level_idc: 42,
        };
        let mut encoder = super::Direct::new(config).unwrap();
        encoder.request_keyframe();
        let mut frame = vec![128u8; 1920 * 1080 * 3 / 2];
        frame[..1920 * 1080].fill(100);
        let unit = encoder.encode_i420(&frame).unwrap();
        let picture = super::decode::Decoder::new(&[]).unwrap().decode(&unit).unwrap();
        let s = picture.sequence;
        assert_eq!((s.width, s.height, s.coded_width, s.coded_height), (1920, 1080, 1920, 1088));
        assert!(picture.i420[..1920 * 1080].iter().all(|y| y.abs_diff(100) < 4));
    }
}
