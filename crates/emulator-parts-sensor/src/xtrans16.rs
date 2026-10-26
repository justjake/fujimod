//! Three-byte sensor writes (81/address/value) and external horizontal/vertical
//! scan clocks. Sensor configuration writes select gain, shutter registers,
//! and vertical timing. Gain is4095/(4095-code).
//! Timing/ADC responsivity remain declared physical calibration, not firmware
//! inspection or a caller signature. The sensor receives actual scan periods.
use crate::sensor::Controls;
use fujimod_emulator_devices::parts::optics::ApertureHandle;
use fujimod_emulator_imaging::exposure::{Exposure, ExposureHandle};
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RollingReference {
    pub mode: u8,
    pub horizontal: u32,
    pub vertical: u32,
    /// One scan's row count, within the receiver's programmed counter window.
    pub lines: u32,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub pixel_clock_hz: u64,
    pub shutter: Option<crate::leaf_shutter::Options>,
    #[serde(default)]
    pub rolling_references: Vec<RollingReference>,
    /// Frames read out without valid sync after a drive-mode change. The
    /// receiver sees no frame edge for them.
    #[serde(default)]
    pub mode_change_frames: u8,
    /// Full preview field containing command 0B..0E's selected rows.
    pub preview_field: Option<[usize; 2]>,
}
#[derive(Serialize, Deserialize)]
struct State {
    registers: Vec<u8>,
    horizontal: u32,
    vertical: u32,
    shutter_drive: Option<u64>,
    global_seconds: Option<f64>,
    /// Integration started for the following rolling readout. Analog gain is
    /// applied when that readout reaches the ADC, rather than at reset time.
    rolling_seconds: Option<f64>,
    /// Invalid frames still to come after the last drive-mode change.
    settling: u8,
}
pub struct Sensor {
    state: State,
    options: Options,
    exposure: ExposureHandle,
    aperture: ApertureHandle,
    shutter: Option<crate::leaf_shutter::LeafShutter>,
    frame_clock: Option<(fujimod_emulator_imaging::timing::FramePeriod, u64)>,
}
impl Sensor {
    pub fn new(
        options: Options,
        exposure: ExposureHandle,
        aperture: ApertureHandle,
    ) -> Result<Self, String> {
        if options.pixel_clock_hz == 0 {
            return Err("sensor needs a physical pixel clock".into());
        }
        Ok(Self {
            state: State {
                registers: vec![0; 256],
                horizontal: 0,
                vertical: 0,
                shutter_drive: None,
                global_seconds: None,
                rolling_seconds: None,
                settling: 0,
            },
            shutter: options
                .shutter
                .clone()
                .map(crate::leaf_shutter::LeafShutter::new)
                .transpose()?,
            options,
            frame_clock: None,
            exposure,
            aperture,
        })
    }
    fn publish(&mut self) -> Result<(), String> {
        if self.state.registers[1] & 4 != 0 && self.state.horizontal != 0 {
            let shutter = self
                .shutter
                .as_ref()
                .ok_or("global reset needs its physical leaf shutter")?;
            let shr =
                u32::from(self.state.registers[5]) | (u32::from(self.state.registers[6] & 15) << 8);
            // Configuring global reset before enabling its external shutter
            // waveform is valid. No exposed frame exists until both are ready.
            if let Some(drive) = self.state.shutter_drive {
                self.state.global_seconds = Some(
                    shutter.integration(
                        drive as f64 / self.options.pixel_clock_hz as f64,
                        f64::from(shr) * f64::from(self.state.horizontal)
                            / self.options.pixel_clock_hz as f64,
                        *self
                            .aperture
                            .lock()
                            .map_err(|_| "iris transmission poisoned")?,
                    )?,
                );
            }
        }
        let s = &self.state;
        let word = |n: usize| u32::from(s.registers[n]) | (u32::from(s.registers[n + 1] & 15) << 8);
        let gain = word(0x32);
        if gain >= 4095 {
            return Err("sensor AGC reaches its singular code4095".into());
        }
        // The input receiver's padded vertical raster can include a long
        // idle interval. Selected sensor drive modes declare one scan's reset
        // reference, independent of that receiver raster's padding. Matching
        // the entire scan tuple keeps shorter rasters on their actual counter.
        // Longer reset windows below can span several of those scans.
        let reference = self
            .options
            .rolling_references
            .iter()
            .find(|r| {
                s.registers[0] & 0xfc == r.mode
                    && s.horizontal == r.horizontal
                    && s.vertical == r.vertical
            })
            .map_or(s.vertical, |r| r.lines);
        let extension = u64::from(word(2)) + 1;
        let reset = u64::from(word(5));
        let base_rows = u64::from(reference) * extension;
        // The reset counter can address a later scan in the receiver's
        // programmed window. A reference describes one scan, not a ceiling
        // on the counter: complete whole scans until the reset is inside the
        // active frame. Keep the receiver's programmed extent as the bound.
        // In particular, SHR2106 in an 857-row scan belongs to a three-scan
        // frame (2571 rows), leaving 465 rows of integration.
        let exposure_rows = if base_rows > 0 && base_rows < u64::from(s.vertical) * extension {
            ((reset / base_rows + 1) * base_rows).min(u64::from(s.vertical) * extension)
        } else {
            base_rows
        };
        if let Some((output, tick_hz)) = &self.frame_clock {
            // The reset window also times frame publication. Padding alone
            // does not lengthen a scan; addressing a later reset cycle does.
            // Unreferenced rasters keep their programmed rows.
            let frame_rows = exposure_rows;
            let clocks = u128::from(frame_rows) * u128::from(s.horizontal) * u128::from(*tick_hz);
            let period = (clocks + u128::from(self.options.pixel_clock_hz) / 2)
                / u128::from(self.options.pixel_clock_hz);
            *output.lock().map_err(|_| "sensor frame clock poisoned")? =
                u64::try_from(period).map_err(|_| "sensor frame clock exceeds guest timebase")?;
        }
        let integration = exposure_rows.saturating_sub(reset).max(1);
        *self
            .exposure
            .lock()
            .map_err(|_| "sensor exposure poisoned")? = Exposure {
            seconds: if s.horizontal == 0
                || s.vertical == 0
                || (s.registers[1] & 4 != 0 && s.global_seconds.is_none())
            {
                0.
            } else {
                s.global_seconds.unwrap_or(
                    integration as f64 * s.horizontal as f64 / self.options.pixel_clock_hz as f64,
                )
            },
            analog_gain: 4095. / f64::from(4095 - gain),
        };
        Ok(())
    }
    pub fn attach_frame_clock(
        &mut self,
        output: fujimod_emulator_imaging::timing::FramePeriod,
        ticks_per_second: u64,
    ) -> Result<(), String> {
        if ticks_per_second == 0 {
            return Err("sensor timebase must be positive".into());
        }
        self.frame_clock = Some((output, ticks_per_second));
        self.publish()
    }
}
impl Sensor {
    fn integrate(&mut self) -> Result<(), String> {
        self.publish()?;
        if self.state.registers[1] & 4 != 0 || self.state.global_seconds.is_some() {
            // Global reset integrates until the physical leaf closes; it does
            // not inherit the previous rolling frame's reset interval.
            self.state.rolling_seconds = None;
            return Ok(());
        }
        let mut readout = self
            .exposure
            .lock()
            .map_err(|_| "sensor exposure poisoned")?;
        if readout.seconds <= 0. {
            self.state.rolling_seconds = None;
            return Ok(());
        }
        let next = readout.seconds;
        if let Some(integrated) = self.state.rolling_seconds {
            readout.seconds = integrated;
        }
        self.state.rolling_seconds = Some(next);
        Ok(())
    }
}
impl Controls for Sensor {
    fn checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        fujimod_emulator_devices::checkpoint::Checkpoint::encode(
            "xtrans16-controls",
            &(
                &self.options,
                &self.state,
                self.frame_clock.as_ref().map(|(_, ticks)| *ticks),
            ),
        )
    }
    fn validate_checkpoint(&self, saved: &fujimod_emulator_devices::checkpoint::Checkpoint) -> Result<(), String> {
        let (options, state, timebase): (Options, State, Option<u64>) =
            saved.decode("xtrans16-controls")?;
        if timebase != self.frame_clock.as_ref().map(|(_, ticks)| *ticks)
            || options != self.options
            || state.registers.len() != 256
            || state
                .global_seconds
                .is_some_and(|v| !v.is_finite() || v <= 0.)
            || state
                .rolling_seconds
                .is_some_and(|v| !v.is_finite() || v <= 0.)
            || state.settling > options.mode_change_frames
            || !saved.blobs().is_empty()
            || !saved.children().is_empty()
        {
            return Err(
                "sensor controller checkpoint configuration or register extent differs".into(),
            );
        }
        Ok(())
    }
    fn restore_checkpoint(&mut self, saved: &fujimod_emulator_devices::checkpoint::Checkpoint) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        let (_, state, _): (Options, State, Option<u64>) = saved.decode("xtrans16-controls")?;
        self.state = state;
        // Shared physical exposure/aperture wires restore independently in place.
        Ok(())
    }
    fn fingerprint(&self) -> serde_json::Value {
        serde_json::json!({"options":self.options,"state":self.state,"frame_clock":self.frame_clock.as_ref().map(|(_,ticks)|*ticks)})
    }

    fn command(&mut self, bytes: &[u8]) -> Result<(), String> {
        if bytes.len() % 8 != 0
            || bytes
                .chunks_exact(8)
                .any(|b| b[0] != 0x81 || b[3..].iter().any(|&v| v != 0))
        {
            return Err("X-Trans sensor requires81/address/value words".into());
        }
        for b in bytes.chunks_exact(8) {
            if b[1] == 0 {
                let changed = b[2] & 0xfc != self.state.registers[0] & 0xfc;
                let restarted = b[2] & 1 == 0 && self.state.registers[0] & 1 != 0;
                if changed {
                    self.state.global_seconds = None;
                }
                if changed || restarted {
                    self.state.rolling_seconds = None;
                    self.state.settling = self.options.mode_change_frames;
                }
            }
            if b[1] == 1 && b[2] & 4 != 0 && self.state.registers[1] & 4 == 0 {
                self.state.global_seconds = None;
            }
            self.state.registers[b[1] as usize] = b[2];
        }
        self.publish()
    }
    fn preview_view(
        &self,
        horizontal: [usize; 2],
    ) -> Result<Option<fujimod_emulator_imaging::scene::view::View>, String> {
        let r = &self.state.registers;
        if r[1] & 1 == 0 {
            return Ok(None);
        }
        let field = self
            .options
            .preview_field
            .ok_or("sensor window has no declared preview field")?;
        let y = usize::from(r[0x0b]) | (usize::from(r[0x0c] & 15) << 8);
        let height = 2 * (usize::from(r[0x0d]) | (usize::from(r[0x0e] & 15) << 8));
        fujimod_emulator_imaging::scene::view::View::new(
            field,
            [horizontal[0], y, horizontal[1], height],
        )
        .map(Some)
    }
    fn frame(&mut self) -> Result<bool, String> {
        // Standby stops readout. A configured clock may continue ticking at
        // the receiver, but it cannot consume the sensor's startup interval.
        if self.state.registers[0] & 1 != 0 {
            return Ok(false);
        }
        let valid = self.state.settling == 0;
        self.state.settling = self.state.settling.saturating_sub(1);
        self.integrate()?;
        Ok(valid)
    }
    fn timing(
        &mut self,
        horizontal: u32,
        vertical: u32,
        shutter_drive: Option<u64>,
    ) -> Result<(), String> {
        self.state.shutter_drive = shutter_drive;
        self.state.horizontal = horizontal;
        self.state.vertical = vertical;
        self.publish()
    }
    fn save(&self) -> serde_json::Value {
        serde_json::to_value(&self.state).unwrap()
    }
    fn restore(&mut self, state: &serde_json::Value) -> Result<(), String> {
        let state: State = serde_json::from_value(state.clone()).map_err(|e| e.to_string())?;
        if state.registers.len() != 256 {
            return Err("sensor register extent changed".into());
        }
        self.state = state;
        self.publish()
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_window_uses_transmitted_coordinates_and_restores_its_command_state() {
        let exposure = std::sync::Arc::new(std::sync::Mutex::new(Exposure::default()));
        let aperture = std::sync::Arc::new(std::sync::Mutex::new(4.));
        let mut sensor = Sensor::new(
            Options {
                pixel_clock_hz: 33_000_000,
                shutter: None,
                rolling_references: vec![],
                mode_change_frames: 0,
                preview_field: Some([2496, 3344]),
            },
            exposure,
            aperture,
        )
        .unwrap();
        assert_eq!(sensor.preview_view([0, 2496]).unwrap(), None);
        sensor
            .command(&[
                0x81, 1, 1, 0, 0, 0, 0, 0, 0x81, 0x0b, 0xf2, 0, 0, 0, 0, 0, 0x81, 0x0c, 3, 0, 0, 0,
                0, 0, 0x81, 0x0d, 0xaa, 0, 0, 0, 0, 0, 0x81, 0x0e, 2, 0, 0, 0, 0, 0,
            ])
            .unwrap();
        let view = sensor.preview_view([735, 1002]).unwrap().unwrap();
        assert_eq!(view.field, [2496, 3344]);
        assert_eq!(view.rectangle, [735, 1010, 1002, 1364]);
        assert!(sensor.preview_view([2490, 1002]).is_err());
        let saved = sensor.save();
        sensor.command(&[0x81, 1, 0, 0, 0, 0, 0, 0]).unwrap();
        assert_eq!(sensor.preview_view([0, 2496]).unwrap(), None);
        sensor.restore(&saved).unwrap();
        assert_eq!(sensor.preview_view([735, 1002]).unwrap(), Some(view));
    }

    #[test]
    fn reset_counter_spans_whole_scans_inside_the_programmed_window() {
        for (horizontal, vertical, scan, reset, frame) in [
            (706, 9348, 857, 840, 857),
            (706, 9348, 857, 2106, 2571),
            (737, 8955, 820, 2280, 2460),
            (734, 1671, 1671, 1600, 1671),
            (750, 1760, 1760, 1700, 1760),
            (706, 2500, 857, 2400, 2500),
        ] {
            let wire: ExposureHandle = Default::default();
            let clock: fujimod_emulator_imaging::timing::FramePeriod = Default::default();
            let mut sensor = Sensor::new(
                Options {
                    pixel_clock_hz: 33_000_000,
                    shutter: None,
                    rolling_references: vec![RollingReference {
                        mode: 0xec, horizontal, vertical, lines: scan,
                    }],
                    mode_change_frames: 0,
                    preview_field: None,
                },
                wire.clone(),
                std::sync::Arc::new(std::sync::Mutex::new(64.)),
            ).unwrap();
            sensor.attach_frame_clock(clock.clone(), 12_000_000).unwrap();
            sensor.timing(horizontal, vertical, None).unwrap();
            let command = |reset: u32| {
                [
                    0x81,
                    0,
                    0xec,
                    0,
                    0,
                    0,
                    0,
                    0,
                    0x81,
                    5,
                    reset as u8,
                    0,
                    0,
                    0,
                    0,
                    0,
                    0x81,
                    6,
                    (reset >> 8) as u8,
                    0,
                    0,
                    0,
                    0,
                    0,
                ]
            };
            sensor.command(&command(reset)).unwrap();
            sensor.frame().unwrap();
            assert!((wire.lock().unwrap().seconds
                - f64::from((frame - reset) * horizontal) / 33_000_000.).abs() < 1e-12);
            assert_eq!(*clock.lock().unwrap(),
                (u64::from(frame) * u64::from(horizontal) * 12_000_000 + 16_500_000) / 33_000_000);
            let saved = sensor.checkpoint().unwrap();
            sensor.command(&command(scan - 17)).unwrap();
            sensor.frame().unwrap();
            sensor.frame().unwrap();
            assert!((wire.lock().unwrap().seconds
                - f64::from(17 * horizontal) / 33_000_000.).abs() < 1e-12);
            sensor.restore_checkpoint(&saved).unwrap();
            sensor.frame().unwrap();
            assert!((wire.lock().unwrap().seconds
                - f64::from((frame - reset) * horizontal) / 33_000_000.).abs() < 1e-12);
        }
    }

    #[test]
    fn rolling_readout_uses_prior_integration_and_current_adc_gain() {
        let wire: ExposureHandle = Default::default();
        let mut sensor = Sensor::new(
            Options {
                pixel_clock_hz: 33_000_000,
                shutter: None,
                rolling_references: vec![],
                mode_change_frames: 0,
                preview_field: None,
            },
            wire.clone(),
            std::sync::Arc::new(std::sync::Mutex::new(8.)),
        )
        .unwrap();
        sensor.timing(660, 1000, None).unwrap();
        sensor.frame().unwrap();
        assert_eq!(wire.lock().unwrap().seconds, 0.02);
        sensor
            .command(&[
                0x81, 5, 244, 0, 0, 0, 0, 0, 0x81, 6, 1, 0, 0, 0, 0, 0, 0x81, 0x32, 0, 0, 0, 0, 0,
                0, 0x81, 0x33, 8, 0, 0, 0, 0, 0,
            ])
            .unwrap();
        sensor.frame().unwrap();
        let readout = *wire.lock().unwrap();
        assert_eq!(readout.seconds, 0.02);
        assert!((readout.analog_gain - 2.).abs() < 0.001);
        sensor
            .command(&[0x81, 5, 32, 0, 0, 0, 0, 0, 0x81, 6, 3, 0, 0, 0, 0, 0])
            .unwrap();
        // The snapshot must retain .01 even though the newest SHR configures
        // .004. Reconstructing pending integration from the registers loses it.
        let checkpoint = sensor.checkpoint().unwrap();
        let fingerprint = sensor.fingerprint();
        assert_eq!(sensor.fingerprint(), fingerprint);
        sensor.save();
        assert_eq!(sensor.fingerprint(), fingerprint);
        sensor.frame().unwrap();
        assert_eq!(wire.lock().unwrap().seconds, 0.01);
        sensor.frame().unwrap();
        assert_eq!(wire.lock().unwrap().seconds, 0.004);
        sensor.restore_checkpoint(&checkpoint).unwrap();
        assert_eq!(sensor.fingerprint(), fingerprint);
        sensor.frame().unwrap();
        assert_eq!(wire.lock().unwrap().seconds, 0.01);
        sensor.frame().unwrap();
        assert_eq!(wire.lock().unwrap().seconds, 0.004);
    }

    #[test]
    fn frame_clock_follows_mode_reference_lines_and_raster_extension() {
        let period: fujimod_emulator_imaging::timing::FramePeriod = Default::default();
        let exposure: ExposureHandle = Default::default();
        let mut s = Sensor::new(
            Options {
                pixel_clock_hz: 33_000_000,
                shutter: None,
                rolling_references: vec![RollingReference {
                    mode: 0xec,
                    horizontal: 706,
                    vertical: 9348,
                    lines: 857,
                }],
                mode_change_frames: 0,
                preview_field: None,
            },
            exposure.clone(),
            std::sync::Arc::new(std::sync::Mutex::new(8.)),
        )
        .unwrap();
        s.attach_frame_clock(period.clone(), 12_000_000).unwrap();
        assert_eq!(*period.lock().unwrap(), 0);
        s.timing(734, 450, None).unwrap();
        assert_eq!(*period.lock().unwrap(), 120109);
        let saved = s.checkpoint().unwrap();
        let saved_period = *period.lock().unwrap();
        s.command(&[0x81, 2, 1, 0, 0, 0, 0, 0]).unwrap();
        assert_eq!(*period.lock().unwrap(), 240218);
        // Exposure SHR and analog gain do not alter the frame period.
        s.command(&[0x81, 5, 100, 0, 0, 0, 0, 0, 0x81, 0x32, 128, 0, 0, 0, 0, 0])
            .unwrap();
        assert_eq!(*period.lock().unwrap(), 240218);
        s.restore_checkpoint(&saved).unwrap();
        *period.lock().unwrap() = saved_period;
        assert_eq!(*period.lock().unwrap(), 120109);
        s.timing(706, 9348, None).unwrap();
        // Before the drive mode selects the reference, the padded receiver
        // raster times frames; mode 0xec's 857 lines then give 54.54 fps
        // (camera: 54.54 fps idle in drive mode MV).
        assert_eq!(*period.lock().unwrap(), 2_399_887);
        s.command(&[0x81, 0, 0xec, 0, 0, 0, 0, 0]).unwrap();
        assert_eq!(*period.lock().unwrap(), 220_015);
        assert!((exposure.lock().unwrap().seconds - 857. * 706. / 33_000_000.).abs() < 1e-12);
        // A reset reference belongs to the complete declared scan tuple.
        // Another raster retains its own counter extent and frame cadence.
        s.timing(706, 935, None).unwrap();
        s.command(&[0x81, 5, 0x9e, 0, 0, 0, 0, 0, 0x81, 6, 3, 0, 0, 0, 0, 0])
            .unwrap();
        assert_eq!(*period.lock().unwrap(), 240040);
        assert!((exposure.lock().unwrap().seconds - 9. * 706. / 33_000_000.).abs() < 1e-12);
        s.command(&[0x81, 2, 1, 0, 0, 0, 0, 0]).unwrap();
        assert_eq!(*period.lock().unwrap(), 480080);
        assert!((exposure.lock().unwrap().seconds - 944. * 706. / 33_000_000.).abs() < 1e-12);
    }
    #[test]
    fn register_shutter_gain_and_restore_change_physical_exposure() {
        let wire: ExposureHandle = Default::default();
        let mut s = Sensor::new(
            Options {
                pixel_clock_hz: 33000000,
                shutter: None,
                rolling_references: vec![],
                mode_change_frames: 0,
                preview_field: None,
            },
            wire.clone(),
            std::sync::Arc::new(std::sync::Mutex::new(8.)),
        )
        .unwrap();
        s.timing(660, 1000, None).unwrap();
        let frame = wire.lock().unwrap().seconds;
        assert_eq!(frame, 0.02);
        s.command(&[0x81, 5, 244, 0, 0, 0, 0, 0, 0x81, 6, 1, 0, 0, 0, 0, 0])
            .unwrap();
        assert_eq!(wire.lock().unwrap().seconds, 0.01);
        s.command(&[0x81, 0x32, 0, 0, 0, 0, 0, 0, 0x81, 0x33, 8, 0, 0, 0, 0, 0])
            .unwrap();
        assert!((wire.lock().unwrap().analog_gain - 2.).abs() < 0.001);
        let saved = s.save();
        s.timing(330, 1000, None).unwrap();
        assert_eq!(wire.lock().unwrap().seconds, 0.005);
        s.restore(&saved).unwrap();
        assert_eq!(wire.lock().unwrap().seconds, 0.01);
    }
    #[test]
    fn reset_reference_and_leaf_closure_survive_the_readout_phase() {
        let wire: ExposureHandle = Default::default();
        let mut s = Sensor::new(
            Options {
                pixel_clock_hz: 33_000_000,
                shutter: Some(crate::leaf_shutter::Options {
                    closing_delay_seconds: vec![(2., 0.003), (16., 0.003)],
                }),
                rolling_references: vec![RollingReference {
                    mode: 0xec,
                    horizontal: 660,
                    vertical: 9000,
                    lines: 1000,
                }],
                mode_change_frames: 0,
                preview_field: None,
            },
            wire.clone(),
            std::sync::Arc::new(std::sync::Mutex::new(8.)),
        )
        .unwrap();
        let command = |pairs: &[(u8, u8)]| {
            pairs
                .iter()
                .flat_map(|&(r, v)| [0x81, r, v, 0, 0, 0, 0, 0])
                .collect::<Vec<_>>()
        };
        s.timing(660, 9000, Some(6600)).unwrap();
        s.command(&command(&[(0, 0xec), (5, 244), (6, 1)])).unwrap();
        assert!((wire.lock().unwrap().seconds - 0.01).abs() < 1e-12);
        s.timing(660, 1000, None).unwrap();
        s.command(&command(&[(0, 0x2d), (1, 4), (5, 100), (6, 0)]))
            .unwrap();
        assert_eq!(wire.lock().unwrap().seconds, 0.);
        s.timing(660, 1000, Some(6600)).unwrap();
        assert!((wire.lock().unwrap().seconds - 0.0012).abs() < 1e-12);
        s.timing(660, 1000, None).unwrap();
        assert!((wire.lock().unwrap().seconds - 0.0012).abs() < 1e-12);
        s.command(&command(&[(1, 0), (0, 0x2c)])).unwrap();
        s.timing(660, 3374, None).unwrap();
        assert!((wire.lock().unwrap().seconds - 0.0012).abs() < 1e-12);
        let saved = s.save();
        s.command(&command(&[(0, 0xec)])).unwrap();
        assert!((wire.lock().unwrap().seconds - 0.06548).abs() < 1e-12);
        s.restore(&saved).unwrap();
        assert!((wire.lock().unwrap().seconds - 0.0012).abs() < 1e-12);
    }

    #[test]
    fn drive_mode_change_withholds_valid_frames() {
        let wire: ExposureHandle = Default::default();
        let mut sensor = Sensor::new(
            Options {
                pixel_clock_hz: 33_000_000,
                shutter: None,
                rolling_references: vec![],
                mode_change_frames: 2,
                preview_field: None,
            },
            wire,
            std::sync::Arc::new(std::sync::Mutex::new(8.)),
        )
        .unwrap();
        sensor.timing(660, 1000, None).unwrap();
        let mode = |value| [0x81, 0, value, 0, 0, 0, 0, 0];
        sensor.command(&mode(0xec)).unwrap();
        let frames: Vec<bool> = (0..3).map(|_| sensor.frame().unwrap()).collect();
        assert_eq!(frames, [false, false, true]);
        // Bit 0 enters standby without changing the drive mode. Receiver
        // clocks cannot consume startup readouts while the sensor is stopped.
        sensor.command(&mode(0xed)).unwrap();
        let saved = sensor.checkpoint().unwrap();
        for _ in 0..5 { assert!(!sensor.frame().unwrap()); }
        sensor.command(&mode(0xec)).unwrap();
        let frames: Vec<bool> = (0..3).map(|_| sensor.frame().unwrap()).collect();
        assert_eq!(frames, [false, false, true]);
        sensor.restore_checkpoint(&saved).unwrap();
        assert!(!sensor.frame().unwrap());
        sensor.command(&mode(0xec)).unwrap();
        let frames: Vec<bool> = (0..3).map(|_| sensor.frame().unwrap()).collect();
        assert_eq!(frames, [false, false, true]);
        sensor.command(&mode(0x28)).unwrap();
        let frames: Vec<bool> = (0..3).map(|_| sensor.frame().unwrap()).collect();
        assert_eq!(frames, [false, false, true]);
    }
}
