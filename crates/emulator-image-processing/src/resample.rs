//! Packed Y0 Y1 Cb Cr sampling over preallocated output slices.
fn sample(source: &[u8], pitch: usize, x: usize, y: usize, channel: usize) -> u8 {
    source[y * pitch + x / 2 * 4 + if channel == 0 { x % 2 } else { channel + 1 }]
}
fn round_even(n: u64, shift: u32) -> u8 {
    let a = n >> shift;
    let r = n & ((1 << shift) - 1);
    let h = 1 << (shift - 1);
    (a + u64::from(r > h || r == h && a & 1 != 0)).min(255) as u8
}

pub fn bilinear(
    source: &[u8],
    sw: usize,
    sh: usize,
    pitch: usize,
    dw: usize,
    dh: usize,
    step: [u32; 2],
    out: &mut [u8],
) -> Result<(), String> {
    if sw == 0
        || !sw.is_multiple_of(2)
        || sh == 0
        || dw == 0
        || !dw.is_multiple_of(2)
        || dh == 0
        || pitch < sw * 2
        || source.len() != pitch * sh
        || out.len() != dw * dh * 2
    {
        return Err("invalid packed422 bilinear geometry".into());
    }
    for y in 0..dh {
        for x in (0..dw).step_by(2) {
            let at = (y * dw + x) * 2;
            for lane in 0..4 {
                let c = if lane < 2 { 0 } else { lane - 1 };
                let column = if c == 0 { x + lane } else { x / 2 };
                let w = if c == 0 { sw } else { sw / 2 };
                let fx = (column as u64 * step[0] as u64).min(((w - 1) as u64) << 16);
                let fy = (y as u64 * step[1] as u64).min(((sh - 1) as u64) << 16);
                let (x0, y0) = ((fx >> 16) as usize, (fy >> 16) as usize);
                let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(sh - 1));
                let (dx, dy) = (fx & 65535, fy & 65535);
                let get = |x, y| sample(source, pitch, if c == 0 { x } else { x * 2 }, y, c) as u64;
                let a = get(x0, y0) * (65536 - dx) + get(x1, y0) * dx;
                let b = get(x0, y1) * (65536 - dx) + get(x1, y1) * dx;
                out[at + lane] = round_even(a * (65536 - dy) + b * dy, 32);
            }
        }
    }
    Ok(())
}

pub fn endpoint(
    source: &[u8],
    sw: usize,
    sh: usize,
    pitch: usize,
    dw: usize,
    dh: usize,
    last: [usize; 2],
    mode: u32,
    out: &mut [u8],
) -> Result<(), String> {
    let plan = Endpoint::new(source.len(), sw, sh, pitch, dw, dh, last, mode, out.len())?;
    plan.render_rows(source, 0..dh, out);
    Ok(())
}

/// Validated endpoint geometry. The pure row kernel cannot inspect registers,
/// live memory or a stream; callers may run it on disjoint worker-owned rows.
#[derive(Clone, Copy)]
pub struct Endpoint {
    pitch: usize,
    dw: usize,
    dh: usize,
    last: [usize; 2],
    mode: u32,
}
impl Endpoint {
    pub fn new(
        source_bytes: usize,
        sw: usize,
        sh: usize,
        pitch: usize,
        dw: usize,
        dh: usize,
        last: [usize; 2],
        mode: u32,
        output_bytes: usize,
    ) -> Result<Self, String> {
        if mode > 2
            || sw == 0
            || !sw.is_multiple_of(2)
            || sh == 0
            || dw == 0
            || !dw.is_multiple_of(2)
            || dh == 0
            || last[0] >= sw
            || last[1] >= sh
            || pitch < sw * 2
            || source_bytes != pitch * sh
            || output_bytes != dw * dh * 2
        {
            return Err("invalid packed422 endpoint geometry".into());
        }
        Ok(Self {
            pitch,
            dw,
            dh,
            last,
            mode,
        })
    }
    pub fn render_rows(&self, source: &[u8], rows: std::ops::Range<usize>, out: &mut [u8]) {
        let Self {
            pitch,
            dw,
            dh,
            last,
            mode,
        } = *self;
        let step = 1usize << mode;
        let sx = |x: usize| x * (last[0] / step) / (dw - 1).max(1) * step;
        for (local_y, y) in rows.enumerate() {
            let sy = y * (last[1] / step) / (dh - 1).max(1) * step;
            for x in (0..dw).step_by(2) {
                let at = (local_y * dw + x) * 2;
                out[at..at + 4].copy_from_slice(&[
                    sample(source, pitch, sx(x), sy, 0),
                    sample(source, pitch, sx(x + 1), sy, 0),
                    sample(source, pitch, sx(x), sy, 1),
                    sample(source, pitch, sx(x), sy, 2),
                ]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identity_and_half_sample_ties() {
        let s = [0, 10, 20, 30, 20, 30, 40, 50];
        let mut out = [0; 8];
        bilinear(&s, 4, 1, 8, 4, 1, [65536, 65536], &mut out).unwrap();
        assert_eq!(out, s);
        bilinear(&s, 4, 1, 8, 4, 1, [32768, 65536], &mut out).unwrap();
        assert_eq!(out, [0, 5, 20, 30, 10, 15, 30, 40]);
    }
}
