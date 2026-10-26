//! Required-hardware VideoToolbox AVC encoding, without guest effects.
use super::Config;
use apple_cf::{
    cm::CMTime,
    cv::{CVPixelBuffer, CVPixelBufferLockFlags},
};
use videotoolbox::{Codec, CompressionSession, HardwareAcceleration, ProfileLevel};

pub struct Encoder {
    session: CompressionSession,
    index: i64,
}
impl Encoder {
    pub fn new(config: Config) -> Result<Self, String> {
        let session =
            CompressionSession::builder(config.width as i32, config.height as i32, Codec::H264)
                .with_hardware_acceleration(HardwareAcceleration::Required)
                .with_real_time(true)
                .with_allow_frame_reordering(false)
                .with_average_bit_rate(config.bitrate as i32)
                .with_expected_frame_rate(config.frames_per_second as f64)
                .with_max_keyframe_interval(config.gop as i32)
                .with_profile_level(ProfileLevel::H264BaselineAutoLevel)
                .build()
                .map_err(|e| format!("VideoToolbox hardware initialization: {e}"))?;
        Ok(Self { session, index: 0 })
    }
    pub fn encode(&mut self, config: Config, frame: &[u8]) -> Result<Vec<u8>, String> {
        let w = config.width;
        let h = config.height;
        let y = w * h;
        // Full-range bi-planar 4:2:0. Populate the exact same Y/Cb/Cr samples
        // as the software backend, without a host RGB round trip.
        let image = CVPixelBuffer::create(w, h, u32::from_be_bytes(*b"420f"))
            .map_err(|e| format!("VideoToolbox pixel allocation: {e}"))?;
        {
            let mut guard = image
                .lock(CVPixelBufferLockFlags::NONE)
                .map_err(|e| format!("VideoToolbox pixel lock: {e}"))?;
            if guard.plane_count() != 2 {
                return Err("VideoToolbox NV12 plane count differs".into());
            }
            for plane in 0..2 {
                let rows = if plane == 0 { h } else { h / 2 };
                let pitch = guard.bytes_per_row_of_plane(plane);
                if pitch < w || guard.height_of_plane(plane) != rows {
                    return Err("VideoToolbox NV12 geometry differs".into());
                }
                let pointer = guard
                    .base_address_of_plane_mut(plane)
                    .ok_or("VideoToolbox missing pixel plane")?;
                // SAFETY: This newly allocated buffer has no native or Rust
                // aliases. The read-write guard pins its validated plane until
                // these slices expire, before submission to the encoder.
                let bytes = unsafe { std::slice::from_raw_parts_mut(pointer, pitch * rows) };
                bytes.fill(0);
                for row in 0..rows {
                    let dst = &mut bytes[row * pitch..row * pitch + w];
                    if plane == 0 {
                        dst.copy_from_slice(&frame[row * w..row * w + w]);
                    } else {
                        for col in 0..w / 2 {
                            let c = row * w / 2 + col;
                            dst[col * 2] = frame[y + c];
                            dst[col * 2 + 1] = frame[y + y / 4 + c];
                        }
                    }
                }
            }
        }
        let sample = futures_lite::future::block_on(self.session.encode_frame_async(
            image,
            CMTime::new(self.index, config.frames_per_second as i32),
            CMTime::new(1, config.frames_per_second as i32),
            None,
        ))
        .map_err(|e| format!("VideoToolbox hardware encode: {e}"))?;
        self.index += 1;
        let format = sample
            .format_description()
            .ok_or("VideoToolbox missing AVC format")?;
        let parameters = format
            .video_parameter_sets()
            .map_err(|e| format!("VideoToolbox AVC parameter sets: {e}"))?;
        let length = usize::try_from(parameters.nal_unit_header_length)
            .map_err(|_| "VideoToolbox invalid NAL length")?;
        if !(1..=4).contains(&length) {
            return Err("VideoToolbox unsupported NAL length field".into());
        }
        let buffer = sample.data_buffer().ok_or("VideoToolbox dropped frame")?;
        let data = buffer
            .copy_data_bytes(0, buffer.data_length())
            .ok_or("VideoToolbox missing AVC data")?;
        let mut nals = Vec::new();
        let mut offset = 0;
        let mut idr = false;
        while offset < data.len() {
            let header = data
                .get(offset..offset + length)
                .ok_or("VideoToolbox truncated AVC length")?;
            let n = header
                .iter()
                .fold(0usize, |n, b| (n << 8) | usize::from(*b));
            offset += length;
            let nal = data
                .get(offset..offset + n)
                .filter(|n| !n.is_empty())
                .ok_or("VideoToolbox truncated AVC NAL")?;
            idr |= nal[0] & 31 == 5;
            nals.push(nal);
            offset += n;
        }
        let mut out = Vec::new();
        if idr {
            for parameter in parameters.parameter_sets {
                out.extend_from_slice(&[0, 0, 0, 1]);
                out.extend(parameter);
            }
        }
        for nal in nals {
            out.extend_from_slice(&[0, 0, 0, 1]);
            out.extend(nal);
        }
        if out.is_empty() {
            return Err("VideoToolbox dropped frame".into());
        }
        Ok(out)
    }
}
