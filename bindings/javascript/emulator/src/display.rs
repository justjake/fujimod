//! Observational EXR II pixels cached for borrowed `Frame` views.
use super::session::Session;
use fujimod_emulator_devices::graphics::scanout::Frame;
use std::sync::Arc;

impl Session {
    pub fn status(&self) -> super::Status {
        super::Status {
            ticks: self.core.board.now(),
            instructions: self.core.board.instruction_counts().iter().sum(),
            failed: self.failed,
        }
    }
    /// The latest presented frame of `output`, or `None` before the first.
    pub fn frame(&mut self, output: usize) -> Result<Option<Arc<Frame>>, String> {
        self.observe_frame(output)?;
        Ok(self.frames.get(&output).cloned())
    }
    fn observe_frame(&mut self, output: usize) -> Result<(), String> {
        if !self.powered_on {
            return Ok(());
        }
        let Some(name) = self.camera.as_ref().and_then(|m| m.scanout.as_ref()) else {
            return Ok(());
        };
        let Some(frame) = self.core.scanout(name, output, true, false)? else {
            return Ok(());
        };
        let rgb = frame.rgb.ok_or("scanout did not return requested pixels")?;
        let previous = self.frames.get(&output);
        if previous.is_some_and(|p| {
            p.width as usize == frame.width
                && p.height as usize == frame.height
                && matches_rgb(&p.rgba, &rgb)
        }) {
            return Ok(());
        }
        let mut rgba = Vec::with_capacity(frame.width * frame.height * 4);
        for pixel in rgb.chunks_exact(3) {
            rgba.extend_from_slice(pixel);
            rgba.push(255);
        }
        // This is the transport's image-change sequence, not a hardware counter.
        let sequence = previous
            .map_or(Some(1), |p| p.sequence.checked_add(1))
            .ok_or("presentation sequence overflow")?;
        self.frames.insert(
            output,
            Arc::new(Frame {
                output,
                sequence,
                width: frame
                    .width
                    .try_into()
                    .map_err(|_| "scanout width overflow")?,
                height: frame
                    .height
                    .try_into()
                    .map_err(|_| "scanout height overflow")?,
                rgba,
            }),
        );
        Ok(())
    }
}

fn matches_rgb(rgba: &[u8], rgb: &[u8]) -> bool {
    rgb.len().is_multiple_of(3)
        && rgba.len() == rgb.len() / 3 * 4
        && rgba
            .chunks_exact(4)
            .zip(rgb.chunks_exact(3))
            .all(|(a, b)| a[..3] == *b && a[3] == 255)
}

#[cfg(test)]
mod tests {
    use super::matches_rgb;
    #[test]
    fn unchanged_presentation_requires_complete_opaque_pixels() {
        assert!(matches_rgb(
            &[1, 2, 3, 255, 4, 5, 6, 255],
            &[1, 2, 3, 4, 5, 6]
        ));
        assert!(!matches_rgb(&[1, 2, 3, 0], &[1, 2, 3]));
        assert!(!matches_rgb(&[1, 2, 3, 255], &[1, 2, 4]));
        assert!(!matches_rgb(&[1, 2, 3, 255], &[1, 2, 3, 4]));
        assert!(!matches_rgb(&[1, 2, 3, 255, 4, 5, 6, 255], &[1, 2, 3]));
    }
}
