//! A command dial emits physical Gray-code levels at declared edge times.
//! Host inspection never emits an edge. Firmware decodes the GPIO levels;
//! optional counter wiring delivers the same phases to the pulse unit.
use fujimod_emulator_devices::parts::pins::{Pin, Pins};
use fujimod_emulator_devices::exr2::pulse::PulsePins;
use fujimod_emulator_devices::{Cx, Device};
use serde::Deserialize;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use fujimod_emulator_devices::parts::encoder_input::EncoderInput;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub pins: [Pin; 2],
    pub phase_ticks: u64,
    pub rest_phase: u8,
}
pub struct Wiring {
    pub gpio: Box<dyn Pins>,
    pub counter: Option<PulsePins>,
}
pub struct Encoder {
    options: Options,
    wiring: Wiring,
    input: EncoderInput,
    phase: u8,
}
impl Encoder {
    pub fn new(options: Options, wiring: Wiring) -> Result<Self, String> {
        for pin in options.pins {
            pin.validate()?;
        }
        if options.pins[0] == options.pins[1] || options.phase_ticks == 0 || options.rest_phase > 3
        {
            return Err(
                "encoder requires distinct pads, a positive edge period and a two-bit rest phase"
                    .into(),
            );
        }
        if wiring.counter.as_ref().is_some_and(|p| {
            !p.quadrature_wired() || p.phase() != [2, 3, 1, 0][options.rest_phase as usize]
        }) {
            return Err("dial counter connection must start at the declared rest phase".into());
        }
        let input = EncoderInput {
            events: Arc::new(Mutex::new(VecDeque::with_capacity(4096))),
            phase_ticks: options.phase_ticks,
            rest_phase: options.rest_phase,
        };
        let phase = options.rest_phase;
        Ok(Self {
            options,
            wiring,
            input,
            phase,
        })
    }
    /// Body initialization drives the declared external rest state explicitly.
    pub fn initialize(&mut self, cx: &mut Cx<'_>) -> Result<(), String> {
        for (pin, level) in self
            .options
            .pins
            .into_iter()
            .zip([self.phase & 2 != 0, self.phase & 1 != 0])
        {
            self.wiring.gpio.input(cx, pin, level)?;
        }
        Ok(())
    }
    pub fn queue_cycles(&mut self, now: u64, cycles: i32) -> Result<(), String> {
        self.input.queue_cycles(now, cycles)
    }
    pub fn input(&self) -> EncoderInput {
        self.input.clone()
    }
    pub fn phase(&self) -> u8 {
        self.phase
    }
}
impl Device for Encoder {
    fn checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        fujimod_emulator_devices::checkpoint::Checkpoint::encode(
            self.name(),
            &(
                self.phase,
                &*self
                    .input
                    .events
                    .lock()
                    .map_err(|_| "encoder queue poisoned")?,
            ),
        )
    }
    fn validate_checkpoint(&self, saved: &fujimod_emulator_devices::checkpoint::Checkpoint) -> Result<(), String> {
        let _: (u8, VecDeque<(u64, u8)>) = saved.decode(self.name())?;
        Ok(())
    }
    fn restore_checkpoint(&mut self, saved: &fujimod_emulator_devices::checkpoint::Checkpoint) -> Result<(), String> {
        let (phase, events) = saved.decode(self.name())?;
        self.phase = phase;
        *self
            .input
            .events
            .lock()
            .map_err(|_| "encoder queue poisoned")? = events;
        Ok(())
    }

    fn fingerprint(&self) -> Result<serde_json::Value, String> {
        Ok(
            serde_json::json!({"phase":self.phase,"rest_phase":self.input.rest_phase,"phase_ticks":self.input.phase_ticks,"events":*self.input.events.lock().unwrap()}),
        )
    }
    fn name(&self) -> &'static str {
        "encoder"
    }
    fn read(&mut self, cx: &mut Cx<'_>, _: u64, _: u8) -> u64 {
        cx.unmodeled();
        0
    }
    fn write(&mut self, cx: &mut Cx<'_>, _: u64, _: u8, _: u64) {
        cx.unmodeled();
    }
    fn next_deadline(&self, _: u64) -> Option<u64> {
        self.input.events.lock().unwrap().front().map(|e| e.0)
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        let mut events = self.input.events.lock().unwrap();
        while let Some(&(at, phase)) = events.front() {
            if at > cx.now() {
                break;
            }
            for (pin, level) in self
                .options
                .pins
                .into_iter()
                .zip([phase & 2 != 0, phase & 1 != 0])
            {
                if let Err(error) = self.wiring.gpio.input(cx, pin, level) {
                    cx.fail(error);
                    return;
                }
            }
            if let Some(counter) = &self.wiring.counter {
                let ordinal = [2, 3, 1, 0][phase as usize];
                if let Err(error) = counter.quadrature_phase(cx, ordinal) {
                    cx.fail(error);
                    return;
                }
            }
            self.phase = phase;
            events.pop_front();
        }
    }
    fn event(&mut self, cx: &mut Cx<'_>, _: u64) {
        self.advance(cx);
    }
}
