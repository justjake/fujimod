use fujimod_emulator_devices::record::Result;
use fujimod_emulator_devices::{Record, Value, WordMap};
use serde::Serialize;
use std::collections::BTreeSet;
pub const BASES: [u64; 2] = [0xfff6b000, 0xfff6e000];
pub const BLACK: &str = "declared-black-canvas-unmeasured";
pub const WHITE: &str = "declared-white-canvas-unmeasured";
// Q12 source increments and half the endpoint residual in Q8 phase units
// (residual >> 5) define the sampling grid. The endpoint uses source
// dimensions rather than output dimensions. Nearest sampling is a declared
// approximation of the filter.
fn video_axis(source: usize, step: u32, phase: u32, limit: usize) -> Result<Vec<usize>> {
    if step == 0 {
        return Err("Q12 video has a zero increment".into());
    }
    let phase = phase as usize * 16;
    let last = (source - 1) * 4096;
    if phase > last {
        return Err("Video phase lies outside the source".into());
    }
    let count = (last - phase) / step as usize + 1;
    if count > limit {
        return Err("Video window outside the canvas".into());
    }
    Ok((0..count)
        .map(|index| ((phase + index * step as usize + 2048) / 4096).min(source - 1))
        .collect())
}
/// Byte positions of a 32-bit graphics pixel: R, G, B and straight alpha.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PixelBytes {
    pub rgb: [usize; 3],
    pub alpha: usize,
}
/// Group 1 at source format 11 uses bytes A,B,G,R (words 0xRRGGBBAA), with
/// straight alpha in byte 0.
const GROUP1_BYTES: PixelBytes = PixelBytes {
    rgb: [3, 2, 1],
    alpha: 0,
};
pub const ABGR8888_STRAIGHT: &str = "mode1-abgr8888-straight";
/// The supported group 1 pixel interpretation.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum GraphicsProfile {
    Abgr8888Straight,
}
impl GraphicsProfile {
    pub fn parse(name: &str) -> std::result::Result<Self, String> {
        match name {
            ABGR8888_STRAIGHT => Ok(Self::Abgr8888Straight),
            _ => Err("unknown graphics interpretation".into()),
        }
    }
    pub fn as_str(&self) -> &str {
        match self {
            Self::Abgr8888Straight => ABGR8888_STRAIGHT,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum BackdropProfile {
    Black,
    White,
}
impl BackdropProfile {
    pub fn parse(name: &str) -> std::result::Result<Self, String> {
        match name {
            BLACK => Ok(Self::Black),
            WHITE => Ok(Self::White),
            _ => Err("unknown backdrop interpretation".into()),
        }
    }
    pub fn as_str(&self) -> &str {
        match self {
            Self::Black => BLACK,
            Self::White => WHITE,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub enum RectangleComposition {
    DeclaredLuma,
    SignedYCbCr,
}
impl RectangleComposition {
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            super::rectangles::PROFILE => Some(Self::DeclaredLuma),
            super::rectangles::SIGNED_YCBCR_PROFILE => Some(Self::SignedYCbCr),
            _ => None,
        }
    }
}
pub const MAIN_REQUIRED: [u32; 9] = [0, 0x10, 0x24, 0x28, 0x2c, 0x30, 0xd4, 0xd8, 0xdc];
pub fn address(address: u64, size: u32) -> Result<(usize, u32)> {
    for (i, b) in BASES.iter().enumerate() {
        if (*b..*b + 0x1000).contains(&address) {
            let o = (address - b) as u32;
            let allowed = (o <= 0x100 && o != 0x18)
                || (0x108..0x19c).contains(&o)
                || [0x1bc, 0x9d0, 0x9d4].contains(&o)
                || i == 1 && o == 0x18;
            if size != 4 || o & 3 != 0 || !allowed {
                return Err(
                    format!("Unknown image-engine register/access {address:#x}/{size}").into(),
                );
            }
            return Ok((i, o));
        }
    }
    Err(format!("Unknown image-engine bank {address:#x}").into())
}
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pipe {
    pub regs: WordMap,
    pub written: BTreeSet<u32>,
    pub generation: i64,
    pub reset_completions: i64,
}
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Compositor {
    pub pipes: [Pipe; 2],
    pub packing: String,
    pub matrix: String,
    pub graphics_profile: Option<GraphicsProfile>,
    pub backdrop_profile: Option<BackdropProfile>,
    pub rectangle_composition: Option<RectangleComposition>,
    pub ram_bytes: i64,
    pub max_pixels: i64,
}
/// Reset completion tokens: bit0 is the pipe, bit1 the requesting CPU,
/// and the remaining bits the generation. Core indices are 0 and 1.
pub fn reset_token(pipe: usize, generation: i64, cpu: i64) -> u64 {
    (generation as u64) << 2 | ((cpu as u64) & 1) << 1 | pipe as u64
}
pub fn reset_fields(token: u64) -> (usize, i64, i64) {
    (
        (token & 1) as usize,
        (token >> 2) as i64,
        ((token >> 1) & 1) as i64,
    )
}
impl Compositor {
    pub fn read(&self, a: u64, size: u32) -> Result<u32> {
        let (i, o) = address(a, size)?;
        Ok(self.pipes[i].regs.get_or_zero(o))
    }
    /// Guest writes return their record and two-tick reset deadline in order.
    pub fn write(
        &mut self,
        cpu: i64,
        a: u64,
        size: u32,
        v: u32,
        actions: &mut Vec<fujimod_emulator_devices::Action>,
    ) -> Result<()> {
        let (i, o) = address(a, size)?;
        let p = &mut self.pipes[i];
        let old = p.regs.get_or_zero(o);
        p.regs.insert(o, v);
        p.written.insert(o);
        if o == 0 && old & 0x10000 != 0 && v & 0x10000 == 0 {
            // Clearing bit 16 starts reset. No other bit is made ready;
            // existing configuration is retained (reset behavior is unmeasured).
            p.generation += 1;
            actions.push(fujimod_emulator_devices::Action::Log(Record::new(
                "Image-engine reset start",
                [("instance", Value::int(i as i64)), ("cpu", Value::int(cpu))],
            )));
            actions.push(fujimod_emulator_devices::Action::Schedule {
                delay: 2,
                token: reset_token(i, p.generation, cpu),
            });
        }
        Ok(())
    }
    pub fn complete(
        &mut self,
        token: u64,
        _now: i64,
        actions: &mut Vec<fujimod_emulator_devices::Action>,
    ) {
        let (i, generation, cpu) = reset_fields(token);
        let p = &mut self.pipes[i];
        if generation == p.generation {
            p.regs.insert(0, p.regs.get_or_zero(0) | 0x10000);
            p.reset_completions += 1;
            actions.push(fujimod_emulator_devices::Action::Log(Record::new(
                "Image-engine reset complete",
                [("instance", Value::int(i as i64)), ("cpu", Value::int(cpu))],
            )));
        }
    }
}

use fujimod_emulator_devices::exr2::scanout_frame::{Frame, State as S, sha256};
/// The board RAM callback and screenshot-only color/image encoders. The
/// Host inspection never writes RAM here (`write_bytes` is accepted but unused).
pub trait ScanoutHost {
    /// Presentation reads pixels without computing diagnostic content hashes.
    fn diagnostic_hashes(&self) -> bool {
        true
    }
    fn unhandled_error(&self) -> bool {
        false
    }
    fn read_bytes(&mut self, address: u64, count: usize, cpu: i64) -> Result<Vec<u8>>;
    fn render_rgb(
        &mut self,
        raw: &[u8],
        width: usize,
        height: usize,
        stride: usize,
        packing: &str,
        matrix: &str,
    ) -> Result<Vec<u8>>;
    fn encode_png(&mut self, rgb: &[u8], width: usize, height: usize) -> Result<Option<Vec<u8>>>;
    fn record(&mut self, r: Record) -> Result<()>;
}

fn frame_hash(host: &dyn ScanoutHost, bytes: &[u8]) -> S {
    if host.diagnostic_hashes() {
        S::str(&sha256(bytes))
    } else {
        S::None
    }
}
/// Group 0 mode-1 byte order per register 0 bits 23:20: source byte index of
/// R, G, B (byte 3 is alpha). Formats 0-5 select the six channel orders, 12-15
/// select the default order, and 6-11 are not supported as 32-bit pixels.
fn group0_byte_order(format: u32) -> Option<[usize; 3]> {
    match format {
        0 | 12..=15 => Some([0, 1, 2]),
        1 => Some([1, 0, 2]),
        2 => Some([0, 2, 1]),
        3 => Some([2, 1, 0]),
        4 => Some([2, 0, 1]),
        5 => Some([1, 2, 0]),
        _ => None,
    }
}
#[derive(Clone)]
struct Channel {
    layer: usize,
    channel: usize,
    enabled: bool,
    configured: bool,
    width: usize,
    height: usize,
    origin: u32,
    source: u32,
    source_control: u32,
    padding: u32,
    alpha: u32,
}

#[cfg(test)]
#[path = "scanout_tests.rs"]
mod tests;
impl Channel {
    fn state(&self) -> S {
        S::dict(vec![
            ("channel", S::Int(self.channel as i64)),
            ("enabled", S::Bool(self.enabled)),
            ("configured", S::Bool(self.configured)),
            ("width", S::Int(self.width as i64)),
            ("height", S::Int(self.height as i64)),
            ("origin_word", S::hex(self.origin as u64)),
            ("source", S::hex(self.source as u64)),
            ("source_control", S::Int(self.source_control as i64)),
            ("padding_units", S::Int(self.padding as i64)),
            ("alpha_field", S::Int(self.alpha as i64)),
        ])
    }
    fn plane_state(&self) -> S {
        let mut d = S::dict(vec![("layer", S::Int(self.layer as i64))]);
        if let S::Dict(items) = self.state() {
            for (k, v) in items {
                d.set(&k, v);
            }
        }
        d
    }
}
impl Compositor {
    fn channels(&self, i: usize) -> Vec<Channel> {
        let p = &self.pipes[i];
        let r = &p.regs;
        let mut channels = Vec::new();
        for l in 0..2 {
            let b = l as u32 * 0x50;
            for c in 0..4 {
                let d = c as u32 * 4;
                let dims = r.get_or_zero(0x54 + b + d);
                let source = r.get_or_zero(0x64 + b + d);
                channels.push(Channel {
                    layer: l,
                    channel: c,
                    enabled: r.get_or_zero(0x40 + b) & (1 << (c * 4)) != 0,
                    configured: [0x54 + b + d, 0x64 + b + d, 0x74 + b + d, 0x44 + b + d]
                        .iter()
                        .all(|o| p.written.contains(o)),
                    width: ((dims >> 16) & 0x1fff) as usize + 1,
                    height: (dims & 0x1fff) as usize + 1,
                    origin: r.get_or_zero(0x44 + b + d),
                    source: source & 0x3fffffff,
                    source_control: source >> 30,
                    padding: r.get_or_zero(0x74 + b + d) & 0xfff,
                    alpha: (r.get_or_zero(0xe0 + l as u32 * 4) >> (c * 8)) & 0xff,
                });
            }
        }
        channels
    }
    pub fn graphics(&self, i: usize) -> S {
        let channels = self.channels(i);
        let r = &self.pipes[i].regs;
        S::List(
            (0..2)
                .map(|l| {
                    let b = l as u32 * 0x50;
                    S::dict(vec![
                        ("layer", S::Int(l as i64)),
                        ("mode_field", S::Int((r.get_or_zero(0x34 + b) & 7) as i64)),
                        ("control_word", S::hex(r.get_or_zero(0x34 + b) as u64)),
                        ("enable_word", S::hex(r.get_or_zero(0x40 + b) as u64)),
                        (
                            "channels",
                            S::List(
                                channels[l * 4..l * 4 + 4]
                                    .iter()
                                    .map(|c| c.state())
                                    .collect(),
                            ),
                        ),
                    ])
                })
                .collect(),
        )
    }
    fn validate_ram_span(&self, address: i64, count: i64) -> Result<()> {
        if address < 0
            || count < 0
            || address
                .checked_add(count)
                .is_none_or(|end| end > self.ram_bytes)
        {
            return Err("Display source extends outside modeled RAM".into());
        }
        Ok(())
    }

    fn plane_bytes(
        &self,
        address: i64,
        count: i64,
        cpu: i64,
        pixels: bool,
        host: &mut dyn ScanoutHost,
    ) -> Result<Vec<u8>> {
        self.validate_ram_span(address, count)?;
        if pixels || host.diagnostic_hashes() {
            self.read_ram(address, count, cpu, host)
        } else {
            // An observational metadata query needs the register layout and
            // range validation, not a copy of every plane's pixel payload.
            Ok(Vec::new())
        }
    }

    pub fn read_ram(
        &self,
        address: i64,
        count: i64,
        cpu: i64,
        host: &mut dyn ScanoutHost,
    ) -> Result<Vec<u8>> {
        self.validate_ram_span(address, count)?;
        // Shared physical alias even when CPU low reads mirror ROM.
        let data = host.read_bytes(address as u64 | 0x40000000, count as usize, cpu)?;
        if data.len() != count as usize {
            return Err("Display RAM read was truncated".into());
        }
        Ok(data)
    }
    pub fn prefixes(
        &self,
        i: usize,
        count: i64,
        cpu: i64,
        host: &mut dyn ScanoutHost,
    ) -> Result<S> {
        let mut captures = Vec::new();
        for c in self.channels(i) {
            if c.enabled && c.configured {
                let n = count.min(self.ram_bytes - c.source as i64);
                let raw = self.read_ram(c.source as i64, n, cpu, host)?;
                let mut s = c.plane_state();
                s.set("requested_prefix_bytes", S::Int(count));
                s.set("captured_bytes", S::Int(raw.len() as i64));
                s.set("raw", S::Bytes(raw));
                captures.push(s);
            }
        }
        Ok(S::List(captures))
    }
    fn overlay_check(&self, i: usize) -> Result<()> {
        let enabled = self.pipes[i].regs.get_or_zero(0x108) & 0x000303ff;
        let unsupported = match self.rectangle_composition {
            Some(_) => 0,
            None => enabled,
        };
        if unsupported != 0 {
            return Err("Enabled rectangle/color overlays are not modeled".into());
        }
        Ok(())
    }
    pub fn main_frame(
        &self,
        i: usize,
        cpu: i64,
        pixels: bool,
        host: &mut dyn ScanoutHost,
    ) -> Result<Option<Frame>> {
        self.main_plane(i, cpu, pixels, true, host)
    }
    fn main_plane(
        &self,
        i: usize,
        cpu: i64,
        pixels: bool,
        encode: bool,
        host: &mut dyn ScanoutHost,
    ) -> Result<Option<Frame>> {
        self.overlay_check(i)?;
        let p = &self.pipes[i];
        let r = &p.regs;
        if r.get_or_zero(0) & 1 == 0 {
            return Ok(None);
        }
        let missing = MAIN_REQUIRED
            .iter()
            .filter(|o| !p.written.contains(o))
            .map(|o| format!("{o:#x}"))
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            return Err(format!("Missing main-video registers: {}", missing.join(", ")).into());
        }
        let dimensions = r.get_or_zero(0x28);
        let w = ((dimensions >> 16) & 0x1fff) as usize + 1;
        let h = (dimensions & 0x1fff) as usize + 1;
        let canvas = r.get_or_zero(0x10);
        let cw = ((canvas >> 16) & 0xfff) as usize + 1;
        let ch = (canvas & 0xfff) as usize + 1;
        if (w * h) as i64 > self.max_pixels {
            return Err("Display pixel budget exceeded".into());
        }
        let origin = r.get_or_zero(0x24);
        // Nonidentity axes use the scaled-origin register with ten-bit
        // positions. Keep the independent destination origin for centered
        // windows too.
        let scaled_origin = r.get_or_zero(0xdc);
        let left = ((origin >> 16) & 0xfff) as usize + ((scaled_origin >> 16) & 0x3ff) as usize;
        let top = (origin & 0xfff) as usize + (scaled_origin & 0x3ff) as usize;
        let increments = r.get_or_zero(0xd4);
        let phases = r.get_or_zero(0xd8);
        if phases & 0xff00ff00 != 0 || scaled_origin & !0x03ff03ff != 0 {
            return Err("Only 8-bit Q8 video phases and 10-bit scaled origins are modeled".into());
        }
        let xs = video_axis(w, increments >> 16, phases >> 16, cw)?;
        let ys = video_axis(h, increments & 0xffff, phases & 0xffff, ch)?;
        let (ww, wh) = (xs.len(), ys.len());
        let scaled = increments != 0x10001000 || phases != 0;
        let placed = (cw, ch) != (w, h) || origin != 0 || scaled_origin != 0 || scaled;
        if (cw * ch) as i64 > self.max_pixels {
            return Err("Display pixel budget exceeded".into());
        }
        if placed && (origin & 0xf000f000 != 0 || left + ww > cw || top + wh > ch) {
            return Err(
                "Video window outside the canvas; scaling or placement requires further modeling"
                    .into(),
            );
        }
        let source_word = r.get_or_zero(0x2c);
        let stride_word = r.get_or_zero(0x30);
        if source_word & 0xc0000000 != 0
            || stride_word & !0xfff != 0
            || dimensions & 0xe000e000 != 0
        {
            return Err("Unresolved video source/format/stride control bits".into());
        }
        let source = source_word & 0x3fffffff;
        let padding = stride_word & 0xfff;
        let stride = (w + padding as usize) * 2;
        let count = (h - 1) * stride + w * 2;
        let raw = self.plane_bytes(source as i64, count as i64, cpu, pixels, host)?;
        let rgb = if pixels {
            let window = host.render_rgb(&raw, w, h, stride, &self.packing, &self.matrix)?;
            if placed {
                let mut canvas = vec![0; cw * ch * 3];
                for (row, &source_y) in ys.iter().enumerate() {
                    let start = ((top + row) * cw + left) * 3;
                    for (column, &source_x) in xs.iter().enumerate() {
                        let source = (source_y * w + source_x) * 3;
                        canvas[start + column * 3..start + (column + 1) * 3]
                            .copy_from_slice(&window[source..source + 3]);
                    }
                }
                Some(canvas)
            } else {
                Some(window)
            }
        } else {
            None
        };
        let png = if let Some(rgb) = &rgb
            && encode
        {
            host.encode_png(
                rgb,
                if placed { cw } else { w },
                if placed { ch } else { h },
            )?
        } else {
            None
        };
        let mut limitations = vec![
            S::str(
                "Packing/matrix is an explicit interpretation; hardware validation remains pending",
            ),
            S::str("Panel timing, filter coefficients and HDMI PHY are not modeled"),
        ];
        if placed {
            limitations.push(S::str("Canvas outside a placed video window is declared black; the hardware background color is not modeled"));
        }
        let mut state = S::dict(vec![
            ("instance", S::Int(i as i64)),
            ("image_base", S::hex(BASES[i])),
            ("width", S::Int(w as i64)),
            ("height", S::Int(h as i64)),
            ("source", S::hex(source as u64)),
            ("source_cpu_alias", S::hex((source | 0x40000000) as u64)),
            ("stride", S::Int(stride as i64)),
            ("padding_pixels", S::Int(padding as i64)),
            ("packing", S::str(&self.packing)),
            ("matrix", S::str(&self.matrix)),
            ("frame_sha256", frame_hash(host, &raw)),
            (
                "png_sha256",
                png.as_ref().map_or(S::None, |png| frame_hash(host, png)),
            ),
            (
                "kind",
                S::str("main video plane at explicit frame boundary"),
            ),
            ("limitations", S::List(limitations)),
        ]);
        if placed {
            state.set(
                "window_origin",
                S::List(vec![S::Int(left as i64), S::Int(top as i64)]),
            );
            state.set(
                "canvas",
                S::List(vec![S::Int(cw as i64), S::Int(ch as i64)]),
            );
        }
        if scaled {
            state.set(
                "window_size",
                S::List(vec![S::Int(ww as i64), S::Int(wh as i64)]),
            );
            state.set("scaling", S::str("declared nearest-neighbor Q12 increments/Q8 phases; hardware filtering unmeasured"));
        }
        Ok(Some(Frame {
            raw,
            rgb,
            png,
            state,
            width: if placed { cw } else { w },
            height: if placed { ch } else { h },
        }))
    }
    pub fn frame(
        &self,
        i: usize,
        cpu: i64,
        pixels: bool,
        host: &mut dyn ScanoutHost,
    ) -> Result<Option<Frame>> {
        let Some(mut frame) = self.plane_frame(i, cpu, pixels, host)? else {
            return Ok(None);
        };
        let rectangles = super::rectangles::decode(&self.pipes[i], self.rectangle_composition)?;
        super::rectangles::compose(&mut frame, &rectangles, host.diagnostic_hashes());
        if let Some(rgb) = &frame.rgb {
            if let Some(png) = host.encode_png(rgb, frame.width, frame.height)? {
                frame.state.set("png_sha256", frame_hash(host, &png));
                frame.png = Some(png);
            }
        }
        Ok(Some(frame))
    }
    fn plane_frame(
        &self,
        i: usize,
        cpu: i64,
        pixels: bool,
        host: &mut dyn ScanoutHost,
    ) -> Result<Option<Frame>> {
        let mut enabled = self
            .channels(i)
            .into_iter()
            .filter(|c| c.enabled)
            .collect::<Vec<_>>();
        if enabled.is_empty() {
            return self.main_plane(i, cpu, pixels, false, host);
        }
        // In layer 1, lower-numbered windows cover higher-numbered windows.
        // Draw the bottom window first.
        enabled.sort_by_key(|c| {
            (
                c.layer,
                if c.layer == 1 {
                    3 - c.channel
                } else {
                    c.channel
                },
            )
        });
        let Some(profile) = &self.graphics_profile else {
            return Err(
                "Enabled graphics plane: pixel format/palette/alpha composition is not modeled"
                    .into(),
            );
        };
        self.overlay_check(i)?;
        let p = &self.pipes[i];
        let r = &p.regs;
        if !p.written.contains(&0x10) {
            return Err("Missing graphics canvas geometry".into());
        }
        let canvas = r.get_or_zero(0x10);
        let w = ((canvas >> 16) & 0xfff) as usize + 1;
        let h = (canvas & 0xfff) as usize + 1;
        if canvas & 0xf000f000 != 0 || (w * h) as i64 > self.max_pixels {
            return Err("Unknown graphics canvas controls or pixel budget".into());
        }
        let mut main = self.main_plane(i, cpu, pixels, false, host)?;
        // Optional canvas colors are declared fixtures, not a panel model.
        let backdrop_color = match self.backdrop_profile {
            Some(BackdropProfile::Black) => Some(0),
            Some(BackdropProfile::White) => Some(255),
            _ => None,
        };
        let backdrop = main.is_none() && backdrop_color.is_some();
        let mut rgb = if pixels {
            Some(
                main.as_mut()
                    .and_then(|f| f.rgb.take())
                    .unwrap_or_else(|| vec![backdrop_color.unwrap_or(0); w * h * 3]),
            )
        } else {
            None
        };
        let mut covered = vec![main.is_some() || backdrop; w * h];
        let mut raw = main
            .as_mut()
            .map_or_else(Vec::new, |f| std::mem::take(&mut f.raw));
        let mut plane_states = Vec::new();
        // Group 1 composes over group 0, including when windows touch, overlap,
        // or sit inside enabled group 0 windows. The ascending sort above draws
        // group 0 first.
        for c in enabled {
            let b = c.layer as u32 * 0x50;
            let control = r.get_or_zero(0x34 + b);
            let yuv = control & 7 == 0;
            if control & 7 > 1 || control & !7 != 0 {
                return Err("Graphics layers support only mode1 32-bit and mode0 packed 4:2:2 with no unresolved control bits".into());
            }
            // Group 1 mode 1 currently supports source format 11, which reads
            // A,B,G,R. Other source formats are rejected until modeled.
            if !yuv && c.layer == 1 && (r.get_or_zero(0) >> 24) & 0xf != 11 {
                return Err("Group 1 mode1 currently supports source format 11 only".into());
            }
            // In group 0 mode 1, register 0 bits 23:20 select the byte order of
            // 32-bit pixels; byte 3 carries straight alpha.
            let bytes =
                if !yuv && c.layer == 0 {
                    match group0_byte_order((r.get_or_zero(0) >> 20) & 0xf) {
                        Some(rgb) => PixelBytes { rgb, alpha: 3 },
                        None => return Err(
                            "Group 0 mode1 source formats 6-11 are unsupported as 32-bit pixels"
                                .into(),
                        ),
                    }
                } else {
                    GROUP1_BYTES
                };
            let group0_rgb = !yuv && c.layer == 0;
            if !c.configured || !p.written.contains(&(0xe0 + c.layer as u32 * 4)) {
                return Err("Missing graphics source/geometry/alpha registers".into());
            }
            if !p.written.contains(&(0x34 + b)) || c.source_control != 0 {
                return Err("Missing or unresolved graphics format/source controls".into());
            }
            // Register 08 stores two 2-bit scale codes per layer. Equal 1:1 and
            // 2x scales are modeled; unequal axes remain unresolved. Nearest
            // replication is a declared interpretation; no filtering is modeled.
            let orientation = (r.get_or_zero(8) >> (if c.layer == 1 { 8 } else { 4 })) & 0xf;
            if orientation != 0 && orientation != 5 {
                return Err(
                    "Graphics scale codes other than 1:1 or equal 2x require further modeling"
                        .into(),
                );
            }
            let scale = if orientation == 5 { 2 } else { 1 };
            // Scaling applies to the window position as well as its dimensions.
            let x = ((c.origin >> 16) & 0x3fff) as usize * scale;
            let y = (c.origin & 0x3fff) as usize * scale;
            let mut pw = c.width * scale;
            let mut ph = c.height * scale;
            if scale != 1 && x < w && y < h {
                pw = pw.min(w - x);
                ph = ph.min(h - y);
            }
            if c.origin & 0xc000c000 != 0 {
                return Err("Graphics origin/crop is outside the supported canvas".into());
            }
            if x >= w || y >= h {
                let mut s = c.plane_state();
                s.set("origin", S::List(vec![S::Int(x as i64), S::Int(y as i64)]));
                s.set("visible", S::Bool(false));
                s.set("clip", S::str("entirely outside the canvas"));
                plane_states.push(s);
                continue;
            }
            if x + pw > w || y + ph > h {
                if !yuv {
                    return Err("Graphics origin/crop is outside the supported canvas".into());
                }
                pw = pw.min(w - x);
                ph = ph.min(h - y);
            }
            let dims = r.get_or_zero(0x54 + b + c.channel as u32 * 4);
            let stride_word = r.get_or_zero(0x74 + b + c.channel as u32 * 4);
            if dims & 0xe000e000 != 0 || stride_word & !0xfff != 0 || c.alpha > 128 {
                return Err("Unresolved graphics dimensions/stride/alpha controls".into());
            }
            if (pw * ph) as i64 > self.max_pixels {
                return Err("Graphics pixel budget exceeded".into());
            }
            let depth = if yuv { 2 } else { 4 };
            let stride = (c.width + c.padding as usize) * depth;
            let count = (c.height - 1) * stride + c.width * depth;
            let plane = self.plane_bytes(c.source as i64, count as i64, cpu, pixels, host)?;
            if let Some(rgb) = &mut rgb {
                let colors = if yuv {
                    Some(host.render_rgb(
                        &plane,
                        c.width,
                        c.height,
                        stride,
                        &self.packing,
                        &self.matrix,
                    )?)
                } else {
                    None
                };
                // Check coverage over the whole plane before modifying it,
                // A fully opaque pixel has alpha 255 and channel alpha 128.
                for yy in 0..ph {
                    for xx in 0..pw {
                        let source = (yy / scale) * stride + (xx / scale) * 4;
                        let target = (y + yy) * w + x + xx;
                        let alpha = if yuv {
                            255
                        } else {
                            plane[source + bytes.alpha] as u32
                        };
                        if !covered[target] && alpha * c.alpha != 255 * 128 {
                            return Err(
                                "Transparent graphics has no actual initialized backdrop".into()
                            );
                        }
                    }
                }
                for yy in 0..ph {
                    for xx in 0..pw {
                        let source = (yy / scale) * stride + (xx / scale) * 4;
                        let target = (y + yy) * w + x + xx;
                        let numerator = if yuv {
                            255
                        } else {
                            plane[source + bytes.alpha] as u32
                        } * c.alpha;
                        let denominator = 255 * 128;
                        // Coverage was checked before drawing. A transparent
                        // pixel preserves that backdrop, and an opaque pixel
                        // replaces it without reading or blending its colors.
                        if numerator == 0 {
                            continue;
                        }
                        let front = if let Some(colors) = &colors {
                            let start = ((yy / scale) * c.width + xx / scale) * 3;
                            [colors[start], colors[start + 1], colors[start + 2]]
                        } else {
                            bytes.rgb.map(|color| plane[source + color])
                        };
                        let back = &mut rgb[target * 3..target * 3 + 3];
                        if numerator == denominator {
                            back.copy_from_slice(&front);
                        } else {
                            for (color, value) in back.iter_mut().zip(front) {
                                let original = if covered[target] { *color as u32 } else { 0 };
                                *color = ((value as u32 * numerator
                                    + original * (denominator - numerator)
                                    + denominator / 2)
                                    / denominator)
                                    .min(255) as u8;
                            }
                        }
                        covered[target] = true;
                    }
                }
            } else {
                for yy in y..y + ph {
                    covered[yy * w + x..yy * w + x + pw].fill(true);
                }
            }
            let mut s = c.plane_state();
            s.set("origin", S::List(vec![S::Int(x as i64), S::Int(y as i64)]));
            let packing_name = format!(
                "bytes ({}, {}, {}) as R,G,B, byte 3 straight alpha",
                bytes.rgb[0], bytes.rgb[1], bytes.rgb[2]
            );
            s.set("stride_bytes", S::Int(stride as i64));
            s.set("captured_bytes", S::Int(plane.len() as i64));
            s.set(
                "packing",
                S::str(if yuv {
                    &self.packing
                } else if group0_rgb {
                    &packing_name
                } else {
                    "ABGR byte order, byte 0 straight alpha"
                }),
            );
            s.set("sha256", frame_hash(host, &plane));
            if scale != 1 {
                s.set("scale", S::Int(scale as i64));
                s.set(
                    "scale_interpretation",
                    S::str("declared nearest-neighbor replication"),
                );
            }
            raw.extend_from_slice(&plane);
            plane_states.push(s);
        }
        if covered.iter().any(|v| !*v) {
            return Err("Graphics planes leave canvas pixels without an actual backdrop".into());
        }
        // Encode only the final composition, after every plane and overlay.
        let png: Option<Vec<u8>> = None;
        let mut limits = vec![
            S::str(
                "Group 1 mode1 format 11 bytes A,B,G,R with straight alpha in byte 0; group 0 mode1 supports selected byte orders; group 1 composes over group 0",
            ),
            S::str(
                "Channel alpha128 means full; pixel alpha255 means full; rounding is an explicit arithmetic approximation",
            ),
            S::str("Ascending group 0 window order is a profile assumption"),
        ];
        limits.push(S::str(
            "Panel timing, orientation/filter controls and HDMI PHY are not modeled",
        ));
        if backdrop {
            limits.push(S::str(
                "Color behind transparent graphics uses the declared canvas backdrop",
            ));
        }
        let state = S::dict(vec![
            ("instance", S::Int(i as i64)),
            ("image_base", S::hex(BASES[i])),
            ("width", S::Int(w as i64)),
            ("height", S::Int(h as i64)),
            ("graphics_profile", S::str(profile.as_str())),
            (
                "backdrop_profile",
                self.backdrop_profile
                    .as_ref()
                    .map_or(S::None, |s| S::str(s.as_str())),
            ),
            ("declared_backdrop_used", S::Bool(backdrop)),
            ("main", main.map_or(S::None, |f| f.state)),
            ("graphics", S::List(plane_states)),
            ("frame_sha256", frame_hash(host, &raw)),
            (
                "png_sha256",
                png.as_ref().map_or(S::None, |p| frame_hash(host, p)),
            ),
            (
                "kind",
                S::str("actual RAM composition with an explicit graphics interpretation"),
            ),
            ("limitations", S::List(limits)),
        ]);
        Ok(Some(Frame {
            raw,
            rgb,
            png,
            state,
            width: w,
            height: h,
        }))
    }
}
