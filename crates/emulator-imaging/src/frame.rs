//! Declared fixed-luma Y0,Y1,Cb,Cr input at programmed imaging DMA outputs.
//! This source is a scenario input, not sensor/ISP or coefficient fidelity.
use fujimod_emulator_devices::Cx;

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

/// A host-supplied virtual scene: colour bars or an RGB picture.
pub enum Scene {
    /// Seven colour bars and black over the top three quarters, a grey ramp below.
    Bars,
    /// An uncompressed BMP, top row first, three bytes (R, G, B) per pixel.
    Picture { width: usize, height: usize, rgb: Vec<u8> },
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

/// The scene selected by the `XT5_SCENE` environment variable: `bars` for colour bars, or the
/// path of an uncompressed 24- or 32-bit BMP. `None` when the variable is unset or the file is
/// unreadable.
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
/// wide centred on that column, so successive live-view frames show motion.
pub fn render_422_row(
    scene: &Scene,
    y: usize,
    width: usize,
    height: usize,
    marker: Option<usize>,
    row: &mut [u8],
) {
    let (w, h) = (width, height);
    let pixel = |x: usize| -> [u8; 3] {
        match scene {
            Scene::Bars => {
                if marker.is_some_and(|m| x.abs_diff(m) < 8) {
                    [235, 235, 235]
                } else if y > h * 3 / 4 {
                    let v = (16 + x * 219 / w) as u8;
                    [v, v, v]
                } else {
                    BARS[x * 8 / w]
                }
            }
            Scene::Picture { width, height, rgb } => {
                let p = ((y * height / h) * width + x * width / w) * 3;
                [rgb[p], rgb[p + 1], rgb[p + 2]]
            }
        }
    };
    for pair in 0..w / 2 {
        let (a, b) = (ycbcr(pixel(pair * 2)), ycbcr(pixel(pair * 2 + 1)));
        row[pair * 4] = a[0].clamp(16, 235) as u8;
        row[pair * 4 + 1] = b[0].clamp(16, 235) as u8;
        row[pair * 4 + 2] = ((a[1] + b[1]) / 2).clamp(16, 240) as u8;
        row[pair * 4 + 3] = ((a[2] + b[2]) / 2).clamp(16, 240) as u8;
    }
}

/// The whole of `scene` at `width` x `height` as packed 4:2:2 lines of `width * 2` bytes,
/// without the motion marker. `None` for an odd or empty size.
pub fn render_422(scene: &Scene, width: usize, height: usize) -> Option<Vec<u8>> {
    if width == 0 || width & 1 != 0 || height == 0 {
        return None;
    }
    let mut out = vec![0u8; width * height * 2];
    for (y, row) in out.chunks_exact_mut(width * 2).enumerate() {
        render_422_row(scene, y, width, height, None, row);
    }
    Some(out)
}

/// The `XT5_SCENE` scene ([`host_scene`]) as a still picture of `width` x `height`, packed
/// 4:2:2; `None` when no scene is set or the size is odd or empty.
pub fn host_still_422(width: usize, height: usize) -> Option<Vec<u8>> {
    render_422(host_scene()?, width, height)
}

/// Host-supplied virtual scene for a packed 4:2:2 output, selected by the `XT5_SCENE` environment
/// variable ([`host_scene`]). Colour bars carry a marker that moves with `frame`; a BMP is scaled
/// (nearest neighbour) to the output. The layout matches [`paint`]: Y0,Y1,Cb,Cr per pixel pair.
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
    let mut row = vec![0u8; w * 2];
    for y in 0..h {
        render_422_row(scene, y, w, h, Some(marker), &mut row);
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
