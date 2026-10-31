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
/// Commands after which the lens moves and holds the ready line low.
const MOVES: [u32; 2] = [0x40, 0x56];

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
                let data = match self.profile.replies.get(&id) {
                    Some(&data) => u32::from(data),
                    None => {
                        *self.line.unknown.entry(id).or_default() += 1;
                        0
                    }
                };
                link.reply = seal(data << 16 | id << 6);
            }
            link.word = seal(ack);
            link.acknowledge = true;
        } else {
            link.word = link.reply;
            link.acknowledge = false;
            if is_message(link.sent) && [READ_SESSION, WRITE_SESSION].contains(&command(link.sent))
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
                    self.line = Line {
                        poll_at: self.line.poll_at,
                        drives: self.line.drives,
                        unknown: std::mem::take(&mut self.line.unknown),
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
        if line.poll_at % self.timing.poll != 0 || link.page_size > 1 << 15 {
            return Err("Invalid lens checkpoint".into());
        }
        self.port.restore(&value["port"])?;
        *self.link.lock().map_err(|_| "Lens link poisoned")? = link;
        self.line = line;
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
        transfer(&mut lens, &mut h, seal(0xa0 << 6));
        assert_eq!(lens.line.unknown.get(&0xa0), Some(&1));
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
}
