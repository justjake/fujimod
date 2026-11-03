//! FFF63000 channel9 JPEG decode, reset/ACK and IRQ76. The decoder is
//! zune-jpeg with SIMD features disabled and unsafe code disabled in options.
//! RGB box downsampling and full-range packed422 conversion are declared;
//! Playback output follows these register-selected decoding steps.
use fujimod_emulator_devices::exr2::irq_routes::Irq;
use fujimod_emulator_devices::{Cx, Device};
use fujimod_emulator_imaging::exr2_isp::{
    dma::{Raster, Transfer, physical},
    due, word,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use zune_core::{bytestream::ZCursor, colorspace::ColorSpace, options::DecoderOptions};
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub completion_ticks: u64,
    pub reset_ticks: u64,
}
#[derive(Clone, Copy, Serialize, Deserialize)]
enum Pending {
    Reset(u64),
    Clear(u64),
    Decode(u32),
    Input(u64),
}
pub struct Decoder {
    r: Vec<u32>,
    options: Options,
    irq: Irq,
    source: Arc<[u8]>,
    dma: Transfer,
    pending: Option<Pending>,
    waiting_input: bool,
}

// DMA streams may reserve sixteen zero bytes between SOI and the first table.
// Keep the addressed input intact for refill/count/checkpoint accounting; only
// the strict software codec's view omits this transport reservation.
fn codec_stream(source: &Arc<[u8]>) -> Arc<[u8]> {
    let end = source.len().min(18);
    if source.len() > 2
        && source.starts_with(&[255, 216])
        && source[2..end].iter().all(|&byte| byte == 0)
    {
        let mut bytes = Vec::with_capacity(source.len().saturating_sub(16));
        bytes.extend_from_slice(&source[..2]);
        if source.len() > 18 {
            bytes.extend_from_slice(&source[18..]);
        }
        bytes.into()
    } else {
        source.clone()
    }
}

// Marker lengths exclude embedded marker-like data. Within entropy FF00
// is data, and restart markers do not end the scan. Split markers wait.
fn complete_jpeg(bytes: &[u8]) -> Result<bool, String> {
    if !bytes.starts_with(&[255, 216]) {
        return Err("JPEG input has no SOI".into());
    }
    let mut position = 2;
    let mut scan = false;
    while position < bytes.len() {
        if scan {
            while position < bytes.len() && bytes[position] != 255 {
                position += 1;
            }
            if position == bytes.len() {
                return Ok(false);
            }
        } else if bytes[position] != 255 {
            return Err("JPEG marker prefix is invalid".into());
        }
        while position < bytes.len() && bytes[position] == 255 {
            position += 1;
        }
        if position == bytes.len() {
            return Ok(false);
        }
        let marker = bytes[position];
        position += 1;
        if scan && (marker == 0 || (208..=215).contains(&marker)) {
            continue;
        }
        if marker == 217 {
            return Ok(true);
        }
        scan = false;
        if marker == 0 || marker == 216 {
            return Err("JPEG marker is invalid".into());
        }
        if marker == 1 || (208..=215).contains(&marker) {
            continue;
        }
        if position + 2 > bytes.len() {
            return Ok(false);
        }
        let size = u16::from_be_bytes([bytes[position], bytes[position + 1]]) as usize;
        if size < 2 {
            return Err("JPEG marker length is invalid".into());
        }
        if size > bytes.len() - position {
            return Ok(false);
        }
        position += size;
        scan = marker == 218;
    }
    Ok(false)
}

#[inline]
fn rgb_pixel(bytes: &[u8], width: usize, factor: usize, x: usize, y: usize) -> [i32; 3] {
    if factor == 1 {
        let at = (y * width + x) * 3;
        return std::array::from_fn(|c| i32::from(bytes[at + c]));
    }
    let mut sums = [0i32; 3];
    for yy in y * factor..(y + 1) * factor {
        for xx in x * factor..(x + 1) * factor {
            let p = &bytes[(yy * width + xx) * 3..][..3];
            for c in 0..3 {
                sums[c] += i32::from(p[c]);
            }
        }
    }
    let n = (factor * factor) as i32;
    sums.map(|v| (v + n / 2) / n)
}

#[inline]
fn rgb_to_ycc([r, g, b]: [i32; 3]) -> [u8; 3] {
    // RGB and rounded box means are 0..=255. The largest intermediate is
    // 16,744,448, so signed 32-bit arithmetic preserves the integer law.
    [
        ((19595 * r + 38470 * g + 7471 * b + 32768) >> 16).clamp(0, 255) as u8,
        (((-11059 * r - 21709 * g + 32768 * b + 32768) >> 16) + 128).clamp(0, 255) as u8,
        (((32768 * r - 27439 * g - 5329 * b + 32768) >> 16) + 128).clamp(0, 255) as u8,
    ]
}
impl Decoder {
    pub fn new(options: Options, irq: Irq) -> Result<Self, String> {
        if options.completion_ticks == 0 || options.reset_ticks == 0 {
            return Err("JPEG decoder timing must be positive".into());
        }
        Ok(Self {
            r: vec![0; 0x12c / 4],
            options,
            irq,
            source: Arc::from([]),
            dma: Transfer::default(),
            pending: None,
            waiting_input: false,
        })
    }
    fn valid(o: usize) -> bool {
        matches!(
            o,
            0 | 4 | 8 | 0x10 | 0x14 | 0x18 | 0x1c | 0xa0 | 0x100..=0x128
        )
    }
    fn start(&mut self, cx: &Cx<'_>, refill: bool) -> Result<(), String> {
        let r = &self.r;
        let (src, dst) = (
            (r[0x10c / 4] & 0x3fffffff) as u64,
            (r[0x100 / 4] & 0x3fffffff) as u64,
        );
        let (count, capacity) = (r[0x110 / 4] as usize, r[0x104 / 4] as usize);
        if r[2] != 0xfd0020
            || r[0xa0 / 4] > 3
            || [0x11c, 0x120, 0x124, 0x128].iter().any(|&o| r[o / 4] != 0)
        {
            return Err("JPEG decode register configuration is unsupported".into());
        }
        physical(src, count)?;
        physical(dst, capacity)?;
        if refill != self.waiting_input {
            return Err("JPEG input refill requires a preceding input request".into());
        }
        let prior = if refill { self.source.len() } else { 0 };
        if count == 0
            || prior
                .checked_add(count)
                .is_none_or(|n| n > 64 * 1024 * 1024)
        {
            return Err("JPEG accumulated input exceeds the byte bound".into());
        }
        let mut input = Vec::with_capacity(prior + count);
        if refill {
            input.extend_from_slice(&self.source);
        }
        input.resize(prior + count, 0);
        cx.ram_read(src, &mut input[prior..])?;
        self.source = input.into();
        if !refill && !self.source.starts_with(&[255, 0xd8]) {
            if count > 4
                && self.source[2] == 255
                && matches!(self.source[3], 0xc4 | 0xdb | 0xe0..=0xef)
            {
                Arc::get_mut(&mut self.source).unwrap()[..2].copy_from_slice(&[255, 0xd8]);
            } else {
                return Err("JPEG decode input has no supported stream header".into());
            }
        }
        let stream = codec_stream(&self.source);
        if !complete_jpeg(&stream)? {
            self.waiting_input = true;
            self.pending = Some(Pending::Input(due(
                cx.now(),
                self.options.completion_ticks,
            )?));
            return Ok(());
        }
        self.waiting_input = false;
        // SOF dimensions are U16. Bound decoded work by the pixel budget and
        // programmed output allocation below, including downsampled images.
        let options = DecoderOptions::new_safe()
            .set_use_unsafe(false)
            .set_strict_mode(true)
            .set_max_width(u16::MAX as usize)
            .set_max_height(u16::MAX as usize)
            .jpeg_set_out_colorspace(ColorSpace::RGB);
        let mut decoder =
            zune_jpeg::JpegDecoder::new_with_options(ZCursor::new(stream.as_ref()), options);
        decoder
            .decode_headers()
            .map_err(|e| format!("invalid JPEG header: {e:?}"))?;
        let info = decoder.info().ok_or("JPEG has no frame")?;
        let (sw, sh) = (info.width as usize, info.height as usize);
        let shift = r[0xa0 / 4] as usize;
        let (w, h) = (sw >> shift, sh >> shift);
        if sw * sh > 32_000_000 || w < 2 || w % 2 != 0 || h == 0 {
            return Err("JPEG decode dimensions are unsupported".into());
        }
        let pitch = (w + 15) & !15;
        let rows = (h + 7) & !7;
        if pitch * rows * 2 > capacity
            || src < dst + (pitch * rows * 2) as u64 && dst < src + count as u64
        {
            return Err("JPEG output exceeds allocation or overlaps input".into());
        }
        let size = decoder
            .output_buffer_size()
            .ok_or("JPEG output size unknown")?;
        self.dma.prepare_deferred(
            cx.now(),
            self.options.completion_ticks,
            Raster {
                address: dst,
                width: pitch,
                height: rows,
                pitch: pitch * 2,
            },
            pitch * 2,
        )?;
        let source = stream;
        self.dma.pixels.defer_fallible(move |mut pixels| {
            pixels.resize(pitch * rows * 2, 0);
            let result = (|| {
                let options = DecoderOptions::new_safe()
                    .set_use_unsafe(false)
                    .set_strict_mode(true)
                    .set_max_width(u16::MAX as usize)
                    .set_max_height(u16::MAX as usize)
                    .jpeg_set_out_colorspace(ColorSpace::RGB);
                let mut decoder = zune_jpeg::JpegDecoder::new_with_options(
                    ZCursor::new(source.as_ref()),
                    options,
                );
                let mut rgb = vec![0; size];
                decoder
                    .decode_into(&mut rgb)
                    .map_err(|e| format!("invalid JPEG image: {e:?}"))?;
                let factor = 1usize << shift;
                for y in 0..rows {
                    if y >= h {
                        continue;
                    }
                    for x in (0..w).step_by(2) {
                        let a = rgb_to_ycc(rgb_pixel(&rgb, sw, factor, x, y));
                        let b = rgb_to_ycc(rgb_pixel(&rgb, sw, factor, x + 1, y));
                        let at = (y * pitch + x) * 2;
                        pixels[at..at + 4].copy_from_slice(&[
                            a[0],
                            b[0],
                            (a[1] as u16 + b[1] as u16).div_ceil(2) as u8,
                            (a[2] as u16 + b[2] as u16).div_ceil(2) as u8,
                        ]);
                    }
                }
                Ok(())
            })();
            (pixels, result)
        });
        if fujimod_emulator_devices::work::worker_count() == 0
            && let Err(error) = self.dma.check_result()
        {
            self.dma.cancel();
            return Err(error);
        }
        self.pending = Some(Pending::Decode(self.source.len() as u32));
        Ok(())
    }
    fn store(&mut self, cx: &Cx<'_>, o: usize, v: u32) -> Result<(), String> {
        if [4, 0x10, 0x14].contains(&o) {
            self.r[o / 4] &= !v;
            return Ok(());
        }
        if self.pending.is_some() {
            return Err("JPEG decoder write while busy".into());
        }
        match (o, v) {
            (0x118, 1) => {
                self.waiting_input = false;
                self.source = Arc::from([]);
                self.pending = Some(Pending::Reset(due(cx.now(), self.options.reset_ticks)?))
            }
            (0, 0x100) => {
                self.pending = Some(Pending::Clear(due(cx.now(), self.options.reset_ticks)?))
            }
            (0, 1) => self.start(cx, false)?,
            (0x118, 4) => {
                self.start(cx, true)?;
                self.r[0x118 / 4] = 0;
                return Ok(());
            }
            (8 | 0xa0 | 0x100 | 0x104 | 0x10c | 0x110 | 0x11c | 0x120 | 0x124 | 0x128, _) => {}
            _ => return Err("unsupported JPEG decoder register write".into()),
        }
        self.r[o / 4] = v;
        Ok(())
    }
}
impl Device for Decoder {
    fujimod_emulator_devices::checkpoint_fields!(
        r: Vec<u32>,
        dma: Transfer,
        pending: Option<Pending>,
        waiting_input: bool,
        ; blobs: [source]
    );
    fn fingerprint(&self) -> std::result::Result<serde_json::Value, String> {
        Ok(self.save())
    }

    fn name(&self) -> &'static str {
        "exr2-jpeg-decoder"
    }
    fn read(&mut self, cx: &mut Cx<'_>, a: u64, s: u8) -> u64 {
        let Some(o) = word(cx, a, s, 0xfff63000, 0x12c) else {
            return 0;
        };
        if !Self::valid(o) {
            cx.unmodeled();
            return 0;
        }
        self.r[o / 4] as u64
    }
    fn write(&mut self, cx: &mut Cx<'_>, a: u64, s: u8, v: u64) {
        let Some(o) = word(cx, a, s, 0xfff63000, 0x12c) else {
            return;
        };
        if !Self::valid(o) || v > u32::MAX as u64 {
            cx.unmodeled();
            return;
        }
        if let Err(e) = self.store(cx, o, v as u32) {
            cx.fail(e);
        }
    }
    fn next_deadline(&self, _: u64) -> Option<u64> {
        match self.pending {
            Some(Pending::Reset(t) | Pending::Clear(t) | Pending::Input(t)) => Some(t),
            Some(Pending::Decode(_)) => self.dma.deadline(),
            None => None,
        }
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        if self.next_deadline(cx.now()).is_none_or(|t| cx.now() < t) {
            return;
        }
        match self.pending.unwrap() {
            Pending::Reset(_) => {
                self.r[1] |= 0x10000;
                self.r[0x118 / 4] = 0;
            }
            Pending::Clear(_) => self.r[0] = 0,
            Pending::Input(_) => {
                // Request the next programmed input pair. No output DMA or
                // decode-complete event exists before the final input.
                self.r[1] |= 0x1000000;
                Transfer::interrupt(cx, self.irq);
            }
            Pending::Decode(count) => {
                match self.dma.write(cx) {
                    Ok(true) => {}
                    Ok(false) => return,
                    Err(e) => {
                        cx.fail(e);
                        return;
                    }
                }
                self.r[0x108 / 4] = count;
                self.r[1] |= 0x10;
                Transfer::interrupt(cx, self.irq);
            }
        }
        self.pending = None;
    }
    fn event(&mut self, cx: &mut Cx<'_>, _: u64) {
        self.advance(cx)
    }
    fn save(&self) -> serde_json::Value {
        serde_json::json!((&self.r, &self.dma, &self.pending, self.waiting_input))
    }
    fn diagnostic_state(&self) -> serde_json::Value {
        serde_json::json!((
            &self.r,
            self.dma.fingerprint(),
            &self.pending,
            self.waiting_input
        ))
    }
    fn diagnostic_buffer(&self, name: &str) -> Result<&[u8], String> {
        if name == "encoded-input" {
            return Ok(&self.source);
        }
        if name != "dma-output" {
            return Err(format!("JPEG decoder has no diagnostic buffer {name}"));
        }
        self.dma.pixels.check()?;
        Ok(&self.dma.pixels)
    }
    fn restore(&mut self, v: &serde_json::Value) -> Result<(), String> {
        let (r, d, p, waiting_input): (Vec<u32>, Transfer, Option<Pending>, bool) =
            serde_json::from_value(v.clone()).map_err(|e| e.to_string())?;
        if r.len() != self.r.len() {
            return Err("JPEG decoder bank size".into());
        }
        self.r = r;
        self.dma = d;
        self.pending = p;
        self.waiting_input = waiting_input;
        Ok(())
    }
}

#[cfg(test)]
mod pixel_tests {
    use super::*;

    #[test]
    fn reserved_prefix_is_exact_and_other_header_junk_still_refuses() {
        let mut bytes = vec![255, 216];
        bytes.extend_from_slice(&[0; 16]);
        bytes.extend_from_slice(&[255, 217]);
        let source: Arc<[u8]> = bytes.clone().into();
        assert_eq!(&*codec_stream(&source), &[255, 216, 255, 217]);
        assert_eq!(&*source, &bytes);
        bytes[8] = 1;
        let source: Arc<[u8]> = bytes.into();
        assert!(complete_jpeg(&codec_stream(&source)).is_err());
    }

    #[test]
    fn marker_completion_ignores_app_payload_and_stuffed_entropy() {
        let prefix = [
            255, 216, 255, 224, 0, 4, 255, 217, 255, 218, 0, 2, 7, 255, 0, 217, 255, 208,
        ];
        assert!(!complete_jpeg(&prefix).unwrap());
        let mut complete = prefix.to_vec();
        complete.extend_from_slice(&[255, 217]);
        for length in 2..complete.len() {
            assert!(!complete_jpeg(&complete[..length]).unwrap());
        }
        assert!(complete_jpeg(&complete).unwrap());
        assert!(complete_jpeg(&[255, 216, 255, 224, 0, 1]).is_err());
    }

    #[test]
    fn box_scaling_and_full_size_fast_path_preserve_i64_reference_colors() {
        let (width, height) = (64, 32);
        let bytes: Vec<_> = (0..width * height * 3)
            .map(|i| ((i * 73 + i / 17) % 256) as u8)
            .collect();
        for factor in [1, 2, 4, 8] {
            for y in 0..height / factor {
                for x in 0..width / factor {
                    let expected: [i64; 3] = std::array::from_fn(|c| {
                        let sum: i64 = (y * factor..(y + 1) * factor)
                            .flat_map(|yy| (x * factor..(x + 1) * factor).map(move |xx| (yy, xx)))
                            .map(|(yy, xx)| i64::from(bytes[(yy * width + xx) * 3 + c]))
                            .sum();
                        let n = (factor * factor) as i64;
                        (sum + n / 2) / n
                    });
                    assert_eq!(
                        rgb_pixel(&bytes, width, factor, x, y).map(i64::from),
                        expected
                    );
                }
            }
        }
    }

    #[test]
    fn narrowed_color_conversion_preserves_i64_rounding_and_clipping() {
        let reference = |[r, g, b]: [i64; 3]| {
            [
                ((19595 * r + 38470 * g + 7471 * b + 32768) >> 16).clamp(0, 255) as u8,
                (((-11059 * r - 21709 * g + 32768 * b + 32768) >> 16) + 128).clamp(0, 255) as u8,
                (((32768 * r - 27439 * g - 5329 * b + 32768) >> 16) + 128).clamp(0, 255) as u8,
            ]
        };
        for r in [0, 1, 127, 128, 254, 255] {
            for g in [0, 1, 127, 128, 254, 255] {
                for b in [0, 1, 127, 128, 254, 255] {
                    assert_eq!(rgb_to_ycc([r, g, b]), reference([r, g, b].map(i64::from)));
                }
            }
        }
        let mut state = 7u64;
        for _ in 0..65_536 {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            let rgb = [state as u8, (state >> 16) as u8, (state >> 32) as u8].map(i32::from);
            assert_eq!(rgb_to_ycc(rgb), reference(rgb.map(i64::from)));
        }
    }
}

#[cfg(test)]
mod checkpoint_tests {
    use super::*;
    #[test]
    fn pixel_and_source_bytes_restore_without_integer_array_expansion() {
        let mut device = Decoder::new(
            Options {
                completion_ticks: 5,
                reset_ticks: 2,
            },
            Irq { core: 0, line: 76 },
        )
        .unwrap();
        device.source = vec![0xf9; 4096].into();
        device
            .dma
            .prepare(
                0,
                5,
                Raster {
                    address: 0x4000,
                    width: 1024,
                    height: 512,
                    pitch: 2048,
                },
                2048,
            )
            .unwrap();
        for (i, byte) in device.dma.pixels.iter_mut().enumerate() {
            *byte = i as u8;
        }
        device.pending = Some(Pending::Decode(41));
        let identity = device.fingerprint().unwrap();
        assert!(
            serde_json::to_vec(&device.diagnostic_state())
                .unwrap()
                .len()
                < 4096
        );
        assert_eq!(
            device.diagnostic_buffer("dma-output").unwrap(),
            &*device.dma.pixels
        );
        assert!(device.diagnostic_buffer("missing").is_err());
        assert_eq!(device.fingerprint().unwrap(), identity);
        let saved = device.checkpoint().unwrap();
        assert!(saved.bytes() < device.source.len() + device.dma.pixels.len() + 1024);
        Arc::make_mut(&mut device.source).fill(0);
        device.dma.pixels.fill(0);
        device.pending = None;
        device.restore_checkpoint(&saved).unwrap();
        assert_eq!(&*device.source, &vec![0xf9; 4096]);
        assert_eq!(
            &**device.dma.pixels,
            &(0..1024 * 512 * 2).map(|i| i as u8).collect::<Vec<_>>()
        );
        assert!(matches!(device.pending, Some(Pending::Decode(41))));
        assert_eq!(device.fingerprint().unwrap(), identity);
    }
}
