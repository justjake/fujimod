//! Badge text: Inter SemiBold glyph outlines, laid out with the font's
//! advances and kerning and filled as anti-aliased paths.
use ab_glyph::{Font, FontRef, OutlineCurve, Point};
use std::sync::OnceLock;
use tiny_skia::{Path, PathBuilder};

/// Inter 4.1 SemiBold (SIL Open Font License 1.1, assets/fonts/Inter-LICENSE.txt),
/// the 600 weight the web badges request.
const INTER: &[u8] = include_bytes!("../assets/fonts/Inter-SemiBold.ttf");

fn font() -> &'static FontRef<'static> {
    static FONT: OnceLock<FontRef<'static>> = OnceLock::new();
    FONT.get_or_init(|| FontRef::try_from_slice(INTER).expect("bundled Inter font"))
}

/// A line of text set at CSS `font-size` `size` px in a line box `line` px
/// tall, as CSS places it: half the leading above the font's ascent.
pub(crate) struct Line {
    pub(crate) path: Option<Path>,
    pub(crate) width: f32,
}

pub(crate) fn layout(text: &str, left: f32, top: f32, size: f32, line: f32) -> Line {
    let font = font();
    let scale = size / font.units_per_em().expect("scalable font");
    let (ascent, descent) = (
        font.ascent_unscaled() * scale,
        font.descent_unscaled() * scale,
    );
    let baseline = top + (line - (ascent - descent)) / 2. + ascent;
    let mut path = PathBuilder::new();
    let mut pen = 0.;
    let mut previous = None;
    for ch in text.chars() {
        let id = font.glyph_id(ch);
        if let Some(before) = previous {
            pen += font.kern_unscaled(before, id) * scale;
        }
        if let Some(outline) = font.outline(id) {
            let at = |p: Point| (left + pen + p.x * scale, baseline - p.y * scale);
            let mut last: Option<Point> = None;
            for curve in &outline.curves {
                let (start, end) = match *curve {
                    OutlineCurve::Line(a, b) => (a, b),
                    OutlineCurve::Quad(a, _, b) => (a, b),
                    OutlineCurve::Cubic(a, _, _, b) => (a, b),
                };
                if last != Some(start) {
                    if last.is_some() {
                        path.close();
                    }
                    let (x, y) = at(start);
                    path.move_to(x, y);
                }
                match *curve {
                    OutlineCurve::Line(_, b) => {
                        let (x, y) = at(b);
                        path.line_to(x, y);
                    }
                    OutlineCurve::Quad(_, c, b) => {
                        let ((cx, cy), (x, y)) = (at(c), at(b));
                        path.quad_to(cx, cy, x, y);
                    }
                    OutlineCurve::Cubic(_, c1, c2, b) => {
                        let ((ax, ay), (bx, by), (x, y)) = (at(c1), at(c2), at(b));
                        path.cubic_to(ax, ay, bx, by, x, y);
                    }
                }
                last = Some(end);
            }
            if last.is_some() {
                path.close();
            }
        }
        pen += font.h_advance_unscaled(id) * scale;
        previous = Some(id);
    }
    Line {
        path: path.finish(),
        width: pen,
    }
}
