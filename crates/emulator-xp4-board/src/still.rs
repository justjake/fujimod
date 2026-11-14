//! Declared still-processing engines of the XP4.
//!
//! After the RAW read-out, the X-T3 5.11 still chain runs one engine per
//! step, each from its own driver. An engine here is the smallest contract
//! that lets the driver run honestly: the words the driver writes (and the
//! bits each may hold), the words it reads, the status bits the engine
//! owns, one start bit, and a completion after declared virtual time that
//! raises a level interrupt until the driver acknowledges it. Any other
//! access refuses. What each engine computes is declared per engine in
//! [`Output`]; an engine whose output is not computed leaves its
//! destination buffers untouched.
//!
//! # IIFP (0xfff6f000)
//!
//! The still step "recfIifpProcess" drives the engine through its driver:
//!
//! - Init powers the engine's clock, then the reset handshake writes +0x00
//!   with bit 31 clear and polls until bit 31 reads set, and writes
//!   +0x84 = 1.
//! - The programmer copies a 0x80-byte descriptor and writes it through one
//!   setter per field: +0x00 bits 5..4, 16, 18..17, bits 9..8 and +0x0c
//!   bits 29..16, bits 13..12, 20, 22..21, 29..24 and +0x48 bits 30..16,
//!   address pairs (low word, bits 1..0 of the high word) at +0x04/+0x08,
//!   +0x10/+0x14, +0x20..+0x2c and +0x30..+0x44, the packed fields
//!   +0x4c..+0x54, +0x58/+0x5c, +0x60..+0x6c and +0x70/+0x74, and +0x78
//!   bits 14..12, 30..28 with +0x7c.
//! - The start registers the handler on interrupt line 134, writes
//!   +0x88 = 1, then polls until +0x78 bits 8 and 24 both read
//!   set, sets +0x78 bits 15 and 31, writes the job buffer pair
//!   +0x18/+0x1c and sets +0x00 bits 31 and 0.
//! - The handler reads +0x80 (the step takes bit 5 clear as
//!   success), writes +0x84 = 1 and calls the job callback.
//! - The end clears +0x78 bits 15 and 31 and polls bits 8 and 24
//!   again, writes +0x88 = 0, then repeats the reset
//!   handshake and +0x84 = 1.
//!
//! +0x00 and +0x78 are updated read-modify-write, so the status bits they
//! read (+0x00 bit 31, +0x78 bits 8 and 24) come back in writes; the engine
//! ignores written status bits. The start bit +0x00 bit 0 never reads back,
//! so the reset handshake's read-modify-write does not start a second job.
//! +0x80 reads 0: no error bits. The role of +0x88 (written 1 before the
//! start and 0 after the end) is not decoded; the engine keeps its value.
//! In an emulated X-T3 5.11 still capture the descriptor points +0x10 at the
//! RAW active area (the read-out buffer + 36 lines of 0x31e0 bytes), +0x48
//! holds 0x18f0 (the line pitch in pixels) and +0x20..+0x40 hold five small
//! DRAM tables. What it computes is not decoded, so its output is declared
//! as not computed.
//!
//! # RESZ output (opt-in)
//!
//! [`Engine::computing`] makes a finished RESZ job scale its picture
//! ([`Output::Resize`]); every other engine has no computed output. The
//! words are the ones the X-T3 5.11 drivers program for a packed 4:2:2
//! picture (Y0 Y1 Cb Cr per pixel pair, the JPEG decoder's output) in
//! still capture and playback:
//!
//! - +0x0c and +0x18: source and output size, (lines - 1) << 16 |
//!   (pixels - 1), as [`fujimod_emulator_xpp::development::size`];
//! - +0x1c and +0x20: horizontal and vertical steps in source pixels per
//!   output pixel, 2^(bits 31..16) x (bits 15..0) / 0x1000, as
//!   [`fujimod_emulator_xpp::development::step`] (playback of a 3120 x 2080
//!   shot to 720 x 480: 0x21155 and 0x21156, 4.333);
//! - +0xc0 / +0xc4: source address (bits 33..32, bits 31..0), +0xd0 its
//!   pitch in bytes; +0xd4 / +0xd8 and +0xe4 the same for the output.
//!
//! Each output pixel takes the luma of the source pixel at the floor of its
//! scaled position, and each output pixel pair the chroma of the pair that
//! holds its first pixel. Nearest neighbour is a declared law: the drivers
//! program no interpolation kernel, so the engine's filter is fixed and
//! unknown. Not modeled: +0x10 (written 0), +0x14 (the size after the
//! pre-decimation the step's upper bits select), the second address pair
//! of each side (+0xc8 / +0xcc, +0xdc / +0xe0, written 0), +0x2c / +0x30
//! (not decoded) and the lens distortion correction at +0x24, +0x28,
//! +0x34..+0x3c and +0x40..+0xb8 (a 31-point radial table written by
//! 0x11131d4), which the firmware leaves at the identity in every job
//! measured so far.
//! The format word +0x08 holds 0x100 in all of them; a job whose source
//! (bits 1..0) or output (bits 5..4) format is not 0 refuses.
//!
//! # RECC output (opt-in)
//!
//! [`Engine::developing`] makes a finished RECC pass write its picture
//! ([`Output::Develop`]) from a RAW the wiring declares ([`Raw`]: where the
//! sensor writes it, its pitch and the X-Trans site of its first sample).
//! X-T3 5.11 programs three passes a still (0x1dc3490):
//!
//! - mode +0x08 = 0x620000, twice: channel 1 reads 16-bit RAW samples at
//!   +0x0c (bits 33..32 at +0x10), pitch +0x14, from line +0x1c >> 16;
//!   +0x18 is the band, lines << 16 | samples (2088 x 6256). Channel 4
//!   writes the packed 4:2:2 picture at +0x70 (+0x74), pitch +0x80, from
//!   line +0x54 >> 16. The two passes are the halves of one picture (lines
//!   0 and 2088), written 64 lines and 128 pixels into the picture buffer.
//! - mode +0x08 = 2: channel 2 reads +0x3c (+0x40), pitch +0x4c, size +0x50
//!   (lines << 16 | pixels, 4176 x 6256), and channel 4 writes it at the
//!   start of the buffer.
//!
//! A band is developed by [`fujimod_emulator_xpp::development::develop`]
//! (its declared X-Trans nearest-site sRGB law) from the site its source
//! address has in the declared RAW; the copy pass copies its lines as they
//! are. What the engine computes beyond that (the firmware's color, noise
//! reduction, sharpening, and the copy pass's own processing, lens
//! distortion correction among it) is not modeled. A pass in another mode,
//! with a column start (bits 15..0 of +0x1c / +0x54) or whose source is not
//! in the declared RAW at its pitch refuses.
use fujimod_emulator_devices::{Cx, Device};
use fujimod_emulator_xpp::development::{develop, size, step};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// One engine's register page.
pub const WINDOW: u64 = 0x1000;

/// Which declared engine a wiring names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// The IIFP engine.
    Iifp,
    /// The control page of the RECC engine.
    Recc,
    /// The RESZ resizer.
    Resz,
    /// The RESZL resizer.
    Reszl,
    /// The MIX engine.
    Mix,
    /// The RETCH engine.
    Retch,
}

/// What an engine's job writes when it finishes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Output {
    /// The job writes nothing: its destination buffers keep their contents.
    NotComputed,
    /// RESZ: the job scales a packed 4:2:2 picture (see the module notes).
    Resize,
    /// RECC: a pass develops its band from the declared RAW or copies the
    /// picture into place (see the module notes).
    Develop(Raw),
}

/// The sensor RAW a developing RECC reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Raw {
    /// Address of the RAW's first sample.
    pub address: u64,
    /// Line pitch in bytes.
    pub pitch: u64,
    /// X-Trans site (line, sample) of the first sample in
    /// [`fujimod_emulator_xpp::development::XTRANS`].
    pub site: (u8, u8),
}

/// The words a driver uses.
pub struct Contract {
    pub name: &'static str,
    /// Words the driver writes and the bits it may set in each.
    pub writable: Vec<(u64, u32)>,
    /// Words the driver reads.
    pub readable: Vec<u64>,
    /// Status bits that always read set (reset done).
    pub ready: Vec<(u64, u32)>,
    /// Status bits that read set while no job is in flight.
    pub idle: Vec<(u64, u32)>,
    /// The bit whose write starts a job; it never reads back.
    pub start: (u64, u32),
    /// The bit whose write acknowledges a finished job, which holds its
    /// level interrupt until then; without one the job pulses an edge.
    pub acknowledge: Option<(u64, u32)>,
    /// An interrupt flag block in place of the acknowledge bit.
    pub flags: Option<Flags>,
    pub output: Output,
}

/// An interrupt flag block: a finished job sets `done` in the raw word and
/// in the pending word, whose written bits clear; the level interrupt is
/// high while the pending word and the enable word share a bit.
#[derive(Clone, Copy, Debug)]
pub struct Flags {
    pub raw: u64,
    pub pending: u64,
    pub enable: u64,
    pub done: u32,
}

impl Contract {
    fn mask(&self, offset: u64) -> Option<u32> {
        self.writable
            .iter()
            .find(|&&(o, _)| o == offset)
            .map(|&(_, m)| m)
    }
    /// Status bits of `offset` the engine owns.
    fn status(&self, offset: u64) -> u32 {
        self.ready
            .iter()
            .chain(&self.idle)
            .filter(|&&(o, _)| o == offset)
            .fold(0, |bits, &(_, m)| bits | m)
    }
    fn check(&self) -> Result<(), String> {
        let mut offsets: Vec<u64> = self.writable.iter().map(|&(o, _)| o).collect();
        let count = offsets.len();
        offsets.sort_unstable();
        offsets.dedup();
        let bit = |&(o, m): &(u64, u32)| self.readable.contains(&o) && m != 0;
        let valid = offsets.len() == count
            && offsets
                .iter()
                .chain(&self.readable)
                .all(|o| o & 3 == 0 && *o < WINDOW)
            && self.ready.iter().chain(&self.idle).all(bit)
            && self.mask(self.start.0).is_some_and(|m| m & self.start.1 != 0)
            && self.start.1.count_ones() == 1
            && self.status(self.start.0) & self.start.1 == 0
            && self.acknowledge.is_none_or(|(o, bit)| {
                self.mask(o).is_some_and(|m| m & bit != 0) && bit.count_ones() == 1
            })
            && self.flags.is_none_or(|f| {
                self.acknowledge.is_none()
                    && [f.raw, f.pending]
                        .iter()
                        .all(|o| self.readable.contains(o))
                    && self.mask(f.pending).is_some_and(|m| m & f.done != 0)
                    && self.mask(f.enable).is_some_and(|m| m & f.done != 0)
                    && self.mask(f.raw).is_none()
                    && f.done != 0
            });
        if valid {
            Ok(())
        } else {
            Err(format!("Invalid {} register contract", self.name))
        }
    }
}

/// The contract of one declared engine.
pub fn contract(kind: Kind) -> Contract {
    match kind {
        Kind::Iifp => {
            let address = u32::MAX;
            let high = 0x3;
            Contract {
                name: "xp4-still-iifp",
                writable: vec![
                    // Bits 0 (start), 5..4, 9..8, 13..12, 18..16, 22..20,
                    // 29..24 and 31.
                    (0x00, 0xbf77_3331),
                    (0x04, address),
                    (0x08, high),
                    (0x0c, 0x3fff_0000),
                    (0x10, address),
                    (0x14, high),
                    (0x18, address),
                    (0x1c, high),
                    (0x20, address),
                    (0x24, high),
                    (0x28, address),
                    (0x2c, high),
                    (0x30, address),
                    (0x34, high),
                    (0x38, address),
                    (0x3c, high),
                    (0x40, address),
                    (0x44, high),
                    (0x48, 0x7fff_0000),
                    (0x4c, 0x3fff_3fff),
                    (0x50, 0x3fff_3fff),
                    (0x54, 0x0fff_0fff),
                    (0x58, 0x7fff_7fff),
                    (0x5c, 0x7fff_7fff),
                    (0x60, 0x3fff_3fff),
                    (0x64, 0x03ff_03ff),
                    (0x68, 0x3fff_3fff),
                    (0x6c, 0x3fff_0000),
                    (0x70, 0x7f7f_0000),
                    (0x74, 0x3fff_3fff),
                    // Bits 14..12 and 30..28, 15 and 31, and the idle bits 8
                    // and 24 the read-modify-write echoes.
                    (0x78, 0xf100_f100),
                    (0x7c, u32::MAX),
                    (0x84, 1),
                    (0x88, 1),
                ],
                readable: vec![0x00, 0x78, 0x80],
                ready: vec![(0x00, 1 << 31)],
                idle: vec![(0x78, 1 << 8 | 1 << 24)],
                start: (0x00, 1),
                acknowledge: Some((0x84, 1)),
                flags: None,
                output: Output::NotComputed,
            }
        }
        Kind::Recc => {
            let address = u32::MAX;
            let high = 0x3;
            let pair9 = 0x01ff_01ff;
            let pair14 = 0x3fff_3fff;
            // Bits 15..0, 18..16 and the channel enable 24.
            let channel = 0x0107_ffff;
            let writable = vec![
                (0x00, 0),
                (0x04, 1),
                (0x08, 0x007f_3117),
                (0x0c, address),
                (0x10, high),
                (0x14, 0x7fff),
                (0x18, pair14),
                (0x1c, pair14),
                (0x20, 0x3fff),
                (0x24, 0x3fff_07ff),
                (0x28, pair9),
                (0x2c, pair9),
                (0x30, high),
                (0x34, pair9),
                (0x38, high),
                (0x3c, address),
                (0x40, high),
                (0x44, address),
                (0x48, high),
                (0x4c, 0xffff),
                (0x50, pair14),
                (0x54, pair14),
                (0x58, 0x3fff),
                (0x5c, 0x3fff_07ff),
                (0x60, pair9),
                (0x64, pair9),
                (0x68, 0x1ff),
                (0x6c, 0x0001_0003),
                (0x70, address),
                (0x74, high),
                (0x78, address),
                (0x7c, high),
                (0x80, 0xffff),
                (0x84, pair9),
                (0x88, channel),
                (0x8c, channel),
                (0x90, channel),
            ];
            // The driver reads back every word through +0x94.
            let readable = (0..=0x94).step_by(4).collect();
            Contract {
                name: "xp4-still-recc",
                writable,
                readable,
                ready: vec![(0x00, 1)],
                idle: vec![(0x94, 0x111)],
                start: (0x04, 1),
                // Nothing acknowledges a pass, so each pass pulses an edge. The
                // firmware configures the interrupt in mode 3 like the countdown
                // timer interrupts; that mode 3 is an edge is inferred, not measured.
                acknowledge: None,
                flags: None,
                output: Output::NotComputed,
            }
        }
        Kind::Resz => {
            let address = u32::MAX;
            let high = 0x3;
            let mut writable = vec![
                (0x000, 0),
                (0x004, 1),
                // One setter per word.
                (0x008, 0x0011_0133),
                (0x00c, 0x3fff_3fff),
                (0x010, 0x3fff_3fff),
                (0x014, 0x3fff_3fff),
                (0x018, 0x3fff_3fff),
                (0x01c, 0x0007_ffff),
                (0x020, 0x0007_ffff),
                (0x024, 0x0001_1fff),
                (0x028, 0x1fff_1fff),
                (0x02c, 0x1_ffff),
                (0x030, 0x1_ffff),
                (0x034, 0x7fff_7fff),
                (0x038, 0x7fff_7fff),
                (0x03c, 0x003f_000f),
            ];
            // Thirty filter coefficients +0x40..+0xb8 (17 bits each).
            writable.extend((0x040..=0x0b8).step_by(4).map(|o| (o, 0x1_ffff)));
            writable.extend([
                // Address pairs (high word bits 1..0, low word) and the
                // pitches between them.
                (0x0c0, high),
                (0x0c4, address),
                (0x0c8, high),
                (0x0cc, address),
                (0x0d0, 0x3_ffff),
                (0x0d4, high),
                (0x0d8, address),
                (0x0dc, high),
                (0x0e0, address),
                (0x0e4, 0x3_ffff),
                (0x0e8, high),
                (0x0ec, address),
                (0x0f0, high),
                (0x0f4, address),
                (0x0f8, 0xffff),
                (0x0fc, 0x3ff),
                // Channel words: bits 15..0, 18..16, 21..20 (+0x104),
                // 24 and the enable 28.
                (0x100, 0x1107_ffff),
                (0x104, 0x1137_ffff),
                (0x108, 0x1003_000f),
                (0x10c, 1),
                (0x114, 0x1111_0007),
                (0x118, 0x3fff_3fff),
            ]);
            // The driver's register dump reads every word; the status
            // +0x10c and the error word +0x110 (bits 12 and 20).
            let mut readable: Vec<u64> = writable.iter().map(|&(o, _)| o).collect();
            readable.push(0x110);
            Contract {
                name: "xp4-still-resz",
                writable,
                readable,
                ready: vec![(0x000, 1)],
                // The stop waits for +0x10c to read exactly 0x01000100.
                idle: vec![(0x10c, 0x0100_0100)],
                start: (0x004, 1),
                acknowledge: Some((0x10c, 1)),
                flags: None,
                output: Output::NotComputed,
            }
        }
        Kind::Reszl => {
            let address = u32::MAX;
            let high = 0x3;
            let mut writable = vec![
                (0x000, 0),
                (0x004, 1),
                // One setter per word.
                (0x008, 0x111),
                (0x00c, 0x3fff_3fff),
                (0x010, 0x3fff_3fff),
                (0x014, 0x3fff_3fff),
                (0x018, 0x3fff_3fff),
                (0x01c, 0x0003_ffff),
                (0x020, 0x0003_ffff),
                (0x024, 0x0001_1fff),
                (0x028, 0x1fff_1fff),
                (0x02c, 0x1_ffff),
                (0x030, 0x1_ffff),
                (0x034, 0x7fff_7fff),
                (0x038, 0x7fff_7fff),
                (0x03c, 0x003f_000f),
            ];
            writable.extend((0x040..=0x0b8).step_by(4).map(|o| (o, 0x1_ffff)));
            writable.extend([
                // Address pairs and pitches.
                (0x0c0, high),
                (0x0c4, address),
                (0x0c8, 0x3_ffff),
                (0x0cc, high),
                (0x0d0, address),
                (0x0d4, 0x3_ffff),
                (0x0d8, 0x1ff),
                (0x0dc, 0x1107_ffff),
                (0x0e0, 0x1137_ffff),
                (0x0e4, 1),
                (0x100, 0x1111_0007),
                (0x104, 0x3fff),
            ]);
            // The driver's dump reads the words back; +0x0e4 status and
            // +0x0e8 error word.
            let mut readable: Vec<u64> = writable.iter().map(|&(o, _)| o).collect();
            readable.push(0x0e8);
            Contract {
                name: "xp4-still-reszl",
                writable,
                readable,
                ready: vec![(0x000, 1)],
                // The stop waits for +0x0e4 to read exactly 0x01000100.
                idle: vec![(0x0e4, 0x0100_0100)],
                start: (0x004, 1),
                acknowledge: Some((0x0e4, 1)),
                flags: None,
                output: Output::NotComputed,
            }
        }
        Kind::Mix => {
            let address = u32::MAX;
            let width = 0x7fff;
            // The driver has one setter per word.
            let writable = vec![
                // The reset writes 0 and waits for bit 0.
                (0x000, 1),
                // Start.
                (0x004, 1),
                (0x008, 0xc577),
                (0x010, address),
                (0x014, width),
                (0x018, address),
                (0x01c, width),
                (0x020, address),
                (0x024, width),
                (0x028, address),
                (0x02c, width),
                (0x030, address),
                (0x034, width),
                (0x038, address),
                (0x03c, address),
                (0x040, width),
                (0x044, 0x3fff_3fff),
                (0x048, 0x3ff),
                (0x054, 0x03ff_03ff),
                (0x058, 0x03ff_03ff),
                (0x060, u32::MAX),
                (0x064, 0x001f_ffff),
                (0x068, 0x7fff_3fff),
                (0x06c, 0x7fff_3fff),
                (0x070, 0x7fff_3fff),
                (0x074, 0x7fff_3fff),
                (0x078, 0x007f_ffff),
                (0x07c, 0x03ff_07ff),
                (0x080, 0x0f03),
                (0x090, 0x71ff_71ff),
                (0x094, 0x70ff),
                // The handler writes 1.
                (0x09c, 1),
            ];
            Contract {
                name: "xp4-still-mix",
                writable,
                readable: vec![0x000, 0x098],
                ready: vec![(0x000, 1)],
                // The driver waits for bits 18 and 2.
                idle: vec![(0x098, 1 << 18 | 1 << 2)],
                start: (0x004, 1),
                acknowledge: Some((0x09c, 1)),
                flags: None,
                output: Output::NotComputed,
            }
        }
        Kind::Retch => {
            let address = u32::MAX;
            let high = 0x3;
            // The six interrupt flags 12, 9, 8, 5, 4 and 0.
            let interrupts = 0x1331;
            // The driver has one setter per word.
            let writable = vec![
                // The reset writes 0 and waits for bit 0.
                (0x000, 1),
                // Start.
                (0x004, 1),
                (0x008, 0x0303_0003),
                (0x010, address),
                (0x014, high),
                (0x018, address),
                (0x01c, high),
                (0x020, 0xffff),
                (0x024, 0x3fff_3fff),
                (0x030, address),
                (0x034, high),
                (0x038, address),
                (0x03c, high),
                (0x040, 0xffff),
                (0x044, 0x3fff_3fff),
                (0x048, 0x3fff_3fff),
                (0x050, 1),
                (0x054, address),
                (0x058, high),
                (0x05c, 0x7_ffff),
                (0x060, 0x7_ffff),
                (0x064, 0x000f_03ff),
                (0x068, 0x7fff_ffff),
                (0x06c, 0x7_ffff),
                (0x070, 0x7_ffff),
                (0x074, 0x000f_03ff),
                (0x078, 0x7fff_ffff),
                (0x07c, 0x3fff_3fff),
                (0x080, 0x3fff_3fff),
                // Channel words: bits 2..0, 19..4 and the enable 28.
                (0x090, 0x100f_fff7),
                (0x094, 0x100f_fff7),
                (0x0a4, interrupts),
                (0x0a8, interrupts),
            ];
            Contract {
                name: "xp4-still-retch",
                writable,
                readable: vec![0x000, 0x008, 0x050, 0x098, 0x0a0, 0x0a4],
                ready: vec![(0x000, 1)],
                // The driver's start and stop wait for bits 16 and 0.
                idle: vec![(0x098, 0x0001_0001)],
                start: (0x004, 1),
                acknowledge: None,
                // The start enables flags 12, 9, 8 and 0 (+0xa8); the handler
                // writes all six to the pending word +0xa4, reads the raw word
                // +0xa0 and writes it back to +0xa4. Which flag reports a
                // finished job is not decoded: flag 0 is declared.
                flags: Some(Flags {
                    raw: 0x0a0,
                    pending: 0x0a4,
                    enable: 0x0a8,
                    done: 1,
                }),
                output: Output::NotComputed,
            }
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    words: BTreeMap<u64, u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    due: Option<u64>,
    #[serde(default)]
    finished: bool,
    #[serde(default)]
    jobs: u64,
    /// Raw and pending flags of an interrupt flag block.
    #[serde(default)]
    raw: u32,
    #[serde(default)]
    pending: u32,
}

pub struct Engine {
    kind: Kind,
    base: u64,
    irq: u32,
    ticks: u64,
    contract: Contract,
    state: State,
}

impl Engine {
    pub fn new(kind: Kind, base: u64, irq: u32, ticks: u64) -> Result<Self, String> {
        let contract = contract(kind);
        contract.check()?;
        if base & (WINDOW - 1) != 0 || !(32..512).contains(&irq) || ticks == 0 {
            return Err(format!("Invalid {} wiring", contract.name));
        }
        Ok(Self {
            kind,
            base,
            irq,
            ticks,
            contract,
            state: State::default(),
        })
    }
    /// The engine also computes its declared output (opt-in). Only RESZ has
    /// one, [`Output::Resize`].
    pub fn computing(mut self) -> Result<Self, String> {
        self.contract.output = match self.kind {
            Kind::Resz => Output::Resize,
            Kind::Recc => {
                return Err(format!(
                    "{} develops from a declared RAW",
                    self.contract.name
                ));
            }
            _ => return Err(format!("{} has no computed output", self.contract.name)),
        };
        Ok(self)
    }
    /// RECC develops its passes from `raw` (opt-in, see the module notes).
    pub fn developing(mut self, raw: Raw) -> Result<Self, String> {
        let name = self.contract.name;
        if self.kind != Kind::Recc {
            return Err(format!("{name} does not develop"));
        }
        if raw.site.0 > 5 || raw.site.1 > 5 || raw.pitch < 12 || !raw.pitch.is_multiple_of(2) {
            return Err(format!("Invalid {name} RAW"));
        }
        self.contract.output = Output::Develop(raw);
        Ok(self)
    }
    /// One RECC pass: a band developed from the RAW, or the copy into place.
    fn pass(&self, cx: &mut Cx<'_>, raw: Raw) -> Result<(), String> {
        let name = self.contract.name;
        let word = |o| u64::from(self.word(o));
        let address = |high, low| (word(high) & 3) << 32 | word(low);
        if word(0x1c) & 0xffff != 0 || word(0x54) & 0xffff != 0 {
            return Err(format!("{name} pass starts at a column"));
        }
        let (first, rows) = match self.word(0x008) {
            0x0062_0000 => {
                let (pitch, band) = (word(0x14), word(0x18));
                let (lines, width) = (band >> 16, band & 0xffff);
                let source = address(0x10, 0x0c) + (word(0x1c) >> 16) * pitch;
                let offset = source
                    .checked_sub(raw.address)
                    .filter(|_| pitch == raw.pitch)
                    .ok_or_else(|| {
                        format!("{name} source {source:#x} is not in the declared RAW")
                    })?;
                let phase = (
                    (usize::from(raw.site.0) + (offset / pitch) as usize) % 6,
                    (usize::from(raw.site.1) + (offset % pitch / 2) as usize) % 6,
                );
                let length = pitch * lines.saturating_sub(1) + 2 * width;
                let bytes = cx.ram_bytes(source, length as usize)?;
                let rows = develop(
                    &bytes,
                    pitch as usize,
                    lines as usize,
                    width as usize,
                    phase,
                    None,
                )?;
                (word(0x54) >> 16, rows)
            }
            2 => {
                let (pitch, picture) = (word(0x4c), word(0x50));
                let (lines, width) = (picture >> 16, picture & 0xffff);
                if lines == 0 || width == 0 || pitch < 2 * width {
                    return Err(format!("{name} copy {width}x{lines} pitch {pitch}"));
                }
                let bytes = cx.ram_bytes(
                    address(0x40, 0x3c),
                    (pitch * (lines - 1) + 2 * width) as usize,
                )?;
                let rows = bytes
                    .chunks(pitch as usize)
                    .map(|row| row[..2 * width as usize].to_vec())
                    .collect();
                (0, rows)
            }
            mode => return Err(format!("{name} mode {mode:#x} is not modeled")),
        };
        let pitch = word(0x80);
        let output = address(0x74, 0x70) + first * pitch;
        for (y, row) in rows.iter().enumerate() {
            if row.len() as u64 > pitch {
                return Err(format!("{name} picture line wider than pitch {pitch}"));
            }
            cx.ram_write(output + y as u64 * pitch, row)?;
        }
        Ok(())
    }
    /// Scales the programmed packed 4:2:2 picture (nearest neighbour).
    fn resize(&self, cx: &mut Cx<'_>) -> Result<(), String> {
        let name = self.contract.name;
        let format = self.word(0x008);
        if format & 0x33 != 0 {
            return Err(format!("{name} format {format:#x} is not packed 4:2:2"));
        }
        let ((sh, sw), (oh, ow)) = (size(self.word(0x00c)), size(self.word(0x018)));
        let (Some(hx), Some(hy)) = (step(self.word(0x01c)), step(self.word(0x020))) else {
            return Err(format!("{name} has no step"));
        };
        let address = |high, low| u64::from(self.word(high) & 3) << 32 | u64::from(self.word(low));
        let (source, spitch) = (address(0x0c0, 0x0c4), self.word(0x0d0) as usize);
        let (output, opitch) = (address(0x0d4, 0x0d8), self.word(0x0e4) as usize);
        if sw % 2 != 0 || ow % 2 != 0 || sw * 2 > spitch || ow * 2 > opitch {
            return Err(format!(
                "{name} {sw}x{sh} pitch {spitch} -> {ow}x{oh} pitch {opitch}"
            ));
        }
        let pixels = cx.ram_bytes(source, spitch * (sh - 1) + sw * 2)?;
        // The steps are dyadic, so the products are exact.
        let at = |i: usize, step: f64, limit: usize| ((i as f64 * step) as usize).min(limit - 1);
        let mut line = vec![0; ow * 2];
        for y in 0..oh {
            let row = &pixels[at(y, hy, sh) * spitch..];
            for (pair, out) in line.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let (x0, x1) = (at(pair * 2, hx, sw), at(pair * 2 + 1, hx, sw));
                let (p0, p1) = (x0 / 2 * 4, x1 / 2 * 4);
                *out = [row[p0 + x0 % 2], row[p1 + x1 % 2], row[p0 + 2], row[p0 + 3]];
            }
            cx.ram_write(output + (y * opitch) as u64, &line)?;
        }
        Ok(())
    }
    pub fn window(&self) -> (u64, u64) {
        (self.base, self.base + WINDOW)
    }
    fn word(&self, offset: u64) -> u32 {
        self.state.words.get(&offset).copied().unwrap_or(0)
    }
    fn offset(&self, address: u64, size: u8) -> Option<u64> {
        let offset = address.wrapping_sub(self.base);
        (size == 4 && offset & 3 == 0 && offset < WINDOW).then_some(offset)
    }
    fn flag_level(&self, cx: &mut Cx<'_>, f: Flags) {
        let level = self.state.pending & self.word(f.enable) != 0;
        cx.set_irq(self.irq, level);
    }
    fn check(&self, state: &State) -> Result<(), String> {
        let declared = |o: &u64, v: &u32| {
            self.contract.mask(*o).is_some_and(|m| {
                v & !m == 0 && v & self.contract.status(*o) == 0
                    && !(*o == self.contract.start.0 && v & self.contract.start.1 != 0)
            })
        };
        if state.words.iter().any(|(o, v)| *v == 0 || !declared(o, v))
            || (state.due.is_some() && state.finished)
            || self
                .contract
                .flags
                .map_or(state.raw | state.pending != 0, |f| {
                    (state.raw | state.pending) & !f.done != 0
                })
        {
            return Err(format!("Invalid {} checkpoint", self.contract.name));
        }
        Ok(())
    }
}

impl Device for Engine {
    fn name(&self) -> &'static str {
        self.contract.name
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let Some(offset) = self
            .offset(address, size)
            .filter(|o| self.contract.readable.contains(o))
        else {
            cx.unmodeled();
            return 0;
        };
        let busy = self.state.due.is_some();
        let status = self
            .contract
            .ready
            .iter()
            .chain(self.contract.idle.iter().filter(|_| !busy))
            .filter(|&&(o, _)| o == offset)
            .fold(0, |bits, &(_, m)| bits | m);
        let done = match self.contract.acknowledge {
            Some((o, bit)) if o == offset && self.state.finished => bit,
            _ => 0,
        };
        match self.contract.flags {
            Some(f) if f.raw == offset => return u64::from(self.state.raw),
            Some(f) if f.pending == offset => return u64::from(self.state.pending),
            _ => {}
        }
        u64::from(self.word(offset) | status | done)
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let (Some(offset), Ok(value)) = (self.offset(address, size), u32::try_from(value)) else {
            return cx.unmodeled();
        };
        let Some(mask) = self.contract.mask(offset) else {
            return cx.unmodeled();
        };
        if value & !mask != 0 {
            return cx.fail(format!(
                "{} +{offset:#x} write {value:#x} sets bits outside {mask:#x}",
                self.contract.name
            ));
        }
        let (start_word, start) = self.contract.start;
        if offset == start_word && value & start != 0 {
            if self.state.due.is_some() || self.state.finished {
                return cx.fail(format!(
                    "{} started while a job is unacknowledged",
                    self.contract.name
                ));
            }
            let Some(due) = cx.now().checked_add(self.ticks) else {
                return cx.fail(format!("{} deadline overflow", self.contract.name));
            };
            self.state.due = Some(due);
            if let Some(f) = self.contract.flags {
                self.state.raw &= !f.done;
            }
        }
        if let Some(f) = self.contract.flags
            && offset == f.pending
        {
            self.state.pending &= !value;
            self.flag_level(cx, f);
            return;
        }
        let mut kept = value & !self.contract.status(offset);
        if offset == start_word {
            kept &= !start;
        }
        if let Some((ack_word, ack)) = self.contract.acknowledge
            && offset == ack_word
        {
            kept &= !ack;
            if value & ack != 0 && self.state.finished {
                self.state.finished = false;
                cx.set_irq(self.irq, false);
            }
        }
        if kept == 0 {
            self.state.words.remove(&offset);
        } else {
            self.state.words.insert(offset, kept);
        }
        if let Some(f) = self.contract.flags
            && offset == f.enable
        {
            self.flag_level(cx, f);
        }
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        if !self.state.due.is_some_and(|at| at <= cx.now()) {
            return;
        }
        self.state.due = None;
        match self.contract.output {
            Output::NotComputed => {}
            Output::Resize => {
                if let Err(error) = self.resize(cx) {
                    return cx.fail(error);
                }
            }
            Output::Develop(raw) => {
                if let Err(error) = self.pass(cx, raw) {
                    return cx.fail(error);
                }
            }
        }
        self.state.jobs += 1;
        if let Some(f) = self.contract.flags {
            self.state.raw |= f.done;
            self.state.pending |= f.done;
            return self.flag_level(cx, f);
        }
        cx.set_irq(self.irq, true);
        if self.contract.acknowledge.is_some() {
            self.state.finished = true;
        } else {
            cx.set_irq(self.irq, false);
        }
    }
    fn next_deadline(&self, _now: u64) -> Option<u64> {
        self.state.due
    }
    fn save(&self) -> Value {
        serde_json::to_value(&self.state).unwrap_or(Value::Null)
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let state: State = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        self.check(&state)?;
        self.state = state;
        Ok(())
    }
    fujimod_emulator_devices::checkpoint_state!(state: State, Self::check);
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::test_host::recording::RecordingHost;

    const BASE: u64 = 0xfff6f000;
    const IRQ: u32 = 134;

    fn engine() -> Engine {
        Engine::new(Kind::Iifp, BASE, IRQ, 100).unwrap()
    }
    fn write(e: &mut Engine, h: &mut RecordingHost, offset: u64, value: u32) {
        let mut cx = Cx::new(h, 0, 0, 0);
        e.write(&mut cx, BASE + offset, 4, value.into());
        assert!(cx.refusal().is_none(), "+{offset:#x} {value:#x}");
    }
    fn read(e: &mut Engine, h: &mut RecordingHost, offset: u64) -> u32 {
        let mut cx = Cx::new(h, 0, 0, 0);
        let value = e.read(&mut cx, BASE + offset, 4) as u32;
        assert!(cx.refusal().is_none(), "+{offset:#x}");
        value
    }
    fn refuses_write(offset: u64, value: u32) -> bool {
        let mut e = engine();
        let mut h = RecordingHost::default();
        let mut cx = Cx::new(&mut h, 0, 0, 0);
        e.write(&mut cx, BASE + offset, 4, value.into());
        cx.refusal().is_some()
    }
    fn advance(e: &mut Engine, h: &mut RecordingHost, now: u64) {
        h.now = now;
        let mut cx = Cx::new(h, 0, 0, 0);
        e.advance(&mut cx);
        assert!(cx.refusal().is_none());
    }
    /// The reset handshake then +0x84 = 1.
    fn reset(e: &mut Engine, h: &mut RecordingHost) {
        let control = read(e, h, 0x00);
        write(e, h, 0x00, control & 0x7fff_ffff);
        assert_ne!(read(e, h, 0x00) & 1 << 31, 0);
        write(e, h, 0x84, 1);
    }

    #[test]
    fn the_driver_sequence_runs_one_job_and_interrupts_until_acknowledged() {
        let mut e = engine();
        let mut h = RecordingHost::default();
        reset(&mut e, &mut h);
        // The programmer's read-modify-write setters and the addresses of an
        // emulated X-T3 5.11 still capture.
        let control = read(&mut e, &mut h, 0x00);
        write(&mut e, &mut h, 0x00, control | 2 << 4 | 1 << 16);
        write(&mut e, &mut h, 0x10, 0x4744_9380);
        write(&mut e, &mut h, 0x48, 0x18f0 << 16);
        write(&mut e, &mut h, 0x88, 1);
        // The start: idle, enable both channels, job buffer, start.
        assert_eq!(read(&mut e, &mut h, 0x78) & 0x0100_0100, 0x0100_0100);
        let channels = read(&mut e, &mut h, 0x78);
        write(&mut e, &mut h, 0x78, channels | 0x8000_8000);
        write(&mut e, &mut h, 0x18, 0x4000_0000);
        let control = read(&mut e, &mut h, 0x00);
        write(&mut e, &mut h, 0x00, control | 0x8000_0001);
        assert_eq!(e.next_deadline(0), Some(100));
        assert_eq!(read(&mut e, &mut h, 0x78) & 0x0100_0100, 0);
        advance(&mut e, &mut h, 99);
        assert!(h.irqs.is_empty());
        advance(&mut e, &mut h, 100);
        assert_eq!(h.irqs.last(), Some(&(IRQ, true)));
        // The handler: result, acknowledge.
        assert_eq!(read(&mut e, &mut h, 0x80) & 1 << 5, 0);
        write(&mut e, &mut h, 0x84, 1);
        assert_eq!(h.irqs.last(), Some(&(IRQ, false)));
        // The end; the start bit never reads back, so the reset's
        // read-modify-write starts nothing.
        let channels = read(&mut e, &mut h, 0x78);
        write(&mut e, &mut h, 0x78, channels & 0x7fff_7fff);
        assert_eq!(read(&mut e, &mut h, 0x78) & 0x0100_0100, 0x0100_0100);
        write(&mut e, &mut h, 0x88, 0);
        assert_eq!(read(&mut e, &mut h, 0x00) & 1, 0);
        reset(&mut e, &mut h);
        assert_eq!(e.next_deadline(0), None);
        assert_eq!(read(&mut e, &mut h, 0x00), 0x8000_0000 | 2 << 4 | 1 << 16);
        let saved = e.save();
        let mut other = engine();
        other.restore(&saved).unwrap();
        assert_eq!(read(&mut other, &mut h, 0x00), 0x8000_0000 | 2 << 4 | 1 << 16);
    }

    #[test]
    fn undeclared_words_and_bits_refuse() {
        assert!(refuses_write(0x8c, 0));
        assert!(refuses_write(0x80, 0));
        assert!(refuses_write(0x00, 1 << 30));
        assert!(refuses_write(0x08, 4));
        assert!(refuses_write(0x84, 2));
        assert!(!refuses_write(0x7c, u32::MAX));
        let mut e = engine();
        let mut h = RecordingHost::default();
        let mut cx = Cx::new(&mut h, 0, 0, 0);
        e.read(&mut cx, BASE + 0x04, 4);
        assert!(cx.refusal().is_some());
    }

    #[test]
    fn the_recc_control_page_pulses_an_edge_per_pass() {
        const RECC: u64 = 0xfff82000;
        let mut e = Engine::new(Kind::Recc, RECC, 129, 50).unwrap();
        let mut h = RecordingHost::default();
        let w = |e: &mut Engine, h: &mut RecordingHost, o: u64, v: u32| {
            let mut cx = Cx::new(h, 0, 0, 0);
            e.write(&mut cx, RECC + o, 4, v.into());
            assert!(cx.refusal().is_none(), "+{o:#x} {v:#x}");
        };
        let r = |e: &mut Engine, h: &mut RecordingHost, o: u64| {
            let mut cx = Cx::new(h, 0, 0, 0);
            let v = e.read(&mut cx, RECC + o, 4) as u32;
            assert!(cx.refusal().is_none(), "+{o:#x}");
            v
        };
        // The three channel enables clear, +0x94 reads idle.
        for o in [0x88, 0x8c, 0x90] {
            let v = r(&mut e, &mut h, o);
            w(&mut e, &mut h, o, v & 0xfeff_ffff);
        }
        assert_eq!(!r(&mut e, &mut h, 0x94) & 0x111, 0);
        // Addresses, enables, the reset wait, start.
        w(&mut e, &mut h, 0x0c, 0x4744_9380);
        w(&mut e, &mut h, 0x3c, 0x5000_0000);
        w(&mut e, &mut h, 0x88, 1 << 24 | 0x1234);
        w(&mut e, &mut h, 0x00, 0);
        assert_eq!(r(&mut e, &mut h, 0x00) & 1, 1);
        for pass in 1..=2u64 {
            w(&mut e, &mut h, 0x04, 1);
            assert_eq!(r(&mut e, &mut h, 0x94) & 0x111, 0);
            assert_eq!(r(&mut e, &mut h, 0x04), 0);
            h.now = 50 * pass + 10 * (pass - 1);
            let mut cx = Cx::new(&mut h, 0, 0, 0);
            e.advance(&mut cx);
            assert!(cx.refusal().is_none());
            assert_eq!(&h.irqs[h.irqs.len() - 2..], &[(129, true), (129, false)]);
            h.now += 10;
        }
        assert_eq!(r(&mut e, &mut h, 0x88), 1 << 24 | 0x1234);
        let mut cx = Cx::new(&mut h, 0, 0, 0);
        e.write(&mut cx, RECC + 0x00, 4, 1);
        assert!(cx.refusal().is_some());
    }

    /// The resizers' status word: done bit 0 reads set until the handler
    /// writes 1, and the stop waits for it to read exactly 0x01000100.
    #[test]
    fn the_resizers_report_done_until_acknowledged_then_idle() {
        for (kind, base, status, irq) in [
            (Kind::Resz, 0xfff7e000u64, 0x10cu64, 132u32),
            (Kind::Reszl, 0xfff7f000, 0x0e4, 133),
        ] {
            let mut e = Engine::new(kind, base, irq, 20).unwrap();
            let mut h = RecordingHost::default();
            let access = |e: &mut Engine, h: &mut RecordingHost, o: u64, v: Option<u32>| {
                let mut cx = Cx::new(h, 0, 0, 0);
                let r = match v {
                    Some(v) => {
                        e.write(&mut cx, base + o, 4, v.into());
                        0
                    }
                    None => e.read(&mut cx, base + o, 4) as u32,
                };
                assert!(cx.refusal().is_none(), "{kind:?} +{o:#x}");
                r
            };
            access(&mut e, &mut h, status, Some(1));
            access(&mut e, &mut h, 0x000, Some(0));
            assert_eq!(access(&mut e, &mut h, 0x000, None) & 1, 1);
            let start = access(&mut e, &mut h, 0x004, None);
            access(&mut e, &mut h, 0x004, Some(start | 1));
            assert_eq!(access(&mut e, &mut h, status, None), 0);
            h.now = 20;
            let mut cx = Cx::new(&mut h, 0, 0, 0);
            e.advance(&mut cx);
            assert_eq!(h.irqs.last(), Some(&(irq, true)));
            assert_eq!(access(&mut e, &mut h, status, None), 0x0100_0101);
            access(&mut e, &mut h, status, Some(1));
            assert_eq!(h.irqs.last(), Some(&(irq, false)));
            assert_eq!(access(&mut e, &mut h, status, None), 0x0100_0100);
        }
    }

    #[test]
    fn every_declared_contract_is_valid() {
        for kind in [
            Kind::Iifp,
            Kind::Recc,
            Kind::Resz,
            Kind::Reszl,
            Kind::Mix,
            Kind::Retch,
        ] {
            Engine::new(kind, 0xfff0_0000, 100, 1).unwrap();
        }
    }

    /// RETCH: the handler clears the pending flags.
    #[test]
    fn a_flag_block_interrupts_while_an_enabled_flag_is_pending() {
        const RETCH: u64 = 0xfffa3000;
        let mut e = Engine::new(Kind::Retch, RETCH, 142, 40).unwrap();
        let mut h = RecordingHost::default();
        let access = |e: &mut Engine, h: &mut RecordingHost, o: u64, v: Option<u32>| {
            let mut cx = Cx::new(h, 0, 0, 0);
            let r = match v {
                Some(v) => {
                    e.write(&mut cx, RETCH + o, 4, v.into());
                    0
                }
                None => e.read(&mut cx, RETCH + o, 4) as u32,
            };
            assert!(cx.refusal().is_none(), "+{o:#x}");
            r
        };
        access(&mut e, &mut h, 0x0a8, Some(0x1301));
        assert_eq!(access(&mut e, &mut h, 0x098, None) & 0x10001, 0x10001);
        access(&mut e, &mut h, 0x090, Some(1 << 28 | 0x10));
        access(&mut e, &mut h, 0x004, Some(1));
        assert_eq!(access(&mut e, &mut h, 0x098, None) & 0x10001, 0);
        h.now = 40;
        let mut cx = Cx::new(&mut h, 0, 0, 0);
        e.advance(&mut cx);
        assert_eq!(h.irqs.last(), Some(&(142, true)));
        access(&mut e, &mut h, 0x0a4, Some(0x1331));
        assert_eq!(h.irqs.last(), Some(&(142, false)));
        assert_eq!(access(&mut e, &mut h, 0x0a0, None), 1);
        assert_eq!(access(&mut e, &mut h, 0x0a4, None), 0);
        // A finished job with its flag disabled holds the line low.
        access(&mut e, &mut h, 0x0a8, Some(0x1300));
        access(&mut e, &mut h, 0x004, Some(1));
        h.now = 80;
        let mut cx = Cx::new(&mut h, 0, 0, 0);
        e.advance(&mut cx);
        assert_eq!(h.irqs.last(), Some(&(142, false)));
        assert_eq!(access(&mut e, &mut h, 0x0a4, None), 1);
    }

    #[test]
    fn a_second_start_before_the_acknowledge_refuses() {
        let mut e = engine();
        let mut h = RecordingHost::default();
        write(&mut e, &mut h, 0x00, 1);
        advance(&mut e, &mut h, 100);
        let mut cx = Cx::new(&mut h, 0, 0, 0);
        e.write(&mut cx, BASE, 4, 1);
        assert!(cx.refusal().is_some());
    }

    const RESZ: u64 = 0xfff7e000;
    const RESZ_IRQ: u32 = 132;

    /// A packed 4:2:2 picture: luma x + 3y, Cb the pixel pair, Cr the line.
    fn picture(w: usize, h: usize, pitch: usize) -> Vec<u8> {
        let mut bytes = vec![0; pitch * h];
        for y in 0..h {
            for x in 0..w {
                bytes[y * pitch + x / 2 * 4 + x % 2] = (x + 3 * y) as u8;
            }
            for pair in 0..w / 2 {
                bytes[y * pitch + pair * 4 + 2] = pair as u8;
                bytes[y * pitch + pair * 4 + 3] = y as u8;
            }
        }
        bytes
    }
    /// Programs one RESZ job, starts it and lets it finish; returns the
    /// engine's refusal, if any.
    fn resize_job(e: &mut Engine, h: &mut RecordingHost, words: &[(u64, u32)]) -> Option<String> {
        for &(o, v) in words.iter().chain(&[(0x004, 1)]) {
            let mut cx = Cx::new(h, 0, 0, 0);
            e.write(&mut cx, RESZ + o, 4, v.into());
            assert!(cx.refusal().is_none(), "+{o:#x} {v:#x}");
        }
        h.now += 20;
        let mut cx = Cx::new(h, 0, 0, 0);
        e.advance(&mut cx);
        cx.take_refusal()
    }
    /// The words of a job: sizes, steps, source and output address and pitch.
    fn job(
        sizes: (u32, u32),
        steps: (u32, u32),
        source: (u32, u32),
        output: (u32, u32),
    ) -> Vec<(u64, u32)> {
        vec![
            (0x008, 0x100),
            (0x00c, sizes.0),
            (0x018, sizes.1),
            (0x01c, steps.0),
            (0x020, steps.1),
            (0x0c4, source.0),
            (0x0d0, source.1),
            (0x0d8, output.0),
            (0x0e4, output.1),
        ]
    }
    /// The source pixel an output index reads: floor(i x step), the step
    /// 2^(bits 31..16) x (bits 15..0) / 0x1000.
    fn source_index(i: usize, step: u32) -> usize {
        ((i * (step as usize & 0xffff)) << (step >> 16)) >> 12
    }

    /// The step words of an emulated X-T3 5.11 playback (3120 x 2080 to
    /// 720 x 480) on a smaller picture: each output pixel takes the luma of
    /// its floor source pixel and the chroma of that pixel's pair.
    #[test]
    fn a_computing_resizer_scales_by_the_programmed_steps() {
        let (sw, sh, spitch, ow, oh, opitch) = (64, 40, 136, 14, 9, 32);
        let (hx, hy) = (0x21155, 0x21156);
        let mut h = RecordingHost {
            ram: vec![0xee; 0x4000],
            ..Default::default()
        };
        h.ram[..spitch * sh].copy_from_slice(&picture(sw, sh, spitch));
        let mut e = Engine::new(Kind::Resz, RESZ, RESZ_IRQ, 20)
            .unwrap()
            .computing()
            .unwrap();
        let words = job(
            (
                (sh as u32 - 1) << 16 | (sw as u32 - 1),
                (oh as u32 - 1) << 16 | (ow as u32 - 1),
            ),
            (hx, hy),
            (0, spitch as u32),
            (0x2000, opitch as u32),
        );
        assert_eq!(resize_job(&mut e, &mut h, &words), None);
        assert_eq!(h.irqs.last(), Some(&(RESZ_IRQ, true)));
        for y in 0..oh {
            let line = &h.ram[0x2000 + y * opitch..][..opitch];
            let sy = source_index(y, hy);
            for x in 0..ow {
                let sx = source_index(x, hx);
                assert_eq!(usize::from(line[x / 2 * 4 + x % 2]), sx + 3 * sy, "{x} {y}");
            }
            for pair in 0..ow / 2 {
                let chroma = &line[pair * 4 + 2..pair * 4 + 4];
                assert_eq!(chroma, [(source_index(pair * 2, hx) / 2) as u8, sy as u8]);
            }
            // The pitch's padding is left alone.
            assert!(line[ow * 2..].iter().all(|&b| b == 0xee));
        }
        // Step 4.333 from the last output pixel stays inside the source.
        assert!(source_index(ow - 1, hx) < sw && source_index(oh - 1, hy) < sh);
    }

    /// A still capture's halving step (0x11000) and a playback copy (0x1000)
    /// into a wider output at an offset.
    #[test]
    fn halving_and_copy_steps_place_the_lines_at_the_output_pitch() {
        let (sw, sh, spitch) = (16, 8, 32);
        let mut h = RecordingHost {
            ram: vec![0; 0x4000],
            ..Default::default()
        };
        h.ram[..spitch * sh].copy_from_slice(&picture(sw, sh, spitch));
        let mut e = Engine::new(Kind::Resz, RESZ, RESZ_IRQ, 20)
            .unwrap()
            .computing()
            .unwrap();
        let half = job(
            (0x0007_000f, 0x0003_0007),
            (0x11000, 0x11000),
            (0, 32),
            (0x1000, 16),
        );
        assert_eq!(resize_job(&mut e, &mut h, &half), None);
        for y in 0..4 {
            let line = &h.ram[0x1000 + y * 16..][..16];
            let luma: Vec<u8> = (0..8).map(|x| line[x / 2 * 4 + x % 2]).collect();
            let expected: Vec<u8> = (0..8).map(|x| (2 * x + 6 * y) as u8).collect();
            assert_eq!(luma, expected, "line {y}");
        }
        // Acknowledge, then copy 1:1 to +24 bytes in a 96-byte pitch.
        let mut cx = Cx::new(&mut h, 0, 0, 0);
        e.write(&mut cx, RESZ + 0x10c, 4, 1);
        let copy = job(
            (0x0007_000f, 0x0007_000f),
            (0x1000, 0x1000),
            (0, 32),
            (0x2018, 96),
        );
        assert_eq!(resize_job(&mut e, &mut h, &copy), None);
        let source = picture(sw, sh, spitch);
        for y in 0..sh {
            assert_eq!(h.ram[0x2018 + y * 96..][..32], source[y * spitch..][..32]);
        }
        assert!(h.ram[0x2000..0x2018].iter().all(|&b| b == 0));
    }

    #[test]
    fn only_a_computing_resizer_writes_its_output() {
        let words = job(
            (0x0001_0003, 0x0001_0003),
            (0x1000, 0x1000),
            (0, 8),
            (0x100, 8),
        );
        let mut h = RecordingHost {
            ram: vec![0x55; 0x200],
            ..Default::default()
        };
        let mut e = Engine::new(Kind::Resz, RESZ, RESZ_IRQ, 20).unwrap();
        assert_eq!(resize_job(&mut e, &mut h, &words), None);
        assert!(h.ram[0x100..0x110].iter().all(|&b| b == 0x55));
        for kind in [Kind::Iifp, Kind::Reszl, Kind::Mix, Kind::Retch] {
            let error = Engine::new(kind, RESZ, RESZ_IRQ, 20)
                .unwrap()
                .computing()
                .err();
            assert!(
                error.is_some_and(|e| e.contains("no computed output")),
                "{kind:?}"
            );
        }
    }

    /// Paired-row formats (+0x08 bits 1..0 or 5..4), odd widths, a pitch
    /// shorter than a line and a zero step refuse when the job ends.
    #[test]
    fn a_job_outside_the_modeled_layout_refuses() {
        let base = job(
            (0x0001_0003, 0x0001_0003),
            (0x1000, 0x1000),
            (0, 8),
            (0x100, 8),
        );
        let with = |o: u64, v: u32| {
            let mut words = base.clone();
            words.retain(|&(w, _)| w != o);
            words.push((o, v));
            words
        };
        for (words, expected) in [
            (with(0x008, 0x133), "not packed"),
            (with(0x008, 0x103), "not packed"),
            (with(0x00c, 0x0001_0002), "3x2"),
            (with(0x0d0, 6), "pitch 6"),
            (with(0x01c, 0x1_0000), "no step"),
        ] {
            let mut h = RecordingHost {
                ram: vec![0; 0x200],
                ..Default::default()
            };
            let mut e = Engine::new(Kind::Resz, RESZ, RESZ_IRQ, 20)
                .unwrap()
                .computing()
                .unwrap();
            let refusal = resize_job(&mut e, &mut h, &words);
            assert!(
                refusal.as_deref().is_some_and(|r| r.contains(expected)),
                "{expected}: {refusal:?}"
            );
        }
    }

    const RECC: u64 = 0xfff82000;
    const RECC_IRQ: u32 = 129;
    /// The declared RAW: 16 lines of 32 samples, first sample at site (5, 1).
    const RAW: Raw = Raw {
        address: 0,
        pitch: 64,
        site: (5, 1),
    };

    fn raw_samples() -> Vec<u8> {
        (0..16 * 32)
            .flat_map(|i: u32| {
                (((i.wrapping_mul(2654435761) >> 18) % 12000 + 1024) as u16).to_le_bytes()
            })
            .collect()
    }
    fn developing_recc() -> Engine {
        Engine::new(Kind::Recc, RECC, RECC_IRQ, 20)
            .unwrap()
            .developing(RAW)
            .unwrap()
    }
    /// Programs one RECC pass, starts it and lets it finish; returns the
    /// engine's refusal, if any.
    fn recc_pass(e: &mut Engine, h: &mut RecordingHost, words: &[(u64, u32)]) -> Option<String> {
        for &(o, v) in words.iter().chain(&[(0x04, 1)]) {
            let mut cx = Cx::new(h, 0, 0, 0);
            e.write(&mut cx, RECC + o, 4, v.into());
            assert!(cx.refusal().is_none(), "+{o:#x} {v:#x}");
        }
        h.now += 20;
        let mut cx = Cx::new(h, 0, 0, 0);
        e.advance(&mut cx);
        cx.take_refusal()
    }
    /// A band pass: 6 lines of 12 samples from RAW + 1 line + 2 samples,
    /// starting at band line `line`, into the picture 2 lines and 4 pixels
    /// into the buffer at 0x1000 (pitch 48).
    fn band(line: u32) -> Vec<(u64, u32)> {
        vec![
            (0x08, 0x0062_0000),
            (0x0c, 64 + 4),
            (0x14, 64),
            (0x18, 6 << 16 | 12),
            (0x1c, line << 16),
            (0x54, line << 16),
            (0x70, 0x1000 + 2 * 48 + 8),
            (0x80, 48),
        ]
    }

    /// The still's three passes: two bands developed from their X-Trans
    /// sites, then the copy of the picture to the start of the buffer.
    #[test]
    fn a_developing_recc_writes_its_bands_then_copies_the_picture_into_place() {
        let mut h = RecordingHost {
            ram: vec![0; 0x2000],
            ..Default::default()
        };
        h.ram[..16 * 64].copy_from_slice(&raw_samples());
        let mut e = developing_recc();
        assert_eq!(recc_pass(&mut e, &mut h, &band(0)), None);
        assert_eq!(
            h.irqs[h.irqs.len() - 2..],
            [(RECC_IRQ, true), (RECC_IRQ, false)]
        );
        assert_eq!(recc_pass(&mut e, &mut h, &band(6)), None);
        // Sites: line (5 + 1) % 6 = 0, sample (1 + 2) % 6 = 3, and the second
        // band 6 lines on, the same site. Each band develops on its own.
        let raw = raw_samples();
        let mut expected = develop(&raw[64 + 4..], 64, 6, 12, (0, 3), None).unwrap();
        expected.extend(develop(&raw[7 * 64 + 4..], 64, 6, 12, (0, 3), None).unwrap());
        for (y, row) in expected.iter().enumerate() {
            let at = 0x1000 + (2 + y) * 48 + 8;
            assert_eq!(h.ram[at..at + 24], row[..], "band line {y}");
        }
        let copy = [
            (0x08, 2),
            (0x1c, 0),
            (0x54, 0),
            (0x3c, 0x1000 + 2 * 48 + 8),
            (0x4c, 48),
            (0x50, 12 << 16 | 12),
            (0x70, 0x1000),
        ];
        assert_eq!(recc_pass(&mut e, &mut h, &copy), None);
        for (y, row) in expected.iter().enumerate() {
            assert_eq!(h.ram[0x1000 + y * 48..][..24], row[..], "picture line {y}");
        }
    }

    #[test]
    fn only_a_developing_recc_writes_and_only_from_the_declared_raw() {
        let mut h = RecordingHost {
            ram: vec![0x55; 0x2000],
            ..Default::default()
        };
        let mut e = Engine::new(Kind::Recc, RECC, RECC_IRQ, 20).unwrap();
        assert_eq!(recc_pass(&mut e, &mut h, &band(0)), None);
        assert!(h.ram[0x1000..].iter().all(|&b| b == 0x55));
        let error = Engine::new(Kind::Recc, RECC, RECC_IRQ, 20)
            .unwrap()
            .computing()
            .err();
        assert!(error.is_some_and(|e| e.contains("declared RAW")));
        for kind in [Kind::Iifp, Kind::Resz, Kind::Reszl, Kind::Mix, Kind::Retch] {
            let error = Engine::new(kind, RECC, RECC_IRQ, 20)
                .unwrap()
                .developing(RAW)
                .err();
            assert!(
                error.is_some_and(|e| e.contains("does not develop")),
                "{kind:?}"
            );
        }
        for raw in [
            Raw {
                site: (6, 0),
                ..RAW
            },
            Raw {
                site: (0, 6),
                ..RAW
            },
            Raw { pitch: 63, ..RAW },
        ] {
            let error = Engine::new(Kind::Recc, RECC, RECC_IRQ, 20)
                .unwrap()
                .developing(raw)
                .err();
            assert!(error.is_some_and(|e| e.contains("Invalid")), "{raw:?}");
        }
    }

    /// The 4K movie mode, a column start, a source outside the RAW or at
    /// another pitch, and a band too small to develop refuse when the pass
    /// ends.
    #[test]
    fn a_pass_outside_the_modeled_modes_refuses() {
        let with = |o: u64, v: u32| {
            let mut words = band(0);
            words.retain(|&(w, _)| w != o);
            words.push((o, v));
            words
        };
        for (words, expected) in [
            (with(0x08, 0x0042_2000), "mode 0x422000"),
            (with(0x1c, 4), "column"),
            (with(0x14, 66), "not in the declared RAW"),
            (with(0x18, 4 << 16 | 12), "Unsupported"),
        ] {
            let mut h = RecordingHost {
                ram: vec![0; 0x2000],
                ..Default::default()
            };
            let mut e = developing_recc();
            let refusal = recc_pass(&mut e, &mut h, &words);
            assert!(
                refusal.as_deref().is_some_and(|r| r.contains(expected)),
                "{expected}: {refusal:?}"
            );
        }
        let mut h = RecordingHost {
            ram: vec![0; 0x2000],
            ..Default::default()
        };
        let mut e = Engine::new(Kind::Recc, RECC, RECC_IRQ, 20)
            .unwrap()
            .developing(Raw {
                address: 0x100,
                ..RAW
            })
            .unwrap();
        let refusal = recc_pass(&mut e, &mut h, &band(0));
        assert!(refusal.is_some_and(|r| r.contains("not in the declared RAW")));
    }
}
