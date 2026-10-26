//! A declared response from driven GPIO pads to sensed GPIO pad levels.
//! Its mechanical identity is unknown; scenarios provide the pins, phases,
//! initial levels and physical GPIO interrupt routes.
use fujimod_emulator_devices::{Cx, Device, parts::pins::{Pin, Pins}};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Phase {
    pub drive: [bool; 2],
    pub sense: [bool; 2],
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub drive_pins: [Pin; 2],
    pub sense_pins: [Pin; 2],
    pub phases: Vec<Phase>,
    pub initial_phase: usize,
    pub settle_ticks: u64,
}

pub struct Wiring {
    pub pins: Box<dyn Pins>,
}

#[derive(Serialize, Deserialize)]
struct State {
    observed_drive: [bool; 2],
    phase: usize,
    due: Option<u64>,
    transitions: u64,
}

pub struct GpioContactResponse {
    options: Options,
    wiring: Wiring,
    state: State,
}

impl GpioContactResponse {
    pub fn new(options: Options, wiring: Wiring) -> Result<Self, String> {
        if options.settle_ticks == 0 {
            return Err("GPIO contact response needs a positive settling time".into());
        }
        if options.phases.is_empty() || options.initial_phase >= options.phases.len() {
            return Err("GPIO contact response needs phases and a valid initial phase".into());
        }
        for (index, phase) in options.phases.iter().enumerate() {
            if options.phases[..index]
                .iter()
                .any(|previous| previous.drive == phase.drive)
            {
                return Err("GPIO contact response has duplicate drive phases".into());
            }
        }
        let pins = options
            .drive_pins
            .into_iter()
            .chain(options.sense_pins)
            .collect::<Vec<_>>();
        for (index, pin) in pins.iter().enumerate() {
            pin.validate()?;
            if pins[..index].contains(pin) {
                return Err("GPIO contact response drive and sense pins must be distinct".into());
            }
        }
        if options.sense_pins.iter().any(|pin| pin.port >= 16) {
            return Err("GPIO contact response sense pins need input-status pads".into());
        }

        let initial_drive = options.phases[options.initial_phase].drive;
        let configured_drive = [
            wiring.pins.output_level(options.drive_pins[0])?,
            wiring.pins.output_level(options.drive_pins[1])?,
        ];
        if configured_drive != initial_drive.map(Some) {
            return Err(
                "GPIO contact response initial drive levels do not match its initial phase".into(),
            );
        }

        let initial_phase = options.initial_phase;
        Ok(Self {
            options,
            wiring,
            state: State {
                observed_drive: initial_drive,
                phase: initial_phase,
                due: None,
                transitions: 0,
            },
        })
    }

    fn drive(&self) -> Result<[bool; 2], String> {
        let levels = [
            self.wiring.pins.output_level(self.options.drive_pins[0])?,
            self.wiring.pins.output_level(self.options.drive_pins[1])?,
        ];
        match levels {
            [Some(first), Some(second)] => Ok([first, second]),
            [None, None] => {
                Err("GPIO contact response drive pins have no declared output levels".into())
            }
            _ => {
                Err("GPIO contact response drive pins must both have declared output levels".into())
            }
        }
    }

    fn step(&mut self, cx: &mut Cx<'_>) -> Result<(), String> {
        let drive = self.drive()?;
        if drive != self.state.observed_drive {
            self.state.observed_drive = drive;
            self.state.due = Some(
                cx.now()
                    .checked_add(self.options.settle_ticks)
                    .ok_or("GPIO contact response deadline overflow")?,
            );
        }
        let Some(due) = self.state.due else {
            return Ok(());
        };
        if due > cx.now() {
            return Ok(());
        }
        self.state.due = None;
        let phase = self
            .options
            .phases
            .iter()
            .position(|phase| phase.drive == drive)
            .ok_or("GPIO contact response has no declared phase for drive levels")?;
        if phase == self.state.phase {
            return Ok(());
        }
        self.state.phase = phase;
        self.state.transitions += 1;
        for index in 0..2 {
            self.wiring.pins.input(
                cx,
                self.options.sense_pins[index],
                self.options.phases[phase].sense[index],
            )?;
        }
        Ok(())
    }
}

impl Device for GpioContactResponse {
    fujimod_emulator_devices::checkpoint_fields!(state: State);
    fn name(&self) -> &'static str {
        "gpio-contact-response"
    }
    fn fingerprint(&self) -> Result<serde_json::Value, String> {
        Ok(serde_json::json!({"options":&self.options,"state":&self.state}))
    }
    fn save(&self) -> serde_json::Value {
        serde_json::to_value(&self.state).unwrap()
    }
    fn restore(&mut self, value: &serde_json::Value) -> Result<(), String> {
        let state: State =
            serde_json::from_value(value.clone()).map_err(|error| error.to_string())?;
        if state.phase >= self.options.phases.len() {
            return Err("GPIO contact response checkpoint has an invalid phase".into());
        }
        self.state = state;
        Ok(())
    }
    fn read(&mut self, cx: &mut Cx<'_>, _: u64, _: u8) -> u64 {
        cx.unmodeled();
        0
    }
    fn write(&mut self, cx: &mut Cx<'_>, _: u64, _: u8, _: u64) {
        cx.unmodeled();
    }
    fn next_deadline(&self, now: u64) -> Option<u64> {
        match self.drive() {
            Ok(drive) if drive != self.state.observed_drive => Some(now),
            Err(_) => Some(now),
            _ => self.state.due,
        }
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        if let Err(error) = self.step(cx) {
            cx.fail(error);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::{bus::InterruptLine, exr2::gpio::{BASE, Edge, Gpio, InitialOutput, InputIrq, Options as GpioOptions, Wiring as GpioWiring}, test_host::{TestHost, advance, write}};

    fn pin(port: u8, pin: u8) -> Pin {
        Pin { port, pin }
    }

    fn options() -> Options {
        Options {
            drive_pins: [pin(1, 1), pin(1, 2)],
            sense_pins: [pin(0, 9), pin(0, 6)],
            phases: vec![
                Phase {
                    drive: [false, false],
                    sense: [true, true],
                },
                Phase {
                    drive: [true, false],
                    sense: [false, false],
                },
                Phase {
                    drive: [true, true],
                    sense: [true, true],
                },
            ],
            initial_phase: 0,
            settle_ticks: 3000,
        }
    }

    fn gpio() -> Gpio {
        let mut input_levels = [0; 16];
        input_levels[0] = (1 << 9) | (1 << 6);
        Gpio::new(
            GpioOptions {
                input_levels,
                output_levels: vec![
                    InitialOutput {
                        pin: pin(1, 1),
                        high: false,
                    },
                    InitialOutput {
                        pin: pin(1, 2),
                        high: false,
                    },
                ],
            },
            GpioWiring {
                irqs: vec![
                    InputIrq {
                        pin: pin(0, 9),
                        edge: Edge::Rising,
                        outputs: vec![InterruptLine::Core { core: 0, line: 17 }],
                    },
                    InputIrq {
                        pin: pin(0, 6),
                        edge: Edge::Falling,
                        outputs: vec![InterruptLine::Core { core: 0, line: 17 }],
                    },
                ],
            },
        )
        .unwrap()
    }

    #[test]
    fn selected_xm1_phase_table_settles_and_gpio_owns_physical_irq_edges() {
        let mut host = TestHost::new();
        let mut gpio = gpio();
        let pads = gpio.pads();
        let mut response = GpioContactResponse::new(
            options(),
            Wiring {
                pins: Box::new(pads.clone()),
            },
        )
        .unwrap();
        assert_eq!(response.name(), "gpio-contact-response");
        assert_eq!(response.next_deadline(0), None);

        write(&mut gpio, &mut host, BASE + 0x40, 0x0006_0002);
        assert_eq!(response.next_deadline(0), Some(0));
        advance(&mut response, &mut host, 0);
        assert_eq!(response.next_deadline(0), Some(3000));
        let saved = response.save();
        let fingerprint = response.fingerprint().unwrap();
        advance(&mut response, &mut host, 2999);
        assert_eq!(response.save(), saved);
        assert_eq!(response.fingerprint().unwrap(), fingerprint);
        assert!(pads.input_level(pin(0, 9)).unwrap());
        advance(&mut response, &mut host, 3000);
        assert!(!pads.input_level(pin(0, 9)).unwrap());
        assert!(!pads.input_level(pin(0, 6)).unwrap());
        assert_eq!(host.irqs, [(0, 17)]);

        write(&mut gpio, &mut host, BASE + 0x40, 0x0006_0006);
        advance(&mut response, &mut host, 3000);
        assert_eq!(response.next_deadline(3001), Some(6000));
        advance(&mut response, &mut host, 5999);
        advance(&mut response, &mut host, 6000);
        assert!(pads.input_level(pin(0, 9)).unwrap());
        assert!(pads.input_level(pin(0, 6)).unwrap());
        assert_eq!(host.irqs, [(0, 17), (0, 17)]);
        assert_eq!(response.save()["transitions"], 2);

        write(&mut gpio, &mut host, BASE + 0x40, 0x0006_0004);
        advance(&mut response, &mut host, 6000);
        host.now = 9000;
        let mut cx = host.cx();
        response.advance(&mut cx);
        assert!(cx.refusal().unwrap().contains("no declared phase"));
    }

    #[test]
    fn constructor_checks_scenario_initial_levels_and_phase_data() {
        let gpio = gpio();
        let wiring = || Wiring {
            pins: Box::new(gpio.pads()),
        };
        let mut invalid_initial = options();
        invalid_initial.initial_phase = 1;
        assert!(
            GpioContactResponse::new(invalid_initial, wiring())
                .err()
                .unwrap()
                .contains("initial drive levels")
        );

        let mut duplicate = options();
        duplicate.phases[2].drive = duplicate.phases[1].drive;
        assert!(
            GpioContactResponse::new(duplicate, wiring())
                .err()
                .unwrap()
                .contains("duplicate drive phases")
        );
    }
}
