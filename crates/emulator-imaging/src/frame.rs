//! Declared fixed-luma Y0,Y1,Cb,Cr input at programmed imaging DMA outputs.
//! This source is a scenario input, not sensor/ISP or coefficient fidelity.
//!
//! Host scene. The `XT5_SCENE` environment variable replaces the flat source with a host
//! picture, for live view ([`paint_scene`], MOVDC) and for still captures ([`host_still_422`],
//! the XP5 JPEG encoder shortcut):
//!
//! - `bars`: colour bars with a moving marker.
//! - `PATH.bmp`: an uncompressed 24- or 32-bit BMP.
//! - `video:avf:N[@WxH[@FPS]]`: AVFoundation video device `N` (webcam, OBS Virtual Camera,
//!   iPhone Continuity Camera, "Capture screen N"), through an ffmpeg child process.
//! - `video:PATH_OR_URL[@WxH[@FPS]]`: a video file, looped in real time, or a stream URL.
//! - `video:lavfi:GRAPH[@WxH[@FPS]]`: an ffmpeg lavfi source, such as `testsrc2`.
//! - `fifo:PATH@WxH`: raw rgb24 frames written by any producer.
//!
//! Live scenes ([`live`]) are sampled at the moment of use: every live-view frame and every
//! JPEG encode takes the newest frame the reader thread has completed, scaled bilinearly to the
//! programmed size. The scene is a host input outside the run definition, the checkpoint and
//! the wiring hash.
use fujimod_emulator_devices::Cx;

pub mod live;

pub fn paint(
    cx: &mut Cx<'_>,
    address: u64,
    width: u32,
    height: u32,
    stride: u32,
    luma: u8,
) -> Result<(), String> {
    if !(16..=235).contains(&luma)
        || width == 0
        || width & 1 != 0
        || height == 0
        || width > 4096
        || height > 4096
        || stride < width * 2
        || stride & 3 != 0
        || u64::from(stride) * u64::from(height) > 32 * 1024 * 1024
    {
        return Err("fixed YUV source geometry/range exceeds modeled bounds".into());
    }
    address
        .checked_add(u64::from(stride) * u64::from(height))
        .ok_or("YUV DMA address overflow")?;
    let row = [luma, luma, 128, 128].repeat(width as usize / 2);
    for y in 0..height {
        cx.ram_write(address + u64::from(y) * u64::from(stride), &row)?;
    }
    Ok(())
}

/// A host-supplied virtual scene: colour bars, an RGB picture or a live feed.
pub enum Scene {
    /// Seven colour bars and black over the top three quarters, a grey ramp below.
    Bars,
    /// An uncompressed BMP, top row first, three bytes (R, G, B) per pixel.
    Picture { width: usize, height: usize, rgb: Vec<u8> },
    /// A video device, file, URL or FIFO; colour bars until its first frame.
    Live(live::LiveFeed),
}

/// What one render of a scene draws: the scene with a live feed resolved to its newest frame.
enum View<'a> {
    Bars,
    Nearest { width: usize, height: usize, rgb: &'a [u8] },
    Bilinear(live::Sampler<'a>),
}

/// Holds a live frame for the duration of one render, so every row comes from one frame.
fn snapshot(scene: &Scene) -> (Option<std::sync::Arc<live::RgbFrame>>, &Scene) {
    match scene {
        Scene::Live(feed) => (feed.latest(), scene),
        _ => (None, scene),
    }
}

fn view<'a>(scene: &'a Scene, frame: Option<&'a live::RgbFrame>, width: usize) -> View<'a> {
    match (scene, frame) {
        (Scene::Picture { width: pw, height: ph, rgb }, _) => {
            View::Nearest { width: *pw, height: *ph, rgb }
        }
        (Scene::Live(_), Some(frame)) => View::Bilinear(live::Sampler::new(frame, width)),
        _ => View::Bars,
    }
}

fn bmp(name: &str) -> Option<Scene> {
    let b = std::fs::read(name).ok()?;
    if b.len() < 54 || &b[..2] != b"BM" {
        return None;
    }
    let u32at = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
    // Compression 0 (BI_RGB) or 3 (BI_BITFIELDS with the default BGRA masks).
    if u32at(30) != 0 && u32at(30) != 3 {
        return None;
    }
    let (offset, w, h) = (u32at(10) as usize, u32at(18) as i32, u32at(22) as i32);
    let bytes = usize::from(u16::from_le_bytes([b[28], b[29]])) / 8;
    if w <= 0 || w > 16384 || h == 0 || h.unsigned_abs() > 16384 || !(3..=4).contains(&bytes) {
        return None;
    }
    let (w, rows) = (w as usize, h.unsigned_abs() as usize);
    let row = (w * bytes).div_ceil(4) * 4;
    if b.len() < offset.checked_add(row * rows)? {
        return None;
    }
    let mut rgb = Vec::with_capacity(w * rows * 3);
    for y in 0..rows {
        // A positive height stores the bottom row first.
        let line = if h > 0 { rows - 1 - y } else { y };
        for x in 0..w {
            let p = offset + line * row + x * bytes;
            rgb.extend_from_slice(&[b[p + 2], b[p + 1], b[p]]);
        }
    }
    Some(Scene::Picture { width: w, height: rows, rgb })
}

/// The scene selected by the `XT5_SCENE` environment variable: `bars` for colour bars, the
/// path of an uncompressed 24- or 32-bit BMP, or a live `video:` or `fifo:` source (see the
/// module documentation). `None` when the variable is unset or the BMP is unreadable; a live
/// source that cannot start falls back to colour bars.
///
/// The variable is read once per process. It is a host input outside the run definition and the
/// checkpoint: a snapshot loads with or without it, and the pictures differ accordingly.
pub fn host_scene() -> Option<&'static Scene> {
    use std::sync::OnceLock;
    static SCENE: OnceLock<Option<Scene>> = OnceLock::new();
    SCENE
        .get_or_init(|| {
            let name = std::env::var("XT5_SCENE").ok().filter(|name| !name.is_empty())?;
            if name == "bars" {
                eprintln!("XT5_SCENE: colour bars");
                return Some(Scene::Bars);
            }
            let live = if let Some(rest) = name.strip_prefix("video:") {
                Some(live::parse_video(rest))
            } else {
                name.strip_prefix("fifo:").map(live::parse_fifo)
            };
            if let Some(spec) = live {
                let started = spec.and_then(|spec| {
                    let feed = live::LiveFeed::start(&spec, &name)?;
                    eprintln!(
                        "XT5_SCENE: {name}: live {}x{} at {} fps; colour bars until the first frame",
                        spec.width, spec.height, spec.fps
                    );
                    Ok(feed)
                });
                return Some(match started {
                    Ok(feed) => Scene::Live(feed),
                    Err(error) => {
                        eprintln!("XT5_SCENE: {name}: {error}; using colour bars");
                        Scene::Bars
                    }
                });
            }
            let scene = bmp(&name);
            match &scene {
                Some(Scene::Picture { width, height, .. }) => {
                    eprintln!("XT5_SCENE: {name} {width}x{height}")
                }
                _ => eprintln!(
                    "XT5_SCENE: {name} is not a readable uncompressed 24/32-bit BMP; using the flat source"
                ),
            }
            scene
        })
        .as_ref()
}

/// Bar colours (R, G, B) left to right.
pub const BARS: [[u8; 3]; 8] = [
    [235, 235, 235],
    [235, 235, 16],
    [16, 235, 235],
    [16, 235, 16],
    [235, 16, 235],
    [235, 16, 16],
    [16, 16, 235],
    [16, 16, 16],
];

/// BT.601 studio-range Y, Cb, Cr of an RGB pixel.
pub fn ycbcr([r, g, b]: [u8; 3]) -> [i32; 3] {
    let (r, g, b) = (i32::from(r), i32::from(g), i32::from(b));
    [
        16 + (66 * r + 129 * g + 25 * b + 128) / 256,
        128 + (-38 * r - 74 * g + 112 * b + 128) / 256,
        128 + (112 * r - 94 * g - 18 * b + 128) / 256,
    ]
}

/// Line `y` of `scene` rendered at `width` x `height` as packed 4:2:2 (Y0, Y1, Cb, Cr) into
/// `row` (`width * 2` bytes; `width` even). `marker` draws a white vertical stripe 16 pixels
/// wide centred on that column, so successive live-view frames show motion. A live scene takes
/// its newest frame for this line alone; [`render_422`] holds one frame for a whole picture.
pub fn render_422_row(
    scene: &Scene,
    y: usize,
    width: usize,
    height: usize,
    marker: Option<usize>,
    row: &mut [u8],
) {
    let (frame, scene) = snapshot(scene);
    row_422(&view(scene, frame.as_deref(), width), y, width, height, marker, row);
}

/// Calls `out` with the sRGB colour of each pixel `x` of line `y` of `view` at `width` x
/// `height`.
fn rgb_line(
    view: &View<'_>,
    y: usize,
    width: usize,
    height: usize,
    marker: Option<usize>,
    mut out: impl FnMut(usize, [u8; 3]),
) {
    let (w, h) = (width, height);
    let bilinear = match view {
        View::Bilinear(sampler) => Some(sampler.row(y, h)),
        _ => None,
    };
    for x in 0..w {
        let pixel = match view {
            View::Bars => {
                if marker.is_some_and(|m| x.abs_diff(m) < 8) {
                    [235, 235, 235]
                } else if y > h * 3 / 4 {
                    let v = (16 + x * 219 / w) as u8;
                    [v, v, v]
                } else {
                    BARS[x * 8 / w]
                }
            }
            View::Nearest { width, height, rgb } => {
                let p = ((y * height / h) * width + x * width / w) * 3;
                [rgb[p], rgb[p + 1], rgb[p + 2]]
            }
            View::Bilinear(_) => bilinear.as_ref().expect("bilinear row")(x),
        };
        out(x, pixel);
    }
}

fn row_422(view: &View<'_>, y: usize, width: usize, height: usize, marker: Option<usize>, row: &mut [u8]) {
    let mut first = [0; 3];
    rgb_line(view, y, width, height, marker, |x, pixel| {
        if x % 2 == 0 {
            first = ycbcr(pixel);
            return;
        }
        let (a, b, pair) = (first, ycbcr(pixel), x / 2);
        row[pair * 4] = a[0].clamp(16, 235) as u8;
        row[pair * 4 + 1] = b[0].clamp(16, 235) as u8;
        row[pair * 4 + 2] = ((a[1] + b[1]) / 2).clamp(16, 240) as u8;
        row[pair * 4 + 3] = ((a[2] + b[2]) / 2).clamp(16, 240) as u8;
    });
}

/// Every line of `scene` at `width` x `height` as sRGB (R, G, B bytes, `width * 3` a line),
/// top first, passed to `line` with its number. A live scene contributes one frame, the newest
/// at the time of the call, to the whole picture. No motion marker is drawn.
pub fn render_rgb_lines(
    scene: &Scene,
    width: usize,
    height: usize,
    mut line: impl FnMut(usize, &[u8]) -> Result<(), String>,
) -> Result<(), String> {
    if width == 0 || height == 0 {
        return Err("empty scene picture".into());
    }
    let (frame, scene) = snapshot(scene);
    let view = view(scene, frame.as_deref(), width);
    let mut row = vec![0u8; width * 3];
    for y in 0..height {
        rgb_line(&view, y, width, height, None, |x, pixel| {
            row[x * 3..x * 3 + 3].copy_from_slice(&pixel)
        });
        line(y, &row)?;
    }
    Ok(())
}

/// The scene of the still data path (sensor RAW of a still capture): the `XT5_SCENE` scene,
/// or colour bars when none is set.
pub fn still_scene() -> &'static Scene {
    static BARS_SCENE: Scene = Scene::Bars;
    host_scene().unwrap_or(&BARS_SCENE)
}

/// Whether the `XT5_STILL_PATH` environment variable selects the modelled still data path
/// (`real`): the X-T5 still RAW channel writes the scene as X-Trans samples, RIFP develops it,
/// RESZ scales it, and [`host_still_422`] no longer supplies the main (larger than 640x480)
/// JPEG encodes. Unset or any other value keeps the encoder shortcut. Read once per process; a
/// host input outside the run definition, like `XT5_SCENE`.
pub fn still_path_real() -> bool {
    use std::sync::OnceLock;
    static REAL: OnceLock<bool> = OnceLock::new();
    *REAL.get_or_init(|| {
        let real = std::env::var("XT5_STILL_PATH").is_ok_and(|v| v == "real");
        if real {
            eprintln!("XT5_STILL_PATH: real (sensor RAW -> RIFP -> RESZ; JPEG shortcut only up to 640x480)");
        }
        real
    })
}

/// Largest encode (in pixels) the host scene still supplies with [`still_path_real`]: the
/// 640x480 preview and 160x120 thumbnail, which come from RZL2 (not modelled).
pub const STILL_PATH_SHORTCUT_PIXELS: usize = 640 * 480;

/// The whole of `scene` at `width` x `height` as packed 4:2:2 lines of `width * 2` bytes,
/// without the motion marker. A live scene contributes its newest frame at the time of the
/// call. `None` for an odd or empty size.
pub fn render_422(scene: &Scene, width: usize, height: usize) -> Option<Vec<u8>> {
    if width == 0 || width & 1 != 0 || height == 0 {
        return None;
    }
    let (frame, scene) = snapshot(scene);
    let view = view(scene, frame.as_deref(), width);
    let mut out = vec![0u8; width * height * 2];
    for (y, row) in out.chunks_exact_mut(width * 2).enumerate() {
        row_422(&view, y, width, height, None, row);
    }
    Some(out)
}

/// The `XT5_SCENE` scene ([`host_scene`]) as a still picture of `width` x `height`, packed
/// 4:2:2; `None` when no scene is set or the size is odd or empty. A live scene gives the
/// newest frame at the moment of the call, so each JPEG encode is a snapshot of the feed.
///
/// With `XT5_STILL_PATH=real` ([`still_path_real`]) it gives `None` for runs larger than
/// [`STILL_PATH_SHORTCUT_PIXELS`], so the main encode reads what the modelled develop chain wrote.
pub fn host_still_422(width: usize, height: usize) -> Option<Vec<u8>> {
    if still_path_real() && width * height > STILL_PATH_SHORTCUT_PIXELS {
        return None;
    }
    render_422(host_scene()?, width, height)
}

/// Host-supplied virtual scene for a packed 4:2:2 output, selected by the `XT5_SCENE` environment
/// variable ([`host_scene`]). Colour bars carry a marker that moves with `frame`; a BMP is scaled
/// (nearest neighbour) to the output; a live scene's newest frame is scaled bilinearly. The
/// layout matches [`paint`]: Y0,Y1,Cb,Cr per pixel pair.
/// Returns `false` when no scene is set, so the caller paints its declared flat source.
pub fn paint_scene(
    cx: &mut Cx<'_>,
    address: u64,
    width: u32,
    height: u32,
    stride: u32,
    frame: u64,
) -> Result<bool, String> {
    let Some(scene) = host_scene() else { return Ok(false) };
    if width == 0
        || width & 1 != 0
        || height == 0
        || width > 4096
        || height > 4096
        || stride < width * 2
        || u64::from(stride) * u64::from(height) > 32 * 1024 * 1024
    {
        return Err("scene geometry exceeds modeled bounds".into());
    }
    address
        .checked_add(u64::from(stride) * u64::from(height))
        .ok_or("YUV DMA address overflow")?;
    let (w, h) = (width as usize, height as usize);
    let marker = (frame % w as u64) as usize * 8 % w;
    let (live, scene) = snapshot(scene);
    let view = view(scene, live.as_deref(), w);
    let mut row = vec![0u8; w * 2];
    for y in 0..h {
        row_422(&view, y, w, h, Some(marker), &mut row);
        cx.ram_write(address + y as u64 * u64::from(stride), &row)?;
    }
    Ok(true)
}

/// Half-size packed422 sampling with the controller's normalized symmetric
/// seven-tap horizontal filter. Co-sited sampling and edge clamping are model
/// assumptions; no camera filter/phase capture has qualified them.
pub fn half_422(
    input: &[u8],
    width: usize,
    height: usize,
    weights: Option<[u8; 4]>,
) -> Result<Vec<u8>, String> {
    if width < 4
        || width & 3 != 0
        || height < 2
        || height & 1 != 0
        || input.len() != width * height * 2
    {
        return Err("invalid packed422 half-size input".into());
    }
    if weights.is_some_and(|w| {
        2 * (u32::from(w[0]) + u32::from(w[1]) + u32::from(w[2])) + u32::from(w[3]) != 256
    }) {
        return Err("MOVRSZ horizontal filter is not normalized".into());
    }
    let taps = weights.map(|w| [w[0], w[1], w[2], w[3], w[2], w[1], w[0]]);
    let sample = |row: usize, channel: usize, x: usize| -> u8 {
        let at = row * width * 2;
        input[at
            + if channel == 0 {
                x / 2 * 4 + x % 2
            } else {
                x * 4 + channel + 1
            }]
    };
    let filter = |row: usize, channel: usize, x: usize| -> u8 {
        let Some(taps) = taps else {
            return sample(row, channel, x);
        };
        let count = if channel == 0 { width } else { width / 2 };
        let sum: u32 = taps
            .into_iter()
            .enumerate()
            .map(|(tap, weight)| {
                let x = (x as isize + tap as isize - 3).clamp(0, count as isize - 1) as usize;
                u32::from(sample(row, channel, x)) * u32::from(weight)
            })
            .sum();
        ((sum + 128) >> 8) as u8
    };
    let mut output = Vec::with_capacity(width * height / 2);
    for y in 0..height / 2 {
        for group in 0..width / 4 {
            output.extend_from_slice(&[
                filter(y * 2, 0, group * 4),
                filter(y * 2, 0, group * 4 + 2),
                filter(y * 2, 1, group * 2),
                filter(y * 2, 2, group * 2),
            ]);
        }
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn half_sampling_keeps_channels_and_applies_programmed_normalized_weights() {
        let mut bytes = Vec::new();
        for row in 0..2 {
            for group in 0..4u8 {
                bytes.extend_from_slice(&[group * 32 + row, group * 32 + 16 + row, 128, 128]);
            }
        }
        let nearest = half_422(&bytes, 8, 2, None).unwrap();
        assert_eq!(nearest, [0, 32, 128, 128, 64, 96, 128, 128]);
        let filtered = half_422(&bytes, 8, 2, Some([0, 0, 64, 128])).unwrap();
        assert_eq!(filtered, [4, 32, 128, 128, 64, 96, 128, 128]);
        assert!(half_422(&bytes, 8, 2, Some([0, 0, 0, 0])).is_err());
    }
}
