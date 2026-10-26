//! Declared grayscale and signed-chroma composition laws. Rasterization is
//! limited to the implemented rectangle shapes.
use fujimod_emulator_devices::exr2::scanout_frame::Frame;
use super::scanout::{Pipe, RectangleComposition};
use fujimod_emulator_devices::exr2::scanout_frame::State as S;
use fujimod_emulator_devices::record::Result;
use sha2::{Digest, Sha256};

pub const PROFILE: &str = "xm1-150-declared-luma-rectangle-overlay-unmeasured";
pub const SIGNED_YCBCR_PROFILE: &str = "exr2-signed-ycbcr-rectangle-overlay-unmeasured";

pub struct Rectangle {
    index: u32,
    left: u32,
    top: u32,
    right: u32,
    bottom: u32,
    thickness: u32,
    luma: u32,
    color: u32,
    rgb: [u8; 3],
    composition: RectangleComposition,
    opacity: u32,
    corners: Option<(u32, u32)>,
}

pub fn decode(pipe: &Pipe, selected: Option<RectangleComposition>) -> Result<Vec<Rectangle>> {
    let r = &pipe.regs;
    let enabled = r.get_or_zero(0x108) & 0x303ff;
    if enabled == 0 {
        return Ok(Vec::new());
    }
    let Some(composition) = selected else {
        return Err("Enabled rectangle/color overlays need an explicit interpretation".into());
    };
    let mut rectangles = Vec::new();
    for index in 0..12 {
        let auxiliary = index >= 10;
        let ordinal = if auxiliary { index - 10 } else { index };
        let bit = if auxiliary { ordinal + 16 } else { index };
        if enabled & (1 << bit) == 0 {
            continue;
        }
        let shift = if auxiliary { 24 + ordinal * 2 } else { index * 2 };
        let start_offset = if auxiliary { 0x10c + ordinal * 8 } else { 0x11c + index * 8 };
        let color_slot = r.get_or_zero(0x174) >> shift & 3;
        let width_slot = r.get_or_zero(0x188) >> shift & 3;
        let color_offset = 0x178 + color_slot * 4;
        let width_offset = 0x16c + width_slot / 2 * 4;
        if [
            start_offset,
            start_offset + 4,
            0x174,
            0x188,
            0x190,
            0x18c,
            color_offset,
            width_offset,
        ]
        .iter()
        .any(|o| !pipe.written.contains(o))
        {
            return Err("Missing rectangle geometry or style registers".into());
        }
        let start = r.get_or_zero(start_offset);
        let end = r.get_or_zero(start_offset + 4);
        if (start | end) & 0xf000f000 != 0 {
            return Err("Unresolved rectangle coordinate controls".into());
        }
        let color = r.get_or_zero(color_offset);
        let rgb = match composition {
            RectangleComposition::DeclaredLuma if color <= 255 => [color as u8; 3],
            RectangleComposition::SignedYCbCr if color <= 0xffffff => {
                // The packed color stores Y | signed Cb << 8 | signed Cr << 16.
                let y = f64::from(color as u8);
                let cb = f64::from((color >> 8) as u8 as i8);
                let cr = f64::from((color >> 16) as u8 as i8);
                let (kr, kb) = (0.299, 0.114);
                let kg = 1.0 - kr - kb;
                [y + 2.0 * (1.0 - kr) * cr,
                 y - 2.0 * kb * (1.0 - kb) / kg * cb - 2.0 * kr * (1.0 - kr) / kg * cr,
                 y + 2.0 * (1.0 - kb) * cb]
                    .map(|v| v.round_ties_even().clamp(0.0, 255.0) as u8)
            }
            _ => return Err("Unresolved rectangle color encoding".into()),
        };
        let corners = if auxiliary {
            let offset = 0x194 + ordinal * 4;
            if !pipe.written.contains(&offset) || r.get_or_zero(offset) & 0xf800f800 != 0 {
                return Err("Missing or unresolved corner-frame inset registers".into());
            }
            let inset = r.get_or_zero(offset);
            Some((inset >> 16, inset & 0x7ff))
        } else { None };
        let rectangle = Rectangle {
            index,
            left: start >> 16,
            top: start & 0xfff,
            right: end >> 16,
            bottom: end & 0xfff,
            thickness: r.get_or_zero(width_offset) >> (width_slot % 2 * 16) & 0xffff,
            luma: color & 255,
            color,
            rgb,
            composition,
            opacity: r.get_or_zero(0x18c) >> ((r.get_or_zero(0x190) >> bit & 1) * 8) & 0xff,
            corners,
        };
        if rectangle.left > rectangle.right || rectangle.top > rectangle.bottom {
            return Err("Reversed rectangle endpoints".into());
        }
        if rectangle.thickness & !0x7ff != 0 || rectangle.opacity > 128 {
            return Err("Unresolved rectangle color, width or alpha controls".into());
        }
        if rectangle.thickness == 0 && rectangle.opacity != 0 {
            return Err("Opaque rectangle has unresolved zero stroke width".into());
        }
        if corners.is_some_and(|(x, y)| (x == 0 || y == 0) && rectangle.opacity != 0) {
            return Err("Opaque corner frame has unresolved zero inset".into());
        }
        rectangles.push(rectangle);
    }
    Ok(rectangles)
}

pub fn compose(frame: &mut Frame, rectangles: &[Rectangle], diagnostic_hashes: bool) {
    if rectangles.is_empty() {
        return;
    }
    frame.state.set(
        "plane_sha256",
        frame.state.get("frame_sha256").unwrap().clone(),
    );
    frame.state.set("rectangle_profile", S::str(match rectangles[0].composition {
        RectangleComposition::DeclaredLuma => PROFILE,
        RectangleComposition::SignedYCbCr => SIGNED_YCBCR_PROFILE,
    }));
    let mut digest = diagnostic_hashes.then(Sha256::new);
    if let Some(digest) = &mut digest {
        digest.update(&frame.raw);
    }
    let mut metadata = Vec::new();
    for r in rectangles {
        for word in [
            r.index,
            r.left,
            r.top,
            r.right,
            r.bottom,
            r.thickness,
            r.color,
            r.opacity,
        ] {
            if let Some(digest) = &mut digest {
                digest.update(word.to_le_bytes());
            }
        }
        if let (Some(digest), Some((x, y))) = (&mut digest, r.corners) {
            digest.update(x.to_le_bytes());
            digest.update(y.to_le_bytes());
        }
        let mut rectangle_metadata = vec![
            ("index", S::Int(r.index.into())),
            (
                "bounds",
                S::dict(vec![
                    ("left", S::Int(r.left.into())),
                    ("top", S::Int(r.top.into())),
                    ("right", S::Int(r.right.into())),
                    ("bottom", S::Int(r.bottom.into())),
                ]),
            ),
            ("thickness", S::Int(r.thickness.into())),
            ("luma", S::Int(r.luma.into())),
            ("opacity", S::Int(r.opacity.into())),
        ];
        if let Some((x, y)) = r.corners {
            rectangle_metadata.push(("kind", S::str("corner_frame")));
            rectangle_metadata.push(("inset", S::List(vec![S::Int(x.into()), S::Int(y.into())])));
        }
        if r.composition == RectangleComposition::SignedYCbCr {
            rectangle_metadata.push(("color_word", S::Int(r.color.into())));
            rectangle_metadata.push(("rgb", S::List(r.rgb.iter().map(|v| S::Int((*v).into())).collect())));
        }
        metadata.push(S::dict(rectangle_metadata));
        if r.opacity == 0 {
            continue;
        }
        if let Some(rgb) = &mut frame.rgb {
            let (left, top, right, bottom) = (r.left, r.top, r.right + 1, r.bottom + 1);
            if let Some((inset_x, inset_y)) = r.corners {
                // The two auxiliary channels carry independent horizontal /
                // vertical corner lengths.
                // Test the union once: overlapping translucent strokes must
                // not blend a pixel twice.
                for y in top.min(frame.height as u32)..bottom.min(frame.height as u32) {
                    for x in left.min(frame.width as u32)..right.min(frame.width as u32) {
                        let horizontal = y - top < r.thickness || bottom - 1 - y < r.thickness;
                        let vertical = x - left < r.thickness || right - 1 - x < r.thickness;
                        let x_corner = x - left < inset_x || right - 1 - x < inset_x;
                        let y_corner = y - top < inset_y || bottom - 1 - y < inset_y;
                        if !(horizontal && x_corner || vertical && y_corner) { continue; }
                        let start = (y as usize * frame.width + x as usize) * 3;
                        for (value, color) in rgb[start..start + 3].iter_mut().zip(r.rgb) {
                            *value = ((u32::from(*value) * (128 - r.opacity)
                                + u32::from(color) * r.opacity + 64) / 128) as u8;
                        }
                    }
                }
                continue;
            }
            let inner_top = (top + r.thickness).min(bottom);
            let inner_bottom = inner_top.max(bottom.saturating_sub(r.thickness));
            let inner_left = (left + r.thickness).min(right);
            let inner_right = inner_left.max(right.saturating_sub(r.thickness));
            for (x0, y0, x1, y1) in [
                (left, top, right, inner_top),
                (left, inner_bottom, right, bottom),
                (left, inner_top, inner_left, inner_bottom),
                (inner_right, inner_top, right, inner_bottom),
            ] {
                let x0 = (x0 as usize).min(frame.width);
                let x1 = (x1 as usize).min(frame.width);
                for y in (y0 as usize).min(frame.height)..(y1 as usize).min(frame.height) {
                    let span = &mut rgb[(y * frame.width + x0) * 3..(y * frame.width + x1) * 3];
                    for pixel in span.chunks_exact_mut(3) {
                        for (value, color) in pixel.iter_mut().zip(r.rgb) {
                            *value = ((u32::from(*value) * (128 - r.opacity)
                                + u32::from(color) * r.opacity + 64) / 128) as u8;
                        }
                    }
                }
            }
        }
    }
    frame.state.set("rectangles", S::List(metadata));
    frame.state.set(
        "frame_sha256",
        digest.map_or(S::None, |d| S::str(&format!("{:x}", d.finalize()))),
    );
    if let Some(S::List(mut limits)) = frame.state.get("limitations").cloned() {
        if rectangles.iter().any(|r| r.corners.is_some()) {
            limits.push(S::str("Auxiliary corner-frame inset lengths, inclusive endpoints and topmost order are declared, unmeasured interpretations"));
        }
        limits.push(S::str(if rectangles[0].composition == RectangleComposition::DeclaredLuma {
            "Rectangle grayscale, inclusive inner strokes, canvas clipping and topmost order are declared, unmeasured interpretations"
        } else {
            "Signed-chroma BT.601 color conversion, inclusive inner strokes, canvas clipping and topmost order are declared, unmeasured interpretations"
        }));
        frame.state.set("limitations", S::List(limits));
    }
}
