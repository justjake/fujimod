//! Bounded Baseline AVC decoding. This module owns no guest memory or clocks.
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Sequence {
    pub width: usize,
    pub height: usize,
    pub coded_width: usize,
    pub coded_height: usize,
}

pub struct Picture {
    pub sequence: Sequence,
    /// Tightly packed I420 planes: Y, then Cb, then Cr.
    pub i420: Vec<u8>,
}

struct Bits {
    bytes: Vec<u8>,
    at: usize,
}
impl Bits {
    fn new(ebsp: &[u8]) -> Result<Self, String> {
        let mut bytes = Vec::with_capacity(ebsp.len());
        let mut zeros = 0;
        for (index, &byte) in ebsp.iter().enumerate() {
            if zeros == 2 && byte < 3 {
                return Err("AVC missing emulation-prevention byte".into());
            }
            if zeros == 2 && byte == 3 {
                if ebsp.get(index + 1).is_none_or(|&next| next > 3) {
                    return Err("AVC invalid emulation-prevention byte".into());
                }
                zeros = 0;
                continue;
            }
            bytes.push(byte);
            zeros = if byte == 0 { zeros + 1 } else { 0 };
        }
        Ok(Self { bytes, at: 0 })
    }
    fn bit(&mut self) -> Result<u32, String> {
        let byte = self.bytes.get(self.at / 8).ok_or("AVC truncated syntax")?;
        let value = (byte >> (7 - self.at % 8)) & 1;
        self.at += 1;
        Ok(value.into())
    }
    fn bits(&mut self, count: usize) -> Result<u32, String> {
        let mut value = 0;
        for _ in 0..count {
            value = (value << 1) | self.bit()?;
        }
        Ok(value)
    }
    fn ue(&mut self) -> Result<u32, String> {
        let mut zeros = 0;
        while self.bit()? == 0 {
            zeros += 1;
            if zeros > 24 {
                return Err("AVC syntax integer exceeds supported bound".into());
            }
        }
        Ok(((1u32 << zeros) - 1) + self.bits(zeros)?)
    }
}

/// Bound dimensions before invoking the host codec, including cropped pictures.
pub fn sequence(nal: &[u8]) -> Result<Sequence, String> {
    if nal.first().map(|b| b & 31) != Some(7) || nal.len() > 256 {
        return Err("AVC requires a bounded SPS".into());
    }
    let mut bits = Bits::new(&nal[1..])?;
    if bits.bits(8)? != 66 {
        return Err("AVC decoder supports Baseline profile".into());
    }
    bits.bits(8)?; // Constraint flags.
    if bits.bits(8)? > 42 || bits.ue()? > 31 || bits.ue()? > 12 {
        return Err("AVC SPS level or identifiers exceed supported bounds".into());
    }
    match bits.ue()? {
        0 => {
            if bits.ue()? > 12 {
                return Err("AVC picture-order size exceeds supported bound".into());
            }
        }
        2 => {}
        _ => return Err("AVC picture-order mode is unsupported".into()),
    }
    if bits.ue()? > 16 || bits.bit()? != 0 {
        return Err("AVC reference configuration is unsupported".into());
    }
    let coded_width = (bits.ue()? as usize + 1) * 16;
    let coded_height = (bits.ue()? as usize + 1) * 16;
    if coded_width > 1920 || coded_height > 1088 || bits.bit()? != 1 {
        return Err("AVC decoder requires bounded progressive pictures".into());
    }
    bits.bit()?; // direct_8x8_inference_flag.
    let [left, right, top, bottom] = if bits.bit()? == 1 {
        [bits.ue()?, bits.ue()?, bits.ue()?, bits.ue()?].map(|v| v as usize * 2)
    } else {
        [0; 4]
    };
    if left != 0 || top != 0 {
        return Err("AVC top/left cropping is unsupported".into());
    }
    let width = coded_width
        .checked_sub(right)
        .ok_or("AVC invalid horizontal crop")?;
    let height = coded_height
        .checked_sub(bottom)
        .ok_or("AVC invalid vertical crop")?;
    if width < 16 || height < 16 {
        return Err("AVC cropped picture is too small".into());
    }
    Ok(Sequence {
        width,
        height,
        coded_width,
        coded_height,
    })
}

fn prefix(bytes: &[u8]) -> usize {
    if bytes.starts_with(&[0, 0, 0, 1]) {
        4
    } else if bytes.starts_with(&[0, 0, 1]) {
        3
    } else {
        0
    }
}

/// Locate one single-slice access unit in an addressed Annex B input span.
/// Padding zeros before the next start code belong to the consumed span.
pub fn access_unit(bytes: &[u8]) -> Result<(usize, bool), String> {
    if bytes.len() > 2 * 1024 * 1024 {
        return Err("AVC input exceeds two bounded stream banks".into());
    }
    let mut at = 0;
    let mut found = false;
    let mut idr = false;
    while at < bytes.len() {
        while at < bytes.len() && prefix(&bytes[at..]) == 0 && bytes[at] == 0 {
            at += 1;
        }
        let count = prefix(&bytes[at..]);
        if count == 0 {
            return Err("AVC input has no Annex B start code".into());
        }
        if found {
            return Ok((at, idr));
        }
        let start = at + count;
        let header = *bytes.get(start).ok_or("AVC truncated NAL header")?;
        if header & 0x80 != 0 {
            return Err("AVC forbidden NAL header bit".into());
        }
        let kind = header & 31;
        let mut end = start + 1;
        // H.264 Annex B, B.1.1: a NAL ends at either 000001 or 000000.
        // The latter starts trailing_zero_8bits, not more coded payload.
        while end < bytes.len()
            && prefix(&bytes[end..]) == 0
            && !bytes[end..].starts_with(&[0, 0, 0])
        {
            end += 1;
        }
        let mut used = end;
        while used > start && bytes[used - 1] == 0 {
            used -= 1;
        }
        match kind {
            1 | 5 => {
                let mut bits = Bits::new(&bytes[start + 1..used])?;
                if bits.ue()? != 0 || ![0, 2, 5, 7].contains(&bits.ue()?) || bits.ue()? > 255 {
                    return Err("AVC requires one I/P slice per picture".into());
                }
                found = true;
                idr = kind == 5;
            }
            7 => {
                sequence(&bytes[start..used])?;
            }
            8 => {
                if used - start > 256 {
                    return Err("AVC PPS exceeds supported bound".into());
                }
                let mut bits = Bits::new(&bytes[start + 1..used])?;
                if bits.ue()? > 255 || bits.ue()? > 31 || bits.bit()? != 0 {
                    return Err("AVC PPS identifiers or entropy mode are unsupported".into());
                }
                bits.bit()?;
                if bits.ue()? != 0 {
                    return Err("AVC slice groups are unsupported".into());
                }
            }
            6 | 9 => {}
            _ => return Err(format!("AVC NAL type {kind} is unsupported")),
        }
        if end == bytes.len() {
            return if found {
                Ok((used, idr))
            } else {
                Err("AVC input has no picture".into())
            };
        }
        at = end;
        while at < bytes.len() && prefix(&bytes[at..]) == 0 && bytes[at] == 0 {
            at += 1;
        }
        if at == bytes.len() || prefix(&bytes[at..]) == 0 {
            // A finite bank can end in zero transport padding followed by
            // unrelated memory. Only a following start code establishes a
            // successor NAL and lets that padding enter the consumed span.
            return if found {
                Ok((used, idr))
            } else {
                Err("AVC input has no picture before trailing padding".into())
            };
        }
    }
    Err("AVC input has no picture".into())
}

/// Retain parameter sets separately from picture history at an IDR boundary.
pub fn parameter_sets(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let count = prefix(&bytes[at..]);
        if count == 0 {
            at += 1;
            continue;
        }
        let start = at + count;
        let mut end = start + 1;
        while end < bytes.len() && prefix(&bytes[end..]) == 0 {
            end += 1;
        }
        let mut used = end;
        while used > start && bytes[used - 1] == 0 {
            used -= 1;
        }
        if matches!(bytes.get(start).map(|b| b & 31), Some(7 | 8)) {
            out.extend_from_slice(&[0, 0, 0, 1]);
            out.extend_from_slice(&bytes[start..used]);
        }
        at = end;
    }
    if out.len() > 128 {
        return Err("AVC parameter sets exceed supported bound".into());
    }
    Ok(out)
}

pub fn packet_sequence(bytes: &[u8]) -> Result<Option<Sequence>, String> {
    let parameters = parameter_sets(bytes)?;
    if parameters.is_empty() {
        return Ok(None);
    }
    if parameters.get(4).map(|b| b & 31) != Some(7) {
        return Err("AVC parameter update requires an SPS".into());
    }
    let mut end = 5;
    while end < parameters.len() && prefix(&parameters[end..]) == 0 {
        end += 1;
    }
    sequence(&parameters[4..end]).map(Some)
}

pub struct Decoder {
    sequence: Option<Sequence>,
    #[cfg(not(target_arch = "wasm32"))]
    codec: openh264::decoder::Decoder,
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use crate::h264::{Backend, Codec, Config};

    fn config() -> Config {
        Config {
            width: 64,
            height: 32,
            frames_per_second: 30,
            bitrate: 1_000_000,
            max_qp: 20,
            gop: 4,
        }
    }
    fn input(bright: bool) -> Vec<u8> {
        let mut pixels = vec![128; 64 * 32 * 3 / 2];
        for row in 0..32 {
            for col in 0..64 {
                pixels[row * 64 + col] = if (col < 32) ^ bright { 40 } else { 200 };
            }
        }
        pixels[64 * 32..64 * 32 * 5 / 4].fill(96);
        pixels[64 * 32 * 5 / 4..].fill(160);
        pixels
    }

    #[test]
    fn actual_i_and_p_picture_planes_survive_stride_normalization_and_replay() {
        let mut encoder = Codec::new(Backend::Software, config(), &[]).unwrap();
        let mut decoder = Decoder::new(&[]).unwrap();
        let mut history = Vec::<Arc<[u8]>>::new();
        for frame in 0..4 {
            let packet = encoder.encode(config(), &input(frame % 2 == 1)).unwrap();
            let (used, idr) = access_unit(&packet).unwrap();
            assert_eq!(used, packet.len());
            assert_eq!(idr, frame == 0);
            let picture = decoder.decode(&packet).unwrap();
            assert_eq!((picture.sequence.width, picture.sequence.height), (64, 32));
            assert_eq!(picture.i420.len(), 3072);
            assert!((picture.i420[0] as i16 - if frame % 2 == 0 { 40 } else { 200 }).abs() < 12);
            assert!((picture.i420[64 * 32] as i16 - 96).abs() < 12);
            assert!((picture.i420[64 * 32 * 5 / 4] as i16 - 160).abs() < 12);
            let mut replay = Decoder::new(&history).unwrap();
            assert_eq!(replay.decode(&packet).unwrap().i420, picture.i420);
            history.push(packet.into());
        }
    }

    #[test]
    fn bounds_and_malformed_nals_refuse_before_host_decode() {
        for packet in [
            vec![],
            vec![0, 0, 0, 1, 0xe7],
            vec![0, 0, 0, 1, 6],
            vec![0; 2 * 1024 * 1024 + 1],
        ] {
            assert!(access_unit(&packet).is_err());
        }
        assert!(
            sequence(&[0x67, 100, 0, 42])
                .unwrap_err()
                .contains("Baseline")
        );
        let mut encoder = Codec::new(Backend::Software, config(), &[]).unwrap();
        let packet = encoder.encode(config(), &input(false)).unwrap();
        let mut joined = packet.clone();
        joined.extend_from_slice(&packet);
        assert_eq!(access_unit(&joined).unwrap().0, packet.len());
        assert!(
            Decoder::new(&[])
                .unwrap()
                .decode(&joined)
                .err()
                .unwrap()
                .contains("multiple pictures")
        );
        let headers = parameter_sets(&packet).unwrap();
        assert!(!headers.is_empty());
        assert!(headers.len() <= 128);
        assert!(access_unit(&headers).is_err());
    }

    #[test]
    fn annex_b_zero_delimiters_exclude_unrelated_bank_tail() {
        let mut encoder = Codec::new(Backend::Software, config(), &[]).unwrap();
        let mut packet = encoder.encode(config(), &input(false)).unwrap();
        let pps = (5..packet.len())
            .find(|&at| prefix(&packet[at..]) != 0)
            .unwrap();
        // Non-VCL NALs can also carry transport padding before their successor.
        packet.splice(pps..pps, [0; 18]);
        let expected = Decoder::new(&[]).unwrap().decode(&packet).unwrap().i420;
        for padding in [3, 19, 128] {
            let mut bank = packet.clone();
            bank.resize(bank.len() + padding, 0);
            // Unrelated nonzero memory must not extend the coded slice.
            bank.extend_from_slice(&[0, 0, 128, 128].repeat(256));
            let (used, idr) = access_unit(&bank).unwrap();
            assert_eq!(used, packet.len());
            assert!(idr);
            assert_eq!(
                Decoder::new(&[])
                    .unwrap()
                    .decode(&bank[..used])
                    .unwrap()
                    .i420,
                expected
            );
            bank.truncate(packet.len() + padding);
            bank.extend_from_slice(&packet);
            assert_eq!(access_unit(&bank).unwrap().0, packet.len() + padding);
        }
        let vcl = (0..packet.len())
            .find(|&at| {
                let count = prefix(&packet[at..]);
                count != 0 && packet[at + count] & 31 == 5
            })
            .unwrap();
        let mut truncated = packet[..vcl + (packet.len() - vcl) / 2].to_vec();
        truncated.extend_from_slice(&[0; 19]);
        truncated.extend_from_slice(&[128; 128]);
        let used = access_unit(&truncated).unwrap().0;
        assert!(
            Decoder::new(&[])
                .unwrap()
                .decode(&truncated[..used])
                .is_err()
        );
    }

    #[test]
    fn emulation_prevention_keeps_escaped_zero_delimiters_and_refuses_missing_escape() {
        for last in [0, 1, 2, 3] {
            let bits = Bits::new(&[0x80, 0, 0, 3, last, 0x80]).unwrap();
            assert_eq!(bits.bytes, [0x80, 0, 0, last, 0x80]);
        }
        for last in [0, 1, 2] {
            assert!(Bits::new(&[0x80, 0, 0, last, 0x80]).is_err());
        }
        let malformed = [0, 0, 0, 1, 0x65, 0xb8, 0, 0, 2, 0x80];
        assert!(
            access_unit(&malformed)
                .unwrap_err()
                .contains("emulation-prevention")
        );
        for last in [0, 1, 2] {
            let valid = [0, 0, 0, 1, 0x65, 0xb8, 0, 0, 3, last, 0x80];
            assert_eq!(access_unit(&valid).unwrap(), (valid.len(), true));
        }
    }

    #[test]
    fn progressive_macroblock_height_crops_1088_to_1080() {
        fn ue(value: u32) -> String {
            let bits = format!("{:b}", value + 1);
            "0".repeat(bits.len() - 1) + &bits
        }
        let mut bits = format!("{:08b}{:08b}{:08b}", 66, 0x40, 42);
        // SPS id, frame-num width, POC mode2, one reference, no gaps.
        bits += &(ue(0) + &ue(0) + &ue(2) + &ue(1) + "0");
        bits += &(ue(119) + &ue(67) + "111" + &ue(0) + &ue(0) + &ue(0) + &ue(4) + "01");
        while bits.len() % 8 != 0 {
            bits.push('0');
        }
        let mut nal = vec![0x67];
        nal.extend(
            bits.as_bytes()
                .chunks(8)
                .map(|chunk| u8::from_str_radix(std::str::from_utf8(chunk).unwrap(), 2).unwrap()),
        );
        assert_eq!(
            sequence(&nal).unwrap(),
            Sequence {
                width: 1920,
                height: 1080,
                coded_width: 1920,
                coded_height: 1088
            }
        );
    }
}
impl Decoder {
    pub fn new(history: &[Arc<[u8]>]) -> Result<Self, String> {
        #[cfg(target_arch = "wasm32")]
        {
            let _ = history;
            Err("AVC decoding is unavailable on this target".into())
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let mut decoder = Self {
                sequence: None,
                codec: openh264::decoder::Decoder::new()
                    .map_err(|e| format!("AVC decoder initialization: {e}"))?,
            };
            if history.len() > 120 {
                return Err("AVC replay exceeds bounded GOP history".into());
            }
            for packet in history {
                decoder.decode(packet)?;
            }
            Ok(decoder)
        }
    }
    pub fn decode(&mut self, bytes: &[u8]) -> Result<Picture, String> {
        let (used, _) = access_unit(bytes)?;
        if bytes[used..].iter().any(|&b| b != 0) {
            return Err("AVC access unit contains multiple pictures".into());
        }
        let mut at = 0;
        while at < used {
            let count = prefix(&bytes[at..]);
            if count == 0 {
                at += 1;
                continue;
            }
            let start = at + count;
            let mut end = start + 1;
            while end < used && prefix(&bytes[end..]) == 0 {
                end += 1;
            }
            if bytes[start] & 31 == 7 {
                let mut payload_end = end;
                while payload_end > start && bytes[payload_end - 1] == 0 {
                    payload_end -= 1;
                }
                let next = sequence(&bytes[start..payload_end])?;
                if self.sequence.is_some_and(|old| old != next) {
                    return Err("AVC dimensions changed without reset".into());
                }
                self.sequence = Some(next);
            }
            at = end;
        }
        let sequence = self.sequence.ok_or("AVC picture has no bounded SPS")?;
        #[cfg(target_arch = "wasm32")]
        {
            let _ = sequence;
            Err("AVC decoding is unavailable on this target".into())
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            use openh264::formats::YUVSource;
            let image = self
                .codec
                .decode(&bytes[..used])
                .map_err(|e| format!("AVC decode failed: {e}"))?
                .ok_or("AVC access unit produced no picture")?;
            if image.dimensions() != (sequence.width, sequence.height) {
                return Err("AVC decoded dimensions differ from bounded SPS".into());
            }
            let (ys, us, vs) = image.strides();
            let luma = sequence.width * sequence.height;
            let mut i420 = vec![0; luma * 3 / 2];
            for row in 0..sequence.height {
                i420[row * sequence.width..(row + 1) * sequence.width]
                    .copy_from_slice(&image.y()[row * ys..row * ys + sequence.width]);
            }
            let chroma_width = sequence.width / 2;
            for row in 0..sequence.height / 2 {
                let start = row * chroma_width;
                i420[luma + start..luma + start + chroma_width]
                    .copy_from_slice(&image.u()[row * us..row * us + chroma_width]);
                i420[luma + luma / 4 + start..luma + luma / 4 + start + chroma_width]
                    .copy_from_slice(&image.v()[row * vs..row * vs + chroma_width]);
            }
            Ok(Picture { sequence, i420 })
        }
    }
}
