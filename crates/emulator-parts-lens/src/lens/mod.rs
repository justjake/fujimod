//! Lens unit with banked 6/10-bit, mixed 4/12 + 7/9-bit, or three-byte
//! register protocols and separate focus, iris, and optional DC zoom mechanics.
//! The banked protocol uses a register-F strobe. No command-history signatures
//! or firmware state select responses. Initial positions and home vanes set
//! the declared starting state.
mod banked_clock;
mod three_byte_clock;
pub use three_byte_clock::ThreeByteClock;
pub mod bridge;
mod mechanics;
use fujimod_emulator_devices::exr2::pulse::{PulsePins, Pwm};
use fujimod_emulator_devices::parts::pins::Pins;
use fujimod_emulator_parts_io::spi::{Endpoint, Shift};
use fujimod_emulator_devices::{Cx, bus::InterruptLine};
pub use banked_clock::BankedClock;
use mechanics::Mechanics;
pub use mechanics::{CounterOutput, EncoderOptions, MotorOptions, Side, TravelBoundary, Vane};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StopResponse {
    Silent,
    Settled,
}
#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Protocol {
    Banked {
        positive_direction_high: bool,
        stop_response: StopResponse,
        clock: [BankedClock; 2],
    },
    MixedWidth {
        period_ticks: u64,
    },
    ThreeByte {
        clock: ThreeByteClock,
    },
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ZoomOptions {
    pub pulse_channel: u8,
    pub counter_channel: u8,
    pub edge_scale: u64,
    pub initial_position: i64,
    pub travel_limits: [i64; 2],
    pub home: Vane,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub protocol: Protocol,
    pub motors: [MotorOptions; 2],
    pub encoder: Option<EncoderOptions>,
    pub zoom: Option<ZoomOptions>,
}
pub struct Wiring {
    pub pulses: PulsePins,
    pub gpio: Box<dyn Pins>,
    pub completion: [InterruptLine; 2],
    pub position: Option<fujimod_emulator_devices::parts::optics::Position>,
}
#[derive(Serialize, Deserialize)]
struct Zoom {
    options: ZoomOptions,
    position: i64,
    clock: Option<Pwm>,
    direction: u8,
    due: Option<u64>,
    level: bool,
    home_level: Option<bool>,
}
pub struct LensUnit {
    protocol: Protocol,
    registers: [u16; 128],
    frame: [u8; 3],
    length: usize,
    mechanics: Mechanics,
    zoom: Option<Zoom>,
    position: Option<fujimod_emulator_devices::parts::optics::Position>,
    aperture: Option<(fujimod_emulator_devices::parts::optics::ApertureHandle, Vec<(i64, f64)>)>,
    focus: Option<(fujimod_emulator_devices::parts::optics::FocusHandle, i64, f64)>,
}
impl LensUnit {
    pub fn new(options: Options, wiring: Wiring) -> Result<Self, String> {
        if let Protocol::Banked { clock, .. } = options.protocol {
            if clock.iter().any(|clock| {
                matches!(
                    clock,
                    BankedClock::EncoderStages {
                        ticks_per_quantum: 0
                    } | BankedClock::MicrostepStages {
                        ticks_per_quantum: 0
                    }
                )
            }) {
                return Err("banked lens clock quantum must be positive".into());
            }
        }
        if matches!(options.protocol, Protocol::MixedWidth { period_ticks: 0 }) {
            return Err("mixed-width lens requires a positive period scale".into());
        }
        if options.encoder.is_some() && !matches!(options.protocol, Protocol::MixedWidth { .. }) {
            return Err("lens quadrature requires the mixed-width protocol".into());
        }
        let zoom = if let Some(o) = options.zoom {
            if !matches!(options.protocol, Protocol::ThreeByte { .. })
                || o.edge_scale == 0
                || o.pulse_channel >= 12
                || !wiring.pulses.counter_wired(o.counter_channel)
                || !wiring.pulses.capture_wired()
                || o.travel_limits[0] >= o.travel_limits[1]
                || !(o.travel_limits[0]..=o.travel_limits[1]).contains(&o.initial_position)
                || !(o.travel_limits[0]..=o.travel_limits[1]).contains(&o.home.threshold)
            {
                return Err(
                    "DC zoom requires finite travel, a home vane, and explicit clocks".into(),
                );
            }
            o.home.pin.validate()?;
            Some(Zoom {
                position: o.initial_position,
                options: o,
                clock: None,
                direction: 0,
                due: None,
                level: false,
                home_level: None,
            })
        } else {
            None
        };
        let mechanics = Mechanics::new(
            options.motors,
            options.encoder,
            wiring.pulses,
            wiring.gpio,
            wiring.completion,
        )?;
        Ok(Self {
            protocol: options.protocol,
            registers: [0; 128],
            frame: [0; 3],
            length: 0,
            mechanics,
            zoom,
            position: wiring.position,
            aperture: None,
            focus: None,
        })
    }
    /// Explicit board initialization, before CPU release. It drives no IRQ
    /// directly: GPIO applies its configured physical edge law.
    pub fn initialize(&mut self, cx: &mut Cx<'_>) -> Result<(), String> {
        if let Some(position) = &self.position {
            position.publish(self.mechanics.motors[0].position);
        }
        self.mechanics.initialize(cx)?;
        if let Some(z) = &mut self.zoom {
            let level = z.options.home.level(z.position);
            self.mechanics.gpio.input(cx, z.options.home.pin, level)?;
            z.home_level = Some(level);
        }
        Ok(())
    }
    pub fn position(&self, axis: usize) -> Option<i64> {
        self.mechanics.motors.get(axis).map(|m| m.position)
    }
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
        self.publish_optics()
    }
    pub fn attach_focus(
        &mut self,
        output: fujimod_emulator_devices::parts::optics::FocusHandle,
        infinity_position: i64,
        steps_per_mm: f64,
    ) -> Result<(), String> {
        if !steps_per_mm.is_finite() || steps_per_mm <= 0. {
            return Err("focus axis requires positive finite steps/mm".into());
        }
        self.focus = Some((output, infinity_position, steps_per_mm));
        self.publish_optics()
    }
    fn publish_optics(&self) -> Result<(), String> {
        if let Some((output, infinity, scale)) = &self.focus {
            *output.lock().map_err(|_| "focus wire poisoned")? =
                (self.mechanics.motors[0].position as f64 - *infinity as f64) / scale;
        }
        if let Some((output, positions)) = &self.aperture {
            let x = self.mechanics.motors[1].position;
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
        }
        Ok(())
    }
    pub fn zoom_position(&self) -> Option<i64> {
        self.zoom.as_ref().map(|z| z.position)
    }
    pub fn register(&self, index: usize) -> Option<u16> {
        self.registers.get(index).copied()
    }
    fn quantity(&self, base: usize) -> u32 {
        u32::from(self.registers[base + 1] & 0x3ff)
            | u32::from(self.registers[base + 2] & 0x3ff) << 10
    }
    fn banked(&mut self, cx: &mut Cx<'_>, word: u16, now: u64) -> Result<(), String> {
        let Protocol::Banked {
            positive_direction_high,
            stop_response,
            clock,
        } = self.protocol
        else {
            unreachable!()
        };
        let address = (word >> 10) as usize;
        let data = word & 0x3ff;
        let bank = address >> 4;
        if word & 0xfc00 == 0xd800 && data & 1 != 0 {
            let stopped = self.mechanics.stop(0);
            if stopped && matches!(stop_response, StopResponse::Settled) {
                self.mechanics.begin(0, 0, 1, None, now, false)?;
            }
        } else if bank < 2 && address & 15 == 15 && data & 0x200 != 0 {
            if self.mechanics.motors[bank].pending.is_some() {
                return Err("lens axis strobed before previous motion completed".into());
            }
            let quantity = self.quantity(bank << 4);
            if !quantity.is_multiple_of(32) {
                return Err("banked lens quantity is not an integral encoder step".into());
            }
            let sign = if (data & 0x40 != 0) == positive_direction_high {
                1
            } else {
                -1
            };
            let profile = clock[bank].profile(&self.registers[bank * 16..bank * 16 + 16])?;
            if quantity == 0 {
                // The completed SPI frame is the only modeled delay for a
                // no-motion request. Do not hold the motor busy for its
                // configured motion-settle approximation.
                self.mechanics.complete_without_motion(cx, bank)?;
            } else {
                match profile {
                    Some(banked_clock::Profile::Encoder(profile)) => {
                        self.mechanics
                            .begin(bank, quantity / 32, sign, None, now, false)?;
                        self.mechanics.profile(bank, profile)?;
                    }
                    Some(banked_clock::Profile::Microstep(clock)) => {
                        self.mechanics
                            .begin_clocked(bank, quantity / 32, sign, clock, now)?;
                    }
                    None => self
                        .mechanics
                        .begin(bank, quantity / 32, sign, None, now, false)?,
                }
            }
        }
        self.registers[address] = data;
        Ok(())
    }
    fn mixed_address(word: u16) -> Option<(usize, u16)> {
        const ADDRESSES: [u16; 34] = [
            0x0000, 0x1000, 0x1200, 0x1400, 0x1600, 0x1800, 0x1a00, 0x1c00, 0x1e00, 0x2000, 0x3000,
            0x4000, 0x5000, 0x6000, 0x7000, 0x8000, 0x9000, 0x9200, 0x9400, 0x9600, 0x9800, 0x9a00,
            0x9c00, 0x9e00, 0xa000, 0xb000, 0xc000, 0xd000, 0xd200, 0xd400, 0xe000, 0xe400, 0xe800,
            0xec00,
        ];
        for address in ADDRESSES {
            let mask = if matches!(
                address,
                0 | 0x2000
                    | 0x3000
                    | 0x4000
                    | 0x5000
                    | 0x6000
                    | 0x7000
                    | 0x8000
                    | 0xa000
                    | 0xb000
                    | 0xc000
            ) {
                0xfff
            } else {
                0x1ff
            };
            if word & !mask == address {
                return Some(((address >> 9) as usize, word & mask));
            }
        }
        None
    }
    fn mixed(&mut self, word: u16, now: u64) -> Result<(), String> {
        let Protocol::MixedWidth { period_ticks } = self.protocol else {
            unreachable!()
        };
        let (address, data) =
            Self::mixed_address(word).ok_or("unknown mixed-width lens register")?;
        if matches!(address, 0x10 | 0x50) {
            let axis = usize::from(address == 0x50);
            if data & 0x800 != 0 {
                let base = axis * 0x40;
                let count = data & 0x3ff;
                let period = u64::from(
                    self.registers[base + 8] & 255 | (self.registers[base + 9] & 255) << 8,
                )
                .checked_mul(period_ticks)
                .ok_or("mixed-width lens period overflow")?;
                if period == 0 {
                    return Err("mixed-width lens requires a programmed period".into());
                }
                if axis != 0 || count != 0 {
                    self.mechanics.begin(
                        axis,
                        u32::from(count),
                        if data & 0x400 != 0 { -1 } else { 1 },
                        Some(period),
                        now,
                        true,
                    )?;
                }
            } else {
                self.mechanics.stop(axis);
            }
        }
        self.registers[address] = data;
        Ok(())
    }
    fn three_byte(&mut self, byte: u8, now: u64) -> Result<(), String> {
        self.frame[self.length] = byte;
        if self.length < 2 {
            self.length += 1;
            return Ok(());
        }
        let address = (self.frame[0] & 0x7f) as usize;
        let data = u16::from_be_bytes([self.frame[1], byte]);
        let bank = address >> 4;
        if bank > 6 && !matches!(address, 0x7e | 0x7f) {
            return Err("unknown three-byte lens bank".into());
        }
        if bank < 2 && address & 15 == 15 {
            if data & 2 == 0 {
                self.mechanics.stop(bank);
            } else if data & 8 != 0 {
                let quantity = self.quantity(bank << 4);
                if !quantity.is_multiple_of(32) {
                    return Err("three-byte lens quantity is not an integral encoder step".into());
                }
                let sign = if self.registers[bank << 4] & 0x40 != 0 {
                    -1
                } else {
                    1
                };
                let Protocol::ThreeByte { clock } = self.protocol else {
                    unreachable!()
                };
                let profile = clock.profile(&self.registers[bank * 16..bank * 16 + 16])?;
                self.mechanics
                    .begin(bank, quantity / 32, sign, None, now, false)?;
                if let Some(profile) = profile {
                    self.mechanics.profile(bank, profile)?;
                }
            }
        }
        self.registers[address] = if bank < 2 && address & 15 == 15 {
            data & !8
        } else {
            data
        };
        self.length = 0;
        Ok(())
    }
    fn zoom_clock(&self) -> Result<(u8, Option<Pwm>), String> {
        let z = self.zoom.as_ref().unwrap();
        let direction = (self.registers[0x30] >> 6 & 3) as u8;
        let clock = if matches!(direction, 1 | 2) {
            self.mechanics.pulses.pwm(z.options.pulse_channel)?
        } else {
            None
        };
        Ok((direction, clock))
    }
    fn zoom_period(z: &Zoom, clock: Pwm) -> Result<u64, String> {
        clock
            .period
            .checked_mul(z.options.edge_scale)
            .and_then(|v| v.checked_add(u64::from(clock.duty) - 1))
            .map(|v| (v / u64::from(clock.duty)).max(1))
            .ok_or_else(|| "DC zoom period overflow".into())
    }
    fn zoom_step(&mut self, cx: &mut Cx<'_>, at: u64) -> Result<(), String> {
        let Some(z) = &mut self.zoom else {
            return Ok(());
        };
        let Some(due) = z.due else {
            return Ok(());
        };
        if at < due {
            return Ok(());
        }
        let position = z
            .position
            .checked_add(if z.direction == 1 { 1 } else { -1 })
            .ok_or("DC zoom position overflow")?;
        if !(z.options.travel_limits[0]..=z.options.travel_limits[1]).contains(&position) {
            z.due = None;
            return Ok(());
        }
        z.position = position;
        let level = z.options.home.level(position);
        if z.home_level != Some(level) {
            self.mechanics.gpio.input(cx, z.options.home.pin, level)?;
            z.home_level = Some(level);
        }
        if self
            .mechanics
            .pulses
            .accepts_edges(z.options.counter_channel)
        {
            self.mechanics
                .pulses
                .counter_edges(z.options.counter_channel, 1)?;
        }
        z.level = !z.level;
        self.mechanics.pulses.capture_level(cx, z.level, due)?;
        z.due = Some(
            due.checked_add(Self::zoom_period(z, z.clock.unwrap())?)
                .ok_or("DC zoom deadline overflow")?,
        );
        Ok(())
    }
    fn advance_to(&mut self, cx: &mut Cx<'_>, at: u64) -> Result<(), String> {
        loop {
            let next = [
                self.mechanics.next_deadline(),
                self.zoom.as_ref().and_then(|z| z.due),
            ]
            .into_iter()
            .flatten()
            .min();
            let Some(next) = next.filter(|next| *next <= at) else {
                break;
            };
            self.mechanics.step(cx, next)?;
            if let Some(position) = &self.position {
                position.publish(self.mechanics.motors[0].position);
            }
            self.publish_optics()?;
            self.zoom_step(cx, next)?;
        }
        if self.zoom.is_some() {
            let (direction, clock) = self.zoom_clock()?;
            let z = self.zoom.as_mut().unwrap();
            if z.direction != direction || z.clock != clock {
                z.due = match clock {
                    Some(c) => Some(
                        at.checked_add(Self::zoom_period(z, c)?)
                            .ok_or("DC zoom deadline overflow")?,
                    ),
                    None => None,
                };
                z.direction = direction;
                z.clock = clock;
            }
        }
        Ok(())
    }
}
impl Endpoint for LensUnit {
    fn checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        Ok(fujimod_emulator_devices::checkpoint::Checkpoint::encode(
            self.name(),
            &(
                self.protocol,
                self.registers.as_slice(),
                self.frame,
                self.length,
                &self.zoom,
                self.focus
                    .as_ref()
                    .map(|(_, infinity, scale)| (*infinity, *scale)),
                self.aperture.as_ref().map(|(_, curve)| curve.as_slice()),
                self.position.is_some(),
            ),
        )?
        .with_children(vec![self.mechanics.checkpoint()?]))
    }
    fn validate_checkpoint(&self, saved: &fujimod_emulator_devices::checkpoint::Checkpoint) -> Result<(), String> {
        let (protocol, r, _, n, _, focus, iris, position): (
            Protocol,
            Vec<u16>,
            [u8; 3],
            usize,
            Option<Zoom>,
            Option<(i64, f64)>,
            Option<Vec<(i64, f64)>>,
            bool,
        ) = saved.decode(self.name())?;
        if protocol != self.protocol
            || focus
                != self
                    .focus
                    .as_ref()
                    .map(|(_, infinity, scale)| (*infinity, *scale))
            || iris.as_deref() != self.aperture.as_ref().map(|(_, curve)| curve.as_slice())
            || position != self.position.is_some()
            || r.len() != 128
            || n > 3
            || saved.children().len() != 1
        {
            return Err("lens snapshot framing differs".into());
        }
        self.mechanics.validate_checkpoint(&saved.children()[0])
    }
    fn restore_checkpoint(&mut self, saved: &fujimod_emulator_devices::checkpoint::Checkpoint) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        let (protocol, r, frame, length, zoom, _, _, _): (
            Protocol,
            Vec<u16>,
            [u8; 3],
            usize,
            Option<Zoom>,
            Option<(i64, f64)>,
            Option<Vec<(i64, f64)>>,
            bool,
        ) = saved.decode(self.name())?;
        self.mechanics.restore_checkpoint(&saved.children()[0])?;
        self.protocol = protocol;
        self.registers.copy_from_slice(&r);
        self.frame = frame;
        self.length = length;
        self.zoom = zoom;
        Ok(())
    }

    fn fingerprint(&self) -> Result<serde_json::Value, String> {
        Ok(
            serde_json::json!({"protocol":self.protocol,"registers":self.registers.as_slice(),
            "frame":self.frame,"length":self.length,"mechanics":self.mechanics.fingerprint(),"zoom":self.zoom,"iris_calibration":self.aperture.as_ref().map(|(_,curve)|curve),
            "focus_calibration":self.focus.as_ref().map(|(_,infinity,scale)|(infinity,scale))}),
        )
    }
    fn state(&self) -> serde_json::Value {
        serde_json::json!({"registers":self.registers.as_slice(),"positions":[self.position(0),self.position(1)],
            "motor_deadline":self.mechanics.next_deadline(),"zoom_position":self.zoom_position(),"zoom_deadline":self.zoom.as_ref().and_then(|z|z.due),"partial_bytes":&self.frame[..self.length]})
    }
    fn validate(&self, shift: Shift) -> Result<(), String> {
        if shift.receive {
            return Err("lens receive transactions are unsupported".into());
        }
        match self.protocol {
            Protocol::ThreeByte { .. } => {
                if shift.bits != 8 || shift.word > 255 || self.length == 0 && shift.word & 0x80 == 0
                {
                    return Err("three-byte lens requires write-bit byte frames".into());
                }
            }
            Protocol::MixedWidth { .. } => {
                if shift.bits != 16 || Self::mixed_address(shift.word).is_none() {
                    return Err("invalid mixed-width lens frame".into());
                }
            }
            Protocol::Banked { .. } => {
                if shift.bits != 16 {
                    return Err("banked lens requires sixteen-bit frames".into());
                }
            }
        }
        Ok(())
    }
    fn transfer(&mut self, cx: &mut Cx<'_>, shift: Shift, at: u64) -> Result<u16, String> {
        self.validate(shift)?;
        self.advance_to(cx, at)?;
        match self.protocol {
            Protocol::Banked { .. } => self.banked(cx, shift.word, at)?,
            Protocol::MixedWidth { .. } => self.mixed(shift.word, at)?,
            Protocol::ThreeByte { .. } => self.three_byte(shift.word as u8, at)?,
        }
        self.advance_to(cx, at)?;
        Ok(0)
    }
    fn advance(&mut self, cx: &mut Cx<'_>, at: u64) -> Result<(), String> {
        self.advance_to(cx, at)
    }
    fn next_deadline(&self, now: u64) -> Option<u64> {
        let changed = self.zoom.as_ref().and_then(|z| match self.zoom_clock() {
            Ok((d, c)) if d == z.direction && c == z.clock => None,
            _ => Some(now),
        });
        [
            changed,
            self.mechanics.next_deadline(),
            self.zoom.as_ref().and_then(|z| z.due),
        ]
        .into_iter()
        .flatten()
        .min()
    }
}

#[cfg(test)]
mod tests;
