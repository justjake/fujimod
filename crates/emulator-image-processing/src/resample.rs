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

pub fn centered(
    source: &[u8],
    sw: usize,
    sh: usize,
    pitch: usize,
    dw: usize,
    dh: usize,
    limits: [usize; 2],
    scale: [u32; 2],
    mode: u32,
    out: &mut [u8],
) -> Result<(), String> {
    let plan = Centered::new(
        source.len(),
        sw,
        sh,
        pitch,
        dw,
        dh,
        limits,
        scale,
        mode,
        out.len(),
    )?;
    plan.render_rows(source, 0..dh, out);
    Ok(())
}

/// One axis of the programmed centered transform. The source limit and
/// destination center are doubled coordinates; scale is Q12 before decimation.
#[derive(Clone, Copy)]
struct Axis {
    bias: usize,
    step: usize,
    shift: u32,
}
impl Axis {
    fn new(count: usize, limit: usize, scale: u32, shift: u32) -> Result<Self, String> {
        if count == 0 || shift > 2 {
            return Err("invalid centered axis geometry".into());
        }
        let decimation = 1usize << shift;
        let scale = (scale as usize)
            .checked_mul(decimation)
            .ok_or("centered axis scale overflow")?;
        let span = (count - 1)
            .checked_mul(scale)
            .ok_or("centered axis span overflow")?;
        let center = limit
            .checked_mul(4096)
            .ok_or("centered axis limit overflow")?;
        let bias = center
            .checked_sub(span)
            .ok_or("centered axis overruns source limit")?;
        center
            .checked_add(span)
            .ok_or("centered axis coordinate overflow")?;
        Ok(Self {
            bias,
            step: scale.checked_mul(2).ok_or("centered axis step overflow")?,
            shift,
        })
    }
    fn at(self, index: usize) -> usize {
        (self.bias + index * self.step) >> (13 + self.shift) << self.shift
    }
}

/// Validated centered Q12 geometry. The pure row kernel cannot inspect registers,
/// live memory or a stream; callers may run it on disjoint worker-owned rows.
#[derive(Clone, Copy)]
pub struct Centered {
    pitch: usize,
    dw: usize,
    axes: [Axis; 2],
}
impl Centered {
    /// Length of the register-addressed source window needed by this centered
    /// operation. Source width describes the logical view; row pitch describes
    /// where each row begins, so the selected view may extend into later rows.
    pub fn source_bytes(
        sw: usize,
        sh: usize,
        pitch: usize,
        dw: usize,
        dh: usize,
        limits: [usize; 2],
        scale: [u32; 2],
        mode: u32,
    ) -> Result<usize, String> {
        if mode > 2
            || sw == 0
            || sh == 0
            || pitch == 0
            || dw == 0
            || dh == 0
            || limits[0] >= sw
            || limits[1] >= sh
        {
            return Err("invalid packed422 centered geometry".into());
        }
        let axes = [
            Axis::new(dw, limits[0], scale[0], mode)?,
            Axis::new(dh, limits[1], scale[1], mode)?,
        ];
        let x = axes[0].at(dw - 1);
        let y = axes[1].at(dh - 1);
        let sample_end = y
            .checked_mul(pitch)
            .and_then(|row| {
                (x / 2)
                    .checked_mul(4)
                    .and_then(|pair| row.checked_add(pair))
            })
            .and_then(|pair| pair.checked_add(4))
            .ok_or("packed422 centered source size overflow")?;
        let raster_bytes = pitch
            .checked_mul(sh)
            .ok_or("packed422 centered source size overflow")?;
        Ok(raster_bytes.max(sample_end))
    }

    pub fn new(
        source_bytes: usize,
        sw: usize,
        sh: usize,
        pitch: usize,
        dw: usize,
        dh: usize,
        limits: [usize; 2],
        scale: [u32; 2],
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
            || limits[0] >= sw
            || limits[1] >= sh
            || source_bytes < Self::source_bytes(sw, sh, pitch, dw, dh, limits, scale, mode)?
            || Some(output_bytes) != dw.checked_mul(dh).and_then(|pixels| pixels.checked_mul(2))
        {
            return Err("invalid packed422 centered geometry".into());
        }
        Ok(Self {
            pitch,
            dw,
            axes: [
                Axis::new(dw, limits[0], scale[0], mode)?,
                Axis::new(dh, limits[1], scale[1], mode)?,
            ],
        })
    }
    pub fn render_rows(&self, source: &[u8], rows: std::ops::Range<usize>, out: &mut [u8]) {
        let Self {
            pitch,
            dw,
            axes: [sx, sy],
        } = *self;
        for (local_y, y) in rows.enumerate() {
            let sy = sy.at(y);
            for x in (0..dw).step_by(2) {
                let at = (local_y * dw + x) * 2;
                out[at..at + 4].copy_from_slice(&[
                    sample(source, pitch, sx.at(x), sy, 0),
                    sample(source, pitch, sx.at(x + 1), sy, 0),
                    sample(source, pitch, sx.at(x), sy, 1),
                    sample(source, pitch, sx.at(x), sy, 2),
                ]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn grid() -> Vec<u8> {
        let mut source = Vec::new();
        for y in 0..8 {
            for x in (0..8).step_by(2) {
                source.extend_from_slice(&[y * 16 + x, y * 16 + x + 1, 100 + x, 200 + y]);
            }
        }
        source
    }

    #[test]
    fn centered_unity_scale_crops_instead_of_stretching_the_source() {
        let source = grid();
        let mut out = vec![0; 4 * 8 * 2];
        centered(&source, 8, 8, 16, 4, 8, [7, 7], [4096; 2], 0, &mut out).unwrap();
        assert_eq!(&out[..8], &[2, 3, 102, 200, 4, 5, 104, 200]);
        assert_eq!(&out[56..], &[114, 115, 102, 207, 116, 117, 104, 207]);
        let mut vertical = vec![0; 8 * 2 * 2];
        centered(&source, 8, 8, 16, 8, 2, [7, 7], [4096; 2], 0, &mut vertical).unwrap();
        assert_eq!(&vertical[..4], &[48, 49, 100, 203]);
        assert_eq!(&vertical[16..20], &[64, 65, 100, 204]);
    }

    #[test]
    fn centered_fractional_scale_and_decimation_use_the_programmed_grid() {
        let source = grid();
        let mut out = vec![0; 4 * 4 * 2];
        centered(&source, 8, 8, 16, 4, 4, [7, 7], [6144; 2], 0, &mut out).unwrap();
        assert_eq!(&out[..8], &[17, 18, 100, 201, 20, 21, 104, 201]);
        let mut decimated = vec![0; 8];
        centered(
            &source,
            8,
            8,
            16,
            2,
            2,
            [6, 6],
            [4096; 2],
            1,
            &mut decimated,
        )
        .unwrap();
        assert_eq!(decimated, [34, 36, 102, 202, 66, 68, 102, 204]);
        assert!(Centered::new(source.len(), 8, 8, 16, 8, 8, [7, 7], [4097; 2], 0, 128).is_err());
    }

    #[test]
    fn centered_disjoint_rows_match_the_complete_kernel() {
        let source = grid();
        let plan = Centered::new(source.len(), 8, 8, 16, 4, 4, [7, 7], [6144; 2], 0, 32).unwrap();
        let mut complete = vec![0; 32];
        let mut rows = vec![0; 32];
        plan.render_rows(&source, 0..4, &mut complete);
        plan.render_rows(&source, 0..2, &mut rows[..16]);
        plan.render_rows(&source, 2..4, &mut rows[16..]);
        assert_eq!(rows, complete);
    }

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
