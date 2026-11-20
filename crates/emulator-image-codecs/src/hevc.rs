//! A minimal, deterministic HEVC (H.265) Main-profile encoder for emulated
//! camera movie recording. No guest memory, clocks or device effects.
//!
//! Every picture is an IDR picture whose 16x16 coding units are all coded as
//! I_PCM: the 8-bit 4:2:0 samples are stored without prediction or transform.
//! The stream is therefore lossless and large (12 bits per pixel), but it is a
//! valid HEVC Main bitstream that any decoder plays. The only CABAC-coded
//! syntax is `split_cu_flag` (always 0, one context), `pcm_flag` and
//! `end_of_slice_segment_flag` (terminate bins).
//!
//! Stream layout (ITU-T H.265 sections 7.3 and 9.3):
//! - VPS, SPS and PPS before the first picture and every `parameter_interval`
//!   pictures after it.
//! - One IDR_W_RADL slice segment per picture. CTB 16x16, minimum CB 8x8,
//!   I_PCM 16x16, no SAO, deblocking disabled, PCM loop filter disabled.
//! - Pictures whose size is not a multiple of 16 are padded by edge
//!   replication and cropped back with the conformance window.
use rusty_h264_common::cabac_tables::{RANGE_LPS, STATE_TRANS};

pub const NAL_VPS: u8 = 32;
pub const NAL_SPS: u8 = 33;
pub const NAL_PPS: u8 = 34;
/// IDR_W_RADL: the X-T5 driver accepts VCL types 0, 1, 19 and 21 only (0x13d178c).
pub const NAL_IDR_W_RADL: u8 = 19;

const CTB: usize = 16;
/// `split_cu_flag` initValue for initType 0 and ctxInc 0 (Table 9-11).
const SPLIT_CU_INIT: u8 = 139;
const SLICE_QP: i32 = 26;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PcmConfig {
    pub width: usize,
    pub height: usize,
    /// `general_level_idc` (30 x level, for example 123 for level 4.1).
    pub level_idc: u8,
    /// Parameter sets precede picture 0 and every picture whose index is a
    /// multiple of this value.
    pub parameter_interval: u32,
}

impl PcmConfig {
    pub fn validate(self) -> Result<(), String> {
        if !(16..=8192).contains(&self.width)
            || !(16..=4352).contains(&self.height)
            || !self.width.is_multiple_of(2)
            || !self.height.is_multiple_of(2)
            || self.level_idc == 0
            || self.parameter_interval == 0
        {
            return Err("HEVC PCM requires an even 8-bit 4:2:0 picture of 16..8192 x 16..4352".into());
        }
        Ok(())
    }
    fn coded(self) -> (usize, usize) {
        (self.width.next_multiple_of(CTB), self.height.next_multiple_of(CTB))
    }
    /// The level for the luma picture size (MaxLumaPs of Table A.8).
    pub fn level_for(width: usize, height: usize) -> u8 {
        match width * height {
            0..=552_960 => 93,
            552_961..=2_228_224 => 123,
            2_228_225..=8_912_896 => 153,
            _ => 183,
        }
    }
}

pub struct PcmEncoder {
    config: PcmConfig,
    pictures: u64,
}

impl PcmEncoder {
    pub fn new(config: PcmConfig) -> Result<Self, String> {
        config.validate()?;
        Ok(Self { config, pictures: 0 })
    }
    pub fn config(&self) -> PcmConfig {
        self.config
    }
    /// The next picture starts with the parameter sets.
    pub fn request_keyframe(&mut self) {
        self.pictures = 0;
    }

    /// Code one tight I420 picture as an Annex B access unit (4-byte start
    /// codes).
    pub fn encode_i420(&mut self, frame: &[u8]) -> Result<Vec<u8>, String> {
        let PcmConfig { width, height, .. } = self.config;
        if frame.len() != width * height * 3 / 2 {
            return Err("HEVC PCM planar input size differs".into());
        }
        let mut out = Vec::with_capacity(frame.len() + frame.len() / 8 + 256);
        if self.pictures.is_multiple_of(u64::from(self.config.parameter_interval)) {
            for (kind, payload) in
                [(NAL_VPS, self.vps()), (NAL_SPS, self.sps()), (NAL_PPS, self.pps())]
            {
                push_nal(&mut out, kind, &payload);
            }
        }
        push_nal(&mut out, NAL_IDR_W_RADL, &self.slice(frame));
        self.pictures += 1;
        Ok(out)
    }

    fn profile_tier_level(&self, w: &mut Bits) {
        w.put(0, 2); // general_profile_space
        w.put(0, 1); // general_tier_flag (Main tier)
        w.put(1, 5); // general_profile_idc = Main
        // general_profile_compatibility_flag[j]: Main (1) and Main 10 (2).
        w.put(0x6000_0000, 32);
        w.put(1, 1); // general_progressive_source_flag
        w.put(0, 1); // general_interlaced_source_flag
        w.put(0, 1); // general_non_packed_constraint_flag
        w.put(1, 1); // general_frame_only_constraint_flag
        w.put(0, 32); // general_reserved_zero_43bits (first 32)
        w.put(0, 11); // general_reserved_zero_43bits (last 11)
        w.put(0, 1); // general_inbld_flag / reserved
        w.put(u32::from(self.config.level_idc), 8);
    }

    fn vps(&self) -> Vec<u8> {
        let mut w = Bits::default();
        w.put(0, 4); // vps_video_parameter_set_id
        w.put(1, 1); // vps_base_layer_internal_flag
        w.put(1, 1); // vps_base_layer_available_flag
        w.put(0, 6); // vps_max_layers_minus1
        w.put(0, 3); // vps_max_sub_layers_minus1
        w.put(1, 1); // vps_temporal_id_nesting_flag
        w.put(0xffff, 16); // vps_reserved_0xffff_16bits
        self.profile_tier_level(&mut w);
        w.put(1, 1); // vps_sub_layer_ordering_info_present_flag
        w.ue(0); // vps_max_dec_pic_buffering_minus1
        w.ue(0); // vps_max_num_reorder_pics
        w.ue(0); // vps_max_latency_increase_plus1
        w.put(0, 6); // vps_max_layer_id
        w.ue(0); // vps_num_layer_sets_minus1
        w.put(0, 1); // vps_timing_info_present_flag
        w.put(0, 1); // vps_extension_flag
        w.trailing();
        w.bytes
    }

    fn sps(&self) -> Vec<u8> {
        let (coded_w, coded_h) = self.config.coded();
        let mut w = Bits::default();
        w.put(0, 4); // sps_video_parameter_set_id
        w.put(0, 3); // sps_max_sub_layers_minus1
        w.put(1, 1); // sps_temporal_id_nesting_flag
        self.profile_tier_level(&mut w);
        w.ue(0); // sps_seq_parameter_set_id
        w.ue(1); // chroma_format_idc = 4:2:0
        w.ue(coded_w as u32); // pic_width_in_luma_samples
        w.ue(coded_h as u32); // pic_height_in_luma_samples
        let (crop_w, crop_h) = (coded_w - self.config.width, coded_h - self.config.height);
        if crop_w == 0 && crop_h == 0 {
            w.put(0, 1); // conformance_window_flag
        } else {
            w.put(1, 1);
            w.ue(0); // conf_win_left_offset
            w.ue((crop_w / 2) as u32); // conf_win_right_offset (chroma units)
            w.ue(0); // conf_win_top_offset
            w.ue((crop_h / 2) as u32); // conf_win_bottom_offset
        }
        w.ue(0); // bit_depth_luma_minus8
        w.ue(0); // bit_depth_chroma_minus8
        w.ue(4); // log2_max_pic_order_cnt_lsb_minus4
        w.put(1, 1); // sps_sub_layer_ordering_info_present_flag
        w.ue(0); // sps_max_dec_pic_buffering_minus1
        w.ue(0); // sps_max_num_reorder_pics
        w.ue(0); // sps_max_latency_increase_plus1
        w.ue(0); // log2_min_luma_coding_block_size_minus3 (8x8)
        w.ue(1); // log2_diff_max_min_luma_coding_block_size (CTB 16x16)
        w.ue(0); // log2_min_luma_transform_block_size_minus2 (4x4)
        w.ue(2); // log2_diff_max_min_luma_transform_block_size (16x16)
        w.ue(0); // max_transform_hierarchy_depth_inter
        w.ue(0); // max_transform_hierarchy_depth_intra
        w.put(0, 1); // scaling_list_enabled_flag
        w.put(0, 1); // amp_enabled_flag
        w.put(0, 1); // sample_adaptive_offset_enabled_flag
        w.put(1, 1); // pcm_enabled_flag
        w.put(7, 4); // pcm_sample_bit_depth_luma_minus1
        w.put(7, 4); // pcm_sample_bit_depth_chroma_minus1
        w.ue(1); // log2_min_pcm_luma_coding_block_size_minus3 (16x16)
        w.ue(0); // log2_diff_max_min_pcm_luma_coding_block_size
        w.put(1, 1); // pcm_loop_filter_disabled_flag
        w.ue(0); // num_short_term_ref_pic_sets
        w.put(0, 1); // long_term_ref_pics_present_flag
        w.put(0, 1); // sps_temporal_mvp_enabled_flag
        w.put(0, 1); // strong_intra_smoothing_enabled_flag
        w.put(0, 1); // vui_parameters_present_flag
        w.put(0, 1); // sps_extension_present_flag
        w.trailing();
        w.bytes
    }

    fn pps(&self) -> Vec<u8> {
        let mut w = Bits::default();
        w.ue(0); // pps_pic_parameter_set_id
        w.ue(0); // pps_seq_parameter_set_id
        w.put(0, 1); // dependent_slice_segments_enabled_flag
        w.put(0, 1); // output_flag_present_flag
        w.put(0, 3); // num_extra_slice_header_bits
        w.put(0, 1); // sign_data_hiding_enabled_flag
        w.put(0, 1); // cabac_init_present_flag
        w.ue(0); // num_ref_idx_l0_default_active_minus1
        w.ue(0); // num_ref_idx_l1_default_active_minus1
        w.se(0); // init_qp_minus26
        w.put(0, 1); // constrained_intra_pred_flag
        w.put(0, 1); // transform_skip_enabled_flag
        w.put(0, 1); // cu_qp_delta_enabled_flag
        w.se(0); // pps_cb_qp_offset
        w.se(0); // pps_cr_qp_offset
        w.put(0, 1); // pps_slice_chroma_qp_offsets_present_flag
        w.put(0, 1); // weighted_pred_flag
        w.put(0, 1); // weighted_bipred_flag
        w.put(0, 1); // transquant_bypass_enabled_flag
        w.put(0, 1); // tiles_enabled_flag
        w.put(0, 1); // entropy_coding_sync_enabled_flag
        w.put(0, 1); // pps_loop_filter_across_slices_enabled_flag
        w.put(1, 1); // deblocking_filter_control_present_flag
        w.put(0, 1); // deblocking_filter_override_enabled_flag
        w.put(1, 1); // pps_deblocking_filter_disabled_flag
        w.put(0, 1); // pps_scaling_list_data_present_flag
        w.put(0, 1); // lists_modification_present_flag
        w.ue(0); // log2_parallel_merge_level_minus2
        w.put(0, 1); // slice_segment_header_extension_present_flag
        w.put(0, 1); // pps_extension_present_flag
        w.trailing();
        w.bytes
    }

    fn slice(&self, frame: &[u8]) -> Vec<u8> {
        let PcmConfig { width, height, .. } = self.config;
        let (coded_w, coded_h) = self.config.coded();
        let mut w = Bits::default();
        w.put(1, 1); // first_slice_segment_in_pic_flag
        w.put(0, 1); // no_output_of_prior_pics_flag (IRAP)
        w.ue(0); // slice_pic_parameter_set_id
        w.ue(2); // slice_type = I
        w.se(0); // slice_qp_delta
        // byte_alignment(): alignment_bit_equal_to_one, then zero bits.
        w.put(1, 1);
        w.align_zero();

        let luma = &frame[..width * height];
        let cb = &frame[width * height..width * height * 5 / 4];
        let cr = &frame[width * height * 5 / 4..];
        let (cw, ch) = (width / 2, height / 2);
        let sample = |plane: &[u8], pw: usize, ph: usize, x: usize, y: usize| {
            plane[y.min(ph - 1) * pw + x.min(pw - 1)]
        };
        let mut cabac = Cabac::new(&mut w);
        let ctus = (coded_w / CTB) * (coded_h / CTB);
        let mut index = 0;
        for ty in 0..coded_h / CTB {
            for tx in 0..coded_w / CTB {
                cabac.decision(0); // split_cu_flag = 0 (16x16 CU)
                cabac.terminate(true); // pcm_flag = 1, then flush
                let out = cabac.bits();
                out.align_zero(); // pcm_alignment_zero_bit
                let (x0, y0) = (tx * CTB, ty * CTB);
                for y in 0..CTB {
                    for x in 0..CTB {
                        out.put(u32::from(sample(luma, width, height, x0 + x, y0 + y)), 8);
                    }
                }
                for plane in [cb, cr] {
                    for y in 0..CTB / 2 {
                        for x in 0..CTB / 2 {
                            out.put(u32::from(sample(plane, cw, ch, x0 / 2 + x, y0 / 2 + y)), 8);
                        }
                    }
                }
                cabac.restart();
                index += 1;
                // end_of_slice_segment_flag; the final flush writes the
                // rbsp_stop_one_bit.
                cabac.terminate(index == ctus);
            }
        }
        w.align_zero();
        w.bytes
    }
}

/// Append one NAL unit with a 4-byte start code, a two-byte header and
/// emulation prevention.
fn push_nal(out: &mut Vec<u8>, kind: u8, rbsp: &[u8]) {
    out.extend_from_slice(&[0, 0, 0, 1, kind << 1, 1]);
    let mut zeros = 0;
    for &byte in rbsp {
        if zeros >= 2 && byte <= 3 {
            out.push(3);
            zeros = 0;
        }
        out.push(byte);
        zeros = if byte == 0 { zeros + 1 } else { 0 };
    }
}

#[derive(Default)]
struct Bits {
    bytes: Vec<u8>,
    acc: u8,
    used: u8,
}
impl Bits {
    fn bit(&mut self, bit: u32) {
        self.acc = (self.acc << 1) | (bit & 1) as u8;
        self.used += 1;
        if self.used == 8 {
            self.bytes.push(self.acc);
            self.acc = 0;
            self.used = 0;
        }
    }
    fn put(&mut self, value: u32, count: u32) {
        if self.used == 0 && count == 8 {
            self.bytes.push(value as u8);
            return;
        }
        for i in (0..count).rev() {
            self.bit(value >> i);
        }
    }
    fn ue(&mut self, value: u32) {
        let v = u64::from(value) + 1;
        let len = 64 - v.leading_zeros();
        for _ in 1..len {
            self.bit(0);
        }
        for i in (0..len).rev() {
            self.bit((v >> i) as u32);
        }
    }
    fn se(&mut self, value: i32) {
        self.ue(if value > 0 { value as u32 * 2 - 1 } else { (-value) as u32 * 2 });
    }
    fn align_zero(&mut self) {
        while self.used != 0 {
            self.bit(0);
        }
    }
    fn trailing(&mut self) {
        self.bit(1);
        self.align_zero();
    }
}

/// The CABAC arithmetic encoder of section 9.3.4.3 with the single context
/// this encoder needs.
struct Cabac<'a> {
    out: &'a mut Bits,
    low: u32,
    range: u32,
    outstanding: u32,
    first: bool,
    state: u8,
    mps: u8,
}
impl<'a> Cabac<'a> {
    fn new(out: &'a mut Bits) -> Self {
        // Section 9.3.2.2 context initialisation.
        let slope = i32::from(SPLIT_CU_INIT >> 4);
        let offset = i32::from(SPLIT_CU_INIT & 15);
        let (m, n) = (slope * 5 - 45, (offset << 3) - 16);
        let pre = (((m * SLICE_QP.clamp(0, 51)) >> 4) + n).clamp(1, 126);
        let (state, mps) = if pre <= 63 { (63 - pre, 0) } else { (pre - 64, 1) };
        Self { out, low: 0, range: 510, outstanding: 0, first: true, state: state as u8, mps }
    }
    fn bits(&mut self) -> &mut Bits {
        self.out
    }
    /// Re-initialise the arithmetic engine after PCM samples (9.3.2.5); the
    /// context variables keep their state.
    fn restart(&mut self) {
        self.low = 0;
        self.range = 510;
        self.outstanding = 0;
        self.first = true;
    }
    fn put_bit(&mut self, bit: u32) {
        if self.first {
            self.first = false;
        } else {
            self.out.bit(bit);
        }
        while self.outstanding > 0 {
            self.out.bit(1 - bit);
            self.outstanding -= 1;
        }
    }
    fn renorm(&mut self) {
        while self.range < 256 {
            if self.low < 256 {
                self.put_bit(0);
            } else if self.low >= 512 {
                self.low -= 512;
                self.put_bit(1);
            } else {
                self.low -= 256;
                self.outstanding += 1;
            }
            self.range <<= 1;
            self.low <<= 1;
        }
    }
    fn decision(&mut self, bin: u8) {
        let lps = u32::from(RANGE_LPS[self.state as usize][((self.range >> 6) & 3) as usize]);
        self.range -= lps;
        if bin != self.mps {
            self.low += self.range;
            self.range = lps;
            if self.state == 0 {
                self.mps = 1 - self.mps;
            }
            self.state = STATE_TRANS[self.state as usize][0];
        } else {
            self.state = STATE_TRANS[self.state as usize][1];
        }
        self.renorm();
    }
    /// EncodeTerminate; a 1 bin also runs EncodeFlush, whose last written
    /// bit is 1.
    fn terminate(&mut self, bin: bool) {
        self.range -= 2;
        if bin {
            self.low += self.range;
            self.range = 2;
            self.renorm();
            self.put_bit((self.low >> 9) & 1);
            let last = ((self.low >> 7) & 3) | 1;
            self.out.bit(last >> 1);
            self.out.bit(last);
        } else {
            self.renorm();
        }
    }
}

/// The NAL units of an Annex B stream (payloads without start codes).
pub fn nal_units(stream: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= stream.len() {
        if stream[i..i + 3] == [0, 0, 1] {
            starts.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    starts
        .iter()
        .enumerate()
        .map(|(n, &start)| {
            let mut end = starts.get(n + 1).map_or(stream.len(), |next| next - 3);
            while end > start && stream[end - 1] == 0 {
                end -= 1;
            }
            &stream[start..end]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture(width: usize, height: usize) -> Vec<u8> {
        let mut frame = vec![128; width * height * 3 / 2];
        for y in 0..height {
            for x in 0..width {
                frame[y * width + x] = ((x * 7 + y * 3) % 256) as u8;
            }
        }
        frame
    }

    #[test]
    fn parameter_sets_and_one_idr_slice() {
        let config = PcmConfig { width: 64, height: 40, level_idc: 93, parameter_interval: 2 };
        let mut encoder = PcmEncoder::new(config).unwrap();
        let first = encoder.encode_i420(&picture(64, 40)).unwrap();
        let kinds: Vec<u8> = nal_units(&first).iter().map(|n| n[0] >> 1).collect();
        assert_eq!(kinds, [NAL_VPS, NAL_SPS, NAL_PPS, NAL_IDR_W_RADL]);
        assert_eq!(&first[..4], &[0, 0, 0, 1]);
        let second = encoder.encode_i420(&picture(64, 40)).unwrap();
        let kinds: Vec<u8> = nal_units(&second).iter().map(|n| n[0] >> 1).collect();
        assert_eq!(kinds, [NAL_IDR_W_RADL]);
        let third = encoder.encode_i420(&picture(64, 40)).unwrap();
        assert_eq!(nal_units(&third).len(), 4);
    }

    #[test]
    fn slices_hold_every_sample_and_are_deterministic() {
        let config = PcmConfig { width: 32, height: 32, level_idc: 93, parameter_interval: 1 };
        let frame = picture(32, 32);
        let a = PcmEncoder::new(config).unwrap().encode_i420(&frame).unwrap();
        let b = PcmEncoder::new(config).unwrap().encode_i420(&frame).unwrap();
        assert_eq!(a, b);
        let slice = *nal_units(&a).last().unwrap();
        // Four CTUs of 384 PCM bytes each, plus header and CABAC bits.
        assert!(slice.len() > 4 * 384 && slice.len() < 4 * 384 + 64, "{}", slice.len());
        // The first CTU's luma row appears verbatim after the PCM alignment.
        let row: Vec<u8> = frame[..16].to_vec();
        assert!(slice.windows(16).any(|w| w == row.as_slice()));
    }

    #[test]
    fn emulation_prevention() {
        let mut out = Vec::new();
        push_nal(&mut out, NAL_PPS, &[0, 0, 1, 0, 0, 0, 5]);
        assert_eq!(out, [0, 0, 0, 1, 68, 1, 0, 0, 3, 1, 0, 0, 3, 0, 5]);
    }

    #[test]
    fn rejects_bad_sizes() {
        let bad = PcmConfig { width: 15, height: 16, level_idc: 93, parameter_interval: 1 };
        assert!(PcmEncoder::new(bad).is_err());
        let ok = PcmConfig { width: 64, height: 64, level_idc: 93, parameter_interval: 1 };
        assert!(PcmEncoder::new(ok).unwrap().encode_i420(&[0; 10]).is_err());
    }

    /// A cropped 40x24 three-picture stream that ffmpeg decodes back to the
    /// exact input samples (checked with `ffmpeg -i pcm40.hevc -f rawvideo
    /// -pix_fmt yuv420p`, 2026-11-20). Pinned so a change to the bitstream is
    /// noticed and re-checked with a real decoder.
    #[test]
    fn cropped_stream_matches_the_decoder_checked_bytes() {
        let (w, h) = (40usize, 24usize);
        let config = PcmConfig { width: w, height: h, level_idc: 93, parameter_interval: 2 };
        let mut encoder = PcmEncoder::new(config).unwrap();
        let mut stream = Vec::new();
        for f in 0..3usize {
            let frame: Vec<u8> = (0..w * h * 3 / 2).map(|i| ((i * 37 + f * 11) % 256) as u8).collect();
            stream.extend(encoder.encode_i420(&frame).unwrap());
        }
        assert_eq!(stream.len(), 7117);
        let fnv = stream.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| {
            (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3)
        });
        assert_eq!(fnv, 0xc8b0_4846_b33e_4271);
    }
}
