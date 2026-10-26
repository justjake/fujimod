//! Baseline 4:2:2 JPEG using integer Chen row/column arithmetic
//! and T.81 Annex K Huffman tables.
//! Quantizer rounding is measured for q=1..3; other uploaded nonzero byte
//! quantizers use the same declared arithmetic. No system image library.
mod tables;
use tables::*;

/// Entropy output reached the destination capacity. Owners decide how the
/// hardware reports it; it is not a refusal of the request.
pub const OUTPUT_OVERFLOW: &str = "JPEG output exceeds capacity";

fn round(v: i32, s: u32) -> i32 {
    (v + (1 << (s - 1))) >> s
}
fn odd(a: [i32; 4], s: u32) -> [i32; 4] {
    let [a4, a5, a6, a7] = a;
    [
        round(a4 * 6393 + a7 * 32138, s),
        round(a6 * 27246 - a5 * 18205, s),
        round(a5 * 27246 + a6 * 18205, s),
        round(a7 * 6393 - a4 * 32138, s),
    ]
}
fn pass(x: [i32; 8], column: bool) -> [i32; 8] {
    let e: [i32; 4] = std::array::from_fn(|i| {
        if column {
            round(x[i] + x[7 - i], 1)
        } else {
            x[i] + x[7 - i]
        }
    });
    let d: [i32; 4] = std::array::from_fn(|i| {
        if column {
            round(x[i] - x[7 - i], 1)
        } else {
            x[i] - x[7 - i]
        }
    });
    let mut o = [0; 8];

    let (halves, shift) = if column {
        let t0 = round(e[0] + e[3], 1);
        let t1 = round(e[1] + e[2], 1);
        o[0] = round(t0 + t1, 4);
        o[4] = round(t0 - t1, 4);
        let p = round(e[0] - e[3], 1);
        let m = round(e[1] - e[2], 1);
        o[2] = round(p * 21407 + m * 8867, 18);
        o[6] = round(p * 8867 - m * 21407, 18);
        let u1 = round(d[1] - d[2], 2);
        let u2 = round(d[1] + d[2], 2);
        let a = round(d[3] * 23170, 16);
        let b = round(d[0] * 23170, 16);
        ([a + u1, a - u1, b - u2, b + u2].map(|v| round(v, 1)), 17)
    } else {
        o[0] = (e[0] + e[1] + e[2] + e[3]) << 3;
        o[4] = (e[0] + e[3] - e[1] - e[2]) << 3;
        let p = e[0] - e[3];
        let m = e[1] - e[2];
        o[2] = round(p * 21407 + m * 8867, 11);
        o[6] = round(p * 8867 - m * 21407, 11);
        let a = round(d[3] * 23170, 8);
        let b = round(d[0] * 23170, 8);
        let t1 = (d[1] - d[2]) << 6;
        let t2 = (d[1] + d[2]) << 6;
        ([a + t1, a - t1, b - t2, b + t2], 18)
    };
    [o[1], o[3], o[5], o[7]] = odd(halves, shift);
    o
}
pub fn fdct(block: [i32; 64]) -> [i32; 64] {
    let mut rows = [0; 64];
    for y in 0..8 {
        rows[y * 8..y * 8 + 8]
            .copy_from_slice(&pass(block[y * 8..y * 8 + 8].try_into().unwrap(), false));
    }
    let mut out = [0; 64];
    for x in 0..8 {
        let c = pass(std::array::from_fn(|y| rows[y * 8 + x]), true);
        for y in 0..8 {
            out[y * 8 + x] = c[y];
        }
    }
    out
}
#[derive(Clone)]
struct Huffman {
    codes: [(u16, u8); 256],
}
impl Huffman {
    fn new(bits: &[u8; 16], values: &[u8]) -> Self {
        let mut codes = [(0, 0); 256];
        let (mut code, mut k) = (0, 0);
        for (i, &n) in bits.iter().enumerate() {
            for _ in 0..n {
                codes[values[k] as usize] = (code, (i + 1) as u8);
                code += 1;
                k += 1;
            }
            code <<= 1;
        }
        Self { codes }
    }
}
struct Writer<'a> {
    out: &'a mut [u8],
    at: usize,
    bits: u64,
    count: u8,
}
impl Writer<'_> {
    fn byte(&mut self, b: u8) -> Result<(), String> {
        let dst = self.out.get_mut(self.at).ok_or(OUTPUT_OVERFLOW)?;
        *dst = b;
        self.at += 1;
        Ok(())
    }
    fn bytes(&mut self, b: &[u8]) -> Result<(), String> {
        for &x in b {
            self.byte(x)?;
        }
        Ok(())
    }
    fn marker(&mut self, m: u8) -> Result<(), String> {
        self.bytes(&[255, m])
    }
    fn word(&mut self, w: u16) -> Result<(), String> {
        self.bytes(&w.to_be_bytes())
    }
    fn put(&mut self, v: u32, n: u8) -> Result<(), String> {
        self.bits = (self.bits << n) | v as u64;
        self.count += n;
        while self.count >= 8 {
            self.count -= 8;
            let b = (self.bits >> self.count) as u8;
            self.byte(b)?;
            if b == 255 {
                self.byte(0)?;
            }
        }
        self.bits &= (1u64 << self.count) - 1;
        Ok(())
    }
    fn flush(&mut self) -> Result<(), String> {
        if self.count != 0 {
            let n = 8 - self.count;
            self.put((1 << n) - 1, n)?;
        }
        Ok(())
    }
    fn symbol(&mut self, h: &Huffman, s: usize, v: i32) -> Result<(), String> {
        let (c, n) = h.codes[s];
        if n == 0 {
            return Err("JPEG coefficient exceeds baseline Huffman domain".into());
        }
        self.put(c as u32, n)?;
        let size = (s & 15) as u8;
        if size != 0 {
            let amp = if v < 0 { v + (1i32 << size) - 1 } else { v };
            self.put((amp as u32) & ((1 << size) - 1), size)?;
        }
        Ok(())
    }
}
fn size(v: i32) -> usize {
    (32 - v.unsigned_abs().leading_zeros()) as usize
}

#[derive(Clone)]
pub struct Codec {
    dc: [Huffman; 2],
    ac: [Huffman; 2],
    quant: [[i32; 64]; 2],
    uploaded: [u8; 128],
}
impl Default for Codec {
    fn default() -> Self {
        Self {
            dc: [
                Huffman::new(&DC_LUMA_BITS, &DC_LUMA_VALUES),
                Huffman::new(&DC_CHROMA_BITS, &DC_CHROMA_VALUES),
            ],
            ac: [
                Huffman::new(&AC_LUMA_BITS, &AC_LUMA_VALUES),
                Huffman::new(&AC_CHROMA_BITS, &AC_CHROMA_VALUES),
            ],
            quant: [[0; 64]; 2],
            uploaded: [0; 128],
        }
    }
}
impl Codec {
    pub fn tables(&mut self, fifo: &[u8]) -> Result<(), String> {
        if fifo.len() != 128 || fifo.contains(&0) {
            return Err("JPEG requires two complete nonzero quantization tables".into());
        }
        if fifo != self.uploaded {
            self.uploaded.copy_from_slice(fifo);
            for t in 0..2 {
                for i in 0..64 {
                    self.quant[t][ZIGZAG[i]] = fifo[t * 64 + i] as i32;
                }
            }
        }
        Ok(())
    }
    fn headers(
        &self,
        w: &mut Writer<'_>,
        width: usize,
        height: usize,
        restart: u16,
    ) -> Result<(), String> {
        w.marker(0xc4)?;
        w.word(418)?;
        for (id, bits, values) in [
            (0, &DC_LUMA_BITS, DC_LUMA_VALUES.as_slice()),
            (0x10, &AC_LUMA_BITS, AC_LUMA_VALUES.as_slice()),
            (1, &DC_CHROMA_BITS, DC_CHROMA_VALUES.as_slice()),
            (0x11, &AC_CHROMA_BITS, AC_CHROMA_VALUES.as_slice()),
        ] {
            w.byte(id)?;
            w.bytes(bits)?;
            w.bytes(values)?;
        }
        w.marker(0xdb)?;
        w.word(132)?;
        w.byte(0)?;
        w.bytes(&self.uploaded[..64])?;
        w.byte(1)?;
        w.bytes(&self.uploaded[64..])?;
        w.marker(0xdd)?;
        w.word(4)?;
        w.word(restart)?;
        w.marker(0xc0)?;
        w.word(17)?;
        w.byte(8)?;
        w.word(height as u16)?;
        w.word(width as u16)?;
        w.bytes(&[3, 1, 0x21, 0, 2, 0x11, 1, 3, 0x11, 1])?;
        w.marker(0xda)?;
        w.word(12)?;
        w.bytes(&[3, 1, 0, 2, 0x11, 3, 0x11, 0, 63, 0])?;
        Ok(())
    }
    fn block(
        &self,
        w: &mut Writer<'_>,
        pixels: [i32; 64],
        component: usize,
        predictor: &mut i32,
    ) -> Result<(), String> {
        let table = usize::from(component != 0);
        let f = fdct(pixels);
        let q: [i32; 64] = std::array::from_fn(|i| {
            (f[i] + self.quant[table][i] / 2).div_euclid(self.quant[table][i])
        });
        let diff = q[0] - *predictor;
        if size(diff) > 11 {
            return Err("JPEG DC exceeds baseline magnitude".into());
        }
        *predictor = q[0];
        w.symbol(&self.dc[table], size(diff), diff)?;
        let mut zeros = 0;
        for &i in &ZIGZAG[1..] {
            let v = q[i];
            if v == 0 {
                zeros += 1;
            } else {
                while zeros >= 16 {
                    w.symbol(&self.ac[table], 0xf0, 0)?;
                    zeros -= 16;
                }
                let n = size(v);
                if n > 10 {
                    return Err("JPEG AC exceeds baseline magnitude".into());
                }
                w.symbol(&self.ac[table], zeros * 16 + n, v)?;
                zeros = 0;
            }
        }
        if zeros != 0 {
            w.symbol(&self.ac[table], 0, 0)?;
        }
        Ok(())
    }
    /// Validate geometry, tables and the minimum header allocation without
    /// reading image bytes or running the transform/entropy kernel.
    pub fn validate(
        &self,
        source_bytes: usize,
        width: usize,
        height: usize,
        restart: u16,
        capacity: usize,
    ) -> Result<(), String> {
        if width < 16
            || !width.is_multiple_of(16)
            || height < 8
            || !height.is_multiple_of(8)
            || width > 65535
            || height > 65535
            || width * height > 32_000_000
            || source_bytes != width * height * 2
            || self.uploaded.contains(&0)
            || capacity == 0
            || capacity > 64 * 1024 * 1024
        {
            return Err("JPEG input escapes bounded packed422 class".into());
        }
        let mut header = [0; 1024];
        self.headers(
            &mut Writer {
                out: &mut header[..capacity.min(1024)],
                at: 0,
                bits: 0,
                count: 0,
            },
            width,
            height,
            restart,
        )
    }
    /// DHT..EOI, excluding SOI/reserved bytes. Pure serial kernel; the owning
    /// device chooses when and where to execute it.
    pub fn encode(
        &self,
        source: &[u8],
        width: usize,
        height: usize,
        restart: u16,
        output: &mut Vec<u8>,
        capacity: usize,
    ) -> Result<(), String> {
        self.validate(source.len(), width, height, restart, capacity)?;
        output.resize(capacity, 0);
        let mut w = Writer {
            out: output,
            at: 0,
            bits: 0,
            count: 0,
        };
        self.headers(&mut w, width, height, restart)?;
        let mut prev = [0; 3];
        let mcus = width / 16 * (height / 8);
        for m in 0..mcus {
            if restart != 0 && m != 0 && m % restart as usize == 0 {
                w.flush()?;
                w.marker(0xd0 + ((m / restart as usize - 1) % 8) as u8)?;
                prev = [0; 3];
            }
            let mx = m % (width / 16) * 16;
            let my = m / (width / 16) * 8;
            for block in 0..4 {
                let c = if block < 2 { 0 } else { block - 1 };
                let pixels = std::array::from_fn(|i| {
                    let y = my + i / 8;
                    let x = mx
                        + if block < 2 {
                            block * 8 + i % 8
                        } else {
                            i % 8 * 2
                        };
                    let at = (y * width + x) / 2 * 4 + if c == 0 { x % 2 } else { c + 1 };
                    source[at] as i32 - 128
                });
                self.block(&mut w, pixels, c, &mut prev[c])?;
            }
        }
        w.flush()?;
        w.marker(0xd9)?;
        let at = w.at;
        output.truncate(at);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn constant_dct_is_dc_only() {
        for v in [-128, -1, 0, 1, 127] {
            let out = fdct([v; 64]);
            assert_eq!(out[0], v * 8);
            assert!(out[1..].iter().all(|&v| v == 0));
        }
    }
    #[test]
    fn bounded_valid_stream_with_restarts() {
        let mut c = Codec::default();
        c.tables(&[3; 128]).unwrap();
        let mut out = Vec::new();
        c.encode(&vec![128; 80 * 8 * 2], 80, 8, 4, &mut out, 4096)
            .unwrap();
        assert_eq!(&out[..2], &[255, 0xc4]);
        assert_eq!(&out[out.len() - 2..], &[255, 0xd9]);
        assert!(out.windows(2).any(|b| b == [255, 0xd0]));
        let mut encoded = vec![255, 0xd8];
        encoded.extend_from_slice(&out);
        let opts = zune_core::options::DecoderOptions::new_safe().set_use_unsafe(false);
        let mut d = zune_jpeg::JpegDecoder::new_with_options(
            zune_core::bytestream::ZCursor::new(&encoded),
            opts,
        );
        let rgb = d.decode().unwrap();
        assert_eq!(rgb.len(), 80 * 8 * 3);
        assert!(rgb.iter().all(|&v| (127..=129).contains(&v)));
        assert!(c.encode(&vec![0; 256], 16, 8, 0, &mut out, 16).is_err());
        assert!(c.tables(&[0; 128]).is_err());
    }
}
