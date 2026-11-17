//! Four-byte I2C motor bridge, clocked by a muxed PWM/GPIO pad.
//! Register 0 selects enable/reset/direction; register 2 selects division.
//! PWM cycle IRQs are independent of microstep angle. Relative position units
//! and the pulse unit's clock scale are declared inputs, not calibrated values.
//! Completed I2C writes call `write_register`; host reads are observational.
use fujimod_emulator_devices::exr2::gpio::Pads;
use fujimod_emulator_devices::exr2::pulse::{PulsePins, Pwm};
use fujimod_emulator_devices::parts::pins::{Pin, Pins};
use fujimod_emulator_devices::{Cx, Device, bus::InterruptLine};
use serde::{Deserialize, Serialize};
use fujimod_emulator_devices::parts::lens_bridge::Registers;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub pulse_channel: u8,
    pub pulse_pin: Pin,
    pub pulse_function: u8,
    pub position_units_per_step: u64,
    pub initial_position: i64,
    /// Mechanical end stops: steps past them are absorbed (an iris homing
    /// against its stop). Unbounded when absent.
    #[serde(default)]
    pub travel_limits: Option<[i64; 2]>,
}
pub struct Wiring {
    pub registers: Registers,
    pub pulses: PulsePins,
    pub gpio: Pads,
    pub clock_irq: InterruptLine,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
struct Configuration {
    control: u8,
    divider: u64,
    clock: Option<Pwm>,
    manual: Option<bool>,
}
pub struct Bridge {
    options: Options,
    wiring: Wiring,
    configuration: Option<Configuration>,
    due: Option<u64>,
    position: i64,
    /// Iris output (f-number squared) and its position/f-number calibration.
    aperture: Option<(fujimod_emulator_devices::parts::optics::ApertureHandle, Vec<(i64, f64)>)>,
}
impl Bridge {
    pub fn new(options: Options, wiring: Wiring) -> Result<Self, String> {
        options.pulse_pin.validate()?;
        if options.pulse_channel >= 12
            || options.position_units_per_step == 0
            || !options.position_units_per_step.is_multiple_of(64)
            || !(1..=3).contains(&options.pulse_function)
        {
            return Err(
                "lens bridge requires a pulse pad and position units divisible by 64".into(),
            );
        }
        if options.travel_limits.is_some_and(|[lo, hi]| {
            lo >= hi || !(lo..=hi).contains(&options.initial_position)
        }) {
            return Err("lens bridge travel limits must enclose its initial position".into());
        }
        Ok(Self {
            position: options.initial_position,
            options,
            wiring,
            configuration: None,
            due: None,
            aperture: None,
        })
    }
    /// Publish the iris f-number from this bridge's position.
    pub fn attach_aperture(
        &mut self,
        output: fujimod_emulator_devices::parts::optics::ApertureHandle,
        positions: Vec<(i64, f64)>,
    ) -> Result<(), String> {
        if positions.len() < 2
            || positions.windows(2).any(|p| p[0].0 >= p[1].0)
            || positions.iter().any(|p| !p.1.is_finite() || p.1 <= 0.)
        {
            return Err("iris requires ordered physical position/f-number calibration".into());
        }
        self.aperture = Some((output, positions));
        self.publish_aperture()
    }
    fn publish_aperture(&self) -> Result<(), String> {
        let Some((output, positions)) = &self.aperture else {
            return Ok(());
        };
        let x = self.position;
        let first = positions[0];
        let last = *positions.last().unwrap();
        let f = if x <= first.0 {
            first.1
        } else if x >= last.0 {
            last.1
        } else {
            let pair = positions
                .windows(2)
                .find(|p| x >= p[0].0 && x <= p[1].0)
                .unwrap();
            let t = (x - pair[0].0) as f64 / (pair[1].0 - pair[0].0) as f64;
            libm::exp(libm::log(pair[0].1) * (1. - t) + libm::log(pair[1].1) * t)
        };
        *output.lock().map_err(|_| "iris transmission poisoned")? = f * f;
        Ok(())
    }
    pub fn position(&self) -> i64 {
        self.position
    }
    fn configuration(&self) -> Result<Configuration, String> {
        let r = self.wiring.registers.snapshot();
        let control = r[0];
        let divider = [64, 32, 16, 8][(r[2] >> 4 & 3) as usize];
        let (clock, manual) = match self.wiring.gpio.function(self.options.pulse_pin)? {
            0 if control & 0xc0 == 0xc0 => (
                None,
                Some(
                    self.wiring
                        .gpio
                        .output_level(self.options.pulse_pin)?
                        .ok_or("GPIO output has no declared or guest-written level")?,
                ),
            ),
            0 => (None, None),
            function if function == self.options.pulse_function => {
                // A constant high output drives a DC motor but supplies
                // no rising clock edges to this step bridge.
                (
                    self.wiring
                        .pulses
                        .pwm(self.options.pulse_channel)?
                        .filter(|pwm| u32::from(pwm.duty) < pwm.counts),
                    None,
                )
            }
            _ if control & 0xc0 != 0xc0 => (None, None),
            _ => return Err("lens bridge clock pad selects an unattached function".into()),
        };
        Ok(Configuration {
            control,
            divider,
            clock,
            manual,
        })
    }
    fn run(&mut self, cx: &mut Cx<'_>) -> Result<(), String> {
        let configuration = self.configuration()?;
        if self.configuration != Some(configuration) {
            if self
                .configuration
                .is_some_and(|old| old.manual == Some(false))
                && configuration.manual == Some(true)
            {
                self.step(configuration, 1)?;
            }
            self.due = if let Some(clock) = &configuration.clock {
                Some(
                    cx.now()
                        .checked_add(clock.period)
                        .ok_or("lens bridge deadline overflow")?,
                )
            } else {
                None
            };
            self.configuration = Some(configuration);
            return Ok(());
        }
        let Some(due) = self.due.filter(|due| *due <= cx.now()) else {
            return Ok(());
        };
        let period = configuration.clock.unwrap().period;
        let edges = 1 + (cx.now() - due) / period;
        self.step(configuration, edges)?;
        self.due = Some(
            edges
                .checked_mul(period)
                .and_then(|d| due.checked_add(d))
                .ok_or("lens bridge deadline overflow")?,
        );
        // The timer IRQ counts PWM cycles, independent of the bridge's
        // microstep division. GPIO-generated clocks do not run that timer.
        match self.wiring.clock_irq {
            InterruptLine::Core { core, line } => cx.raise_irq(core, line)?,
            InterruptLine::Shared(line) => cx.set_irq(line, true),
        }
        Ok(())
    }
    fn step(&mut self, configuration: Configuration, edges: u64) -> Result<(), String> {
        if configuration.control & 0xc0 != 0xc0 {
            return Ok(());
        }
        let delta = edges
            .checked_mul(self.options.position_units_per_step / configuration.divider)
            .and_then(|v| i64::try_from(v).ok())
            .ok_or("lens bridge travel overflow")?;
        self.position = self
            .position
            .checked_add(if configuration.control & 0x10 != 0 {
                -delta
            } else {
                delta
            })
            .ok_or("lens bridge position overflow")?;
        if let Some([lo, hi]) = self.options.travel_limits {
            self.position = self.position.clamp(lo, hi);
        }
        self.publish_aperture()
    }
}
impl Device for Bridge {
    fujimod_emulator_devices::checkpoint_fields!(configuration:Option<Configuration>,due:Option<u64>,position:i64);
    fn fingerprint(&self) -> Result<serde_json::Value, String> {
        Ok(
            serde_json::json!({"configuration":self.configuration,"due":self.due,"position":self.position}),
        )
    }

    fn name(&self) -> &'static str {
        "i2c-motor-bridge"
    }
    fn save(&self) -> serde_json::Value {
        serde_json::json!({"configuration":self.configuration,"due":self.due,"position":self.position})
    }
    fn read(&mut self, cx: &mut Cx<'_>, _: u64, _: u8) -> u64 {
        cx.unmodeled();
        0
    }
    fn write(&mut self, cx: &mut Cx<'_>, _: u64, _: u8, _: u64) {
        cx.unmodeled();
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        if let Err(e) = self.run(cx) {
            cx.fail(e);
        }
    }
    fn event(&mut self, cx: &mut Cx<'_>, _: u64) {
        self.advance(cx);
    }
    fn next_deadline(&self, now: u64) -> Option<u64> {
        match self.configuration() {
            Ok(c) if Some(c) == self.configuration => self.due,
            _ => Some(now),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::exr2::gpio;
    use fujimod_emulator_devices::exr2::pulse::{Options as PulseOptions, PulseUnit, Wiring as PulseWiring};
    use std::sync::{Arc, Mutex};

    fn bridge(travel_limits: Option<[i64; 2]>, initial_position: i64) -> Result<Bridge, String> {
        let gpio = gpio::Gpio::new(
            gpio::Options { input_levels: [0; 16], output_levels: vec![], high_port_input_levels: None },
            gpio::Wiring { irqs: vec![] },
        )
        .unwrap();
        let pulse = PulseUnit::new(
            PulseOptions {
                counter_channels: Vec::new(),
                timer_channels: Vec::new(),
                one_shot_channels: Vec::new(),
                counter_readback_channels: vec![],
                free_running_channels: vec![],
                mirrored_irqs: vec![],
                quadrature: false,
                initial_phase: 0,
                capture_channel: None,
            },
            PulseWiring { irqs: [None; 12] },
        )
        .unwrap();
        Bridge::new(
            Options {
                pulse_channel: 6,
                pulse_pin: Pin { port: 1, pin: 6 },
                pulse_function: 1,
                position_units_per_step: 64,
                initial_position,
                travel_limits,
            },
            Wiring {
                registers: Registers::default(),
                pulses: pulse.pins(),
                gpio: gpio.pads(),
                clock_irq: InterruptLine::Core { core: 0, line: 12 },
            },
        )
    }
    // X100T-like iris: position -102 is f/2, -11 is f/16.
    const IRIS: [(i64, f64); 3] = [(-102, 2.0), (-54, 5.6568), (-11, 16.0)];
    fn moving(close: bool) -> Configuration {
        Configuration { control: 0xc0 | if close { 0x10 } else { 0 }, divider: 64, clock: None, manual: None }
    }

    #[test]
    fn travel_limits_must_enclose_the_initial_position() {
        assert!(bridge(Some([-109, 0]), 5).is_err());
        assert!(bridge(Some([0, -109]), 0).is_err());
        assert!(bridge(Some([-109, 0]), 0).is_ok());
    }

    #[test]
    fn iris_output_follows_the_clamped_position() {
        let mut b = bridge(Some([-109, 0]), 0).unwrap();
        let aperture = Arc::new(Mutex::new(1.0));
        assert!(b.attach_aperture(aperture.clone(), vec![(0, 2.0)]).is_err());
        assert!(b.attach_aperture(aperture.clone(), vec![(0, 2.0), (-5, 4.0)]).is_err());
        b.attach_aperture(aperture.clone(), IRIS.to_vec()).unwrap();
        assert_eq!(*aperture.lock().unwrap(), 256.0); // beyond the last point: f/16
        b.step(moving(true), 500).unwrap(); // homing into the open stop
        assert_eq!(b.position(), -109);
        assert_eq!(*aperture.lock().unwrap(), 4.0); // f/2
        b.step(moving(false), 48).unwrap();
        assert_eq!(b.position(), -61);
        // Log-interpolated between f/2 (-102) and f/5.6568 (-54).
        let t = 41.0 / 48.0;
        let expected = libm::exp(libm::log(2.0) * (1.0 - t) + libm::log(5.6568) * t);
        assert!((*aperture.lock().unwrap() - expected * expected).abs() < 1e-9);
    }
}
