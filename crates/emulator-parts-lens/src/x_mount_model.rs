//! A powered X-mount lens with focus and aperture motion, for bodies that drive
//! the full lens-control command set (written for the X-T5 4.31).
//!
//! Framing is the X-mount link of [`super::x_mount_link`]: 4-byte header and
//! acknowledgment frames with the same 5-bit check, replies queued for the
//! next exchange, and a 128-byte bulk read after the lens sends its
//! family-0x14 (identification) or family-0x96 (extended record) reply. Unlike that bounded part, this model answers
//! every request family. Focus (0x54..0x56, 0x58) and aperture (0x60..0x62)
//! requests start motion that advances with board time; 0x21 reports the focus
//! position. Other families answer a declared or default payload, and the first
//! request of a family outside the known X-T5 set is logged once. Malformed or
//! out-of-phase frames are logged and resynchronized instead of refused,
//! because the body's exact sequence is not established yet.
//!
//! Power: both supply enables must be driven high. The ready output is high
//! while off and starting, low for a declared busy period after startup, then
//! released high. Each motion request holds a released ready output low until
//! the motion ends (at least `min_operation_ticks`), so the body sees a rising
//! edge when the lens has settled.
//!
//! The identification record, the status words and every timing are invented
//! scenario inputs. None is copied from lens or camera firmware.
use super::x_mount_link::{FixedReply, Packet};
use fujimod_emulator_devices::checkpoint::Checkpoint;
use fujimod_emulator_parts_io::serial::{Frame, Wire};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Arc, Mutex};

pub mod family {
    pub const SYNC: u16 = 0xff;
    pub const PARAMETERS: u16 = 0x18;
    pub const IDENTIFICATION: u16 = 0x14;
    /// Second (extended) identification record, also a 128-byte bulk read.
    pub const EXTENDED_IDENTIFICATION: u16 = 0x96;
    pub const STATUS: u16 = 0x20;
    pub const FEEDBACK: u16 = 0x21;
    /// Aperture state (X-T5 4.31 API 0x13e51cc, message 0x18e8; decoder 0x13ea1f8: bit 15 a
    /// flag, 14-bit value). 0x1536b64 copies it to 0x256e7b8 when the flag is clear and 0x156fd08
    /// takes the high byte as the lens's aperture code (to 0x221b994) and the low byte as
    /// 1/256 of a code.
    pub const APERTURE_STATE: u16 = 0x22;
    /// Focus state (message 0x18e9; decoder 0x13ea224: a flag and a signed 15-bit value,
    /// stored at 0x24abfe0). Its consumer is not traced; answered with the focus position.
    pub const FOCUS_STATE: u16 = 0x23;
    /// Zoom state (X-T5 decoder 0x13ea25c: flag bit 15, a 7-bit value in bits
    /// 8..14 and a byte in bits 0..7). The 7-bit value is taken as the zoom step
    /// index into the lens data tables (at most 100 steps); inferred, not traced
    /// to its consumer.
    pub const ZOOM_STATE: u16 = 0x24;
    pub const DETAIL_STATUS: u16 = 0x26;
    pub const APERTURE_RING: u16 = 0x30;
    pub const FOCUS_RING: u16 = 0x32;
    /// Focus drive request (X-T5 4.31 lenscom 0x13e5d4c, message 0x18f8): 0x54 carries
    /// request +0x0, 0x55 the flag bytes +0x2..+0x8, 0x56 request +0xa. The AF drive
    /// 0x1572620 -> 0x156fb08 -> 0x1535814 puts the speed (0x156f91c: a shifted rate that
    /// saturates to 0x7fff) at +0x0 and the absolute target (0x156fc24: the body's lens
    /// position [0x221b98c] plus the amount, saturated to 16 bits) at +0xa. 0x1535760 sends
    /// 0x7fff at +0x0 with a signed target at +0xa. So 0x54 is the speed and 0x56 the
    /// signed absolute target; the three families arrive in that order.
    pub const FOCUS_SPEED: u16 = 0x54;
    pub const FOCUS_FLAGS: u16 = 0x55;
    pub const FOCUS_TARGET: u16 = 0x56;
    pub const FOCUS_DRIVE: u16 = 0x58;
    pub const APERTURE_TARGET: u16 = 0x60;
    pub const APERTURE_STEP: u16 = 0x61;
    /// Aperture setpoint (wrapper 0x15359e8 -> lenscom 0x13e61b0, message 0x18fd): request +6,
    /// 12 bits, is the aperture code times 64 (0x156f7ec: (code & 0x3ff) << 6), plus four flag
    /// bits that the model ignores. 0x61 (request +0) is 0 on every X-T5 call site.
    pub const APERTURE_DRIVE: u16 = 0x62;
    pub const READ_CATALOG: u16 = 0xc0;
    /// Two-bit lens state request (X-T5 4.31 API 0x13e5ac0, message 0x18f3; the reply payload
    /// is not decoded, handler 0x13ea0bc). PLAY sends payload 1 from the lens control layer
    /// (0x156f5d8 -> 0x15356d0). Without a ready-line busy period after it the body never
    /// leaves REC (LCD black); with one it switches the lens supplies off and shows playback
    /// (scratch/regress runs a-cur and a-play). Which body wait needs the edge is inferred.
    pub const STATE_REQUEST: u16 = 0x40;
}
use family::*;

/// Families the X-T5 sends whose meaning is not modeled; they answer zero
/// unless the scenario declares a payload. No log line.
const KNOWN_READS: &[u16] = &[
    0x10, 0x11, 0x12, 0x13, 0x15, 0x16, 0x19, 0x2b, 0x31, 0x33, 0x3e, 0x3f, 0xc8, 0xc9,
    0x11a,
];
/// Families the X-T5 sends as settings (OIS, transitions/focus mode); they
/// echo the request payload and store it.
const KNOWN_WRITES: &[u16] = &[
    0x80, 0x81, 0x82, 0xa0, 0xa1, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7, 0xa8, 0xa9, 0xaa,
];
const IDENTIFICATION_BYTES: usize = 128;
const REPLY_LIMIT: usize = 8;
/// The byte a frame receives when the lens does not drive its output (off,
/// starting, or power lost during the frame): the receive line idles high.
pub const UNDRIVEN_BYTE: u16 = 0xff;

/// One motor axis in lens units.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Axis {
    pub min: u16,
    pub max: u16,
    pub initial: u16,
    /// Default speed in units per millisecond; zero moves instantly.
    pub units_per_ms: u16,
}
impl Axis {
    fn validate(self) -> Result<(), String> {
        if self.min > self.max || !(self.min..=self.max).contains(&self.initial) {
            return Err("X-mount lens axis needs min <= initial <= max".into());
        }
        Ok(())
    }
    fn clamp(self, value: i64) -> u16 {
        value.clamp(self.min as i64, self.max as i64) as u16
    }
}

/// The lens scenario: identity, constant status words and motion ranges.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    pub name: String,
    /// The 128-byte identification record sent by the family-0x14 bulk read.
    pub identification: Vec<u8>,
    /// The 128-byte extended record sent by the family-0x96 bulk read.
    pub extended: Vec<u8>,
    pub status: u16,
    pub detail_status: u16,
    pub aperture_ring: u16,
    pub focus_ring: u16,
    pub focus: Axis,
    pub aperture: Axis,
    /// Declared payloads for families this model does not drive itself.
    #[serde(default)]
    pub replies: Vec<FixedReply>,
    /// Data catalogs served by family 0xc0 and its bulk read. Undeclared
    /// selectors serve one zero block.
    #[serde(default)]
    pub catalogs: Vec<Catalog>,
}

/// A family-0xc0 catalog of 2048-byte blocks. The content is generated: zero
/// bytes, the declared sections, and the 0x55 0xaa marker at +0x47 of
/// `marker_block`.
///
/// X-T5 4.31 (0x13ed614): the 0xc0 reply word is the block count. For selector 6
/// the body treats the last four blocks as one section and requires the marker
/// 0xaa55 (little-endian at +0x47) in the first of them (index count - 4),
/// else the read fails and the lens is shut down (0x13f1754 -> 0x13f0084(6)).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    pub selector: u16,
    pub blocks: u16,
    pub marker_block: Option<u16>,
    /// Non-zero content, each at a byte offset in the whole catalog
    /// (block * 2048 + offset in the block). Later sections win where they overlap.
    #[serde(default)]
    pub sections: Vec<Section>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Section {
    pub offset: u32,
    pub bytes: Vec<u8>,
}
impl Catalog {
    /// Data byte `offset` of block `block`, without the marker.
    fn byte(&self, block: u16, offset: usize) -> u8 {
        let at = block as usize * CATALOG_BLOCK_BYTES + offset;
        self.sections
            .iter()
            .rev()
            .find_map(|s| at.checked_sub(s.offset as usize).and_then(|i| s.bytes.get(i)).copied())
            .unwrap_or(0)
    }
}
pub const CATALOG_BLOCK_BYTES: usize = 2048;

/// Identification record. Content is invented; the fields the X-T5 4.31
/// lens manager checks (startup states 0x15 and 0x18, 0x13f2ef8 and
/// 0x13f3034) are set to values those checks accept:
/// - +0x08..+0x0f is not all 0xff (registered lens), and is an ASCII class
///   code with +0x09 = 'L', +0x0a in {P,S,Z}, +0x0b in {G,N,S},
///   +0x0c in {A,F,W}; any other code ends in the lens-error result 3;
/// - +0x62/+0x63 is a protocol version listed in the body's table 0x1971540
///   (1.00 here, the lowest entry, so the body never treats the lens as newer);
/// - +0x65 bit 1 announces the extended record (family 0x96); without it the
///   body skips that read and the class check above fails the startup;
/// - +0x64 bits 4..5 = 0 and the other flag bits clear (meanings unknown).
/// Everything else (code at +0x00, focal and F-number words, the name) is a
/// scenario layout the body is not known to read.
pub struct Identity<'a> {
    pub model: &'a str,
    /// Eight-byte ASCII model code at +0x00.
    pub code: [u8; 8],
    /// Eight-byte ASCII class code at +0x08 (see the checks above).
    pub class: [u8; 8],
    /// Focal lengths in 0.1 mm.
    pub focal: (u16, u16),
    /// Widest F-number at the short and long ends, and the smallest aperture, in 0.1 steps.
    pub f_numbers: (u16, u16, u16),
    pub ois: bool,
    pub aperture_ring: bool,
    pub focus: Axis,
    pub aperture: Axis,
    /// Closest focus distance in centimetres.
    pub closest_cm: u16,
    /// Ten-character lens serial number (record +0x50, data block +0x14).
    pub serial: [u8; 10],
    /// Zoom steps in the lens data (1 for a prime).
    pub zoom_steps: u8,
}

/// Nominal F-numbers x 100 in 1/3 stops from F1 (index = 1/3-stop count).
const NOMINAL_F: [u16; 31] = [
    100, 110, 120, 140, 160, 180, 200, 220, 250, 280, 320, 360, 400, 450, 500, 560, 640, 710,
    800, 900, 1000, 1100, 1300, 1400, 1600, 1800, 2000, 2200, 2500, 2900, 3200,
];
/// The 1/3-stop index of an F-number given in 0.1 steps (nearest nominal value).
fn thirds(f_tenths: u16) -> u8 {
    let f = f_tenths as i32 * 10;
    (0..NOMINAL_F.len())
        .min_by_key(|&i| (NOMINAL_F[i] as i32 - f).abs())
        .unwrap() as u8
}
impl Identity<'_> {
    pub fn record(&self) -> Vec<u8> {
        let mut r = vec![0u8; IDENTIFICATION_BYTES];
        let mut put = |offset: usize, value: u16| {
            r[offset..offset + 2].copy_from_slice(&value.to_be_bytes());
        };
        // Invented scenario words; the body is not known to read them.
        put(0x68, self.focal.0);
        put(0x6a, self.focal.1);
        put(0x6c, self.f_numbers.0);
        put(0x6e, self.f_numbers.1);
        put(0x70, self.f_numbers.2);
        put(0x72, self.focus.max);
        put(0x74, self.aperture.max);
        put(0x76, self.closest_cm);
        r[0x00..0x08].copy_from_slice(&self.code);
        r[0x08..0x10].copy_from_slice(&self.class);
        // The body copies +0x10..+0x17 and the serial at +0x50..+0x59 into its lens info
        // (0x24ae428, 0x24ae468) and compares them with the lens data block (0x1536870).
        r[0x10..0x18].copy_from_slice(&self.class);
        r[0x50..0x5a].copy_from_slice(&self.serial);
        r[0x48] = u8::from(self.ois) | (u8::from(self.aperture_ring) << 1);
        r[0x62] = 1; // protocol version 1.00 (table 0x1971540 row 0)
        r[0x63] = 0;
        r[0x64] = 0;
        r[0x65] = 0x02; // extended record present
        let name = self.model.as_bytes();
        let n = name.len().min(32);
        r[0x20..0x20 + n].copy_from_slice(&name[..n]);
        r[0x40..0x48].copy_from_slice(b"FUJIFILM");
        // Final byte makes the record's byte sum zero.
        let sum = r[..IDENTIFICATION_BYTES - 1]
            .iter()
            .fold(0u8, |s, b| s.wrapping_add(*b));
        r[IDENTIFICATION_BYTES - 1] = sum.wrapping_neg();
        r
    }
    /// Zoom step `z` of `zoom_steps`: focal length in 0.1 mm (geometric
    /// interpolation) and the widest aperture as a 1/3-stop index from F1.
    fn zoom_step(&self, z: usize) -> (u16, u8) {
        let last = (self.zoom_steps.max(1) - 1) as f64;
        let t = if last == 0. { 0. } else { z as f64 / last };
        let (short, long) = (self.focal.0 as f64, self.focal.1 as f64);
        let focal = (short * (long / short).powf(t)).round() as u16;
        let (open_short, open_long) = (thirds(self.f_numbers.0), thirds(self.f_numbers.1));
        let open = (open_short as f64 + (open_long as f64 - open_short as f64) * t).round() as u8;
        (focal, open)
    }
    /// Catalog 6, the lens data the X-T5 4.31 body stores in its configuration
    /// memory. Content is generated from this identity; the layout is what the
    /// body reads (static, 4.31 image):
    ///
    /// The read (0x13ed614, selector 6, count T from the 0xc0 reply) stores blocks
    /// 0..T-5 at configuration offset 0x722000 + 0x800 * block and the last four
    /// blocks at 0x720000. The body's lens-data window is 0x720000..0x735cff
    /// (0x1494e70), so T - 4 is at most 39; this catalog uses all 39.
    ///
    /// Last four blocks (offset 0x720000):
    /// - +0x00..+0x07 and +0x14..+0x1d: compared with record +0x10 and +0x50
    ///   (0x1536870); +0x14..+0x1d is also the Exif LensSerialNumber (0x2d961c8);
    /// - +0x09: lens kind; 0 is the normal case (1 disables the aperture tables
    ///   in 0x1538958, 0x1538014 and others);
    /// - +0x38..+0x43: Exif LensMake (copied by 0x2d96138);
    /// - +0x47/+0x48: the marker (served by the marker rule);
    /// - +0x4a..+0x89: Exif LensModel (0x2d96180);
    /// - +0x317 + 2z (s16): focus position at infinity for zoom step z, and u16
    ///   offsets from it at +0x3e0, +0x4a9, +0x572, +0x63b, +0x704, +0x7cd (+2z),
    ///   the positions of nearer subject distances (0x1538f7c, 0x1539228;
    ///   0x15397cc compares a position with them). The distances are not known;
    ///   this catalog spreads the offsets from infinity to the closest focus.
    ///   +0x63b and +0x704 apply only to zoom steps below the byte at +0x1213
    ///   (else +0x572 or infinity is used), so +0x1213 is set to M;
    /// - +0xf80: zoom step count M (1..100, 0x1539d08), +0xf82: aperture code
    ///   count N (1..31, 0x1537c68).
    ///
    /// Blocks from offset 0x722000:
    /// - 0x722134 + 2z and 0x7221fd + 2z (u16): focal length of zoom step z in
    ///   0.1 mm (0x1538cec, 0x1538d40; the Exif FocalLength, LensSpecification
    ///   and FocalLengthIn35mmFormat use the first);
    /// - 0x7222c8 + z and 0x72232d + z (u8): the widest and the smallest
    ///   aperture code at zoom step z (0x15383fc, 0x15384ac);
    /// - 0x734600 + 31z + c (u8): index k of aperture code c at zoom step z into
    ///   the F-number table, 0xff for none (0x1537e4c);
    /// - 0x722530 + 2k (u16): F-number x 100 (0x1537e4c; the OSD, the Exif
    ///   FNumber and LensSpecification use it);
    /// - 0x732400 + 2(31z + c) (s16): aperture value x 1000 (APEX Av, 0x1537f68).
    ///
    /// Aperture code c is the 1/3-stop index from the lens's widest aperture at
    /// the short end. Everything else in the catalog is zero; the body reads more
    /// fields (correction and autofocus tables) whose meaning is not known.
    pub fn lens_data(&self) -> Catalog {
        const LENS_BLOCKS: u16 = 39;
        let at = |config: u32| -> u32 {
            match config {
                0x722000.. => config - 0x722000,
                _ => LENS_BLOCKS as u32 * CATALOG_BLOCK_BYTES as u32 + (config - 0x720000),
            }
        };
        let mut sections = vec![];
        let mut put = |config: u32, bytes: Vec<u8>| sections.push(Section { offset: at(config), bytes });
        let le = |values: &[u16]| values.iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<u8>>();
        // Identity, maker and model.
        put(0x720000, self.class.to_vec());
        put(0x720014, self.serial.to_vec());
        put(0x720038, b"FUJIFILM".to_vec());
        let name = self.model.as_bytes();
        put(0x72004a, name[..name.len().min(63)].to_vec());
        // Zoom steps and apertures.
        let steps = self.zoom_steps.clamp(1, 100) as usize;
        let open_short = thirds(self.f_numbers.0);
        let smallest = thirds(self.f_numbers.2);
        let count = (smallest - open_short + 1).min(31) as usize;
        put(0x720f80, vec![steps as u8]);
        put(0x720f82, vec![count as u8]);
        let zoom: Vec<(u16, u8)> = (0..steps).map(|z| self.zoom_step(z)).collect();
        let focal: Vec<u16> = zoom.iter().map(|(f, _)| *f).collect();
        put(0x722134, le(&focal));
        put(0x7221fd, le(&focal));
        put(0x7222c8, zoom.iter().map(|(_, open)| open - open_short).collect());
        put(0x72232d, vec![(count - 1) as u8; steps]);
        let f_numbers: Vec<u16> = (0..count).map(|c| NOMINAL_F[open_short as usize + c]).collect();
        put(0x722530, le(&f_numbers));
        let mut index = vec![0xffu8; 31 * steps];
        let mut av = vec![0u16; 31 * steps];
        for z in 0..steps {
            for c in 0..count {
                index[31 * z + c] = c as u8;
                av[31 * z + c] = (((open_short as u32 + c as u32) * 1000 + 1) / 3) as u16;
            }
        }
        put(0x734600, index);
        put(0x732400, le(&av));
        // Focus positions (static, X-T5 4.31): infinity is the s16 at +0x317 + 2z
        // (0x1538f7c) and nearer subjects lie above it; 0x1539228 adds the u16 offsets of
        // +0x3e0 .. +0x7cd to it. The contrast scan's far limit is infinity and its near
        // limit infinity + [+0x572] (0x153b6b8 -> 0x1539a30, AF worker 0x2fbdb20); the range
        // table 0x1ae9678 also uses +0x63b as the near end of an extended range and the
        // margins below (0x153e704). Infinity sits near the low end of the travel and the
        // ordinary range reaches the closest focus near the high end. No public lens data
        // gives these values; they are generated from the travel.
        let travel = (self.focus.max - self.focus.min) as u32;
        let infinity = self.focus.min + (travel / 20) as u16;
        put(0x720317, le(&vec![infinity; steps]));
        let span = travel * 9 / 10;
        // Offsets from infinity: 1/8, 1/2, 17/18, then the whole span for the extended and
        // nearest tables (each table at least as far as the one before).
        let offsets = [span / 8, span / 2, span * 17 / 18, span, span, span];
        for (table, offset) in [0x7203e0u32, 0x7204a9, 0x720572, 0x72063b, 0x720704, 0x7207cd].iter().zip(offsets) {
            put(*table, le(&vec![offset as u16; steps]));
        }
        // Three more offset tables (0x1539ac8; the AF worker clamps the fourth scan limit
        // with the first): the ordinary near end.
        for table in [0x7207f7u32, 0x7208c0, 0x720989] {
            put(table, le(&vec![offsets[2] as u16; steps]));
        }
        // Scan margins beyond the limits (0x153e704: pairs 0x720cd2/0x720d9b and
        // 0x720b40/0x720c09), 1/80 of the travel, so infinity - margin stays in the travel.
        for table in [0x720cd2u32, 0x720d9b, 0x720b40, 0x720c09] {
            put(table, le(&vec![(travel / 80) as u16; steps]));
        }
        // Drive speeds: 0x1539fb4 reads a u16 from one of fourteen tables (0x1aeb5c0) and
        // shifts it by the byte at +0x162f (0 here); zero means "no limit" (0xffff, then
        // 0x7fff). 0x1539c98 / 0x1539cd0 read the limits at +0x15a6 / +0x15a8. The body
        // sends the result as family 0x54, which the model takes as units per second: scans
        // at 1.5 travels per second, limits at the lens's own speed.
        let scan = (travel * 3 / 2).min(0x7ffe) as u16;
        for table in [0x721674u32, 0x7216c4, 0x721714, 0x721764, 0x7217b4, 0x721804, 0x721854, 0x7218a4,
                      0x7218f4, 0x721944, 0x721994, 0x7219e4, 0x721a34, 0x721a84] {
            put(table, le(&vec![scan; steps]));
        }
        let fastest = (self.focus.units_per_ms as u32 * 1000).min(0x7ffe) as u16;
        put(0x7215a6, le(&[fastest, fastest]));
        // Zoom steps below which the nearest-distance tables (+0x63b, +0x704) apply
        // (0x1539228, 0x153ea04; 0x1565360 reads it as a step count): all of them.
        put(0x721213, vec![steps as u8]);
        Catalog { selector: 6, blocks: LENS_BLOCKS + 4, marker_block: Some(LENS_BLOCKS), sections }
    }
    /// Invented extended record: layout version 1, zero fill, zero byte sum.
    pub fn extended_record(&self) -> Vec<u8> {
        let mut r = vec![0u8; IDENTIFICATION_BYTES];
        r[0] = 1;
        r[IDENTIFICATION_BYTES - 1] = 0xff;
        r
    }
}

impl Spec {
    /// Named scenario lenses. Values are plausible public specifications;
    /// the identification layout and type ids are invented.
    pub fn preset(name: &str) -> Option<Self> {
        let (model, code, class, focal, f_numbers, ois, ring, focus, aperture, closest, serial, zoom_steps) =
            match name {
                "xf18-55" => (
                    "XF18-55mmF2.8-4 R LM OIS",
                    *b"EMX1855\0",
                    *b"XLZSA001",
                    (180, 550),
                    (28, 40, 220),
                    true,
                    true,
                    Axis { min: 0, max: 4000, initial: 2000, units_per_ms: 40 },
                    // Index 0 is wide open; 1/3-stop steps from F2.8 to F22.
                    Axis { min: 0, max: 18, initial: 0, units_per_ms: 2 },
                    30,
                    *b"EMU1855001",
                    8,
                ),
                "prime" => (
                    "XF35mmF2 R WR",
                    *b"EMX3520\0",
                    *b"XLPSA001",
                    (350, 350),
                    (20, 20, 160),
                    false,
                    true,
                    Axis { min: 0, max: 2000, initial: 1000, units_per_ms: 50 },
                    // F2 to F16 in 1/3 stops.
                    Axis { min: 0, max: 18, initial: 0, units_per_ms: 2 },
                    35,
                    *b"EMU3520001",
                    1,
                ),
                // Public specification of the XF23mmF2.8 R WR (23 mm, F2.8..F16).
                "xf23" => (
                    "XF23mmF2.8 R WR",
                    *b"EMX2328\0",
                    *b"XLPSA001",
                    (230, 230),
                    (28, 28, 160),
                    false,
                    true,
                    Axis { min: 0, max: 2000, initial: 1000, units_per_ms: 50 },
                    // F2.8 to F16 in 1/3 stops.
                    Axis { min: 0, max: 15, initial: 0, units_per_ms: 2 },
                    20,
                    *b"EMU2328001",
                    1,
                ),
                _ => return None,
            };
        let identity = Identity {
            model,
            code,
            class,
            focal,
            f_numbers,
            ois,
            aperture_ring: ring,
            focus,
            aperture,
            closest_cm: closest,
            serial,
            zoom_steps,
        };
        Some(Self {
            name: model.into(),
            identification: identity.record(),
            extended: identity.extended_record(),
            // EXR II steady status and detail words (x_mount_link tests); their
            // meaning for the X-T5 is not established.
            status: 0x04c0,
            detail_status: 0x0014,
            aperture_ring: 0,
            focus_ring: 0xffff,
            focus,
            aperture,
            replies: vec![],
            catalogs: vec![identity.lens_data()],
        })
    }
    fn validate(&self) -> Result<(), String> {
        if self.identification.len() != IDENTIFICATION_BYTES
            || self.extended.len() != IDENTIFICATION_BYTES
        {
            return Err("X-mount identification records must contain 128 bytes".into());
        }
        self.focus.validate()?;
        self.aperture.validate()?;
        for (i, c) in self.catalogs.iter().enumerate() {
            if c.selector > 15
                || c.blocks == 0
                || c.blocks >= BULK_FINISH
                || c.marker_block.is_some_and(|m| m >= c.blocks)
                || c.sections.iter().any(|s| {
                    s.offset as usize + s.bytes.len() > c.blocks as usize * CATALOG_BLOCK_BYTES
                })
                || self.catalogs[..i].iter().any(|o| o.selector == c.selector)
            {
                return Err("invalid X-mount lens catalog".into());
            }
        }
        for (i, reply) in self.replies.iter().enumerate() {
            if reply.family == 0
                || reply.family > 0x1ff
                || Self::driven(reply.family)
                || self.replies[..i].iter().any(|r| r.family == reply.family)
            {
                return Err(format!(
                    "invalid X-mount declared reply family {:#x}",
                    reply.family
                ));
            }
        }
        Ok(())
    }
    /// Families whose answers come from lens state, never from a declared payload.
    fn driven(family: u16) -> bool {
        matches!(
            family,
            SYNC | IDENTIFICATION
                | EXTENDED_IDENTIFICATION
                | FEEDBACK
                | FOCUS_STATE
                | APERTURE_STATE
                | FOCUS_SPEED
                | FOCUS_FLAGS
                | FOCUS_TARGET
                | FOCUS_DRIVE
                | APERTURE_TARGET
                | APERTURE_STEP
                | APERTURE_DRIVE
        )
    }
    fn catalog(&self, selector: u16) -> Cow<'_, Catalog> {
        match self.catalogs.iter().find(|c| c.selector == selector) {
            Some(c) => Cow::Borrowed(c),
            None => Cow::Owned(Catalog { selector, blocks: 1, marker_block: None, sections: vec![] }),
        }
    }
    fn declared(&self, family: u16) -> Option<u16> {
        self.replies
            .iter()
            .find(|r| r.family == family)
            .map(|r| r.payload)
    }
}

/// Declared durations in board ticks; none is measured lens timing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Timing {
    pub ticks_per_ms: u64,
    /// From both supplies high until the lens pulls ready low and serves frames.
    pub startup_ticks: u64,
    /// From then until the lens releases ready high.
    pub busy_ticks: u64,
    /// Shortest ready-low pulse for a motion request.
    pub min_operation_ticks: u64,
}
impl Timing {
    fn validate(self) -> Result<(), String> {
        if self.ticks_per_ms == 0
            || self.startup_ticks == 0
            || self.busy_ticks == 0
            || self.min_operation_ticks == 0
        {
            return Err("X-mount lens timing values must be positive".into());
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Motion {
    from: u16,
    target: u16,
    start: u64,
    units_per_ms: u16,
}
impl Motion {
    fn at_rest(position: u16) -> Self {
        Self { from: position, target: position, start: 0, units_per_ms: 0 }
    }
    fn distance(self) -> u64 {
        self.from.abs_diff(self.target) as u64
    }
    fn end(self, ticks_per_ms: u64) -> u64 {
        if self.units_per_ms == 0 {
            return self.start;
        }
        let ticks = (self.distance() as u128 * ticks_per_ms as u128)
            .div_ceil(self.units_per_ms as u128);
        self.start.saturating_add(u64::try_from(ticks).unwrap_or(u64::MAX))
    }
    fn position(self, now: u64, ticks_per_ms: u64) -> u16 {
        if self.units_per_ms == 0 || now >= self.end(ticks_per_ms) {
            return self.target;
        }
        let moved = (now.saturating_sub(self.start) as u128 * self.units_per_ms as u128
            / ticks_per_ms as u128) as u64;
        let step = moved.min(self.distance()) as u16;
        if self.target >= self.from {
            self.from + step
        } else {
            self.from - step
        }
    }
    fn retarget(&mut self, now: u64, ticks_per_ms: u64, target: u16, units_per_ms: u16) {
        let from = self.position(now, ticks_per_ms);
        *self = Self { from, target, start: now, units_per_ms };
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
enum Power {
    Off,
    Starting { at: u64 },
    Busy { until: u64 },
    Released,
}
impl Power {
    fn serving(self) -> bool {
        matches!(self, Self::Busy { .. } | Self::Released)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Message {
    packet: u32,
    sequence: u8,
    /// A catalog reply names the selector whose blocks the bulk read serves.
    #[serde(default)]
    catalog: Option<u16>,
}
const IDLE: Message = Message { packet: 0, sequence: 0, catalog: None };
/// Bulk header the lens sends after the last block, and the body's "no further block" request.
const BULK_END: u16 = 0xffff;
/// Bulk header with which the body ends a bulk read.
const BULK_FINISH: u16 = 0xff00;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
enum Source {
    Identification,
    Extended,
    Catalog { selector: u16 },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
enum Phase {
    #[default]
    Header,
    Acknowledgment {
        received: u32,
        sent: Message,
    },
    /// Block transfers of `width + 3` bytes: 2-byte header, data, 8-bit sum.
    /// The lens sends block `next` (or the end header) while the body's header
    /// names the block it wants next; the body ends the read with 0xff00.
    Bulk {
        source: Source,
        next: u16,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Link {
    input: Vec<u8>,
    outgoing: Option<Message>,
    phase: Phase,
    replies: VecDeque<Message>,
    transmit_counter: u8,
    receive_counter: u8,
    frames: u64,
    /// Completed identification bulk reads since power-up.
    identification_reads: u64,
    /// Completed extended-record bulk reads since power-up.
    #[serde(default)]
    extended_reads: u64,
    /// Completed catalog bulk reads since power-up.
    #[serde(default)]
    catalog_reads: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    powered: bool,
    power: Power,
    epoch: u32,
    link: Link,
    focus: Motion,
    focus_speed: u16,
    aperture: Motion,
    /// A motion request holds ready low until this tick.
    operation_until: Option<u64>,
    /// Last payload of every setting family, for echo and inspection.
    settings: BTreeMap<u16, u16>,
    /// Families (and protocol events above 0xffff) already logged.
    logged: BTreeSet<u32>,
}

/// Event codes kept in `State::logged` next to family numbers.
const EVENT_BAD_FRAME: u32 = 0x1_0000;
const EVENT_OUT_OF_PHASE: u32 = 0x1_0001;
const EVENT_BULK_ABORT: u32 = 0x1_0002;
const EVENT_NOT_SERVING: u32 = 0x1_0003;
const EVENT_REPLY_OVERFLOW: u32 = 0x1_0004;
const EVENT_IDENTIFICATION: u32 = 0x1_0005;
const EVENT_FIRST_FOCUS: u32 = 0x1_0006;
const EVENT_FIRST_APERTURE: u32 = 0x1_0007;
/// First of `MOVES_LOGGED` codes for logged focus moves, and for aperture moves.
const EVENT_FOCUS_MOVES: u32 = 0x2_0000;
const EVENT_APERTURE_MOVES: u32 = 0x3_0000;
const MOVES_LOGGED: u32 = 64;
const EVENT_FRAME_SHAPE: u32 = 0x1_0008;
const EVENT_EXTENDED: u32 = 0x1_0009;
/// Plus the selector (0..15).
const EVENT_CATALOG: u32 = 0x1_0010;

struct Unit {
    spec: Spec,
    timing: Timing,
    state: State,
    /// Host log lines waiting for the board; never part of a checkpoint.
    logs: Vec<String>,
}

fn valid_word(word: u32) -> bool {
    let packet = Packet(word);
    word == 0
        || (word & 1 == 0 && Packet::encode(packet.payload(), packet.family(), packet.ack()) == packet)
}

impl Unit {
    fn new(spec: Spec, timing: Timing) -> Result<Self, String> {
        spec.validate()?;
        timing.validate()?;
        let state = Self::reset_state(&spec, 0, false, Power::Off, BTreeSet::new());
        Ok(Self { spec, timing, state, logs: vec![] })
    }
    fn reset_state(spec: &Spec, epoch: u32, powered: bool, power: Power, logged: BTreeSet<u32>) -> State {
        State {
            powered,
            power,
            epoch,
            link: Link::default(),
            focus: Motion::at_rest(spec.focus.initial),
            focus_speed: spec.focus.units_per_ms,
            aperture: Motion::at_rest(spec.aperture.initial),
            operation_until: None,
            settings: BTreeMap::new(),
            logged,
        }
    }
    fn log_once(&mut self, event: u32, line: impl FnOnce() -> String) {
        if self.state.logged.insert(event) {
            self.logs.push(format!("x-mount lens ({}): {}", self.spec.name, line()));
        }
    }
    fn ready_high(&self) -> bool {
        match self.state.power {
            Power::Off | Power::Starting { .. } => true,
            Power::Busy { .. } => false,
            Power::Released => self.state.operation_until.is_none(),
        }
    }
    fn next(&self, now: u64, powered: bool) -> Option<u64> {
        if powered != self.state.powered {
            return Some(now);
        }
        match self.state.power {
            Power::Starting { at } => Some(at),
            Power::Busy { until } => Some(until),
            Power::Released => self.state.operation_until,
            Power::Off => None,
        }
        .map(|at| at.max(now))
    }
    /// Apply a supply change at `now`, then every time-driven transition due by `now`.
    fn tick(&mut self, now: u64, powered: bool) {
        if powered != self.state.powered {
            if powered {
                let epoch = self.state.epoch.wrapping_add(1);
                let logged = std::mem::take(&mut self.state.logged);
                self.state = Self::reset_state(
                    &self.spec,
                    epoch,
                    true,
                    Power::Starting { at: now.saturating_add(self.timing.startup_ticks) },
                    logged,
                );
                self.logs.push(format!("x-mount lens ({}): supplies on", self.spec.name));
            } else {
                let epoch = self.state.epoch.wrapping_add(1);
                let logged = std::mem::take(&mut self.state.logged);
                self.state = Self::reset_state(&self.spec, epoch, false, Power::Off, logged);
                self.logs.push(format!("x-mount lens ({}): supplies off", self.spec.name));
            }
        }
        self.settle(now);
    }
    fn settle(&mut self, now: u64) {
        if let Power::Starting { at } = self.state.power
            && at <= now
        {
            self.state.power = Power::Busy { until: at.saturating_add(self.timing.busy_ticks) };
        }
        if let Power::Busy { until } = self.state.power
            && until <= now
        {
            self.state.power = Power::Released;
        }
        if self.state.operation_until.is_some_and(|until| until <= now) {
            self.state.operation_until = None;
        }
    }
    fn operation(&mut self, now: u64, motion: Motion) {
        let end = motion
            .end(self.timing.ticks_per_ms)
            .max(now.saturating_add(self.timing.min_operation_ticks));
        self.state.operation_until = Some(self.state.operation_until.map_or(end, |u| u.max(end)));
    }
    fn move_focus(&mut self, now: u64, target: i64) {
        let target = self.spec.focus.clamp(target);
        let tpm = self.timing.ticks_per_ms;
        let speed = self.state.focus_speed;
        let from = self.state.focus.position(now, tpm);
        self.state.focus.retarget(now, tpm, target, speed);
        let focus = self.state.focus;
        self.operation(now, focus);
        self.log_once(EVENT_FIRST_FOCUS, || format!("first focus move {from} -> {target}"));
        self.log_move(EVENT_FOCUS_MOVES, "focus", from, target, now);
    }
    /// Logs the first `MOVES_LOGGED` moves of an axis that change its target, with the board
    /// time, so a run shows the body's focus and aperture sequence (AF scan, exposure).
    fn log_move(&mut self, base: u32, axis: &str, from: u16, target: u16, now: u64) {
        let n = self.state.logged.range(base..base + MOVES_LOGGED).count() as u32;
        if n < MOVES_LOGGED && from != target {
            let ms = now / self.timing.ticks_per_ms;
            self.log_once(base + n, || format!("{axis} move {from} -> {target} at {ms} ms"));
        }
    }
    fn move_aperture(&mut self, now: u64, target: i64) {
        let target = self.spec.aperture.clamp(target);
        let tpm = self.timing.ticks_per_ms;
        let speed = self.spec.aperture.units_per_ms;
        let from = self.state.aperture.position(now, tpm);
        self.state.aperture.retarget(now, tpm, target, speed);
        let aperture = self.state.aperture;
        self.operation(now, aperture);
        self.log_once(EVENT_FIRST_APERTURE, || format!("first aperture move {from} -> {target}"));
        self.log_move(EVENT_APERTURE_MOVES, "aperture", from, target, now);
    }
    fn respond(&mut self, family: u16, payload: u16, now: u64) -> Option<u16> {
        let tpm = self.timing.ticks_per_ms;
        if family == 0 {
            return None;
        }
        let known = Spec::driven(family)
            || matches!(
                family,
                PARAMETERS | STATUS | DETAIL_STATUS | APERTURE_RING | FOCUS_RING | READ_CATALOG
                    | STATE_REQUEST
            )
            || KNOWN_READS.contains(&family)
            || KNOWN_WRITES.contains(&family)
            || self.spec.declared(family).is_some();
        self.log_once(family as u32, || {
            format!(
                "first request family {family:#05x} payload {payload:#06x}{}",
                if known { "" } else { " (unmodeled family, answered 0)" }
            )
        });
        Some(match family {
            SYNC | IDENTIFICATION | EXTENDED_IDENTIFICATION => 0,
            FEEDBACK | FOCUS_STATE => self.state.focus.position(now, tpm),
            // The code in the high byte, no fraction, flag clear.
            APERTURE_STATE => (self.state.aperture.position(now, tpm) & 0x3f) << 8,
            // Zoom step 0 (the short end); the lens has no zoom ring model.
            ZOOM_STATE if self.spec.declared(ZOOM_STATE).is_none() => 0,
            // 0x54: the speed of the next move. 0x7fff is the saturated rate (0x156f91c) and
            // 0 is taken as no request; both select the lens's own speed. Other values are
            // taken as position units per second (the unit is inferred, not measured).
            FOCUS_SPEED => {
                self.state.focus_speed = if payload == 0 || payload == 0x7fff {
                    self.spec.focus.units_per_ms
                } else {
                    ((payload / 1000).max(1)).min(self.spec.focus.units_per_ms)
                };
                payload
            }
            FOCUS_FLAGS => {
                self.state.settings.insert(FOCUS_FLAGS, payload);
                payload
            }
            // 0x56: a signed absolute target; the move starts here. A target equal to the
            // current position still holds the ready line busy for the shortest operation,
            // so the body sees the drive complete.
            FOCUS_TARGET => {
                self.move_focus(now, payload as i16 as i64);
                payload
            }
            FOCUS_DRIVE => {
                self.move_focus(now, payload as i64);
                payload
            }
            APERTURE_DRIVE => {
                self.move_aperture(now, ((payload & 0xfff) >> 6) as i64);
                payload
            }
            APERTURE_TARGET => {
                self.move_aperture(now, payload as i64);
                payload
            }
            APERTURE_STEP => {
                let target = self.state.aperture.target as i64 + payload as i16 as i64;
                self.move_aperture(now, target);
                payload
            }
            f if self.spec.declared(f).is_some() => self.spec.declared(f).unwrap(),
            PARAMETERS => payload,
            STATUS => self.spec.status,
            DETAIL_STATUS => self.spec.detail_status,
            APERTURE_RING => self.spec.aperture_ring,
            FOCUS_RING => self.spec.focus_ring,
            // The block count; the bulk read follows the reply.
            READ_CATALOG => self.spec.catalog(payload & 0xf).blocks,
            // Echo and store it, and hold the ready line busy like an operation.
            STATE_REQUEST => {
                self.state.settings.insert(STATE_REQUEST, payload);
                self.operation(now, Motion::at_rest(0));
                payload
            }
            f if KNOWN_WRITES.contains(&f) => {
                self.state.settings.insert(f, payload);
                payload
            }
            _ => 0,
        })
    }
    fn deliver(&mut self, received: u32, now: u64) {
        if received == 0 {
            return;
        }
        let packet = Packet(received);
        if let Some(payload) = self.respond(packet.family(), packet.payload(), now) {
            let link = &mut self.state.link;
            link.replies.push_back(Message {
                packet: Packet::encode(payload, packet.family(), false).0,
                sequence: link.transmit_counter,
                catalog: (packet.family() == READ_CATALOG).then_some(packet.payload() & 0xf),
            });
            link.transmit_counter = (link.transmit_counter + 1) & 7;
            if link.replies.len() > REPLY_LIMIT {
                link.replies.pop_front();
                self.log_once(EVENT_REPLY_OVERFLOW, || {
                    "body did not collect queued replies; oldest dropped".into()
                });
            }
        }
        self.state.link.receive_counter = (self.state.link.receive_counter + 1) & 7;
    }
    fn bulk_geometry(&self, source: Source) -> (usize, u16) {
        match source {
            Source::Identification | Source::Extended => (IDENTIFICATION_BYTES, 1),
            Source::Catalog { selector } => (CATALOG_BLOCK_BYTES, self.spec.catalog(selector).blocks),
        }
    }
    /// Data byte `offset` of block `block`; past the last block the last one repeats.
    fn bulk_data(&self, source: Source, block: u16, offset: usize) -> u8 {
        let (_, count) = self.bulk_geometry(source);
        let block = block.min(count - 1);
        match source {
            Source::Identification => self.spec.identification[offset],
            Source::Extended => self.spec.extended[offset],
            Source::Catalog { selector } => {
                let catalog = self.spec.catalog(selector);
                let marked = catalog.marker_block == Some(block);
                match offset {
                    0x47 if marked => 0x55,
                    0x48 if marked => 0xaa,
                    o => catalog.byte(block, o),
                }
            }
        }
    }
    fn bulk_byte(&self, source: Source, next: u16, index: usize) -> u8 {
        let (width, _) = self.bulk_geometry(source);
        let header = next.to_be_bytes();
        match index {
            0..=1 => header[index],
            i if i < width + 2 => self.bulk_data(source, next, i - 2),
            _ => (0..width).fold(header[0].wrapping_add(header[1]), |sum, i| {
                sum.wrapping_add(self.bulk_data(source, next, i))
            }),
        }
    }
    /// One byte-wide frame while serving.
    fn accept(&mut self, byte: u8, now: u64) -> u8 {
        self.settle(now);
        self.state.link.frames = self.state.link.frames.wrapping_add(1);
        match self.state.link.phase.clone() {
            Phase::Header => self.header_byte(byte, now),
            Phase::Acknowledgment { received, sent } => {
                let index = self.state.link.input.len();
                let reply = Packet::encode(
                    0x0800 | ((sent.sequence as u16) << 8),
                    Packet(received).family(),
                    true,
                )
                .bytes()[index];
                self.state.link.input.push(byte);
                if self.state.link.input.len() == 4 {
                    let word = u32::from_be_bytes(
                        self.state.link.input.as_slice().try_into().unwrap(),
                    );
                    self.state.link.input.clear();
                    if !valid_word(word) {
                        self.log_once(EVENT_BAD_FRAME, || {
                            format!("invalid acknowledgment {word:08x}; resynchronized")
                        });
                    }
                    self.deliver(received, now);
                    if word != 0 && valid_word(word) && !Packet(word).ack() {
                        // The body sent a new header instead of acknowledging.
                        self.log_once(EVENT_OUT_OF_PHASE, || {
                            format!("header {word:08x} in the acknowledgment phase")
                        });
                        self.state.link.phase = Phase::Acknowledgment { received: word, sent: IDLE };
                    } else if let Some(source) = (sent.packet != 0)
                        .then(|| match Packet(sent.packet).family() {
                            IDENTIFICATION => Some(Source::Identification),
                            EXTENDED_IDENTIFICATION => Some(Source::Extended),
                            READ_CATALOG => sent.catalog.map(|selector| Source::Catalog { selector }),
                            _ => None,
                        })
                        .flatten()
                    {
                        self.state.link.phase = Phase::Bulk { source, next: 0 };
                    } else {
                        self.state.link.phase = Phase::Header;
                    }
                }
                reply
            }
            Phase::Bulk { source, next } => {
                let index = self.state.link.input.len();
                let (width, count) = self.bulk_geometry(source);
                let out = self.bulk_byte(source, next, index);
                self.state.link.input.push(byte);
                if self.state.link.input.len() == width + 3 {
                    let request = u16::from_be_bytes([self.state.link.input[0], self.state.link.input[1]]);
                    self.state.link.input.clear();
                    match request {
                        BULK_FINISH => {
                            self.state.link.phase = Phase::Header;
                            match source {
                                Source::Identification => {
                                    self.state.link.identification_reads += 1;
                                    self.log_once(EVENT_IDENTIFICATION, || "identification record read".into());
                                }
                                Source::Extended => {
                                    self.state.link.extended_reads += 1;
                                    self.log_once(EVENT_EXTENDED, || "extended record read".into());
                                }
                                Source::Catalog { selector } => {
                                    self.state.link.catalog_reads += 1;
                                    self.log_once(EVENT_CATALOG + selector as u32, || {
                                        format!("catalog {selector} read ({count} blocks)")
                                    });
                                }
                            }
                        }
                        block if block < count => {
                            self.state.link.phase = Phase::Bulk { source, next: block };
                        }
                        other => {
                            if other != BULK_END {
                                self.log_once(EVENT_BULK_ABORT, || {
                                    format!("bulk request for block {other:#06x} beyond {count} blocks")
                                });
                            }
                            self.state.link.phase = Phase::Bulk { source, next: BULK_END };
                        }
                    }
                }
                out
            }
        }
    }
    fn header_byte(&mut self, byte: u8, _now: u64) -> u8 {
        let index = self.state.link.input.len();
        if index == 0 {
            let next = self.state.link.replies.pop_front().unwrap_or(IDLE);
            self.state.link.outgoing = Some(next);
        }
        let sent = self.state.link.outgoing.unwrap_or(IDLE);
        let out = sent.packet.to_be_bytes()[index];
        self.state.link.input.push(byte);
        if self.state.link.input.len() == 4 {
            let word = u32::from_be_bytes(self.state.link.input.as_slice().try_into().unwrap());
            self.state.link.input.clear();
            self.state.link.outgoing = None;
            if !valid_word(word) || Packet(word).ack() {
                if valid_word(word) {
                    self.log_once(EVENT_OUT_OF_PHASE, || {
                        format!("acknowledgment {word:08x} in the header phase")
                    });
                } else {
                    self.log_once(EVENT_BAD_FRAME, || {
                        format!("invalid header {word:08x}; resynchronized")
                    });
                }
                // The reply was not acknowledged: send it again.
                if sent.packet != 0 {
                    self.state.link.replies.push_front(sent);
                }
            } else {
                self.state.link.phase = Phase::Acknowledgment { received: word, sent };
            }
        }
        out
    }
    fn validate_state(&self, state: &State) -> Result<(), String> {
        let link = &state.link;
        let bad_motion = |m: Motion, axis: Axis| {
            !(axis.min..=axis.max).contains(&m.from) || !(axis.min..=axis.max).contains(&m.target)
        };
        if link.transmit_counter > 7
            || link.receive_counter > 7
            || link.replies.len() > REPLY_LIMIT
            || link.replies.iter().any(|m| m.sequence > 7 || !valid_word(m.packet))
            || bad_motion(state.focus, self.spec.focus)
            || bad_motion(state.aperture, self.spec.aperture)
            || state.powered != !matches!(state.power, Power::Off)
            || (!state.power.serving() && (link.frames != 0 || state.operation_until.is_some()))
        {
            return Err("invalid X-mount lens checkpoint".into());
        }
        match &link.phase {
            Phase::Header if link.input.len() <= 3 && link.outgoing.is_some() == !link.input.is_empty() => {}
            Phase::Acknowledgment { received, sent }
                if link.input.len() <= 3
                    && link.outgoing.is_none()
                    && valid_word(*received)
                    && valid_word(sent.packet) => {}
            Phase::Bulk { source, next }
                if (*next < self.bulk_geometry(*source).1 || *next == BULK_END)
                    && link.input.len() < self.bulk_geometry(*source).0 + 3
                    && link.outgoing.is_none() => {}
            _ => return Err("invalid X-mount lens link phase checkpoint".into()),
        }
        Ok(())
    }
}

/// The board's GPIO boundary and serial controller share one lens.
#[derive(Clone)]
pub struct Port(Arc<Mutex<Unit>>);
impl Port {
    pub fn new(spec: Spec, timing: Timing) -> Result<Self, String> {
        Ok(Self(Arc::new(Mutex::new(Unit::new(spec, timing)?))))
    }
    pub fn name(&self) -> String {
        self.0.lock().unwrap().spec.name.clone()
    }
    pub fn next_event_tick(&self, now: u64, powered: bool) -> Option<u64> {
        self.0.lock().unwrap().next(now, powered)
    }
    pub fn tick(&self, now: u64, powered: bool) {
        self.0.lock().unwrap().tick(now, powered)
    }
    pub fn powered(&self) -> bool {
        self.0.lock().unwrap().state.powered
    }
    pub fn serving(&self) -> bool {
        self.0.lock().unwrap().state.power.serving()
    }
    pub fn ready_high(&self) -> bool {
        self.0.lock().unwrap().ready_high()
    }
    /// Focus position and target at `now`.
    pub fn focus(&self, now: u64) -> (u16, u16) {
        let unit = self.0.lock().unwrap();
        let s = unit.state.focus;
        (s.position(now, unit.timing.ticks_per_ms), s.target)
    }
    /// Lowest and highest focus position (the motor travel), for image models that turn the
    /// position into defocus.
    pub fn focus_range(&self) -> (u16, u16) {
        let unit = self.0.lock().unwrap();
        (unit.spec.focus.min, unit.spec.focus.max)
    }
    /// Aperture index and target at `now`.
    pub fn aperture(&self, now: u64) -> (u16, u16) {
        let unit = self.0.lock().unwrap();
        let s = unit.state.aperture;
        (s.position(now, unit.timing.ticks_per_ms), s.target)
    }
    pub fn identification_reads(&self) -> u64 {
        self.0.lock().unwrap().state.link.identification_reads
    }
    pub fn extended_reads(&self) -> u64 {
        self.0.lock().unwrap().state.link.extended_reads
    }
    pub fn catalog_reads(&self) -> u64 {
        self.0.lock().unwrap().state.link.catalog_reads
    }
    pub fn take_logs(&self) -> Vec<String> {
        std::mem::take(&mut self.0.lock().unwrap().logs)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    spec: Spec,
    timing: Timing,
    state: State,
}
const KIND: &str = "x-mount-motion-lens";

impl Wire for Port {
    fn checkpoint(&self) -> Result<Checkpoint, String> {
        let unit = self.0.lock().unwrap();
        Checkpoint::encode(
            KIND,
            &Saved { spec: unit.spec.clone(), timing: unit.timing, state: unit.state.clone() },
        )
    }
    fn validate_checkpoint(&self, saved: &Checkpoint) -> Result<(), String> {
        let candidate: Saved = saved.decode(KIND)?;
        let unit = self.0.lock().unwrap();
        if candidate.spec != unit.spec
            || candidate.timing != unit.timing
            || !saved.children().is_empty()
            || !saved.blobs().is_empty()
        {
            return Err("X-mount lens checkpoint differs from its declared lens".into());
        }
        unit.validate_state(&candidate.state)
    }
    fn restore_checkpoint(&mut self, saved: &Checkpoint) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        let candidate: Saved = saved.decode(KIND)?;
        let mut unit = self.0.lock().unwrap();
        unit.state = candidate.state;
        unit.logs.clear();
        Ok(())
    }
    fn validate_pending(&self, saved: &Checkpoint, token: u64, frame: Frame) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        let candidate: Saved = saved.decode(KIND)?;
        if frame.bits > 16 || frame.mode > 3 {
            return Err("invalid X-mount lens pending frame".into());
        }
        if candidate.state.power.serving()
            && token >> 32 == candidate.state.epoch as u64
            && token & u32::MAX as u64 != candidate.state.link.frames & u32::MAX as u64
        {
            return Err("X-mount lens pending serial token differs".into());
        }
        Ok(())
    }
    fn begin(&mut self, _frame: Frame) -> Result<u64, String> {
        let unit = self.0.lock().unwrap();
        Ok(((unit.state.epoch as u64) << 32) | (unit.state.link.frames & u32::MAX as u64))
    }
    fn finish(&mut self, token: u64, frame: Frame, now: u64) -> Result<u16, String> {
        let mut unit = self.0.lock().unwrap();
        unit.settle(now);
        if token >> 32 != unit.state.epoch as u64 {
            // Power changed while the byte shifted: the line was undriven.
            return Ok(UNDRIVEN_BYTE);
        }
        if !unit.state.power.serving() {
            unit.log_once(EVENT_NOT_SERVING, || {
                "body clocked a frame before the lens was ready; answered 0xff".into()
            });
            return Ok(UNDRIVEN_BYTE);
        }
        if frame.bits != 8 || frame.mode != 3 {
            let (bits, mode) = (frame.bits, frame.mode);
            unit.log_once(EVENT_FRAME_SHAPE, || {
                format!("expected 8-bit SPI mode 3 frames, got {bits} bits mode {mode}")
            });
        }
        Ok(unit.accept(frame.word as u8, now) as u16)
    }
    fn save(&self) -> Value {
        let unit = self.0.lock().unwrap();
        json!({"kind": KIND, "spec": unit.spec, "timing": unit.timing, "state": unit.state})
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let mut unit = self.0.lock().unwrap();
        if value["kind"] != KIND
            || value["spec"] != json!(unit.spec)
            || value["timing"] != json!(unit.timing)
        {
            return Err("X-mount lens checkpoint differs from its declared lens".into());
        }
        let state: State = serde_json::from_value(value["state"].clone()).map_err(|e| e.to_string())?;
        unit.validate_state(&state)?;
        unit.state = state;
        unit.logs.clear();
        Ok(())
    }
}

#[cfg(test)]
mod tests;
