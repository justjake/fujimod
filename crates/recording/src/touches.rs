//! Touch indicators: a translucent circle at the finger while it is down,
//! popping in as it lands and following it. A tap (a short contact) ends in a
//! ripple, a ring that grows from the circle and fades as the circle goes; a
//! longer contact's circle fades where the finger lifts. Every touch stays
//! visible for a minimum time, so the shortest tap still registers.
use crate::{
    CONTROLS, Result,
    canvas::{self, Canvas},
};
use serde::Deserialize;
use std::{str::FromStr, sync::OnceLock};
use tiny_skia::PathBuilder;

#[derive(Deserialize)]
struct Style {
    radius: f64,
    fill: String,
    border: String,
    border_width: f32,
    /// Landing: the circle grows from `press_scale` of its radius.
    press_ms: f64,
    press_scale: f64,
    /// The shortest time a touch is shown as down.
    minimum_ms: f64,
    /// Contacts shorter than this are taps and end in a ripple.
    tap_ms: f64,
    /// The ripple grows to `ripple_scale` of the radius while fading.
    ripple_ms: f64,
    ripple_scale: f64,
    /// A longer contact's circle fades over this.
    release_ms: f64,
}
#[derive(Deserialize)]
struct Spec {
    touch: Style,
}
fn style() -> &'static Style {
    static SPEC: OnceLock<Spec> = OnceLock::new();
    &SPEC
        .get_or_init(|| serde_json::from_str(CONTROLS).expect("checked touch presentation"))
        .touch
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TouchPhase {
    Start,
    Move,
    End,
}
impl FromStr for TouchPhase {
    type Err = String;
    fn from_str(value: &str) -> Result<Self> {
        match value {
            "start" => Ok(Self::Start),
            "move" => Ok(Self::Move),
            "end" => Ok(Self::End),
            _ => Err(format!("Unknown touch phase: {value}")),
        }
    }
}

/// One finger in source-frame coordinates; `down` and `lifted` are the
/// output frames of its landing and its lift.
pub(crate) struct Touch {
    pub(crate) x: f64,
    pub(crate) y: f64,
    pub(crate) down: u64,
    pub(crate) lifted: Option<u64>,
}
/// What to draw, in canvas pixels (unrounded, pixel centers at +0.5), with
/// opacity 0..1: the finger's circle, or a tap's ripple ring.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Mark {
    Circle {
        x: f64,
        y: f64,
        radius: f64,
        opacity: f64,
    },
    Ripple {
        x: f64,
        y: f64,
        radius: f64,
        opacity: f64,
    },
}

/// Seconds since `frame` at `fps`, for `frames` now.
fn since(frames: u64, frame: u64, fps: u32) -> f64 {
    frames.saturating_sub(frame) as f64 / f64::from(fps)
}
fn ease_out(t: f64) -> f64 {
    let t = t.clamp(0., 1.);
    1. - (1. - t).powi(3)
}
/// When the touch stops being shown as down (seconds after landing), and
/// how long its ending lasts, or None while the finger is down.
fn ending(touch: &Touch, fps: u32) -> Option<(f64, f64, bool)> {
    let style = style();
    let contact = since(touch.lifted?, touch.down, fps);
    let tap = contact * 1000. < style.tap_ms;
    let release = contact.max(style.minimum_ms / 1000.);
    Some((
        release,
        if tap {
            style.ripple_ms
        } else {
            style.release_ms
        } / 1000.,
        tap,
    ))
}
/// Whether a touch still draws anything at frame `frames`.
pub(crate) fn showing(touch: &Touch, frames: u64, fps: u32) -> bool {
    ending(touch, fps)
        .is_none_or(|(release, length, _)| since(frames, touch.down, fps) < release + length)
}
/// Marks for fingers on a source of `source` size fitted centered into
/// `canvas`, as `Frame::fit` places it. Touches whose ending is over are
/// dropped from `touches`.
pub(crate) fn marks(
    touches: &mut Vec<Touch>,
    frames: u64,
    fps: u32,
    source: (u32, u32),
    canvas: (u32, u32),
) -> Vec<Mark> {
    let style = style();
    touches.retain(|t| showing(t, frames, fps));
    let (sw, sh) = (f64::from(source.0), f64::from(source.1));
    let scale = (f64::from(canvas.0) / sw).min(f64::from(canvas.1) / sh);
    let (w, h) = ((sw * scale).round(), (sh * scale).round());
    let (left, top) = (
        ((f64::from(canvas.0) - w) / 2.).floor(),
        ((f64::from(canvas.1) - h) / 2.).floor(),
    );
    let radius = style.radius * scale;
    let mut marks = Vec::new();
    for t in touches.iter() {
        let (x, y) = (left + t.x * w / sw, top + t.y * h / sh);
        let age = since(frames, t.down, fps);
        let landed =
            style.press_scale + (1. - style.press_scale) * ease_out(age * 1000. / style.press_ms);
        let circle = |radius: f64, opacity: f64| Mark::Circle {
            x,
            y,
            radius,
            opacity,
        };
        match ending(t, fps) {
            Some((release, length, tap)) if age >= release => {
                let progress = (age - release) / length;
                if tap {
                    // The circle goes in the first half while the ring grows.
                    marks.push(circle(radius, (1. - 2. * progress).max(0.)));
                    marks.push(Mark::Ripple {
                        x,
                        y,
                        radius: radius * (1. + (style.ripple_scale - 1.) * ease_out(progress)),
                        opacity: 1. - progress,
                    });
                } else {
                    marks.push(circle(radius, 1. - progress));
                }
            }
            _ => marks.push(circle(radius * landed, 1.)),
        }
    }
    marks.retain(|mark| match *mark {
        Mark::Circle { opacity, .. } | Mark::Ripple { opacity, .. } => opacity > 0.,
    });
    marks
}
/// A circle is a filled disc inside a ring, as an SVG circle with a stroke
/// along its inside edge; a ripple is the ring alone. All anti-aliased.
pub(crate) fn draw_on(canvas: &mut Canvas, marks: &[Mark]) {
    let style = style();
    for mark in marks {
        match *mark {
            Mark::Circle {
                x,
                y,
                radius,
                opacity,
            } => {
                let (x, y, radius, opacity) = (x as f32, y as f32, radius as f32, opacity as f32);
                let ring = style.border_width.min(radius);
                if let Some(disc) = PathBuilder::from_circle(x, y, radius - ring) {
                    canvas.fill(&disc, canvas::color(&style.fill, opacity));
                }
                if let Some(edge) = PathBuilder::from_circle(x, y, radius - ring / 2.) {
                    canvas.stroke(&edge, canvas::color(&style.border, opacity), ring);
                }
            }
            Mark::Ripple {
                x,
                y,
                radius,
                opacity,
            } => {
                let (x, y, radius, opacity) = (x as f32, y as f32, radius as f32, opacity as f32);
                let ring = style.border_width.min(radius);
                if let Some(edge) = PathBuilder::from_circle(x, y, radius - ring / 2.) {
                    canvas.stroke(&edge, canvas::color(&style.border, opacity), ring);
                }
            }
        }
    }
}
