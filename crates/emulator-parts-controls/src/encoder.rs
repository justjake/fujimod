//! A command dial emits physical Gray-code levels at declared edge times.
//! Host inspection never emits an edge. Firmware decodes GPIO or ADC levels;
//! optional counter wiring delivers the same phases to the pulse unit.
//! ADC channels and their low/high raw codes are explicit body configuration.
use fujimod_emulator_devices::parts::pins::{Pin, Pins};
use fujimod_emulator_devices::parts::analog::Samples;
use fujimod_emulator_devices::exr2::pulse::PulsePins;
use fujimod_emulator_devices::{Cx, Device};
use serde::Deserialize;
use std::collections::VecDeque;
use parking_lot::Mutex;
use std::sync::Arc;
use fujimod_emulator_devices::parts::encoder_input::{EncoderInput, EncoderQueue};

#[derive(Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Levels {
    pub low: u16,
    pub high: u16,
}

#[derive(Default, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Outputs {
    #[default]
    Gpio,
    Adc {
        channels: [usize; 2],
        levels: [Levels; 2],
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    #[serde(default)]
    pub pins: Option<[Pin; 2]>,
    #[serde(default)]
    pub outputs: Outputs,
    pub phase_ticks: u64,
    pub rest_phase: u8,
}
pub enum OutputWiring {
    Gpio(Box<dyn Pins>),
    Adc(Samples),
}

pub struct Wiring {
    pub outputs: OutputWiring,
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
        if options.phase_ticks == 0 || options.rest_phase > 3 {
            return Err("encoder requires a positive edge period and a two-bit rest phase".into());
        }
        match (&options.outputs, &wiring.outputs) {
            (Outputs::Gpio, OutputWiring::Gpio(_)) => {
                let pins = options.pins.ok_or("GPIO encoder requires two pads")?;
                for pin in pins {
                    pin.validate()?;
                }
                if pins[0] == pins[1] {
                    return Err("encoder requires distinct pads".into());
                }
            }
            (Outputs::Adc { channels, levels }, OutputWiring::Adc(samples)) => {
                if options.pins.is_some() || channels[0] == channels[1] {
                    return Err("ADC encoder requires distinct channels and no GPIO pads".into());
                }
                for (&channel, level) in channels.iter().zip(levels) {
                    samples.get(channel)?;
                    if level.low >= level.high {
                        return Err("ADC encoder low code must be below its high code".into());
                    }
                }
            }
            _ => return Err("encoder output configuration differs from its wiring".into()),
        }
        if wiring.counter.as_ref().is_some_and(|p| {
            !p.quadrature_wired() || p.phase() != [2, 3, 1, 0][options.rest_phase as usize]
        }) {
            return Err("dial counter connection must start at the declared rest phase".into());
        }
        if let (Outputs::Adc { channels, levels }, OutputWiring::Adc(samples)) =
            (&options.outputs, &wiring.outputs)
        {
            Self::drive_adc(samples, channels, levels, options.rest_phase)?;
        }
        let input = EncoderInput {
            events: Arc::new(Mutex::new(EncoderQueue {
                events: VecDeque::with_capacity(4096),
                final_phase: options.rest_phase,
            })),
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
        Self::drive_phase(&self.options, &mut self.wiring.outputs, cx, self.phase)
    }
    fn drive_adc(
        samples: &Samples,
        channels: &[usize; 2],
        levels: &[Levels; 2],
        phase: u8,
    ) -> Result<(), String> {
        for ((&channel, level), high) in channels
            .iter()
            .zip(levels)
            .zip([phase & 2 != 0, phase & 1 != 0])
        {
            samples.set(channel, if high { level.high } else { level.low })?;
        }
        Ok(())
    }
    fn drive_phase(
        options: &Options,
        outputs: &mut OutputWiring,
        cx: &mut Cx<'_>,
        phase: u8,
    ) -> Result<(), String> {
        let high = [phase & 2 != 0, phase & 1 != 0];
        match (&options.outputs, outputs) {
            (Outputs::Gpio, OutputWiring::Gpio(gpio)) => {
                for (pin, level) in options.pins.unwrap().into_iter().zip(high) {
                    gpio.input(cx, pin, level)?;
                }
            }
            (Outputs::Adc { channels, levels }, OutputWiring::Adc(samples)) => {
                Self::drive_adc(samples, channels, levels, phase)?;
            }
            _ => return Err("encoder output configuration differs from its wiring".into()),
        }
        Ok(())
    }
    pub fn queue_cycles(&mut self, now: u64, cycles: i32) -> Result<(), String> {
        self.input.queue_cycles(now, cycles)
    }
    pub fn queue_edges(&mut self, now: u64, edges: i32) -> Result<(), String> {
        self.input.queue_edges(now, edges)
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
                &self.input.events.lock().events,
            ),
        )
    }
    fn validate_checkpoint(&self, saved: &fujimod_emulator_devices::checkpoint::Checkpoint) -> Result<(), String> {
        let _: (u8, VecDeque<(u64, u8)>) = saved.decode(self.name())?;
        Ok(())
    }
    fn restore_checkpoint(&mut self, saved: &fujimod_emulator_devices::checkpoint::Checkpoint) -> Result<(), String> {
        let (phase, events): (u8, VecDeque<(u64, u8)>) = saved.decode(self.name())?;
        self.phase = phase;
        let mut queue = self.input.events.lock();
        queue.final_phase = events.back().map_or(phase, |event| event.1);
        queue.events = events;
        Ok(())
    }

    fn fingerprint(&self) -> Result<serde_json::Value, String> {
        Ok(
            serde_json::json!({"phase":self.phase,"rest_phase":self.input.rest_phase,"phase_ticks":self.input.phase_ticks,"events":self.input.events.lock().events}),
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
        self.input.events.lock().events.front().map(|e| e.0)
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        let mut queue = self.input.events.lock();
        while let Some(&(at, phase)) = queue.events.front() {
            if at > cx.now() {
                break;
            }
            if let Err(error) = Self::drive_phase(
                &self.options,
                &mut self.wiring.outputs,
                cx,
                phase,
            ) {
                cx.fail(error);
                return;
            }
            if let Some(counter) = &self.wiring.counter {
                let ordinal = [2, 3, 1, 0][phase as usize];
                if let Err(error) = counter.quadrature_phase(cx, ordinal) {
                    cx.fail(error);
                    return;
                }
            }
            self.phase = phase;
            queue.events.pop_front();
        }
    }
    fn event(&mut self, cx: &mut Cx<'_>, _: u64) {
        self.advance(cx);
    }
}
