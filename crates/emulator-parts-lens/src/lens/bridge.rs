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
        Ok(Self {
            position: options.initial_position,
            options,
            wiring,
            configuration: None,
            due: None,
        })
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
            self.due = if configuration.clock.is_some() {
                Some(
                    cx.now()
                        .checked_add(configuration.clock.unwrap().period)
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
        Ok(())
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
