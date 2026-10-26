//! Anti-aliased vector drawing over an RGB frame, through tiny-skia.
use tiny_skia::{
    FillRule, LineCap, LineJoin, Mask, Paint, Path, PathBuilder, Pixmap, PixmapPaint, Stroke,
    Transform,
};

/// An opaque RGB image being annotated. Drawing composites source-over, so
/// the pixmap stays opaque and its premultiplied RGBA is plain RGB.
pub(crate) struct Canvas {
    pixmap: Pixmap,
}
/// Straight RGBA, as the presentation spec writes colors (`#rrggbb[aa]`).
pub(crate) type Color = [u8; 4];

pub(crate) fn color(value: &str, opacity: f32) -> Color {
    let number = u32::from_str_radix(&value[1..], 16).expect("checked presentation color");
    let rgba = if value.len() == 7 {
        (number << 8) | 255
    } else {
        number
    };
    let alpha = f32::from((rgba & 255) as u8) * opacity.clamp(0., 1.);
    [
        (rgba >> 24) as u8,
        (rgba >> 16) as u8,
        (rgba >> 8) as u8,
        alpha.round() as u8,
    ]
}
fn paint(color: Color) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color_rgba8(color[0], color[1], color[2], color[3]);
    paint.anti_alias = true;
    paint
}

impl Canvas {
    pub(crate) fn from_rgb(rgb: &[u8], width: u32, height: u32) -> Self {
        let mut pixmap = Pixmap::new(width, height).expect("nonempty frame");
        for (out, pixel) in pixmap
            .data_mut()
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(rgb.as_chunks::<3>().0.iter())
        {
            out.copy_from_slice(&[pixel[0], pixel[1], pixel[2], 255]);
        }
        Self { pixmap }
    }
    pub(crate) fn width(&self) -> f32 {
        self.pixmap.width() as f32
    }
    pub(crate) fn height(&self) -> f32 {
        self.pixmap.height() as f32
    }
    pub(crate) fn rgb(&self) -> Vec<u8> {
        self.pixmap
            .data()
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2]])
            .collect()
    }
    pub(crate) fn image(
        &mut self,
        frame: &crate::Frame,
        x: f32,
        y: f32,
        width: u32,
        height: u32,
        radius: f32,
    ) -> crate::Result<()> {
        let fitted = frame.fit(width, height)?;
        let source = Self::from_rgb(fitted.pixels(), width, height);
        let mut mask =
            Mask::new(self.pixmap.width(), self.pixmap.height()).ok_or("Invalid panel mask")?;
        let path = rounded_rect(x, y, width as f32, height as f32, radius)
            .ok_or("Invalid display panel")?;
        mask.fill_path(&path, FillRule::Winding, true, Transform::identity());
        self.pixmap.draw_pixmap(
            x.round() as i32,
            y.round() as i32,
            source.pixmap.as_ref(),
            &PixmapPaint::default(),
            Transform::identity(),
            Some(&mask),
        );
        Ok(())
    }
    pub(crate) fn fill(&mut self, path: &Path, color: Color) {
        if color[3] > 0 {
            self.pixmap.fill_path(
                path,
                &paint(color),
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    }
    /// A round-capped, round-joined stroke centered on `path`.
    pub(crate) fn stroke(&mut self, path: &Path, color: Color, width: f32) {
        if color[3] > 0 && width > 0. {
            let stroke = Stroke {
                width,
                line_cap: LineCap::Round,
                line_join: LineJoin::Round,
                ..Stroke::default()
            };
            self.pixmap
                .stroke_path(path, &paint(color), &stroke, Transform::identity(), None);
        }
    }
}

/// A rounded rectangle; `radius` is clamped to half the shorter side.
pub(crate) fn rounded_rect(x: f32, y: f32, width: f32, height: f32, radius: f32) -> Option<Path> {
    let r = radius.min(width / 2.).min(height / 2.).max(0.);
    // Cubic quarter circles: control points at the standard 0.5523 r.
    let k = r * 0.552_284_8;
    let (right, bottom) = (x + width, y + height);
    let mut path = PathBuilder::new();
    path.move_to(x + r, y);
    path.line_to(right - r, y);
    path.cubic_to(right - r + k, y, right, y + r - k, right, y + r);
    path.line_to(right, bottom - r);
    path.cubic_to(
        right,
        bottom - r + k,
        right - r + k,
        bottom,
        right - r,
        bottom,
    );
    path.line_to(x + r, bottom);
    path.cubic_to(x + r - k, bottom, x, bottom - r + k, x, bottom - r);
    path.line_to(x, y + r);
    path.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    path.close();
    path.finish()
}
