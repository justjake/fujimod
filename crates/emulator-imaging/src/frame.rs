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

/// Holds the frame one render draws, so every row comes from one frame: the newest frame of a
/// live scene, or, while the virtual lens is out of focus ([`crate::focus::blur`]), the scene
/// blurred by the current blur circle ([`defocused`]). `None` for a sharp static scene.
fn snapshot(scene: &Scene) -> (Option<std::sync::Arc<live::RgbFrame>>, &Scene) {
    let frame = match scene {
        Scene::Live(feed) => feed.latest(),
        _ => None,
    };
    let blur = crate::focus::blur();
    if blur > 0. {
        if let Some(blurred) = defocused(scene, frame.as_deref(), blur) {
            return (Some(blurred), scene);
        }
    }
    (frame, scene)
}

fn view<'a>(scene: &'a Scene, frame: Option<&'a live::RgbFrame>, width: usize) -> View<'a> {
    match (scene, frame) {
        // A live frame or a defocused raster ([`snapshot`]).
        (_, Some(frame)) => View::Bilinear(live::Sampler::new(frame, width)),
        (Scene::Picture { width: pw, height: ph, rgb }, None) => {
            View::Nearest { width: *pw, height: *ph, rgb }
        }
        _ => View::Bars,
    }
}

/// Widest raster a defocused scene is blurred at; wider pictures are reduced first (the blur
/// removes the detail the reduction drops).
const DEFOCUS_WIDTH: usize = 1920;

/// `scene` as a raster no wider than [`DEFOCUS_WIDTH`], without blur and without the motion
/// marker: colour bars at 960x640, a picture at its own size or reduced (nearest neighbour), a
/// live scene's frame (`live`) or colour bars before its first frame.
fn raster(scene: &Scene, live: Option<&live::RgbFrame>) -> live::RgbFrame {
    let (source, width, height) = match (scene, live) {
        (_, Some(frame)) => (View::Nearest { width: frame.width, height: frame.height, rgb: &frame.rgb }, frame.width, frame.height),
        (Scene::Picture { width, height, rgb }, None) => {
            (View::Nearest { width: *width, height: *height, rgb }, *width, *height)
        }
        _ => (View::Bars, 960, 640),
    };
    let (w, h) = if width > DEFOCUS_WIDTH {
        (DEFOCUS_WIDTH, (height * DEFOCUS_WIDTH / width).max(1))
    } else {
        (width, height)
    };
    let mut rgb = vec![0u8; w * h * 3];
    for (y, row) in rgb.chunks_exact_mut(w * 3).enumerate() {
        rgb_line(&source, y, w, h, None, |x, pixel| row[x * 3..x * 3 + 3].copy_from_slice(&pixel));
    }
    live::RgbFrame { width: w, height: h, rgb }
}

/// `scene` blurred by a blur circle of `blur` picture widths ([`crate::focus`]); `None` when
/// the circle is under one pixel of the working raster (the scene is drawn sharp). The newest
/// result for a static scene is kept, because live view paints it again for every frame while
/// the lens stands still.
fn defocused(
    scene: &Scene,
    live: Option<&live::RgbFrame>,
    blur: f64,
) -> Option<std::sync::Arc<live::RgbFrame>> {
    use std::sync::{Arc, Mutex};
    static KEPT: Mutex<Option<((usize, usize, usize), Arc<live::RgbFrame>)>> = Mutex::new(None);
    // A static scene is identified by its storage; a live frame is never kept.
    let identity = match (scene, live) {
        (_, Some(_)) => None,
        (Scene::Picture { width, rgb, .. }, None) => Some((rgb.as_ptr() as usize, *width)),
        _ => Some((0, 0)),
    };
    let width = match (scene, live) {
        (_, Some(frame)) => frame.width,
        (Scene::Picture { width, .. }, None) => *width,
        _ => 960,
    }
    .min(DEFOCUS_WIDTH);
    // Whole pixels of the working raster: nearby lens positions share one raster.
    let diameter = (blur * width as f64).round() as usize;
    if diameter < 2 {
        // `blur_rgb` leaves such a raster unchanged.
        return None;
    }
    let key = identity.map(|(storage, w)| (storage, w, diameter));
    if let Some(key) = key {
        if let Some((kept, frame)) = KEPT.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            if *kept == key {
                return Some(frame.clone());
            }
        }
    }
    let mut frame = raster(scene, live);
    crate::focus::blur_rgb(&mut frame.rgb, frame.width, frame.height, diameter as f64);
    let frame = Arc::new(frame);
    if let Some(key) = key {
        *KEPT.lock().unwrap_or_else(|e| e.into_inner()) = Some((key, frame.clone()));
    }
    Some(frame)
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

/// Rows of `view` at output height `height` with the same key are identical for one marker:
/// colour bars depend only on whether the row lies in the grey-ramp band, a nearest-neighbour
/// picture only on its source row. A live scene has no key (every row is rendered).
fn row_key(view: &View<'_>, y: usize, height: usize) -> Option<usize> {
    match view {
        View::Bars => Some(usize::from(y > height * 3 / 4)),
        View::Nearest { height: source, .. } => Some(y * source / height),
        View::Bilinear(_) => None,
    }
}

/// Calls `emit` with every line of `view` at `width` x `height` as packed 4:2:2 (`width * 2`
/// bytes), top first. A line whose [`row_key`] matches the previous line's is not rendered
/// again: live view paints two MOVDC outputs every frame and a movie encodes the host scene at
/// 3840x2160 every frame, where colour bars have only two distinct lines.
fn rows_422(
    view: &View<'_>,
    width: usize,
    height: usize,
    marker: Option<usize>,
    emit: impl FnMut(usize, &[u8]) -> Result<(), String>,
) -> Result<(), String> {
    rows_422_mapped(view, width, height, marker, None, emit)
}

/// [`rows_422`] with every sRGB byte mapped through `map` first ([`exposure_map`]).
fn rows_422_mapped(
    view: &View<'_>,
    width: usize,
    height: usize,
    marker: Option<usize>,
    map: Option<&[u8; 256]>,
    mut emit: impl FnMut(usize, &[u8]) -> Result<(), String>,
) -> Result<(), String> {
    let mut row = vec![0u8; width * 2];
    let mut previous = None;
    for y in 0..height {
        let key = row_key(view, y, height);
        if key.is_none() || key != previous {
            row_422_mapped(view, y, width, height, marker, map, &mut row);
        }
        previous = key;
        emit(y, &row)?;
    }
    Ok(())
}

fn row_422(view: &View<'_>, y: usize, width: usize, height: usize, marker: Option<usize>, row: &mut [u8]) {
    row_422_mapped(view, y, width, height, marker, None, row);
}

fn row_422_mapped(
    view: &View<'_>,
    y: usize,
    width: usize,
    height: usize,
    marker: Option<usize>,
    map: Option<&[u8; 256]>,
    row: &mut [u8],
) {
    let mut first = [0; 3];
    rgb_line(view, y, width, height, marker, |x, pixel| {
        let pixel = match map {
            Some(map) => pixel.map(|v| map[usize::from(v)]),
            None => pixel,
        };
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

/// The sRGB byte of each sRGB byte after its linear level is scaled by `exposure` (relative to
/// the reference, [`crate::exposure::Exposure::relative`]) and clipped at white; `None` at the
/// reference exposure (no change).
pub fn exposure_map(exposure: f64) -> Option<[u8; 256]> {
    use crate::xtrans::{srgb_byte, srgb_to_linear};
    if !exposure.is_finite() || exposure <= 0. || exposure == 1. {
        return None;
    }
    Some(std::array::from_fn(|v| srgb_byte(srgb_to_linear(v as u8) * exposure)))
}

/// Every line of `scene` at `width` x `height` as sRGB (R, G, B bytes, `width * 3` a line),
/// top first, passed to `line` with its number. A live scene contributes one frame, the newest
/// at the time of the call, to the whole picture. No motion marker is drawn.
pub fn render_rgb_lines(
    scene: &Scene,
    width: usize,
    height: usize,
    line: impl FnMut(usize, &[u8]) -> Result<(), String>,
) -> Result<(), String> {
    render_rgb_lines_from(snapshot(scene), width, height, line)
}

/// [`render_rgb_lines`] of the scene in focus, whatever the lens position: the input of
/// models that apply the defocus themselves (the focus statistics, which need a finer blur
/// than the picture raster holds).
pub fn render_sharp_rgb_lines(
    scene: &Scene,
    width: usize,
    height: usize,
    line: impl FnMut(usize, &[u8]) -> Result<(), String>,
) -> Result<(), String> {
    let frame = match scene {
        Scene::Live(feed) => feed.latest(),
        _ => None,
    };
    render_rgb_lines_from((frame, scene), width, height, line)
}

fn render_rgb_lines_from(
    (frame, scene): (Option<std::sync::Arc<live::RgbFrame>>, &Scene),
    width: usize,
    height: usize,
    mut line: impl FnMut(usize, &[u8]) -> Result<(), String>,
) -> Result<(), String> {
    if width == 0 || height == 0 {
        return Err("empty scene picture".into());
    }
    let view = view(scene, frame.as_deref(), width);
    let mut row = vec![0u8; width * 3];
    let mut previous = None;
    for y in 0..height {
        let key = row_key(&view, y, height);
        if key.is_none() || key != previous {
            rgb_line(&view, y, width, height, None, |x, pixel| {
                row[x * 3..x * 3 + 3].copy_from_slice(&pixel)
            });
        }
        previous = key;
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
/// RESZ scales it to the main picture and RZL2 to the preview and thumbnail, and
/// [`host_still_422`] no longer supplies any JPEG encode. Unset or any other value keeps the
/// encoder shortcut. Read once per process; a host input outside the run definition, like
/// `XT5_SCENE`.
pub fn still_path_real() -> bool {
    use std::sync::OnceLock;
    static REAL: OnceLock<bool> = OnceLock::new();
    *REAL.get_or_init(|| {
        let real = std::env::var("XT5_STILL_PATH").is_ok_and(|v| v == "real");
        if real {
            eprintln!("XT5_STILL_PATH: real (sensor RAW -> RIFP -> RESZ / RZL2; no JPEG shortcut)");
        }
        real
    })
}

/// Whether the `XT5_MOVIE_PATH` environment variable selects the modelled movie data path
/// (`real`): the H.264 encoder reads the guest Y/C planes instead of the host scene, and the
/// movie develop chain (RIFP inline YC -> RESZ two-plane 4:2:0 -> TNR) moves the MOVDC
/// picture into the encoder input. Unset or any other value keeps the encoder shortcut. Read
/// once per process; a host input outside the run definition, like `XT5_SCENE`.
pub fn movie_path_real() -> bool {
    use std::sync::OnceLock;
    static REAL: OnceLock<bool> = OnceLock::new();
    *REAL.get_or_init(|| {
        let real = std::env::var("XT5_MOVIE_PATH").is_ok_and(|v| v == "real");
        if real {
            eprintln!(
                "XT5_MOVIE_PATH: real (MOVDC -> RIFP inline -> RESZ -> TNR -> H.264 encoder; no scene shortcut)"
            );
        }
        real
    })
}

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
    let mut lines = out.chunks_exact_mut(width * 2);
    rows_422(&view, width, height, None, |_, row| {
        lines.next().expect("one output line per row").copy_from_slice(row);
        Ok(())
    })
    .ok()?;
    Some(out)
}

/// The `XT5_SCENE` scene ([`host_scene`]) as a still picture of `width` x `height`, packed
/// 4:2:2; `None` when no scene is set or the size is odd or empty. A live scene gives the
/// newest frame at the moment of the call, so each JPEG encode is a snapshot of the feed.
///
/// With `XT5_STILL_PATH=real` ([`still_path_real`]) it gives `None` for every run, so the main,
/// preview and thumbnail encodes read what the modelled develop chain wrote.
pub fn host_still_422(width: usize, height: usize) -> Option<Vec<u8>> {
    host_still_422_with(still_path_real(), width, height)
}

/// [`host_still_422`] with the still path choice given: `real` gives `None`.
fn host_still_422_with(real: bool, width: usize, height: usize) -> Option<Vec<u8>> {
    if real {
        return None;
    }
    render_422(host_scene()?, width, height)
}

/// Memory layout of a YCbCr DMA output picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum YuvLayout {
    /// One plane, Y0, Y1, Cb, Cr per horizontal pixel pair: `width * 2` bytes a line.
    Packed422,
    /// 8-bit two-plane 4:2:0 (NV12): a Y plane of one byte a pixel at the output address, and
    /// at `chroma` an interleaved Cb, Cr plane with one pair per 2x2 pixel block, `width` bytes
    /// a line and `ceil(height / 2)` lines, `chroma_stride` bytes apart.
    TwoPlane420 { chroma: u64, chroma_stride: u32 },
}

/// Largest modeled output width and height, and the largest total of programmed plane bytes.
const MAX_SIDE: u32 = 8192;
const MAX_BYTES: u64 = 128 * 1024 * 1024;

/// Checks a `width` x `height` output in `layout` with luma (or packed) `stride` at `address`
/// against the modeled bounds. The error names the whole geometry.
fn check_geometry(address: u64, width: u32, height: u32, stride: u32, layout: YuvLayout) -> Result<(), String> {
    let sides = width != 0 && width & 1 == 0 && height != 0 && width <= MAX_SIDE && height <= MAX_SIDE;
    let luma_bytes = u64::from(stride) * u64::from(height);
    let (fits, chroma_end) = match layout {
        YuvLayout::Packed422 => (sides && stride >= width * 2 && luma_bytes <= MAX_BYTES, Some(0)),
        YuvLayout::TwoPlane420 { chroma, chroma_stride } => {
            let chroma_bytes = u64::from(chroma_stride) * u64::from(height.div_ceil(2));
            (
                sides
                    && chroma != 0
                    && stride >= width
                    && chroma_stride >= width
                    && luma_bytes + chroma_bytes <= MAX_BYTES,
                chroma.checked_add(chroma_bytes),
            )
        }
    };
    let describe = || match layout {
        YuvLayout::Packed422 => format!("{width}x{height} stride {stride:#x} at {address:#x}"),
        YuvLayout::TwoPlane420 { chroma, chroma_stride } => format!(
            "{width}x{height} two-plane 4:2:0 luma stride {stride:#x} at {address:#x}, \
             chroma stride {chroma_stride:#x} at {chroma:#x}"
        ),
    };
    if !fits {
        return Err(format!("scene geometry exceeds modeled bounds: {}", describe()));
    }
    if address.checked_add(luma_bytes).is_none() || chroma_end.is_none() {
        return Err(format!("YUV DMA address overflow: {}", describe()));
    }
    Ok(())
}

/// Writes packed 4:2:2 lines (Y0, Y1, Cb, Cr; `emit` order, top first) as two-plane 4:2:0:
/// the Y bytes of each line to the luma plane, and the Cb, Cr of each pair of lines, averaged
/// (rounding up, like the codec's packed-to-I420 conversion), to one chroma line. The last
/// line of an odd height is its own pair.
struct TwoPlaneWriter {
    address: u64,
    stride: u32,
    chroma: u64,
    chroma_stride: u32,
    height: usize,
    even: Vec<u8>,
    line: Vec<u8>,
}
impl TwoPlaneWriter {
    fn new(address: u64, stride: u32, chroma: u64, chroma_stride: u32, width: usize, height: usize) -> Self {
        Self { address, stride, chroma, chroma_stride, height, even: vec![0; width * 2], line: vec![0; width] }
    }
    fn row(&mut self, cx: &mut Cx<'_>, y: usize, packed: &[u8]) -> Result<(), String> {
        for (pair, px) in packed.chunks_exact(4).enumerate() {
            self.line[pair * 2] = px[0];
            self.line[pair * 2 + 1] = px[1];
        }
        cx.ram_write(self.address + y as u64 * u64::from(self.stride), &self.line)?;
        if y.is_multiple_of(2) {
            self.even.copy_from_slice(packed);
            if y + 1 < self.height {
                return Ok(());
            }
        }
        for (pair, (a, b)) in self.even.chunks_exact(4).zip(packed.chunks_exact(4)).enumerate() {
            self.line[pair * 2] = (u16::from(a[2]) + u16::from(b[2])).div_ceil(2) as u8;
            self.line[pair * 2 + 1] = (u16::from(a[3]) + u16::from(b[3])).div_ceil(2) as u8;
        }
        cx.ram_write(self.chroma + (y / 2) as u64 * u64::from(self.chroma_stride), &self.line)
    }
}

/// The declared flat source ([`paint`]) in any [`YuvLayout`]: a packed 4:2:2 output goes
/// through [`paint`] unchanged; a two-plane 4:2:0 output gets Y = `luma` and Cb = Cr = 128.
pub fn paint_layout(
    cx: &mut Cx<'_>,
    address: u64,
    width: u32,
    height: u32,
    stride: u32,
    layout: YuvLayout,
    luma: u8,
) -> Result<(), String> {
    let YuvLayout::TwoPlane420 { chroma, chroma_stride } = layout else {
        return paint(cx, address, width, height, stride, luma);
    };
    if !(16..=235).contains(&luma) {
        return Err("fixed YUV source luma is outside 16..=235".into());
    }
    check_geometry(address, width, height, stride, layout)?;
    let y_line = vec![luma; width as usize];
    for y in 0..u64::from(height) {
        cx.ram_write(address + y * u64::from(stride), &y_line)?;
    }
    let c_line = vec![128; width as usize];
    for y in 0..u64::from(height.div_ceil(2)) {
        cx.ram_write(chroma + y * u64::from(chroma_stride), &c_line)?;
    }
    Ok(())
}

/// Host-supplied virtual scene for a YCbCr output, selected by the `XT5_SCENE` environment
/// variable ([`host_scene`]). Colour bars carry a marker that moves with `frame`; a BMP is scaled
/// (nearest neighbour) to the output; a live scene's newest frame is scaled bilinearly. A packed
/// 4:2:2 output matches [`paint`]: Y0,Y1,Cb,Cr per pixel pair. A two-plane 4:2:0 output gets
/// the same picture as a Y plane and a Cb,Cr plane ([`YuvLayout::TwoPlane420`]).
/// Returns `false` when no scene is set, so the caller paints its declared flat source.
pub fn paint_scene(
    cx: &mut Cx<'_>,
    address: u64,
    width: u32,
    height: u32,
    stride: u32,
    layout: YuvLayout,
    frame: u64,
) -> Result<bool, String> {
    paint_scene_exposed(cx, address, width, height, stride, layout, frame, 1.)
}

/// [`paint_scene`] with the scene's linear levels scaled by `exposure` (relative to the
/// reference exposure, [`crate::exposure::Exposure::relative`]) and clipped at white: the live
/// view of a sensor whose exposure follows the firmware's programming.
#[allow(clippy::too_many_arguments)]
pub fn paint_scene_exposed(
    cx: &mut Cx<'_>,
    address: u64,
    width: u32,
    height: u32,
    stride: u32,
    layout: YuvLayout,
    frame: u64,
    exposure: f64,
) -> Result<bool, String> {
    let Some(scene) = host_scene() else { return Ok(false) };
    check_geometry(address, width, height, stride, layout)?;
    // Live view follows the focus motor: every frame takes the lens position of its time.
    crate::focus::sample(cx.now());
    let (w, h) = (width as usize, height as usize);
    let marker = (frame % w as u64) as usize * 8 % w;
    let (live, scene) = snapshot(scene);
    let view = view(scene, live.as_deref(), w);
    let map = exposure_map(exposure);
    match layout {
        YuvLayout::Packed422 => rows_422_mapped(&view, w, h, Some(marker), map.as_ref(), |y, row| {
            cx.ram_write(address + y as u64 * u64::from(stride), row)
        })?,
        YuvLayout::TwoPlane420 { chroma, chroma_stride } => {
            let mut writer = TwoPlaneWriter::new(address, stride, chroma, chroma_stride, w, h);
            rows_422_mapped(&view, w, h, Some(marker), map.as_ref(), |y, row| writer.row(cx, y, row))?
        }
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
    /// The live-view exposure map is the identity at the reference, brightens linear levels
    /// in proportion above it and clips white.
    #[test]
    fn the_exposure_map_scales_linear_levels_and_clips() {
        assert!(exposure_map(1.).is_none());
        assert!(exposure_map(0.).is_none());
        let double = exposure_map(2.).unwrap();
        assert_eq!((double[0], double[255]), (0, 255));
        // sRGB 128 is linear 0.216; doubled 0.431 is sRGB 176.
        assert_eq!(double[128], 176);
        assert_eq!(double[200], 255);
        let half = exposure_map(0.5).unwrap();
        assert_eq!(half[255], 188);
        let mut a = vec![0u8; 16];
        let view = View::Bars;
        rows_422_mapped(&view, 8, 1, None, Some(&std::array::from_fn(|v| v as u8)), |_, row| {
            a.copy_from_slice(row);
            Ok(())
        })
        .unwrap();
        let mut b = vec![0u8; 16];
        row_422(&view, 0, 8, 1, None, &mut b);
        assert_eq!(a, b);
    }
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

    /// Reusing rows with the same key gives the same lines as rendering every row.
    #[test]
    fn reused_rows_match_rows_rendered_one_by_one() {
        let rgb: Vec<u8> = (0..5 * 3 * 3).map(|n| (n * 37 % 251) as u8).collect();
        let picture = Scene::Picture { width: 5, height: 3, rgb };
        for scene in [&Scene::Bars, &picture] {
            for (width, height) in [(960, 640), (10, 7), (4, 2), (2, 1)] {
                let view = view(scene, None, width);
                for marker in [None, Some(3)] {
                    let mut lines = Vec::new();
                    rows_422(&view, width, height, marker, |y, row| {
                        lines.push((y, row.to_vec()));
                        Ok(())
                    })
                    .unwrap();
                    assert_eq!(lines.len(), height);
                    for (y, line) in lines {
                        let mut expected = vec![0u8; width * 2];
                        row_422(&view, y, width, height, marker, &mut expected);
                        assert_eq!(line, expected, "{width}x{height} row {y} marker {marker:?}");
                    }
                }
                let mut rgb_rows = Vec::new();
                render_rgb_lines(scene, width, height, |y, row| {
                    rgb_rows.push((y, row.to_vec()));
                    Ok(())
                })
                .unwrap();
                for (y, row) in rgb_rows {
                    let mut expected = vec![0u8; width * 3];
                    rgb_line(&view, y, width, height, None, |x, pixel| {
                        expected[x * 3..x * 3 + 3].copy_from_slice(&pixel)
                    });
                    assert_eq!(row, expected, "{width}x{height} RGB row {y}");
                }
                let whole = render_422(scene, width, height).unwrap();
                for (y, line) in whole.chunks_exact(width * 2).enumerate() {
                    let mut expected = vec![0u8; width * 2];
                    row_422(&view, y, width, height, None, &mut expected);
                    assert_eq!(line, expected, "{width}x{height} render_422 row {y}");
                }
            }
        }
    }

    /// X-T5 FHD movie MOVDC channel 2 (1920x1080, stride 0xc00) fits as two-plane 4:2:0 but
    /// not as packed 4:2:2, and every refusal names the geometry.
    #[test]
    fn two_plane_geometry_accepts_the_fhd_movie_output_and_names_refusals() {
        let two_plane = YuvLayout::TwoPlane420 { chroma: 0x1_cdf0_f000, chroma_stride: 0xc00 };
        assert_eq!(check_geometry(0x9c24_3000, 1920, 1080, 0xc00, two_plane), Ok(()));
        let packed = check_geometry(0x9c24_3000, 1920, 1080, 0xc00, YuvLayout::Packed422).unwrap_err();
        assert!(packed.contains("1920x1080 stride 0xc00 at 0x9c243000"), "{packed}");
        assert_eq!(check_geometry(0x9c24_3000, 960, 640, 0x780, YuvLayout::Packed422), Ok(()));
        for (width, height, stride, chroma, chroma_stride) in [
            (1920, 1080, 1918, 0x1000, 0xc00),
            (1920, 1080, 0xc00, 0x1000, 1918),
            (1920, 1080, 0xc00, 0, 0xc00),
            (8194, 2, 0x4000, 0x1000, 0x4000),
            (8192, 8192, 0x4000, 0x1000, 0x4000),
            (1919, 1080, 0xc00, 0x1000, 0xc00),
        ] {
            let layout = YuvLayout::TwoPlane420 { chroma, chroma_stride };
            let error = check_geometry(0x1000, width, height, stride, layout).unwrap_err();
            assert!(error.contains(&format!("{width}x{height} two-plane 4:2:0")), "{error}");
        }
        let overflow = YuvLayout::TwoPlane420 { chroma: u64::MAX - 8, chroma_stride: 16 };
        assert!(check_geometry(0x1000, 16, 4, 16, overflow).unwrap_err().contains("overflow"));
    }

    /// The two-plane writer gives the packed lines' Y bytes and, per pair of lines, their
    /// Cb,Cr averaged rounding up; an odd last line is its own pair.
    #[test]
    fn two_plane_writer_splits_packed_lines_into_luma_and_half_height_chroma() {
        use fujimod_emulator_devices::test_host::recording::RecordingHost;
        const BASE: u64 = 0x1000;
        let (width, height, stride, chroma_stride) = (8usize, 5usize, 12u32, 16u32);
        let chroma = BASE + 0x100;
        let mut host = RecordingHost { ram_base: BASE, ram: vec![0x55; 0x200], ..Default::default() };
        let lines: Vec<Vec<u8>> = (0..height as u8)
            .map(|y| (0..width as u8 / 2).flat_map(|p| [y * 10 + p, y * 10 + p + 1, 100 + y, 200 - y]).collect())
            .collect();
        let mut writer = TwoPlaneWriter::new(BASE, stride, chroma, chroma_stride, width, height);
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        for (y, line) in lines.iter().enumerate() {
            writer.row(&mut cx, y, line).unwrap();
        }
        for (y, line) in lines.iter().enumerate() {
            let at = y * stride as usize;
            let luma: Vec<u8> = line.chunks_exact(4).flat_map(|p| [p[0], p[1]]).collect();
            assert_eq!(&host.ram[at..at + width], &luma[..], "luma line {y}");
            assert!(host.ram[at + width..at + stride as usize].iter().all(|v| *v == 0x55));
        }
        for (row, (cb, cr)) in [(101, 200), (103, 198), (104, 196)].into_iter().enumerate() {
            let at = 0x100 + row * chroma_stride as usize;
            assert_eq!(&host.ram[at..at + width], &[cb, cr].repeat(width / 2)[..], "chroma line {row}");
            assert!(host.ram[at + width..at + chroma_stride as usize].iter().all(|v| *v == 0x55));
        }
        assert!(host.ram[0x100 + 3 * chroma_stride as usize..].iter().all(|v| *v == 0x55));
    }

    /// The modelled still path gives the JPEG encoder no host picture for any encode (main,
    /// 640x480 preview, 160x120 thumbnail); without it the host scene, if any, is rendered.
    #[test]
    fn the_real_still_path_supplies_no_encode_and_the_shortcut_renders_the_scene() {
        for (width, height) in [(7728, 5152), (640, 480), (160, 120)] {
            assert_eq!(host_still_422_with(true, width, height), None);
            assert_eq!(
                host_still_422_with(false, width, height),
                host_scene().and_then(|scene| render_422(scene, width, height))
            );
        }
    }

    /// Squared horizontal differences of the first channel: a sharpness measure.
    fn sharpness(frame: &live::RgbFrame) -> u64 {
        frame
            .rgb
            .chunks_exact(frame.width * 3)
            .map(|row| {
                (1..frame.width)
                    .map(|x| u64::from(row[x * 3].abs_diff(row[x * 3 - 3])).pow(2))
                    .sum::<u64>()
            })
            .sum()
    }

    /// A defocused scene is the scene's raster blurred by the blur circle: less sharp for a
    /// larger circle, the same size, and drawn through the bilinear view for every scene kind.
    #[test]
    fn a_defocused_scene_is_its_blurred_raster() {
        let checker: Vec<u8> = (0..64 * 16)
            .flat_map(|n| if (n % 64) / 8 % 2 == 0 { [230u8; 3] } else { [20u8; 3] })
            .collect();
        let picture = Scene::Picture { width: 64, height: 16, rgb: checker };
        for scene in [&Scene::Bars, &picture] {
            let sharp = raster(scene, None);
            let soft = defocused(scene, None, 0.04).unwrap();
            let softer = defocused(scene, None, 0.16).unwrap();
            assert_eq!((soft.width, soft.height), (sharp.width, sharp.height));
            assert!(sharpness(&soft) < sharpness(&sharp));
            assert!(sharpness(&softer) < sharpness(&soft));
            // The kept raster is reused for the same scene and circle.
            assert!(std::sync::Arc::ptr_eq(
                &defocused(scene, None, 0.16).unwrap(),
                &defocused(scene, None, 0.16).unwrap()
            ));
            assert!(matches!(view(scene, Some(&softer), 32), View::Bilinear(_)));
            // A circle under one raster pixel leaves the scene sharp.
            assert!(defocused(scene, None, 0.0001).is_none());
        }
        assert!(matches!(view(&picture, None, 32), View::Nearest { .. }));
        assert!(matches!(view(&Scene::Bars, None, 32), View::Bars));
        // A wide picture is reduced before the blur.
        let wide = Scene::Picture { width: 4000, height: 2, rgb: vec![9; 4000 * 2 * 3] };
        let reduced = defocused(&wide, None, 0.01).unwrap();
        assert_eq!((reduced.width, reduced.height), (DEFOCUS_WIDTH, 1));
    }
}
