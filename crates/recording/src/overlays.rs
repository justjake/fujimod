//! Presentation-only panel motion and drawing shared by secondary displays.
use crate::{
    Frame, Result,
    canvas::{Canvas, rounded_rect},
    text,
};

pub(crate) const DURATION: f64 = 0.45;
pub(crate) const BODY: [u8; 4] = [25, 25, 28, 255];
pub(crate) const BORDER: [u8; 4] = [75, 75, 82, 255];

pub(crate) struct Motion {
    start: u64,
    from: f32,
    pub target: f32,
}
impl Motion {
    pub fn new(frame: u64) -> Self {
        Self {
            start: frame,
            from: 0.,
            target: 1.,
        }
    }
    pub fn value(&self, frame: u64, fps: u32) -> f32 {
        let t = ((frame - self.start) as f64 / f64::from(fps) / DURATION).clamp(0., 1.) as f32;
        self.from + (self.target - self.from) * (1. - (1. - t).powi(3))
    }
    pub fn set(&mut self, visible: bool, frame: u64, fps: u32) {
        let target = if visible { 1. } else { 0. };
        if self.target != target {
            self.from = self.value(frame, fps);
            self.target = target;
            self.start = frame;
        }
    }
    pub fn active(&self, frame: u64, fps: u32) -> bool {
        (frame - self.start) as f64 / f64::from(fps) < DURATION
    }
}

pub(crate) fn label(
    canvas: &mut Canvas,
    value: &str,
    x: f32,
    y: f32,
    size: f32,
    width: f32,
    color: [u8; 4],
) {
    let mut shown = value.to_owned();
    while text::layout(&shown, x, y, size, size * 1.5).width > width && !shown.is_empty() {
        shown.pop();
    }
    if shown != value && !shown.is_empty() {
        shown.pop();
        shown.push('…');
    }
    if let Some(path) = text::layout(&shown, x, y, size, size * 1.5).path {
        canvas.fill(&path, color);
    }
}

pub(crate) struct DisplayPanel {
    pub frame: Frame,
    pub label: String,
    pub width: f32,
    pub motion: Motion,
}
impl DisplayPanel {
    pub fn new(frame: Frame, label: String, width: f32, at: u64) -> Result<Self> {
        if label.is_empty()
            || label.chars().count() > 32
            || label.chars().any(char::is_control)
            || !width.is_finite()
            || !(0.25..=0.75).contains(&width)
        {
            return Err("Invalid display overlay label or width".into());
        }
        Ok(Self {
            frame,
            label,
            width,
            motion: Motion::new(at),
        })
    }
    pub fn draw(&self, canvas: &mut Canvas, at: u64, fps: u32) -> Result<()> {
        let visibility = self.motion.value(at, fps);
        if visibility <= 0. {
            return Ok(());
        }
        let scale = canvas.height() / 480.;
        let w = (canvas.width() * self.width).round();
        let inner = w - 8. * scale;
        let image_h = (inner * self.frame.height() as f32 / self.frame.width() as f32).round();
        let h = image_h + 30. * scale;
        let x = canvas.width() - w - 14. * scale + (1. - visibility) * (w + 14. * scale);
        let y = 14. * scale;
        if let Some(body) = rounded_rect(x, y, w, h, 13. * scale) {
            canvas.fill(&body, BODY);
            canvas.stroke(&body, BORDER, scale);
        }
        label(
            canvas,
            &self.label,
            x + 13. * scale,
            y + 5. * scale,
            11. * scale,
            w - 26. * scale,
            [222, 222, 228, 255],
        );
        canvas.image(
            &self.frame,
            x + 4. * scale,
            y + 26. * scale,
            inner as u32,
            image_h as u32,
            9. * scale,
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cubic_entry_and_reversal_are_continuous() {
        let mut motion = Motion::new(0);
        assert_eq!(motion.value(0, 60), 0.);
        assert!((motion.value(9, 60) - (1. - (2_f32 / 3.).powi(3))).abs() < 0.0001);
        let before = motion.value(9, 60);
        motion.set(false, 9, 60);
        assert_eq!(motion.value(9, 60), before);
        assert_eq!(motion.value(40, 60), 0.);
    }
}
