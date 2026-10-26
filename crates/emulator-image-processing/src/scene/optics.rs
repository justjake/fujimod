//! A declared optical input law, driven by the physical focus-axis output.
//! The external raster is sharp at target_position. Defocus applies a bounded
//! horizontal box PSF; pulse/depth scale and the PSF are not calibrated optics.
use fujimod_emulator_imaging::scene::frames::{ImageFrame, Pixels};
use fujimod_emulator_devices::parts::optics::Position;
use fujimod_emulator_imaging::scene::optics::Options;
use std::sync::Arc;

pub struct Optics {
    options: Options,
    position: Position,
    source: Arc<[u8]>,
    last: Option<i64>,
}
impl Optics {
    pub fn new(options: Options, position: Position) -> Result<Self, String> {
        if options.pulses_per_radius == 0
            || options.maximum_radius == 0
            || options.maximum_radius > 64
        {
            return Err("optical blur needs a positive bounded pulse/radius scale".into());
        }
        Ok(Self {
            options,
            position,
            source: Arc::from([]),
            last: None,
        })
    }
    pub(super) fn checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        Ok(
            fujimod_emulator_devices::checkpoint::Checkpoint::encode("optical-input", &(&self.options, self.last))?
                .with_blobs(vec![self.source.clone()]),
        )
    }
    pub(super) fn validate_checkpoint(
        &self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
        bytes: usize,
    ) -> Result<(), String> {
        let (options, last): (Options, Option<i64>) = saved.decode("optical-input")?;
        if options != self.options || saved.blobs().len() != 1 || !saved.children().is_empty() {
            return Err("optical input checkpoint configuration differs".into());
        }
        let source = &saved.blobs()[0];
        if source.is_empty() != last.is_none() || !source.is_empty() && source.len() != bytes {
            return Err("optical input checkpoint raster extent differs".into());
        }
        Ok(())
    }
    pub(super) fn restore_checkpoint(
        &mut self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
        bytes: usize,
    ) -> Result<(), String> {
        self.validate_checkpoint(saved, bytes)?;
        let (_, last): (Options, Option<i64>) = saved.decode("optical-input")?;
        self.source = saved.blobs()[0].clone();
        self.last = last;
        Ok(())
    }
    pub(super) fn render(
        &mut self,
        image: &mut ImageFrame,
        changed: bool,
    ) -> Result<(), String> {
        let position = self.position.value();
        if changed || self.source.is_empty() {
            self.source = image.pixels.render(image.layout, 0)?;
            self.last = None;
        }
        if self.last == Some(position) {
            return Ok(());
        }
        self.last = Some(position);
        let radius = (position.abs_diff(self.options.target_position)
            / u64::from(self.options.pulses_per_radius))
        .min(u64::from(self.options.maximum_radius)) as usize;
        if radius == 0 {
            image.pixels = Pixels::Packed(self.source.clone());
            return Ok(());
        }
        let pixels = Arc::make_mut(image.pixels.packed_mut(image.layout, 0)?);
        pixels.copy_from_slice(&self.source);
        let l = image.layout;
        let mut sums = vec![0u32; l.width + 1];
        for y in 0..l.height {
            let start = y * l.stride;
            for x in 0..l.width {
                sums[x + 1] = sums[x] + u32::from(self.source[start + (x / 2) * 4 + x % 2]);
            }
            for x in 0..l.width {
                let lo = x.saturating_sub(radius);
                let hi = (x + radius + 1).min(l.width);
                pixels[start + (x / 2) * 4 + x % 2] =
                    ((sums[hi] - sums[lo]) / (hi - lo) as u32) as u8;
            }
        }
        Ok(())
    }
    pub(super) fn last_position(&self) -> Option<i64> {
        self.last
    }
    pub fn fingerprint(&self) -> serde_json::Value {
        serde_json::json!({"position":self.position.value(),"last":self.last,
            "target":self.options.target_position,"pulses_per_radius":self.options.pulses_per_radius,
            "maximum_radius":self.options.maximum_radius,"source":fujimod_emulator_devices::fingerprint::bytes(&self.source)})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_imaging::{exr2_isp::contrast::energies, analysis::ImageLayout, scene::ImageFrames};
    #[test]
    fn physical_position_changes_pixels_and_sharp_input_is_recoverable() {
        let l = ImageLayout {
            width: 32,
            height: 4,
            stride: 64,
        };
        let bytes = (0..4)
            .flat_map(|_| (0..16).flat_map(|i| [if i < 8 { 0 } else { 255 }; 4]))
            .collect::<Vec<u8>>();
        let frames = ImageFrames::from_pixels(l, bytes.clone()).unwrap();
        let position = Position::default();
        let mut optics = Optics::new(
            Options {
                target_position: 0,
                pulses_per_radius: 8,
                maximum_radius: 32,
            },
            position.clone(),
        )
        .unwrap();
        let mut frame = frames.0.lock().unwrap();
        optics.render(&mut frame, true).unwrap();
        let retained_sharp = frame.pixels.render(l, 0).unwrap();
        let sharp = energies(&frame.pixels.render(l, 0).unwrap(), l, [0, 0, 32, 4])
            .unwrap()
            .1;
        position.publish(64);
        optics.render(&mut frame, false).unwrap();
        let blurred = energies(&frame.pixels.render(l, 0).unwrap(), l, [0, 0, 32, 4])
            .unwrap()
            .1;
        assert!(blurred < sharp);
        assert_eq!(retained_sharp.as_ref(), bytes.as_slice());
        let saved = optics.fingerprint();
        let pixels = frame.pixels.render(l, 0).unwrap();
        optics.render(&mut frame, false).unwrap();
        assert_eq!(pixels, frame.pixels.render(l, 0).unwrap());
        assert_eq!(saved, optics.fingerprint());
        let checkpoint = optics.checkpoint().unwrap();
        let corrupt = checkpoint.clone().with_blobs(vec![Arc::from([0u8; 4])]);
        assert!(
            optics
                .restore_checkpoint(&corrupt, l.bytes().unwrap())
                .is_err()
        );
        assert_eq!(optics.fingerprint(), saved);
        frame.pixels = Pixels::Packed(vec![128; bytes.len()].into());
        optics.render(&mut frame, true).unwrap();
        assert_ne!(optics.fingerprint(), saved);
        optics
            .restore_checkpoint(&checkpoint, l.bytes().unwrap())
            .unwrap();
        frame.pixels = Pixels::Packed(pixels);
        assert_eq!(optics.fingerprint(), saved);
        position.publish(0);
        optics.render(&mut frame, false).unwrap();
        assert_eq!(
            frame.pixels.render(l, 0).unwrap().as_ref(),
            bytes.as_slice()
        );
        assert_eq!(
            energies(&frame.pixels.render(l, 0).unwrap(), l, [0, 0, 32, 4])
                .unwrap()
                .1,
            sharp
        );
    }
}
