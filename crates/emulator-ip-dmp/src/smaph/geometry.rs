//! VG segment geometry selected through path registers 2 through 5.
//! Software raster law: flatten Beziers, affine transform, winding fill.
use fujimod_emulator_devices::Cx;
use std::collections::BTreeMap;
pub type Point = [f64; 2];
pub type Contours = Vec<Vec<Point>>;
pub fn load(cx: &Cx<'_>, r: &BTreeMap<u16, u32>) -> Result<Contours, String> {
    let get = |i| {
        r.get(&i)
            .copied()
            .ok_or_else(|| format!("DMP path missing {i:#x}"))
    };
    let n = (get(3)? & 0x0fffffff) as usize;
    let desc = get(5)?;
    let count = (desc & 0x0fffffff) as usize;
    let dtype = desc >> 30;
    if n == 0 && count == 0 {
        return Ok(Vec::new());
    }
    if n == 0 || n > 65536 || count > 262144 || get(3)? & 0x10000000 == 0 || desc & 0x10000000 == 0
    {
        return Err("DMP external path extent/control invalid".into());
    }
    let commands = cx.ram_bytes(get(2)? as u64, n)?;
    let data = cx.ram_bytes(get(4)? as u64, count * [1, 2, 4, 4][dtype as usize])?;
    let scale = f32::from_bits(get(0x5e)?) as f64;
    let bias = f32::from_bits(get(0x5f)?) as f64;
    let values: Vec<f64> = data
        .chunks_exact([1, 2, 4, 4][dtype as usize])
        .map(|b| {
            (match dtype {
                0 => b[0] as i8 as f64,
                1 => i16::from_le_bytes(b.try_into().unwrap()) as f64,
                2 => i32::from_le_bytes(b.try_into().unwrap()) as f64,
                _ => f32::from_le_bytes(b.try_into().unwrap()) as f64,
            }) * scale
                + bias
        })
        .collect();
    decode(&commands, &values)
}
pub fn decode(commands: &[u8], values: &[f64]) -> Result<Contours, String> {
    if values.iter().any(|v| !v.is_finite() || v.abs() > 1e7) {
        return Err("DMP nonfinite/oversized path coordinate".into());
    }
    let mut contours = Vec::new();
    let mut current = Vec::new();
    let mut p = [0., 0.];
    let mut start = p;
    let mut control = p;
    let mut pos = 0;
    for &cmd in commands {
        let code = cmd & !1;
        let n = match code {
            0 => 0,
            2 | 4 | 0xe => 2,
            6 | 8 => 1,
            0xa | 0x10 => 4,
            0xc => 6,
            0x12..=0x18 => 5,
            _ => return Err(format!("DMP unknown VG segment {cmd:#x}")),
        };
        let v = values
            .get(pos..pos + n)
            .ok_or("DMP truncated VG coordinates")?;
        pos += n;
        let point = |a: f64, b: f64| {
            if cmd & 1 != 0 {
                [p[0] + a, p[1] + b]
            } else {
                [a, b]
            }
        };
        // A non-MOVE segment after CLOSE begins another subpath at the
        // current (closed contour's initial) point. Preserve that first edge;
        // relative MOVE still uses the same current point, without a reset.
        if current.is_empty() && !matches!(code, 0 | 2) {
            start = p;
            current.push(p);
        }
        match code {
            0 => {
                if !current.is_empty() {
                    current.push(start);
                    contours.push(std::mem::take(&mut current));
                    p = start;
                }
            }
            2 => {
                if !current.is_empty() {
                    contours.push(std::mem::take(&mut current));
                }
                p = point(v[0], v[1]);
                start = p;
                current.push(p);
            }
            4 => {
                p = point(v[0], v[1]);
                current.push(p);
            }
            6 => {
                p[0] = if cmd & 1 != 0 { p[0] + v[0] } else { v[0] };
                current.push(p);
            }
            8 => {
                p[1] = if cmd & 1 != 0 { p[1] + v[0] } else { v[0] };
                current.push(p);
            }
            0xa | 0xe => {
                let c = if code == 0xe {
                    [2. * p[0] - control[0], 2. * p[1] - control[1]]
                } else {
                    point(v[0], v[1])
                };
                let end = if code == 0xe {
                    point(v[0], v[1])
                } else {
                    point(v[2], v[3])
                };
                for i in 1..=16 {
                    let t = i as f64 / 16.;
                    let u = 1. - t;
                    current.push([
                        u * u * p[0] + 2. * u * t * c[0] + t * t * end[0],
                        u * u * p[1] + 2. * u * t * c[1] + t * t * end[1],
                    ]);
                }
                p = end;
                control = c;
            }
            0xc | 0x10 => {
                let a = if code == 0x10 {
                    [2. * p[0] - control[0], 2. * p[1] - control[1]]
                } else {
                    point(v[0], v[1])
                };
                let b = if code == 0x10 {
                    point(v[0], v[1])
                } else {
                    point(v[2], v[3])
                };
                let end = if code == 0x10 {
                    point(v[2], v[3])
                } else {
                    point(v[4], v[5])
                };
                for i in 1..=24 {
                    let t = i as f64 / 24.;
                    let u = 1. - t;
                    current.push([
                        u * u * u * p[0]
                            + 3. * u * u * t * a[0]
                            + 3. * u * t * t * b[0]
                            + t * t * t * end[0],
                        u * u * u * p[1]
                            + 3. * u * u * t * a[1]
                            + 3. * u * t * t * b[1]
                            + t * t * t * end[1],
                    ]);
                }
                p = end;
                control = b;
            }
            // Endpoint-to-center SVG/VG ellipse construction.
            0x12..=0x18 => {
                let end = point(v[3], v[4]);
                arc(
                    &mut current,
                    p,
                    end,
                    [v[0].abs(), v[1].abs()],
                    v[2],
                    code >= 0x16,
                    matches!(code, 0x14 | 0x18),
                );
                p = end;
            }
            _ => unreachable!(),
        }
        // OpenVG reflects the last control point across curve degrees too;
        // non-curves set that reference point to their endpoint.
        if !matches!(code, 0xa | 0xc | 0xe | 0x10) {
            control = p;
        }
        if current.len() + contours.iter().map(Vec::len).sum::<usize>() > 131072 {
            return Err("DMP flattened path exceeds edge bound".into());
        }
    }
    if pos != values.len() {
        return Err("DMP unused VG coordinates".into());
    }
    if !current.is_empty() {
        contours.push(current);
    }
    Ok(contours)
}
fn arc(
    out: &mut Vec<Point>,
    p: Point,
    q: Point,
    radii: Point,
    angle: f64,
    large: bool,
    clockwise: bool,
) {
    let [mut rx, mut ry] = radii;
    if rx == 0. || ry == 0. || p == q {
        out.push(q);
        return;
    }
    let phi = angle.to_radians();
    let (s, c) = phi.sin_cos();
    let dx = (p[0] - q[0]) / 2.;
    let dy = (p[1] - q[1]) / 2.;
    let x = c * dx + s * dy;
    let y = -s * dx + c * dy;
    let k = (x * x / (rx * rx) + y * y / (ry * ry)).sqrt().max(1.);
    rx *= k;
    ry *= k;
    let sign = if large == clockwise { -1. } else { 1. };
    let factor = sign
        * ((rx * rx * ry * ry - rx * rx * y * y - ry * ry * x * x)
            / (rx * rx * y * y + ry * ry * x * x))
            .max(0.)
            .sqrt();
    let cx = factor * rx * y / ry;
    let cy = -factor * ry * x / rx;
    let center = [
        c * cx - s * cy + (p[0] + q[0]) / 2.,
        s * cx + c * cy + (p[1] + q[1]) / 2.,
    ];
    let start = ((y - cy) / ry).atan2((x - cx) / rx);
    let end = ((-y - cy) / ry).atan2((-x - cx) / rx);
    let mut delta = end - start;
    if clockwise && delta < 0. {
        delta += std::f64::consts::TAU;
    } else if !clockwise && delta > 0. {
        delta -= std::f64::consts::TAU;
    }
    let steps = (delta.abs() * rx.max(ry).sqrt()).ceil().clamp(8., 256.) as usize;
    for i in 1..=steps {
        let a = start + delta * i as f64 / steps as f64;
        let (sn, co) = a.sin_cos();
        out.push([
            center[0] + c * rx * co - s * ry * sn,
            center[1] + s * rx * co + c * ry * sn,
        ]);
    }
}
