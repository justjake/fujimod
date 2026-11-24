//! The in-body stabilizer controller behind one XP4 SPI controller, as the
//! camera sees it over its link. Observed protocol behavior of the link:
//!
//! - Wiring. The camera selects the controller with a GPIO output (chip
//!   select), powers it through GPIO outputs (powered while all are high) and
//!   holds it in reset with another output. The controller reports on a GPIO
//!   input pad ("ready"), which also drives interrupt inputs.
//! - Ready pad. Unpowered or held in reset, the pad reads its declared idle
//!   level. When the reset output falls while the controller is powered (tick
//!   r), the controller boots: the pad goes low at r + `boot_ticks`, a
//!   declared virtual delay. Once it is low, the first complete frame drives
//!   it high at that frame's last-word tick (declared: the controller reports
//!   ready once the camera starts the link). Removing power or raising reset
//!   returns the pad to idle and forgets the link.
//! - Frames. Words are eight bits; a frame is four of them, most significant
//!   first, counted from the chip-select assertion (a select edge drops a
//!   partial frame). In a frame word W, bit 0 is 0 and W[5:1] holds the
//!   checksum (W[31:27] + W[26:22] + W[21:17] + W[16:12] + W[11:7] + W[6])
//!   mod 32; W[14:6] is a nine-bit id.
//! - Camera frames: a command (W[15] = 0, id W[14:6]; all zero is a dummy
//!   command with id 0), a poll (W[31:27] = 1, W[26:24] a sequence number,
//!   W[15] = 1) or a resync (A5 5A 3C C3).
//! - Replies come from earlier frames only. During a command the controller
//!   returns the reply to the previous command: exactly 0 when that command's
//!   id is 0 (at link start or after a dummy), else data << 16 | id << 6 |
//!   checksum with W[15] = 0, where data is the declared reply of that id
//!   (0 when none is declared: the periodic status traffic at the shooting
//!   screen, ids 0x29, 0x2a and 0x11a, accepts data 0). During a poll it
//!   returns the acknowledgement (0x08 | dseq) << 24 | 0x8000 | id << 6 |
//!   checksum for the latest command's id, never busy (W[20] = 0) with
//!   W[19:16] = 0; dseq counts, mod 8 from 0 at link start, the nonzero
//!   command replies delivered. During a resync it returns C3 3C A5 5A.
//! - The first byte of a camera frame already tells the three kinds apart
//!   (A5 resync, W[31:27] = 1 poll, otherwise command), so the reply kind is
//!   chosen with it; a frame whose later bytes contradict its first byte, or
//!   whose checksum is wrong, is refused.
//!
//! Inputs are the GPIO output levels and their change ticks, the SPI words
//! and virtual time only.
use crate::spi::Input;
use fujimod_emulator_devices::checkpoint::Checkpoint;
use fujimod_emulator_ip::socionext::gpio::{OutputWatcher, Pins, SharedPins, SharedWatcher};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

const KIND: &str = "xp4-stabilizer-link";
/// Longest declared boot delay (about 1.4 hours at 54 MHz): keeps every
/// deadline far from overflow.
const MAX_BOOT_TICKS: u64 = 1 << 38;
const RESYNC: u32 = 0xa55a_3cc3;
const RESYNC_REPLY: u32 = 0xc33c_a55a;

/// One GPIO pad.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pad {
    pub port: usize,
    pub pin: usize,
}
/// The chip-select output and its active level.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Select {
    pub port: usize,
    pub pin: usize,
    pub active_high: bool,
}
/// Declared wiring, boot delay and command replies.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// SPI controller base.
    pub base: u64,
    /// Data input level while the controller is not selected.
    pub idle: bool,
    pub select: Select,
    /// Outputs that power the controller while all are high.
    pub power: Vec<Pad>,
    /// Output that holds the controller in reset while high.
    pub reset: Pad,
    /// Input pad the controller reports on.
    pub ready: Pad,
    /// Virtual ticks from the reset release to the ready pad going low.
    pub boot_ticks: u64,
    /// Reply data by command id; an id not listed replies 0.
    #[serde(default)]
    pub replies: BTreeMap<u16, u16>,
}
impl Config {
    fn validate(&self) -> Result<(), String> {
        let select = Pad {
            port: self.select.port,
            pin: self.select.pin,
        };
        let pads: Vec<Pad> = [select, self.reset, self.ready]
            .into_iter()
            .chain(self.power.iter().copied())
            .collect();
        let unique: BTreeSet<_> = pads.iter().map(|p| (p.port, p.pin)).collect();
        if self.power.is_empty()
            || self.power.len() > 8
            || pads.iter().any(|p| p.port >= 16 || p.pin >= 16)
            || unique.len() != pads.len()
            || !(1..=MAX_BOOT_TICKS).contains(&self.boot_ticks)
            || self.replies.keys().any(|id| *id >= 512)
        {
            return Err("Invalid stabilizer link wiring, boot delay or reply ids".into());
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Frame {
    Command,
    Poll,
    Resync,
}
impl Frame {
    /// The kind a camera frame starting with `byte` is.
    fn of(byte: u8) -> Self {
        match byte {
            0xa5 => Self::Resync,
            byte if byte >> 3 == 1 => Self::Poll,
            _ => Self::Command,
        }
    }
}

/// W[5:1] of a frame word.
fn checksum(word: u32) -> u32 {
    ((word >> 27)
        + (word >> 22 & 31)
        + (word >> 17 & 31)
        + (word >> 12 & 31)
        + (word >> 7 & 31)
        + (word >> 6 & 1))
        & 31
}
fn seal(word: u32) -> u32 {
    let word = word & !63;
    word | checksum(word) << 1
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    kind: String,
    config: Config,
    /// Ready pad level while the controller does not drive it.
    pad_idle: bool,
    /// Tick the ready pad goes low after the reset release.
    ready_at: Option<u64>,
    /// The ready pad went high after the first complete frame.
    reported: bool,
    /// Chip-select change count at the last word.
    edge: u64,
    phase: u8,
    frame: [u8; 4],
    /// Kind and reply of the frame in progress.
    current: Frame,
    reply: [u8; 4],
    /// Id of the latest command since link start.
    last_id: u16,
    /// Nonzero command replies delivered since link start, mod 8.
    dseq: u8,
}
impl State {
    fn start_link(&mut self) {
        self.reported = false;
        self.phase = 0;
        self.last_id = 0;
        self.dseq = 0;
    }
    /// Unpowered or held in reset: the pad returns to idle and the link is
    /// forgotten.
    fn stop(&mut self, pins: &mut Pins, now: u64) {
        let ready = self.config.ready;
        self.ready_at = None;
        self.start_link();
        pins.cancel_inputs(ready.port, ready.pin);
        if pins.input(ready.port, ready.pin) != self.pad_idle {
            // Validated wiring: the pad exists.
            let _ = pins.drive_input(now, ready.port, ready.pin, self.pad_idle);
        }
    }
    fn reply_for(&self, frame: Frame) -> u32 {
        match frame {
            Frame::Command if self.last_id == 0 => 0,
            Frame::Command => {
                let data = u32::from(*self.config.replies.get(&self.last_id).unwrap_or(&0));
                seal(data << 16 | u32::from(self.last_id) << 6)
            }
            Frame::Poll => {
                seal((0x08 | u32::from(self.dseq)) << 24 | 0x8000 | u32::from(self.last_id) << 6)
            }
            Frame::Resync => RESYNC_REPLY,
        }
    }
    /// The four bytes of the frame in progress have shifted.
    fn complete(&mut self) -> Result<(), String> {
        let word = u32::from_be_bytes(self.frame);
        let contradicts = || {
            format!(
                "Stabilizer link frame {word:08x} contradicts its first byte ({:?} frame)",
                self.current
            )
        };
        if self.current == Frame::Resync {
            return if word == RESYNC {
                Ok(())
            } else {
                Err(contradicts())
            };
        }
        if word & 1 != 0 || word >> 1 & 31 != checksum(word) {
            return Err(format!(
                "Stabilizer link frame {word:08x} has a wrong checksum"
            ));
        }
        if (word & 0x8000 != 0) != (self.current == Frame::Poll) {
            return Err(contradicts());
        }
        if self.current == Frame::Command {
            if u32::from_be_bytes(self.reply) != 0 {
                self.dseq = (self.dseq + 1) & 7;
            }
            self.last_id = (word >> 6 & 0x1ff) as u16;
        }
        Ok(())
    }
}
impl OutputWatcher for State {
    fn output_changed(&mut self, pins: &mut Pins, port: usize, pin: usize, high: bool, now: u64) {
        let reset = self.config.reset;
        let at = |pad: &Pad| (pad.port, pad.pin) == (port, pin);
        if !self.config.power.iter().any(at) && !at(&reset) {
            return;
        }
        let powered = self
            .config
            .power
            .iter()
            .all(|p| pins.output(p.port, p.pin).0);
        if !powered || pins.output(reset.port, reset.pin).0 {
            self.stop(pins, now);
        } else if at(&reset) && !high {
            // The reset fell while powered: the controller boots.
            self.start_link();
            let ready = self.config.ready;
            let low = now + self.config.boot_ticks;
            self.ready_at = Some(low);
            pins.cancel_inputs(ready.port, ready.pin);
            let _ = pins.drive_input(low, ready.port, ready.pin, false);
        }
    }
}

/// The controller as the SPI controller's input; it also follows the GPIO
/// outputs it is wired to.
pub struct StabilizerLink {
    pins: SharedPins,
    state: Arc<Mutex<State>>,
}
impl StabilizerLink {
    /// Connects the controller to the GPIO bank; the ready pad's present
    /// input level is its idle level.
    pub fn new(pins: SharedPins, config: Config) -> Result<Self, String> {
        config.validate()?;
        let mut bank = pins.lock().map_err(|_| "GPIO pins poisoned")?;
        let state = Arc::new(Mutex::new(State {
            kind: KIND.into(),
            pad_idle: bank.input(config.ready.port, config.ready.pin),
            edge: bank.output(config.select.port, config.select.pin).1,
            config,
            ready_at: None,
            reported: false,
            phase: 0,
            frame: [0; 4],
            current: Frame::Command,
            reply: [0; 4],
            last_id: 0,
            dseq: 0,
        }));
        let watcher: SharedWatcher = state.clone();
        bank.watch(watcher);
        drop(bank);
        Ok(Self { pins, state })
    }
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, State>, String> {
        self.state
            .lock()
            .map_err(|_| "Stabilizer link state poisoned".to_owned())
    }
    fn validate(&self, state: &State) -> Result<(), String> {
        let current = self.lock()?;
        if state.kind != KIND
            || state.config != current.config
            || state.pad_idle != current.pad_idle
            || state.phase > 3
            || state.last_id >= 512
            || state.dseq > 7
            || state.reported && state.ready_at.is_none()
        {
            return Err("Invalid stabilizer link checkpoint or wiring".into());
        }
        Ok(())
    }
}
impl Input for StabilizerLink {
    fn exchange(&mut self, bits: u32, tx: u16, tick: u64) -> Result<u16, String> {
        let (select, ready) = {
            let state = self.lock()?;
            (state.config.select, state.config.ready)
        };
        let (level, edge) = self
            .pins
            .lock()
            .map_err(|_| "GPIO pins poisoned")?
            .output(select.port, select.pin);
        let mut state = self.lock()?;
        if edge != state.edge {
            state.edge = edge;
            state.phase = 0;
        }
        if level != select.active_high {
            return Ok(if state.config.idle {
                u16::MAX >> (16 - bits)
            } else {
                0
            });
        }
        if bits != 8 {
            return Err("The stabilizer link requires eight-bit serial words".into());
        }
        if !state.ready_at.is_some_and(|at| tick >= at) {
            return Err(
                "Stabilizer link word before the controller is ready (unpowered, held in reset or still booting)"
                    .into(),
            );
        }
        let byte = tx as u8;
        let phase = usize::from(state.phase);
        if phase == 0 {
            state.current = Frame::of(byte);
            state.reply = state.reply_for(state.current).to_be_bytes();
        }
        state.frame[phase] = byte;
        let reply = state.reply[phase];
        state.phase = (state.phase + 1) % 4;
        let mut report = false;
        if phase == 3 {
            state.complete()?;
            report = !state.reported;
            state.reported = true;
        }
        drop(state);
        if report {
            // The first complete frame: the controller reports ready.
            self.pins
                .lock()
                .map_err(|_| "GPIO pins poisoned")?
                .drive_input(tick, ready.port, ready.pin, true)?;
        }
        Ok(u16::from(reply))
    }
    fn checkpoint(&self) -> Result<Checkpoint, String> {
        Checkpoint::encode(KIND, &*self.lock()?)
    }
    fn validate_checkpoint(&self, data: &Checkpoint) -> Result<(), String> {
        self.validate(&data.decode(KIND)?)
    }
    fn restore_checkpoint(&mut self, data: &Checkpoint) -> Result<(), String> {
        let state: State = data.decode(KIND)?;
        self.validate(&state)?;
        *self.lock()? = state;
        Ok(())
    }
    fn save(&self) -> Value {
        self.lock()
            .map_or(Value::Null, |state| serde_json::to_value(&*state).unwrap())
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let state: State = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        self.validate(&state)?;
        *self.lock()? = state;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::parts::gpio::InputIrq;
    use fujimod_emulator_devices::test_host::recording::RecordingHost;
    use fujimod_emulator_devices::{Cx, Device};
    use fujimod_emulator_ip::socionext::gpio::Gpio;
    const BANK: u64 = 0xfff41000;
    const BOOT: u64 = 1000;

    fn config() -> Config {
        Config {
            base: 0xfff34000,
            idle: false,
            select: Select {
                port: 12,
                pin: 3,
                active_high: false,
            },
            power: vec![Pad { port: 0, pin: 12 }, Pad { port: 1, pin: 14 }],
            reset: Pad { port: 12, pin: 6 },
            ready: Pad { port: 0, pin: 4 },
            boot_ticks: BOOT,
            replies: [(0x14, 0x1234)].into(),
        }
    }
    struct Rig {
        pins: SharedPins,
        gpio: Gpio,
        link: StabilizerLink,
        host: RecordingHost,
    }
    fn rig() -> Rig {
        let mut levels = [0; 16];
        levels[0] = 0x7553;
        let pins = Pins::new(levels);
        let routes = vec![
            InputIrq {
                port: 0,
                pin: 4,
                line: 171,
                active_high: false,
            },
            InputIrq {
                port: 0,
                pin: 4,
                line: 155,
                active_high: true,
            },
        ];
        let gpio = Gpio::new(BANK, pins.clone())
            .unwrap()
            .with_interrupts(routes)
            .unwrap();
        let link = StabilizerLink::new(pins.clone(), config()).unwrap();
        Rig {
            pins,
            gpio,
            link,
            host: RecordingHost::default(),
        }
    }
    impl Rig {
        fn output(&mut self, now: u64, port: u64, pin: u32, high: bool) {
            self.host.now = now;
            let value = 1 << (pin + 16) | u64::from(high) << pin;
            self.gpio.write(
                &mut Cx::new(&mut self.host, 0, 0, 0),
                BANK + port * 64,
                4,
                value,
            );
        }
        fn pad(&mut self, now: u64) -> bool {
            self.host.now = now;
            self.gpio
                .read(&mut Cx::new(&mut self.host, 0, 0, 0), BANK + 0x14, 4)
                & 0x10
                != 0
        }
        fn settle(&mut self, now: u64) {
            self.host.now = now;
            self.gpio.advance(&mut Cx::new(&mut self.host, 0, 0, 0));
        }
        /// Powers the controller and pulses its reset; the reset falls at 40.
        fn open(&mut self) {
            self.output(5, 12, 3, true);
            self.output(10, 0, 12, true);
            self.output(20, 1, 14, true);
            self.output(30, 12, 6, true);
            self.output(40, 12, 6, false);
            self.settle(40 + BOOT);
            self.output(40 + BOOT, 12, 3, false);
        }
        /// One frame, its words shifting at `tick + 1..=tick + 4`.
        fn frame(&mut self, tick: u64, bytes: [u8; 4]) -> Result<[u8; 4], String> {
            let mut reply = [0; 4];
            for (index, byte) in bytes.into_iter().enumerate() {
                reply[index] = self
                    .link
                    .exchange(8, u16::from(byte), tick + 1 + index as u64)?
                    as u8;
            }
            Ok(reply)
        }
    }

    #[test]
    fn the_checksum_matches_the_observed_frames() {
        for word in [
            0x0000_0514,
            0x0810_8022,
            0x0800_8526,
            0x0910_802a,
            0x0900_801a,
            0x1234_051c,
            0,
        ] {
            assert_eq!(seal(word), word, "{word:08x}");
        }
    }
    #[test]
    fn the_ready_pad_follows_power_reset_boot_and_the_first_frame() {
        let mut r = rig();
        r.settle(0);
        assert_eq!(
            r.host.irqs,
            [(155, true)],
            "the idle-high pad holds the active-high input"
        );
        // Half powered, then powered with reset low: nothing yet.
        r.output(10, 0, 12, true);
        r.output(20, 1, 14, true);
        assert!(r.pins.lock().unwrap().pending_inputs().is_empty());
        // A reset pulse: held, then released at 40.
        r.output(30, 12, 6, true);
        assert!(r.pins.lock().unwrap().pending_inputs().is_empty());
        r.output(40, 12, 6, false);
        assert_eq!(r.gpio.next_deadline(40), Some(40 + BOOT));
        assert!(r.pad(40 + BOOT - 1));
        let early = r.frame(40 + BOOT - 5, [0; 4]).unwrap_err();
        assert!(early.contains("before the controller is ready"), "{early}");
        assert!(!r.pad(40 + BOOT));
        assert_eq!(r.host.irqs, [(155, true), (171, true), (155, false)]);
        // The first complete frame drives it high at its last word.
        assert_eq!(r.frame(2000, [0, 0, 5, 0x14]).unwrap(), [0; 4]);
        assert_eq!(r.gpio.next_deadline(2004), Some(2004));
        r.settle(2004);
        assert!(r.pad(2004));
        assert_eq!(&r.host.irqs[3..], [(171, false), (155, true)]);
        // Later frames leave it high.
        r.frame(3000, [0x08, 0x10, 0x80, 0x22]).unwrap();
        assert!(r.pins.lock().unwrap().pending_inputs().is_empty());
        // Removing power forgets the link: the pad is idle (high) already.
        r.output(4000, 1, 14, false);
        assert!(r.pins.lock().unwrap().pending_inputs().is_empty());
        assert!(r.frame(4100, [0; 4]).is_err());
        // Powered again, a new pulse boots a new link from the start.
        r.output(5000, 1, 14, true);
        r.output(5010, 12, 6, true);
        r.output(5020, 12, 6, false);
        r.settle(5020 + BOOT);
        assert!(!r.pad(5020 + BOOT));
        assert_eq!(
            r.frame(7000, [0, 0, 0, 0]).unwrap(),
            [0; 4],
            "link start replies 0"
        );
        // Reset while the pad is low returns it to idle at once.
        r.output(8000, 12, 6, true);
        assert!(r.pad(8000));
    }
    #[test]
    fn the_worked_example_after_link_open() {
        let mut r = rig();
        r.open();
        let frames: [([u8; 4], [u8; 4]); 6] = [
            // The first command: nothing was commanded before it.
            ([0x00, 0x00, 0x05, 0x14], [0, 0, 0, 0]),
            // The poll acknowledges id 0x14 with no reply delivered yet.
            ([0x08, 0x10, 0x80, 0x22], [0x08, 0x00, 0x85, 0x26]),
            // The dummy command collects the reply to 0x14.
            ([0, 0, 0, 0], [0x12, 0x34, 0x05, 0x1c]),
            // A poll for id 0: one reply delivered.
            ([0x09, 0x10, 0x80, 0x2a], [0x09, 0x00, 0x80, 0x1a]),
            // Resync, then the link carries on unchanged.
            ([0xa5, 0x5a, 0x3c, 0xc3], [0xc3, 0x3c, 0xa5, 0x5a]),
            ([0x09, 0x10, 0x80, 0x2a], [0x09, 0x00, 0x80, 0x1a]),
        ];
        for (index, (camera, peer)) in frames.into_iter().enumerate() {
            assert_eq!(
                r.frame(2000 + index as u64 * 10, camera).unwrap(),
                peer,
                "frame {index}"
            );
        }
        // An id without declared data replies 0 data (the periodic status
        // traffic): status 0x29 then a dummy collects 0x29 << 6 | checksum.
        let status = seal(0x29 << 6).to_be_bytes();
        assert_eq!(r.frame(3000, status).unwrap(), [0; 4], "after the dummy");
        assert_eq!(
            r.frame(3010, [0; 4]).unwrap(),
            seal(0x29 << 6).to_be_bytes()
        );
    }
    #[test]
    fn frames_outside_the_protocol_are_refused() {
        let cases: [(&[u8], &str); 4] = [
            (&[0, 0, 5, 0x15], "wrong checksum"),
            (&[0, 0, 5, 0x16], "wrong checksum"),
            (&[0x08, 0x10, 0x00, 0x12], "contradicts its first byte"),
            (&[0xa5, 0x5a, 0x3c, 0xc2], "contradicts its first byte"),
        ];
        for (bytes, text) in cases {
            let mut r = rig();
            r.open();
            let error = r.frame(2000, bytes.try_into().unwrap()).unwrap_err();
            assert!(error.contains(text), "{bytes:02x?}: {error}");
        }
        let mut r = rig();
        r.open();
        assert!(
            r.link
                .exchange(16, 0, 2000)
                .unwrap_err()
                .contains("eight-bit")
        );
        // Not selected, the data input idles; a select edge drops a partial frame.
        r.link.exchange(8, 0x08, 2001).unwrap();
        r.output(2002, 12, 3, true);
        assert_eq!(r.link.exchange(8, 0x55, 2003), Ok(0));
        r.output(2004, 12, 3, false);
        assert_eq!(r.frame(2010, [0, 0, 5, 0x14]).unwrap(), [0; 4]);
    }
    #[test]
    fn a_session_survives_a_checkpoint_mid_frame() {
        let mut r = rig();
        r.open();
        r.frame(2000, [0, 0, 5, 0x14]).unwrap();
        r.link.exchange(8, 0x08, 2100).unwrap();
        r.link.exchange(8, 0x10, 2101).unwrap();
        let link = r.link.checkpoint().unwrap();
        let bank = r.gpio.checkpoint().unwrap();
        let saved = r.link.save();
        let mut resumed = rig();
        resumed.gpio.restore_checkpoint(&bank).unwrap();
        resumed.link.restore_checkpoint(&link).unwrap();
        assert_eq!(resumed.link.save(), saved);
        let mut json = rig();
        json.link.restore(&saved).unwrap();
        for board in [&mut r, &mut resumed] {
            assert_eq!(board.link.exchange(8, 0x80, 2102), Ok(0x85));
            assert_eq!(board.link.exchange(8, 0x22, 2103), Ok(0x26));
        }
        assert_eq!(resumed.link.save(), r.link.save());
        // The restored link still follows the outputs it is wired to.
        resumed.output(3000, 0, 12, false);
        assert!(resumed.frame(3100, [0; 4]).is_err());
        // Other wiring or an impossible state is refused.
        let mut other = config();
        other.boot_ticks += 1;
        let pins = Pins::new([0; 16]);
        let mut different = StabilizerLink::new(pins, other).unwrap();
        assert!(different.restore_checkpoint(&link).is_err());
        let mut bad = saved.clone();
        bad["phase"] = serde_json::json!(4);
        assert!(json.link.restore(&bad).is_err());
    }
    #[test]
    fn invalid_wiring_is_refused() {
        let pins = Pins::new([0; 16]);
        let mut shared = config();
        shared.ready = shared.reset;
        let mut unpowered = config();
        unpowered.power.clear();
        let mut instant = config();
        instant.boot_ticks = 0;
        let mut wide = config();
        wide.replies.insert(512, 1);
        let mut far = config();
        far.select.port = 16;
        for bad in [shared, unpowered, instant, wide, far] {
            assert!(StabilizerLink::new(pins.clone(), bad).is_err());
        }
    }
}
