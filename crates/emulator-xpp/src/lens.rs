//! An X-mount lens on X-Processor Pro serial channel 0, as X-T20 2.11 talks
//! to it.
//!
//! The body moves link bytes with the PL330: one channel stores the
//! transmit bytes into +0x60, another loads the received bytes from +0x64,
//! one byte per peripheral request. +0x4c frames a transfer (3 starts it, 0
//! ends it; section 5 0xc4a908 / 0xc49eec) and +0x08 bit 0 re-enables the
//! port. The link is full duplex: the lens's bytes leave while the body's
//! arrive, so an answer can only depend on earlier transfers. Both
//! directions share the link clock, so the body's frame has left when its
//! received bytes are in; when the body ends a word frame before the
//! transmit channel's store has landed, the frame completes when it lands.
//! Other port registers behave as the [`DiscoveryPort`].
//!
//! Word frames are four bytes, most significant first: bit 0 error, bits
//! 1-5 the 5-bit sum of the fields at bits 7, 12, 17, 22 and 27 plus bit 6,
//! bits 6-14 the command, bit 15 the acknowledge flag, bits 16-31 data. A
//! transaction is a message frame, where the body sends a command and the
//! lens the reply to the body's previous command, then an acknowledge frame,
//! where each side sends `0x08008000 | sequence << 24 | command << 6` of the
//! message it received (the sequence counts the sender's messages). The
//! reply to command 0x14 (read) or 0x15 (write) starts bulk transfers of a
//! page of the size command 0x18 sets (2^n bytes, data bits 11-14) plus
//! three bytes: a read sends a 2-byte header (0 = page follows, 0xffff = no
//! more pages), the page and an 8-bit sum of header and page; a write
//! answers 0xffff ("ready"), then 0xff00 ("received"), each followed by
//! zeros and 'Z'. The read pages are the lens's information page, which the
//! body keeps at 0x11e3958 (names, serial, versions).
//!
//! The ready line: when the body switches its power output high (lswatch,
//! after it checked the contact levels), the lens pulls the ready pin low
//! after its start-up and lets it go high when its initialization is done
//! (lensdc's wait 0xc8a3a0 ends). After the start-up, a focus drive (command
//! 0x56) or command 0x40 holds the pin low while the lens moves. The pin
//! reaches the interrupt controller through an external interrupt line.
//! Commands the profile does not answer are answered with 0 and counted.
//!
//! Lens state, from X-T2 4.41 captures of two lenses (a third-party 25mm F1.8 and a zoom with
//! a variable maximum aperture), the body's message words and the lens's replies logged on
//! the camera. Both lenses echo the body's data for the set commands in [`ECHOES`]. The focus
//! group sits at a position the lens reports for command 0x21. Command 0x56 sets a target, and
//! the lens reaches it when its drive ends. The status reply (0x20) carries bit 3 while a drive
//! runs. Command 0x62 sets the iris: 0x1000 is open, each further 0x40 one step closed, 0x7000
//! (sent at exposure) opens it again. The 0x22 reply is the step count times 0x100. Drive speed,
//! zoom and the focus scale are not modeled: the drive takes the declared drive time,
//! whatever the distance.
//! The lens's delays are declared inputs, not measurements; the lens samples
//! the power output once per declared poll period.
use crate::resets::ExternalLines;
use crate::serial::DiscoveryPort;
use fujimod_emulator_devices::{Cx, Device};
use fujimod_emulator_ip::socionext::gpio::SharedPins;
use fujimod_emulator_ip_arm::pl330;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

/// Largest information page.
pub const PAGE_BYTES: usize = 0x80;
const ENABLE: u64 = 0x08;
const TRANSFER: u64 = 0x4c;
const TRANSMIT: u64 = 0x60;
const RECEIVE: u64 = 0x64;
const ACK: u32 = 0x0800_8000;
const READ_SESSION: u32 = 0x14;
const WRITE_SESSION: u32 = 0x15;
const PAGE_SIZE: u32 = 0x18;
/// A read session of 2 KiB pages that X-T2 4.41 starts after the
/// information pages: command 0xc0 with data 6, then 0xc8dd58 reads one
/// 0x800-byte page (0x8000001: page size 0x800, request header 1) through
/// 0xc8e674. What the pages hold is not known; this lens has none, so it
/// answers the first request with the end header 0xffff.
const DATA_SESSION: u32 = 0xc0;
const DATA_PAGE_BYTES: usize = 0x800;
/// Commands after which the lens moves and holds the ready line low.
const MOVES: [u32; 2] = [0x40, 0x56];
/// Focus drives kept in the lens state.
const MOVE_HISTORY: usize = 64;
/// Set commands whose reply carries the body's data back, as both captured lenses answer.
const ECHOES: [u32; 12] = [
    0x18, 0x54, 0x55, 0x56, 0x58, 0x61, 0x62, 0x80, 0xa0, 0xa8, 0xa9, 0xff,
];
const STATUS: u32 = 0x20;
/// Status bit set while a drive runs.
const STATUS_MOVING: u32 = 0x08;
const FOCUS_POSITION: u32 = 0x21;
const FOCUS_TARGET: u32 = 0x56;
const IRIS_POSITION: u32 = 0x22;
const IRIS_DRIVE: u32 = 0x62;
const IRIS_OPEN: u32 = 0x1000;
const IRIS_STEP: u32 = 0x40;
/// The iris word the body sends at exposure; afterwards the lens reports the iris open.
const IRIS_EXPOSURE: u32 = 0x7000;

fn checksum(word: u32) -> u32 {
    ([7, 12, 17, 22, 27]
        .iter()
        .map(|lo| word >> lo & 0x1f)
        .sum::<u32>()
        + (word >> 6 & 1))
        & 0x1f
}
fn seal(word: u32) -> u32 {
    word & !0x3f | checksum(word) << 1
}
fn command(word: u32) -> u32 {
    (word & 0x7fff) >> 6
}
fn is_message(word: u32) -> bool {
    word != 0 && word & 0x8000 == 0
}

/// What the lens answers: its information page and command replies.
#[derive(Clone, Debug)]
pub struct Profile {
    pub page: Vec<u8>,
    pub replies: BTreeMap<u32, u16>,
}
/// Declared lens delays and the power sampling period, in ticks.
#[derive(Clone, Copy, Debug)]
pub struct Timing {
    pub ready: u64,
    pub release: u64,
    pub drive: u64,
    pub poll: u64,
}
/// Where the lens sits: the serial port, the body's power output (bank,
/// register, bit), the ready pin (bank, pin) and its external interrupt
/// line, and the contact levels the attached lens presents.
#[derive(Clone, Debug)]
pub struct Wiring {
    pub base: u64,
    pub power: (usize, usize, usize),
    pub ready: (usize, usize),
    pub line: usize,
    pub contacts: Vec<(usize, usize, bool)>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Link {
    acknowledge: bool,
    word: u32,
    reply: u32,
    sequence: u32,
    sent: u32,
    bulk: Option<u32>,
    page: usize,
    page_size: usize,
    transmitted: Vec<u8>,
    out: Vec<u8>,
    received: usize,
    busy: bool,
    /// A word frame ended before its transmit store landed.
    #[serde(default)]
    closing: bool,
}
impl Link {
    fn reset(&mut self) {
        *self = Self {
            page_size: PAGE_BYTES,
            ..Self::default()
        };
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Line {
    powered: bool,
    ready_at: Option<u64>,
    release_at: Option<u64>,
    released: bool,
    drive_until: Option<u64>,
    poll_at: u64,
    drives: u64,
    unknown: BTreeMap<u32, u64>,
    /// Focus group position, the 0x21 reply.
    #[serde(default)]
    focus: u16,
    /// The 0x56 target the running drive moves to.
    #[serde(default)]
    focus_target: Option<u16>,
    /// Iris steps closed from open, the 0x22 reply over 0x100.
    #[serde(default)]
    iris_steps: u16,
    /// The latest focus drives: each target and where its drive ended, `None` when a newer
    /// target replaced it first. `moves_total` counts every drive, so a reader can take the
    /// ones after a point.
    #[serde(default)]
    moves: Vec<FocusMove>,
    #[serde(default)]
    moves_total: u64,
}

/// One focus drive: the body's target and the position the drive ended at.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FocusMove {
    sequence: u64,
    target: u16,
    reached: Option<u16>,
}

/// The link state shared by the port and the PL330, which reaches the
/// port's data registers through this window.
#[derive(Clone)]
pub struct LensLink {
    base: u64,
    link: Arc<Mutex<Link>>,
}
impl LensLink {
    pub fn new(base: u64) -> Result<Self, String> {
        if base & 0xfff != 0 {
            return Err("Invalid lens port base".into());
        }
        let mut link = Link::default();
        link.reset();
        Ok(Self {
            base,
            link: Arc::new(Mutex::new(link)),
        })
    }
}
impl pl330::Source for LensLink {
    fn contains(&self, address: u64, _len: usize) -> bool {
        address == self.base + TRANSMIT || address == self.base + RECEIVE
    }
    fn read(&self, address: u64, len: usize) -> Result<Vec<u8>, String> {
        if address != self.base + RECEIVE {
            return Err("Lens port load from the transmit register".into());
        }
        let mut link = self.link.lock().map_err(|_| "Lens link poisoned")?;
        let start = link.received;
        link.received += len;
        Ok((start..start + len)
            .map(|i| link.out.get(i).copied().unwrap_or(0))
            .collect())
    }
    fn writable(&self) -> bool {
        true
    }
    fn write(&self, address: u64, bytes: &[u8]) -> Result<(), String> {
        if address != self.base + TRANSMIT {
            return Err("Lens port store into the receive register".into());
        }
        let mut link = self.link.lock().map_err(|_| "Lens link poisoned")?;
        link.transmitted.extend_from_slice(bytes);
        Ok(())
    }
}

pub struct LensPort {
    port: DiscoveryPort,
    link: Arc<Mutex<Link>>,
    line: Line,
    profile: Profile,
    timing: Timing,
    wiring: Wiring,
    pins: SharedPins,
    lines: ExternalLines,
}
impl LensPort {
    pub fn new(
        link: &LensLink,
        profile: Profile,
        timing: Timing,
        wiring: Wiring,
        pins: SharedPins,
        lines: ExternalLines,
    ) -> Result<Self, String> {
        let pin = |(port, pin): (usize, usize)| port < 16 && pin < 16;
        if link.base != wiring.base
            || profile.page.is_empty()
            || profile.page.len() > PAGE_BYTES
            || profile.replies.keys().any(|id| *id >= 0x200)
            || [timing.ready, timing.release, timing.drive, timing.poll].contains(&0)
            || !pin((wiring.power.0, wiring.power.2))
            || wiring.power.1 >= 5
            || !pin(wiring.ready)
            || wiring.contacts.iter().any(|&(port, p, _)| !pin((port, p)))
        {
            return Err("Invalid lens profile, timing or wiring".into());
        }
        lines.connect(pins.clone(), wiring.line, wiring.ready.0, wiring.ready.1)?;
        {
            let mut pins = pins.lock().map_err(|_| "GPIO pins poisoned")?;
            for &(port, pin, high) in &wiring.contacts {
                pins.set_input(port, pin, high);
            }
            pins.set_input(wiring.ready.0, wiring.ready.1, true);
        }
        Ok(Self {
            port: DiscoveryPort::new(wiring.base)?,
            link: link.link.clone(),
            line: Line {
                poll_at: timing.poll,
                ..Line::default()
            },
            profile,
            timing,
            wiring,
            pins,
            lines,
        })
    }
    fn set_ready(&mut self, cx: &mut Cx<'_>, high: bool) {
        let (port, pin) = self.wiring.ready;
        self.pins.lock().unwrap().set_input(port, pin, high);
        self.lines.update(cx);
    }
    fn begin(&self, link: &mut Link) {
        link.transmitted.clear();
        link.received = 0;
        link.busy = true;
        link.out = match link.bulk {
            None => link.word.to_be_bytes().to_vec(),
            Some(DATA_SESSION) => {
                let mut out = vec![0xff, 0xff];
                out.resize(2 + DATA_PAGE_BYTES, 0);
                out.push(0xfe);
                out
            }
            Some(WRITE_SESSION) => {
                let mut out = if link.page == 0 {
                    vec![0xff, 0xff]
                } else {
                    vec![0xff, 0]
                };
                out.resize(2 + link.page_size, 0);
                out.push(b'Z');
                out
            }
            Some(_) => {
                let mut head = match link.page {
                    0 => vec![0, 0],
                    _ => vec![0xff, 0xff],
                };
                if link.page == 0 {
                    head.extend(self.profile.page.iter().take(link.page_size));
                }
                head.resize(2 + link.page_size, 0);
                let sum = head.iter().fold(0u8, |s, b| s.wrapping_add(*b));
                head.push(sum);
                head
            }
        };
    }
    fn end(&mut self, cx: &mut Cx<'_>, link: &mut Link) {
        link.busy = false;
        let head = link
            .transmitted
            .iter()
            .take(4)
            .fold(0u128, |w, b| w << 8 | u128::from(*b));
        cx.record(fujimod_emulator_devices::Record::new(
            "X-Processor Pro lens frame end",
            [
                (
                    "transmitted",
                    fujimod_emulator_devices::Value::int(link.transmitted.len() as i128),
                ),
                ("head", fujimod_emulator_devices::Value::hex(head)),
                (
                    "received",
                    fujimod_emulator_devices::Value::int(link.received as i128),
                ),
                (
                    "bulk",
                    fujimod_emulator_devices::Value::int(link.bulk.map_or(-1, i128::from)),
                ),
                (
                    "page",
                    fujimod_emulator_devices::Value::int(link.page as i128),
                ),
                (
                    "page_size",
                    fujimod_emulator_devices::Value::int(link.page_size as i128),
                ),
                ("tick", fujimod_emulator_devices::Value::int(cx.now())),
            ],
        ));
        if let Some(session) = link.bulk {
            let last: &[u8] = if session == WRITE_SESSION {
                &[0xff, 0]
            } else {
                &[0xff, 0xff]
            };
            link.page += 1;
            if link.out.starts_with(last) {
                link.bulk = None;
                link.acknowledge = false;
                link.word = 0;
            }
            return;
        }
        if link.transmitted.len() < 4 && link.received == 4 {
            link.closing = true;
            return;
        }
        self.close(cx, link);
    }
    fn close(&mut self, cx: &mut Cx<'_>, link: &mut Link) {
        link.closing = false;
        let Ok(bytes) = <[u8; 4]>::try_from(link.transmitted.as_slice()) else {
            cx.unmodeled();
            return;
        };
        let word = u32::from_be_bytes(bytes);
        if !link.acknowledge {
            link.sent = link.word;
            let received = if is_message(word) { word } else { 0 };
            let mut ack = ACK | command(received) << 6;
            if link.sent != 0 {
                ack |= link.sequence << 24;
                link.sequence = (link.sequence + 1) & 7;
            }
            link.reply = 0;
            if received != 0 {
                let id = command(received);
                if id == PAGE_SIZE {
                    link.page_size = 1 << (received >> 27 & 0xf);
                } else if MOVES.contains(&id) && self.line.released {
                    self.line.drives += 1;
                    if self.line.drive_until.is_none() {
                        self.set_ready(cx, false);
                    }
                    self.line.drive_until = cx.now().checked_add(self.timing.drive);
                }
                let body = received >> 16;
                if self.line.released {
                    if id == FOCUS_TARGET {
                        self.line.focus_target = Some(body as u16);
                        self.line.moves_total += 1;
                        if self.line.moves.len() == MOVE_HISTORY {
                            self.line.moves.remove(0);
                        }
                        self.line.moves.push(FocusMove {
                            sequence: self.line.moves_total,
                            target: body as u16,
                            reached: None,
                        });
                    } else if id == IRIS_DRIVE && body == IRIS_EXPOSURE {
                        self.line.iris_steps = 0;
                    } else if id == IRIS_DRIVE && (IRIS_OPEN..IRIS_EXPOSURE).contains(&body) {
                        self.line.iris_steps = ((body - IRIS_OPEN) / IRIS_STEP) as u16;
                    }
                }
                let data = match id {
                    FOCUS_POSITION => u32::from(self.line.focus),
                    IRIS_POSITION => u32::from(self.line.iris_steps) << 8,
                    STATUS => {
                        let moving = if self.line.drive_until.is_some() {
                            STATUS_MOVING
                        } else {
                            0
                        };
                        u32::from(self.profile.replies.get(&id).copied().unwrap_or(0)) | moving
                    }
                    id if ECHOES.contains(&id) => body,
                    _ => match self.profile.replies.get(&id) {
                        Some(&data) => u32::from(data),
                        None => {
                            *self.line.unknown.entry(id).or_default() += 1;
                            0
                        }
                    },
                };
                link.reply = seal(data << 16 | id << 6);
            }
            link.word = seal(ack);
            link.acknowledge = true;
        } else {
            link.word = link.reply;
            link.acknowledge = false;
            if is_message(link.sent)
                && [READ_SESSION, WRITE_SESSION, DATA_SESSION].contains(&command(link.sent))
            {
                link.bulk = Some(command(link.sent));
                link.page = 0;
            }
            link.sent = 0;
        }
    }
    fn power(&self) -> bool {
        let (port, register, bit) = self.wiring.power;
        self.pins.lock().unwrap().register(port, register) >> bit & 1 != 0
    }
}
impl Device for LensPort {
    fn name(&self) -> &'static str {
        "xpp-lens-port"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        match address.wrapping_sub(self.wiring.base) {
            TRANSMIT | RECEIVE => {
                cx.unmodeled();
                0
            }
            _ => self.port.read(cx, address, size),
        }
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let offset = address.wrapping_sub(self.wiring.base);
        if matches!(offset, TRANSMIT | RECEIVE) {
            return cx.unmodeled();
        }
        self.port.write(cx, address, size, value);
        if size != 4 {
            return;
        }
        let link = self.link.clone();
        let mut link = link.lock().unwrap();
        match offset {
            ENABLE if value & 1 != 0 => link.reset(),
            TRANSFER if value & 3 == 3 && link.closing => {
                cx.fail("Lens transfer started before the previous frame's transmit store landed")
            }
            TRANSFER if value & 3 == 3 && !link.busy => self.begin(&mut link),
            TRANSFER if value & 3 == 0 && link.busy => self.end(cx, &mut link),
            _ => {}
        }
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        let link = self.link.clone();
        let mut link = link.lock().unwrap();
        if link.closing && link.transmitted.len() >= 4 {
            self.close(cx, &mut link);
        }
        drop(link);
        let now = cx.now();
        if now >= self.line.poll_at {
            self.line.poll_at = (now / self.timing.poll + 1) * self.timing.poll;
            match (self.power(), self.line.powered) {
                (true, false) => {
                    self.line.powered = true;
                    self.line.ready_at = now.checked_add(self.timing.ready);
                }
                (false, true) => {
                    // The focus group stays where it was; the iris rests open.
                    self.line = Line {
                        poll_at: self.line.poll_at,
                        drives: self.line.drives,
                        unknown: std::mem::take(&mut self.line.unknown),
                        focus: self.line.focus,
                        moves: std::mem::take(&mut self.line.moves),
                        moves_total: self.line.moves_total,
                        ..Line::default()
                    };
                    self.set_ready(cx, true);
                }
                _ => {}
            }
        }
        if self.line.ready_at.is_some_and(|at| now >= at) {
            self.line.ready_at = None;
            self.set_ready(cx, false);
            self.line.release_at = now.checked_add(self.timing.release);
        }
        if self.line.release_at.is_some_and(|at| now >= at) {
            self.line.release_at = None;
            self.line.released = true;
            self.set_ready(cx, true);
        }
        if self.line.drive_until.is_some_and(|at| now >= at) {
            self.line.drive_until = None;
            if let Some(target) = self.line.focus_target.take() {
                self.line.focus = target;
                if let Some(last) = self.line.moves.last_mut() {
                    last.reached = Some(target);
                }
            }
            self.set_ready(cx, true);
        }
    }
    fn next_deadline(&self, now: u64) -> Option<u64> {
        let closing = self.link.lock().unwrap().closing;
        [
            closing.then(|| now + 1),
            Some(self.line.poll_at),
            self.line.ready_at,
            self.line.release_at,
            self.line.drive_until,
        ]
        .into_iter()
        .flatten()
        .min()
    }
    fn pollable(&self, address: u64, size: u8) -> bool {
        self.port.pollable(address, size)
    }
    fn save(&self) -> Value {
        json!({
            "port": self.port.save(),
            "link": &*self.link.lock().unwrap(),
            "line": self.line,
        })
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let link: Link =
            serde_json::from_value(value["link"].clone()).map_err(|e| e.to_string())?;
        let line: Line =
            serde_json::from_value(value["line"].clone()).map_err(|e| e.to_string())?;
        self.check(&link, &line)?;
        self.port.restore(&value["port"])?;
        *self.link.lock().map_err(|_| "Lens link poisoned")? = link;
        self.line = line;
        Ok(())
    }
    /// The port is a child; the link shared with the PL330 source is restored
    /// in place.
    fn checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        let link = self.link.lock().map_err(|_| "Lens link poisoned")?;
        Ok(fujimod_emulator_devices::checkpoint::Checkpoint::encode(
            self.name(),
            &(&*link, &self.line),
        )?
        .with_children(vec![self.port.checkpoint()?]))
    }
    fn validate_checkpoint(
        &self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<(), String> {
        if saved.children().len() != 1 || !saved.blobs().is_empty() {
            return Err("Lens checkpoint parts differ".into());
        }
        let (link, line): (Link, Line) = saved.decode(self.name())?;
        self.check(&link, &line)?;
        self.port.validate_checkpoint(&saved.children()[0])
    }
    fn restore_checkpoint(
        &mut self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        let (link, line): (Link, Line) = saved.decode(self.name())?;
        self.port.restore_checkpoint(&saved.children()[0])?;
        *self.link.lock().map_err(|_| "Lens link poisoned")? = link;
        self.line = line;
        Ok(())
    }
    fn fingerprint(&self) -> Result<Value, String> {
        Ok(self.checkpoint()?.state())
    }
}
impl LensPort {
    fn check(&self, link: &Link, line: &Line) -> Result<(), String> {
        if !line.poll_at.is_multiple_of(self.timing.poll) || link.page_size > 1 << 15 {
            return Err("Invalid lens checkpoint".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::test_host::{TestHost, advance, write};
    use fujimod_emulator_ip::socionext::gpio::Pins;
    use fujimod_emulator_ip_arm::pl330::Source;

    const BASE: u64 = 0xfff2_8000;

    fn lens() -> (LensPort, SharedPins) {
        let pins = Pins::new([0; 16]);
        let profile = Profile {
            page: b"LY043A\0\0FLSNW039AF 25/1.7 XF".to_vec(),
            replies: BTreeMap::from([(0x20, 0x300), (0x26, 0x1c)]),
        };
        let timing = Timing {
            ready: 100,
            release: 1000,
            drive: 50,
            poll: 10,
        };
        let wiring = Wiring {
            base: BASE,
            power: (14, 1, 0),
            ready: (14, 1),
            line: 15,
            contacts: vec![(14, 0, true), (11, 0, true), (11, 1, false), (11, 2, true)],
        };
        let link = LensLink::new(BASE).unwrap();
        let port = LensPort::new(
            &link,
            profile,
            timing,
            wiring,
            pins.clone(),
            ExternalLines::default(),
        );
        (port.unwrap(), pins)
    }
    fn data(lens: &LensPort) -> LensLink {
        LensLink {
            base: lens.wiring.base,
            link: lens.link.clone(),
        }
    }
    /// One transfer: the body's word out, the lens's word back.
    fn transfer(lens: &mut LensPort, h: &mut TestHost, body: u32) -> u32 {
        let data = data(lens);
        write(lens, h, BASE + TRANSFER, 3);
        data.write(BASE + TRANSMIT, &body.to_be_bytes()).unwrap();
        let back = data.read(BASE + RECEIVE, 4).unwrap();
        write(lens, h, BASE + TRANSFER, 0);
        u32::from_be_bytes(back.try_into().unwrap())
    }

    #[test]
    fn a_frame_ended_before_its_transmit_store_completes_when_it_lands() {
        let (mut lens, _) = lens();
        let mut h = TestHost::new();
        let data = data(&lens);
        let query = seal(0x20 << 6);
        write(&mut lens, &mut h, BASE + TRANSFER, 3);
        data.read(BASE + RECEIVE, 4).unwrap();
        write(&mut lens, &mut h, BASE + TRANSFER, 0);
        assert!(lens.link.lock().unwrap().closing);
        data.write(BASE + TRANSMIT, &query.to_be_bytes()).unwrap();
        advance(&mut lens, &mut h, 1);
        assert!(!lens.link.lock().unwrap().closing);
        assert_eq!(transfer(&mut lens, &mut h, 0x0800_8832), 0x0800_8832);
        assert_eq!(
            transfer(&mut lens, &mut h, 0),
            seal(0x300 << 16 | 0x20 << 6)
        );
    }

    #[test]
    fn frames_carry_checksums_and_replies_follow_one_transaction_later() {
        assert_eq!(seal(0x0800_8000 | 0x20 << 6), 0x0800_8832);
        let (mut lens, _) = lens();
        let mut h = TestHost::new();
        let query = seal(0x20 << 6);
        assert_eq!(transfer(&mut lens, &mut h, query), 0);
        // The ack frame: the lens acknowledges command 0x20, sequence 0.
        assert_eq!(transfer(&mut lens, &mut h, 0x0800_8832), 0x0800_8832);
        // Next message frame: the reply to 0x20.
        assert_eq!(
            transfer(&mut lens, &mut h, 0),
            seal(0x300 << 16 | 0x20 << 6)
        );
        // The lens sent a message, so its acknowledge carries sequence 0.
        assert_eq!(transfer(&mut lens, &mut h, seal(ACK)), seal(ACK));
        let saved = lens.save();
        lens.restore(&saved).unwrap();
        // An unanswered command is answered with 0 and counted.
        transfer(&mut lens, &mut h, seal(0x30 << 6));
        assert_eq!(lens.line.unknown.get(&0x30), Some(&1));
    }

    #[test]
    fn a_read_session_sends_the_information_page() {
        let (mut lens, _) = lens();
        let mut h = TestHost::new();
        transfer(&mut lens, &mut h, seal(0x18 << 6 | 3 << 27));
        transfer(&mut lens, &mut h, 0);
        transfer(&mut lens, &mut h, seal(0x14 << 6));
        transfer(&mut lens, &mut h, 0);
        assert_eq!(lens.link.lock().unwrap().page_size, 8);
        // The reply to 0x14 goes out, then bulk mode starts after its ack.
        transfer(&mut lens, &mut h, 0);
        transfer(&mut lens, &mut h, 0);
        let data = data(&lens);
        write(&mut lens, &mut h, BASE + TRANSFER, 3);
        let page = data.read(BASE + RECEIVE, 11).unwrap();
        write(&mut lens, &mut h, BASE + TRANSFER, 0);
        assert_eq!(&page[..10], b"\0\0LY043A\0\0");
        assert_eq!(
            page[10],
            b"LY043A".iter().fold(0u8, |s, b| s.wrapping_add(*b))
        );
        write(&mut lens, &mut h, BASE + TRANSFER, 3);
        assert_eq!(data.read(BASE + RECEIVE, 2).unwrap(), [0xff, 0xff]);
        write(&mut lens, &mut h, BASE + TRANSFER, 0);
        assert_eq!(lens.link.lock().unwrap().bulk, None);
    }

    #[test]
    fn a_data_session_answers_one_empty_two_kib_page() {
        let (mut lens, _) = lens();
        let mut h = TestHost::new();
        // X-T2 4.41: command 0xc0 with data 6, then one 0x803-byte read.
        transfer(&mut lens, &mut h, seal(6 << 16 | DATA_SESSION << 6));
        transfer(&mut lens, &mut h, 0);
        transfer(&mut lens, &mut h, 0);
        transfer(&mut lens, &mut h, 0);
        assert_eq!(lens.link.lock().unwrap().bulk, Some(DATA_SESSION));
        let data = data(&lens);
        write(&mut lens, &mut h, BASE + TRANSFER, 3);
        let page = data.read(BASE + RECEIVE, 2 + DATA_PAGE_BYTES + 1).unwrap();
        write(&mut lens, &mut h, BASE + TRANSFER, 0);
        assert_eq!(page[..2], [0xff, 0xff]);
        assert!(page[2..2 + DATA_PAGE_BYTES].iter().all(|b| *b == 0));
        assert_eq!(page[2 + DATA_PAGE_BYTES], 0xfe);
        assert_eq!(lens.link.lock().unwrap().bulk, None);
    }

    #[test]
    fn power_starts_the_ready_handshake_and_moves_hold_the_line() {
        let (mut lens, pins) = lens();
        let mut h = TestHost::new();
        assert!(pins.lock().unwrap().input(14, 1));
        assert!(pins.lock().unwrap().input(11, 2) && !pins.lock().unwrap().input(11, 1));
        // lswatch drives bank 14 bit 0 high through masked register 1.
        let gpio = &mut fujimod_emulator_ip::socionext::gpio::Gpio::new(0xfff0_c000, pins.clone())
            .unwrap();
        write(gpio, &mut h, 0xfff0_c000 + 14 * 0x40 + 4, 0x0001_0001);
        advance(&mut lens, &mut h, 10);
        assert_eq!(lens.next_deadline(10), Some(20));
        advance(&mut lens, &mut h, 110);
        assert!(!pins.lock().unwrap().input(14, 1));
        advance(&mut lens, &mut h, 1110);
        assert!(pins.lock().unwrap().input(14, 1));
        transfer(&mut lens, &mut h, seal(0x56 << 6));
        assert!(!pins.lock().unwrap().input(14, 1));
        advance(&mut lens, &mut h, 1160);
        assert!(pins.lock().unwrap().input(14, 1));
        assert_eq!(lens.line.drives, 1);
    }

    /// A powered lens past its start-up, with the GPIO the body drives.
    fn released() -> (LensPort, TestHost) {
        let (mut lens, pins) = lens();
        let mut h = TestHost::new();
        let gpio = &mut fujimod_emulator_ip::socionext::gpio::Gpio::new(0xfff0_c000, pins).unwrap();
        write(gpio, &mut h, 0xfff0_c000 + 14 * 0x40 + 4, 0x0001_0001);
        for at in [10, 110, 1110] {
            advance(&mut lens, &mut h, at);
        }
        assert!(lens.line.released);
        (lens, h)
    }
    /// The lens's reply to one message: the message, the acknowledge frame, the frame that
    /// carries the reply, and the body's acknowledge of it.
    fn reply(lens: &mut LensPort, h: &mut TestHost, id: u32, data: u32) -> u32 {
        transfer(lens, h, seal(data << 16 | id << 6));
        transfer(lens, h, seal(ACK | id << 6));
        let answer = transfer(lens, h, 0);
        assert_eq!(command(answer), id);
        transfer(lens, h, seal(ACK));
        answer >> 16
    }

    #[test]
    fn set_commands_echo_the_body_data() {
        let (mut lens, mut h) = released();
        // Body data the captured lenses echoed: 0x54 0x7ef4, 0x55 0x203, 0xa9 1, 0xa0 0x8004.
        for (id, data) in [(0x54, 0x7ef4), (0x55, 0x203), (0xa9, 1), (0xa0, 0x8004)] {
            assert_eq!(reply(&mut lens, &mut h, id, data), data, "{id:#x}");
        }
        assert!(lens.line.unknown.is_empty());
    }

    #[test]
    fn a_focus_target_is_reached_when_the_drive_ends_and_status_shows_the_move() {
        let (mut lens, mut h) = released();
        assert_eq!(reply(&mut lens, &mut h, FOCUS_POSITION, 1), 0);
        assert_eq!(reply(&mut lens, &mut h, FOCUS_TARGET, 0x1855), 0x1855);
        assert_eq!(reply(&mut lens, &mut h, STATUS, 0), 0x300 | STATUS_MOVING);
        assert_eq!(reply(&mut lens, &mut h, FOCUS_POSITION, 1), 0);
        advance(&mut lens, &mut h, 1200);
        assert_eq!(reply(&mut lens, &mut h, STATUS, 0), 0x300);
        assert_eq!(reply(&mut lens, &mut h, FOCUS_POSITION, 1), 0x1855);
        // A target replaced before its drive ends is kept without a position.
        reply(&mut lens, &mut h, FOCUS_TARGET, 0xd20);
        reply(&mut lens, &mut h, FOCUS_TARGET, 0x16f8);
        advance(&mut lens, &mut h, 2400);
        let moves: Vec<_> = lens.line.moves.iter().map(|m| (m.sequence, m.target, m.reached)).collect();
        assert_eq!(moves, [(1, 0x1855, Some(0x1855)), (2, 0xd20, None), (3, 0x16f8, Some(0x16f8))]);
        assert_eq!(lens.line.moves_total, 3);
    }

    #[test]
    fn the_iris_reports_its_steps_and_opens_at_exposure() {
        let (mut lens, mut h) = released();
        // The captured shot: 0x1040, 0x1080, 0x10c0 (replies 0x100, 0x200, 0x300), then 0x7000.
        for (word, steps) in [(0x1040, 0x100), (0x1080, 0x200), (0x10c0, 0x300)] {
            reply(&mut lens, &mut h, IRIS_DRIVE, word);
            assert_eq!(reply(&mut lens, &mut h, IRIS_POSITION, 1), steps);
        }
        reply(&mut lens, &mut h, IRIS_DRIVE, IRIS_EXPOSURE);
        assert_eq!(reply(&mut lens, &mut h, IRIS_POSITION, 1), 0);
    }

    #[test]
    fn checkpoint_restores_the_link_shared_with_the_dma_source_in_place() {
        use fujimod_emulator_devices::test_host::assert_exact_restore;
        let (mut lens, _) = lens();
        let mut h = TestHost::new();
        write(&mut lens, &mut h, BASE + ENABLE, 1);
        assert_eq!(transfer(&mut lens, &mut h, seal(0x20 << 6)), 0);
        assert_eq!(transfer(&mut lens, &mut h, 0x0800_8832), 0x0800_8832);
        let link = data(&lens);
        let saved = assert_exact_restore(&mut lens, |d| {
            let mut h = TestHost::new();
            write(d, &mut h, BASE + ENABLE, 1);
            advance(d, &mut h, 5000);
        });
        // The source handle still reaches the restored link.
        assert!(Arc::ptr_eq(&link.link, &lens.link));
        let (other, _) = self::lens();
        other.validate_checkpoint(&saved).unwrap();
        let mut slower = self::lens().0;
        slower.timing.poll = 7;
        assert!(slower.validate_checkpoint(&saved).is_err());
    }
}
