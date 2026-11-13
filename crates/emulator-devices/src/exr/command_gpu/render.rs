//! Software execution of EXR Processor Pro GPU command lists (OpenVG-era 2D GPU).
//!
//! Header forms: 0 writes consecutive registers (register in bits 27:16, count-1 in 7:0), 1 writes one register
//! repeatedly, 2 writes the registers selected by mask bits 15:0, 4 writes GPU-side state memory
//! ((count-1) in 23:20, word address in 17:0; retained, read for gradient paint), 0xf ends a batch and 0xffffffff is
//! the list fence.
//!
//! Registers executed here: 0x200..0x207 destination surface and 0x220..0x227 source image (address, stride at +4,
//! (w-1) | (h-1) << 16 at +5, format at +6); 0x440..0x448 the 3x3 path matrix, applied when 0x400 bit 15 is set;
//! 0x548 the paint color (RGBA, red in bits 31:24); 0x604 = 0x140 or 0x150 selects image paint through the
//! surface-to-image matrix 0x550..0x558, always blended source-over; 0x410 segment ops (1 move, 2 line, 3 horizontal, 4 vertical, 5 quadratic, 6 cubic, 0 close,
//! 0xf draw, 0xe integer rectangle from 0x412/0x413) with coordinates in 0x414..0x419. Surfaces have their origin at
//! the bottom-left. Pixels are 32-bit, bytes B, G, R, A in memory. Fill is non-zero. With 0x400 bit 7 set flat and
//! gradient paint is blended source-over; otherwise it replaces the destination pixels, alpha included.
//! 0x604 with bits 15 and 13 set selects linear-gradient paint: the gradient and its color ramp come from state
//! memory (`STATE_GRADIENT`, `STATE_RAMP`), through the surface-to-paint matrix 0x574..0x57c (decoded from the
//! X-Pro1 3.82 menu's lists; the ramp's 1/(width) words agree with the stop positions; no register manual).
//! Blend modes (0x600), radial gradients, other paint control words, stroking, scissor lists and antialiasing are
//! not modeled.
use crate::Cx;
use serde::{Deserialize, Serialize};

const REGISTERS: usize = 0x1000;
const CURVE_STEPS: usize = 12;
/// 0x400 bit 7: source-over blending. Without it the paint replaces the destination, alpha included, which is how
/// the firmware clears OSD areas (fills with paint 0x00000000).
const BLEND: u32 = 0x80;
/// 0x604 = 0x140 or 0x150: image paint. The images are RGBA with transparent backgrounds (pre-rendered labels and
/// icons); they blend source-over whatever 0x400 bit 7 says, so a label keeps the row background behind its text.
const IMAGE: u32 = 0x140;
const IMAGE_VARIANT: u32 = 0x10;
/// 0x604 bits 15 and 13: linear-gradient paint (flat paint is 0x50040, gradients 0x5a040). Bits 31:26 hold the
/// index of the last color-ramp stop; the flat color 0x548 then carries only a black placeholder.
const GRADIENT: u32 = 0xa000;
/// GPU state memory, written by header form 4: the gradient (start x, y, direction dx, dy, 1/(dx²+dy²)) and the
/// color-ramp stops, 4 words apart: (position << 5 | stop index, RGBA color, 1/(next position - position)), with
/// positions 0..255 along the gradient.
const STATE_GRADIENT: usize = 0x67;
const STATE_RAMP: usize = 0x6e;
const STATE_WORDS: usize = 0x40000;

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Renderer {
    registers: Vec<u32>,
    #[serde(default)]
    state: Vec<u32>,
    subpaths: Vec<Vec<(f32, f32)>>,
    open: bool,
    last: (f32, f32),
    pub draws: u64,
    pub image_draws: u64,
}

fn f(v: u32) -> f32 {
    f32::from_bits(v)
}

/// A linear gradient through the surface-to-paint matrix 0x574..0x57c, with its color ramp (bytes B, G, R, A).
struct Gradient {
    matrix: [f32; 6],
    start: (f32, f32),
    direction: (f32, f32),
    scale: f32,
    stops: Vec<(f32, [f32; 4])>,
}

impl Gradient {
    fn at(&self, x: f32, y: f32) -> [u8; 4] {
        let m = &self.matrix;
        let (px, py) = (m[0] * x + m[1] * y + m[2], m[3] * x + m[4] * y + m[5]);
        let t = ((px - self.start.0) * self.direction.0 + (py - self.start.1) * self.direction.1) * self.scale * 255.0;
        // Pad outside the ramp.
        let k = self.stops.iter().rposition(|(p, _)| *p <= t).unwrap_or(0);
        let (p0, c0) = self.stops[k];
        let color = match self.stops.get(k + 1) {
            Some(&(p1, c1)) if t > p0 && p1 > p0 => {
                let u = ((t - p0) / (p1 - p0)).clamp(0.0, 1.0);
                std::array::from_fn(|c| c0[c] + (c1[c] - c0[c]) * u)
            }
            _ => c0,
        };
        color.map(|v| v.round().clamp(0.0, 255.0) as u8)
    }
}

struct Surface {
    address: u64,
    stride: u64,
    width: i64,
    height: i64,
}

impl Renderer {
    pub fn seeded(&self) -> bool {
        !self.registers.is_empty()
    }

    pub fn seed(&mut self, registers: &[(u32, u32)]) -> Result<(), String> {
        self.registers.resize(REGISTERS, 0);
        for &(r, v) in registers {
            *self.registers.get_mut(r as usize).ok_or("EXR GPU seeded register is outside the register file")? = v;
        }
        Ok(())
    }

    fn reg(&self, r: usize) -> u32 {
        self.registers.get(r).copied().unwrap_or(0)
    }

    fn set(&mut self, cx: &mut Cx<'_>, r: usize, v: u32) -> Result<(), String> {
        if self.registers.len() < REGISTERS {
            self.registers.resize(REGISTERS, 0);
        }
        if r >= REGISTERS {
            return Err(format!("EXR GPU register {r:#x} is outside the register file"));
        }
        self.registers[r] = v;
        if r == 0x410 {
            self.segment(cx, v)?;
        }
        Ok(())
    }

    /// Execute one consumed command list.
    pub fn execute(&mut self, cx: &mut Cx<'_>, words: &[u32]) -> Result<(), String> {
        let mut i = 0;
        while i < words.len() {
            let h = words[i];
            if h == 0xffff_ffff {
                i += 1;
                continue;
            }
            let reg = ((h >> 16) & 0xfff) as usize;
            match h >> 28 {
                0xf => i += 1,
                0 | 1 => {
                    let n = (h & 0xff) as usize + 1;
                    let end = (i + 1 + n).min(words.len());
                    for (k, &v) in words[i + 1..end].iter().enumerate() {
                        self.set(cx, if h >> 28 == 0 { reg + k } else { reg }, v)?;
                    }
                    i += 1 + n;
                }
                2 => {
                    let mask = h & 0xffff;
                    let mut k = i + 1;
                    let mut pending = Vec::new();
                    for b in 0..16 {
                        if mask >> b & 1 != 0 {
                            if let Some(&v) = words.get(k) {
                                pending.push((reg + b, v));
                            }
                            k += 1;
                        }
                    }
                    // Coordinates land before the segment op takes effect.
                    for &(r, v) in pending.iter().filter(|(r, _)| *r != 0x410) {
                        self.set(cx, r, v)?;
                    }
                    for &(r, v) in pending.iter().filter(|(r, _)| *r == 0x410) {
                        self.set(cx, r, v)?;
                    }
                    i = k;
                }
                4 => {
                    let n = ((h >> 20) & 15) as usize + 1;
                    let at = (h & 0x3ffff) as usize;
                    let body = &words[(i + 1).min(words.len())..(i + 1 + n).min(words.len())];
                    if at + body.len() > STATE_WORDS {
                        return Err(format!("EXR GPU state memory write at {at:#x} runs past the state memory"));
                    }
                    if self.state.len() < at + body.len() {
                        self.state.resize(at + body.len(), 0);
                    }
                    self.state[at..at + body.len()].copy_from_slice(body);
                    i += 1 + n
                }
                form => return Err(format!("EXR GPU command header form {form:#x} is not decoded ({h:#010x})")),
            }
        }
        Ok(())
    }

    fn surface(&self, base: usize) -> Surface {
        let size = self.reg(base + 5);
        Surface {
            address: u64::from(self.reg(base) & 0x0fff_ffff),
            stride: u64::from(self.reg(base + 4)),
            width: i64::from(size & 0xffff) + 1,
            height: i64::from(size >> 16) + 1,
        }
    }

    fn word(&self, at: usize) -> u32 {
        self.state.get(at).copied().unwrap_or(0)
    }

    /// The linear-gradient paint selected by 0x604, or None for flat paint.
    fn gradient(&self) -> Option<Gradient> {
        let control = self.reg(0x604);
        if control & GRADIENT != GRADIENT {
            return None;
        }
        let last = (control >> 26) as usize;
        let stops = (0..=last)
            .map(|k| {
                let at = STATE_RAMP + 4 * k;
                let color = self.word(at + 1);
                let rgba = [(color >> 8) as u8, (color >> 16) as u8, (color >> 24) as u8, color as u8];
                ((self.word(at) >> 5) as f32, rgba.map(f32::from))
            })
            .collect();
        let g = |k: usize| f(self.word(STATE_GRADIENT + k));
        let m = |k: usize| f(self.reg(0x574 + k));
        Some(Gradient {
            matrix: [m(0), m(1), m(2), m(3), m(4), m(5)],
            start: (g(0), g(1)),
            direction: (g(2), g(3)),
            scale: g(4),
            stops,
        })
    }

    fn transform(&self, x: f32, y: f32) -> (f32, f32) {
        let m: Vec<f32> = (0..9).map(|k| f(self.reg(0x440 + k))).collect();
        if self.reg(0x400) & 0x8000 == 0 || m.iter().all(|v| *v == 0.0) {
            (x, y)
        } else {
            (m[0] * x + m[1] * y + m[2], m[3] * x + m[4] * y + m[5])
        }
    }

    fn segment(&mut self, cx: &mut Cx<'_>, op: u32) -> Result<(), String> {
        let (x, y) = (f(self.reg(0x418)), f(self.reg(0x419)));
        match op {
            1 => {
                self.subpaths.push(vec![self.transform(x, y)]);
                self.open = true;
            }
            2..=6 if self.open => {
                let p0 = self.last;
                let end = match op {
                    3 => (x, p0.1),
                    4 => (p0.0, y),
                    _ => (x, y),
                };
                if op == 5 || op == 6 {
                    let (c1, c2) = if op == 6 {
                        ((f(self.reg(0x414)), f(self.reg(0x415))), (f(self.reg(0x416)), f(self.reg(0x417))))
                    } else {
                        let q = (f(self.reg(0x416)), f(self.reg(0x417)));
                        (
                            (p0.0 + 2.0 / 3.0 * (q.0 - p0.0), p0.1 + 2.0 / 3.0 * (q.1 - p0.1)),
                            (x + 2.0 / 3.0 * (q.0 - x), y + 2.0 / 3.0 * (q.1 - y)),
                        )
                    };
                    for s in 1..=CURVE_STEPS {
                        let t = s as f32 / CURVE_STEPS as f32;
                        let u = 1.0 - t;
                        let px = u * u * u * p0.0 + 3.0 * u * u * t * c1.0 + 3.0 * u * t * t * c2.0 + t * t * t * x;
                        let py = u * u * u * p0.1 + 3.0 * u * u * t * c1.1 + 3.0 * u * t * t * c2.1 + t * t * t * y;
                        let p = self.transform(px, py);
                        self.subpaths.last_mut().unwrap().push(p);
                    }
                } else {
                    let p = self.transform(end.0, end.1);
                    self.subpaths.last_mut().unwrap().push(p);
                }
                self.last = end;
                return Ok(());
            }
            0 => self.open = false,
            0xf => {
                let result = self.fill(cx);
                self.subpaths.clear();
                self.open = false;
                return result;
            }
            0xe => {
                let (a, b) = (self.reg(0x412), self.reg(0x413));
                let s = self.surface(0x200);
                let (x0, x1) = (i64::from(a & 0xffff).min(i64::from(b & 0xffff)), i64::from(a & 0xffff).max(i64::from(b & 0xffff)));
                let (ya, yb) = (s.height - i64::from(a >> 16), s.height - i64::from(b >> 16));
                let spans: Vec<(i64, i64, i64)> = (ya.min(yb)..ya.max(yb)).map(|row| (row, x0, x1)).collect();
                return self.paint(cx, &spans);
            }
            _ => {}
        }
        if matches!(op, 1..=6) {
            self.last = (x, y);
        }
        Ok(())
    }

    fn fill(&mut self, cx: &mut Cx<'_>) -> Result<(), String> {
        let s = self.surface(0x200);
        let mut edges = Vec::new();
        for sp in &self.subpaths {
            for k in 0..sp.len().saturating_sub(1).max(if sp.len() > 1 { sp.len() } else { 0 }) {
                let a = sp[k % sp.len()];
                let b = sp[(k + 1) % sp.len()];
                if a.1 != b.1 {
                    edges.push((a, b));
                }
            }
        }
        if edges.is_empty() {
            return Ok(());
        }
        // Surface y grows upward from the bottom-left; memory rows grow downward.
        let lo = edges.iter().map(|(a, b)| a.1.min(b.1)).fold(f32::MAX, f32::min).floor().max(0.0) as i64;
        let hi = edges.iter().map(|(a, b)| a.1.max(b.1)).fold(f32::MIN, f32::max).ceil().min(s.height as f32) as i64;
        let mut spans = Vec::new();
        for vy in lo..hi {
            let sy = vy as f32 + 0.5;
            let mut crossings: Vec<(f32, i32)> = edges
                .iter()
                .filter(|(a, b)| (a.1 <= sy && b.1 > sy) || (b.1 <= sy && a.1 > sy))
                .map(|(a, b)| (a.0 + (sy - a.1) * (b.0 - a.0) / (b.1 - a.1), if b.1 > a.1 { 1 } else { -1 }))
                .collect();
            crossings.sort_by(|p, q| p.0.total_cmp(&q.0));
            let mut winding = 0;
            for k in 0..crossings.len().saturating_sub(1) {
                winding += crossings[k].1;
                if winding != 0 {
                    let x0 = (crossings[k].0 - 0.5).ceil().max(0.0) as i64;
                    let x1 = ((crossings[k + 1].0 - 0.5).ceil() as i64).min(s.width);
                    if x1 > x0 {
                        spans.push((s.height - 1 - vy, x0, x1));
                    }
                }
            }
        }
        self.paint(cx, &spans)
    }

    /// Blend the paint over the destination spans (row in memory order, x0..x1).
    fn paint(&mut self, cx: &mut Cx<'_>, spans: &[(i64, i64, i64)]) -> Result<(), String> {
        let d = self.surface(0x200);
        if d.address == 0 || d.stride < d.width as u64 * 4 {
            return Ok(());
        }
        let image = self.reg(0x604) & !IMAGE_VARIANT == IMAGE;
        let src = self.surface(0x220);
        let m: Vec<f32> = (0..9).map(|k| f(self.reg(0x550 + k))).collect();
        let color = self.reg(0x548);
        let flat = [(color >> 8) as u8, (color >> 16) as u8, (color >> 24) as u8, color as u8];
        let blend = image || self.reg(0x400) & BLEND != 0;
        if blend && !image && flat[3] == 0 && self.reg(0x604) & GRADIENT != GRADIENT {
            return Ok(());
        }
        let gradient = (!image).then(|| self.gradient()).flatten();
        let mut src_row = vec![0u8; if image { src.stride as usize } else { 0 }];
        for &(row, x0, x1) in spans {
            if row < 0 || row >= d.height || x1 <= x0 {
                continue;
            }
            let at = d.address + row as u64 * d.stride + x0 as u64 * 4;
            let mut px = vec![0u8; (x1 - x0) as usize * 4];
            cx.ram_read(at, &mut px)?;
            let vy = (d.height - 1 - row) as f32 + 0.5;
            let mut loaded_row = -1i64;
            for (k, out) in px.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let s = if image {
                    let vx = (x0 + k as i64) as f32 + 0.5;
                    let u = (m[0] * vx + m[1] * vy + m[2]).floor() as i64;
                    let v = (m[3] * vx + m[4] * vy + m[5]).floor() as i64;
                    if u < 0 || v < 0 || u >= src.width || v >= src.height {
                        continue;
                    }
                    if v != loaded_row {
                        cx.ram_read(src.address + v as u64 * src.stride, &mut src_row)?;
                        loaded_row = v;
                    }
                    let o = u as usize * 4;
                    [src_row[o], src_row[o + 1], src_row[o + 2], src_row[o + 3]]
                } else if let Some(g) = &gradient {
                    g.at((x0 + k as i64) as f32 + 0.5, vy)
                } else {
                    flat
                };
                if !blend {
                    *out = s;
                    continue;
                }
                let a = u32::from(s[3]);
                if a == 0 {
                    continue;
                }
                let da = u32::from(out[3]);
                let oa = a * 255 + da * (255 - a);
                for c in 0..3 {
                    let num = u32::from(s[c]) * a * 255 + u32::from(out[c]) * da * (255 - a);
                    out[c] = num.checked_div(oa).map_or(0, |value| value as u8);
                }
                out[3] = (oa / 255) as u8;
            }
            cx.ram_write(at, &px)?;
        }
        if image {
            self.image_draws += 1;
        } else {
            self.draws += 1;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_host::TestHost;

    const SURFACE: usize = 0x2000;

    fn rectangle(renderer: &mut Renderer, host: &mut TestHost, control: u32, color: u32) {
        let words = [
            0x0400_0000,
            control,
            0x0548_0000,
            color,
            0x0412_0001,
            0,
            4 | 4 << 16,
            0x0410_0000,
            0xe,
            0xf000_0000,
        ];
        let mut cx = host.cx();
        renderer.execute(&mut cx, &words).unwrap();
    }

    #[test]
    fn image_paint_blends_over_the_destination_without_control_bit_7() {
        const IMAGE_SURFACE: usize = 0x3000;
        for variant in [IMAGE, IMAGE | IMAGE_VARIANT] {
            let mut renderer = Renderer::default();
            renderer
                .seed(&[
                    (0x200, SURFACE as u32),
                    (0x204, 16),
                    (0x205, 3 | 3 << 16),
                    (0x220, IMAGE_SURFACE as u32),
                    (0x224, 16),
                    (0x225, 3 | 3 << 16),
                    (0x550, 1f32.to_bits()),
                    (0x554, 1f32.to_bits()),
                    (0x558, 1f32.to_bits()),
                    (0x604, variant),
                ])
                .unwrap();
            let mut host = TestHost::new();
            host.ram[SURFACE..SURFACE + 64].fill(0x80);
            // A label image: one opaque black text pixel, the rest transparent.
            host.ram[IMAGE_SURFACE..IMAGE_SURFACE + 4].copy_from_slice(&[0, 0, 0, 0xff]);
            rectangle(&mut renderer, &mut host, 0, 0xff);
            // Image row 0 lands on the bottom surface row (origin bottom-left).
            let bottom = SURFACE + 3 * 16;
            assert_eq!(&host.ram[bottom..bottom + 4], &[0, 0, 0, 0xff]);
            assert!(host.ram[bottom + 4..bottom + 16].iter().all(|b| *b == 0x80));
            assert!(host.ram[SURFACE..bottom].iter().all(|b| *b == 0x80));
        }
    }

    #[test]
    fn gradient_paint_reads_the_ramp_from_state_memory() {
        let mut renderer = Renderer::default();
        renderer
            .seed(&[
                (0x200, SURFACE as u32),
                (0x204, 16),
                (0x205, 3 | 3 << 16),
                // Identity surface-to-paint matrix.
                (0x574, 1f32.to_bits()),
                (0x578, 1f32.to_bits()),
                (0x57c, 1f32.to_bits()),
            ])
            .unwrap();
        let mut host = TestHost::new();
        let words = [
            // Gradient along x from 0 to 4: start (0, 0), direction (4, 0), 1/16.
            0x4040_0067,
            0,
            0,
            4f32.to_bits(),
            0,
            (1.0f32 / 16.0).to_bits(),
            // Two stops: black at 0, white at 255.
            0x4020_006e,
            0,
            0x0000_00ff,
            (1.0f32 / 255.0).to_bits(),
            0x4020_0072,
            255 << 5 | 1,
            0xffff_ffff,
            0,
            // Gradient paint with stop index 1 last, black placeholder color.
            0x0604_0000,
            1 << 26 | 0x5a040,
        ];
        renderer.execute(&mut host.cx(), &words).unwrap();
        rectangle(&mut renderer, &mut host, BLEND, 0xff);
        // Pixel centers 0.5, 1.5, 2.5, 3.5 of 4 along the ramp; opaque throughout.
        let row: Vec<u8> = (0..4).map(|k| host.ram[SURFACE + 4 * k]).collect();
        assert_eq!(row, [32, 96, 159, 223]);
        assert!((0..4).all(|k| host.ram[SURFACE + 4 * k + 3] == 0xff));
    }

    #[test]
    fn paint_blends_with_control_bit_7_and_replaces_without_it() {
        let mut renderer = Renderer::default();
        renderer
            .seed(&[(0x200, SURFACE as u32), (0x204, 16), (0x205, 3 | 3 << 16)])
            .unwrap();
        let mut host = TestHost::new();
        host.ram[SURFACE..SURFACE + 64].fill(0xff);
        // Transparent paint with blending changes nothing.
        rectangle(&mut renderer, &mut host, BLEND, 0);
        assert!(host.ram[SURFACE..SURFACE + 64].iter().all(|b| *b == 0xff));
        // Without blending it clears the area to transparent, as the firmware clears OSD regions.
        rectangle(&mut renderer, &mut host, 0, 0);
        assert!(host.ram[SURFACE..SURFACE + 64].iter().all(|b| *b == 0));
        // A translucent paint is stored as is, alpha included (bytes B, G, R, A).
        rectangle(&mut renderer, &mut host, 0, 0x1122_3380);
        assert_eq!(&host.ram[SURFACE..SURFACE + 4], &[0x33, 0x22, 0x11, 0x80]);
        // Blended over it, an opaque paint covers it.
        rectangle(&mut renderer, &mut host, BLEND, 0x4455_66ff);
        assert_eq!(
            &host.ram[SURFACE + 60..SURFACE + 64],
            &[0x66, 0x55, 0x44, 0xff]
        );
    }
}
