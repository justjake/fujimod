//! Input badges, drawn as the web overlay draws them from the same presentation
//! spec: a rounded plate with a border, and the control's vector icon (24-unit
//! box, 2-unit round strokes) or its label in Inter SemiBold.
use crate::{
    CONTROLS, Frame, Result,
    canvas::{self, Canvas},
    text,
};
use serde::Deserialize;
use std::{collections::BTreeMap, sync::OnceLock};
use tiny_skia::PathBuilder;

#[derive(Deserialize)]
struct Control {
    label: String,
    text: Option<String>,
    icon: Option<String>,
}
#[derive(Deserialize)]
struct Shape {
    #[serde(default)]
    lines: Vec<Vec<[f32; 2]>>,
    circle: Option<[f32; 3]>,
}
#[derive(Deserialize)]
struct Style {
    font_size: f32,
    icon_size: f32,
    padding_x: f32,
    padding_y: f32,
    border_width: f32,
    gap: f32,
    bottom: f32,
    radius: f32,
    foreground: String,
    background: String,
    border: String,
    release_ms: u32,
}
#[derive(Deserialize)]
struct Spec {
    controls: BTreeMap<String, Control>,
    icons: BTreeMap<String, Shape>,
    badge: Style,
}
fn spec() -> &'static Spec {
    static SPEC: OnceLock<Spec> = OnceLock::new();
    SPEC.get_or_init(|| serde_json::from_str(CONTROLS).expect("checked control presentation"))
}
pub fn check(name: &str) -> Result<()> {
    if spec().controls.contains_key(name) {
        Ok(())
    } else {
        Err(format!("Missing input presentation: {name}"))
    }
}
pub fn fade_frames(fps: u32) -> u64 {
    (f64::from(fps) * f64::from(spec().badge.release_ms) / 1000.)
        .round()
        .max(1.) as u64
}
pub fn draw(frame: &Frame, labels: &[(String, u8)]) -> Vec<u8> {
    let mut canvas = Canvas::from_rgb(frame.pixels(), frame.width(), frame.height());
    draw_on(&mut canvas, labels);
    canvas.rgb()
}
/// The badge stack, newest at the bottom right, over existing pixels.
pub(crate) fn draw_on(canvas: &mut Canvas, labels: &[(String, u8)]) {
    if labels.is_empty() {
        return;
    }
    let spec = spec();
    let style = &spec.badge;
    let height = style.icon_size + 2. * style.padding_y + 2. * style.border_width;
    let count = labels.len() as f32;
    let mut y = canvas.height() - style.bottom - height * count - style.gap * (count - 1.);
    for (name, alpha) in labels {
        let opacity = f32::from(*alpha) / 255.;
        let item = &spec.controls[name];
        let label = item.text.as_ref().unwrap_or(&item.label);
        let top = y + style.padding_y + style.border_width;
        let line = item
            .icon
            .is_none()
            .then(|| text::layout(label, 0., top, style.font_size, style.icon_size));
        let content = line.as_ref().map_or(style.icon_size, |line| line.width);
        let width = content + 2. * style.padding_x + 2. * style.border_width;
        let x = canvas.width() - width - 12.;
        // The border lies inside the plate's bounds, as a CSS border does.
        let inset = style.border_width / 2.;
        if let Some(plate) = canvas::rounded_rect(
            x + inset,
            y + inset,
            width - style.border_width,
            height - style.border_width,
            style.radius - inset,
        ) {
            canvas.fill(&plate, canvas::color(&style.background, opacity));
            canvas.stroke(
                &plate,
                canvas::color(&style.border, opacity),
                style.border_width,
            );
        }
        let left = x + style.padding_x + style.border_width;
        let foreground = canvas::color(&style.foreground, opacity);
        if let Some(icon) = &item.icon {
            let shape = &spec.icons[icon];
            let unit = style.icon_size / 24.;
            let at = |p: [f32; 2]| (left + p[0] * unit, top + p[1] * unit);
            for points in &shape.lines {
                let mut path = PathBuilder::new();
                for (index, &point) in points.iter().enumerate() {
                    let (px, py) = at(point);
                    if index == 0 {
                        path.move_to(px, py)
                    } else {
                        path.line_to(px, py)
                    }
                }
                if let Some(path) = path.finish() {
                    canvas.stroke(&path, foreground, 2. * unit);
                }
            }
            if let Some([cx, cy, r]) = shape.circle {
                let (px, py) = at([cx, cy]);
                if let Some(dot) = PathBuilder::from_circle(px, py, r * unit) {
                    canvas.fill(&dot, foreground);
                }
            }
        } else if let Some(path) = line.and_then(|line| line.path) {
            let shifted = path.transform(tiny_skia::Transform::from_translate(left, 0.));
            if let Some(path) = shifted {
                canvas.fill(&path, foreground);
            }
        }
        y += height + style.gap;
    }
}
