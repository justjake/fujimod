//! Modelled still data path of the X-T5 4.31 (opt-in, `XT5_STILL_PATH=real`):
//! sensor RAW (RAW DMA unit 3) -> RIFP ConvertToYC -> RESZ ResizeToRecordSize -> JPEG main,
//! and RZL2 CreateInnerYC -> JPEG preview and thumbnail.
//!
//! # RIFP 0xffd02000 still runs
//!
//! The recf queue (0x2d3a980 -> 0x2c51a9c) programs the RIFP top page through the register-list
//! DMA 0xffd01000 and starts it (+0x04 = 1). At the end of such a run (the I/O DMA is in
//! register-list mode, +0x4c bit 0, and both addresses below are set), [`rifp_run`] writes the
//! output picture. Words (setters 0x2c52638 / 0x2c52728 / 0x2c52c48, roles decoded statically):
//!
//! * +0x0c / +0x10 input address (bits 31..0 / 34..32), +0x1c input pitch in bytes;
//! * +0x20 input size, +0x24 crop origin, +0x28 crop size, +0x2c output size, each
//!   lines << 16 | pixels (counts, not minus one). A zero crop size takes the input from the
//!   crop origin on; a zero output size takes the crop size;
//! * +0x40 / +0x44 output address, +0x50 output pitch in bytes; packed 4:2:2 (Y0 Y1 Cb Cr).
//!
//! The mode word +0x08 is not decoded, so the run's input decides what it does:
//!
//! * ConvertToYC: an input inside the RAW the still channel wrote last, at its pitch, is
//!   developed with the declared X-Trans law ([`fujimod_emulator_imaging::xtrans`]). The tile
//!   site of the first input sample follows from its offset in that RAW (site (0, 0) at the
//!   RAW origin), so a band or a column stripe of the picture develops like the whole. An input
//!   pitch from 1.5 up to (not including) 2 bytes a sample reads packed 12-bit samples (two in
//!   three bytes, the still channel's packed lines), scaled to the law's 14-bit range; the
//!   input must then start on a sample pair.
//! * PostYC (any other input): the input is a packed 4:2:2 picture, copied.
//!
//! Either way output pixel (x, y) takes input (crop x + x * crop width / output width, crop y +
//! y * crop lines / output lines), nearest neighbour. Every run, banded or not, is handled on
//! its own; the last output is kept ([`YcOutput`]) for RESZ.
//!
//! # RESZ 0xffd0c000 ([`Resz`])
//!
//! A completion stub (manifest options as for `completion-stub`) whose start (+0x04 bit 0)
//! also scales a packed 4:2:2 picture, nearest neighbour (registers from the drivers
//! 0x13bd5d0.. / 0x13be318, roles decoded statically):
//!
//! * +0x14 source size, +0x18 crop origin, +0x1c crop size, +0x20 output size: (lines - 1) << 16
//!   | (pixels - 1) for the sizes, y << 16 | x for the origin. A zero crop size takes the source.
//! * +0x24 / +0x28 horizontal / vertical: bits 16..17 a power-of-two pre-decimation of the
//!   source (0x13bd86c / 0x13bd918 for RESZ, 0x13c06ac / 0x13c06e4 for RZL2; the shift is
//!   chosen by 0x148942c: 2 when (source - 1) / (output - 1) > 3, 1 when > 1, else 0), bits
//!   0..15 the Q12 step ((decimated crop - 1) << 12) / (output - 1). The crop origin and crop
//!   size count decimated pixels and lines (0x2d17678 shifts the source size right before it
//!   writes the crop), so input pixel x of the crop is source pixel (origin + x) << shift.
//! * +0xd0 / +0xd4 source address bits 34..32 / 31..0, +0xe0 source pitch; +0xe4 / +0xe8 and
//!   +0xf4 the same for the destination.
//! * +0xd8 / +0xdc and +0xec / +0xf0 (second planes) are logged and ignored: only packed pictures are
//!   modelled. The Q12 steps (the output follows crop / output instead), phases and the
//!   31-tap filter are not applied; the pre-decimation is.
//!
//! In RCIZ queue mode (0x1486de0 -> 0x13be318) the driver writes only the destination words:
//! the source is the RIFP inline stream. A start with source address 0 therefore reads the
//! last RIFP output ([`YcOutput`]) at its pitch.
//!
//! # RZL2 0xff984000 (the same [`Resz`] with [`RZL2`])
//!
//! CreateInnerYC (step 0x26, 0x2d81e2c) runs RZL2 twice through 0x2d1d914 -> 0x2d3ba68: the
//! main picture to buffer id 0x25 (640x480 preview), then id 0x25 to id 0x26 (160x120
//! thumbnail). Its driver (parameters 0x14896d4 -> setter 0x13c055c, addresses 0x1489874 ->
//! 0x13c0384; the mode 2 parameter block from 0x2d17678 / 0x2d15c20) programs the RESZ layout
//! with every address word 4 bytes lower:
//!
//! * +0x10 formats (source class bits 0..1, destination class bits 4..5), +0x14 source size,
//!   +0x18 crop origin, +0x1c crop size, +0x20 output size and the +0x24 / +0x28
//!   pre-decimation, encoded as for RESZ (0x2d17678 subtracts one from even counts). Both
//!   CreateInnerYC passes decimate by 4: 7728x2576 -> crop 1932x644 -> 640x480, then 640x480
//!   -> crop 160x120 -> 160x120;
//! * +0xcc / +0xd0 source address bits 34..32 / 31..0, +0xd4 / +0xd8 source second plane,
//!   +0xdc source pitch; +0xe0 / +0xe4, +0xe8 / +0xec and +0xf0 the same for the destination.
//!
//! The first pass reads every other line of the main picture (0x2d1776c doubles the source
//! pitch and halves its lines), which the programmed words already express. RZL2 has no inline
//! source: a start with source address 0 refuses.
use fujimod_emulator_devices::checkpoint::Checkpoint;
use fujimod_emulator_devices::{Cx, Device};
use fujimod_emulator_imaging::xtrans;
use fujimod_emulator_sensor_input::sensor::StillRaw;
use fujimod_emulator_xp5_io::completion_stub::CompletionStub;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::{Arc, Mutex};

/// A packed 4:2:2 picture in guest memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct YcPicture {
    pub address: u64,
    pub pitch: u64,
    pub width: u32,
    pub lines: u32,
}

#[derive(Default)]
struct Shared {
    output: Option<YcPicture>,
    rifp_busy: bool,
    deferred: Option<ReszJob>,
}

/// What RIFP and RESZ share: the last picture a RIFP still run wrote, whether a RIFP run is in
/// progress, and a RESZ job that waits for that run's output (an inline start, see the module
/// notes). Only the output is part of a checkpoint (RIFP's); a checkpoint taken between an
/// inline RESZ start and the RIFP completion loses the waiting job.
#[derive(Clone, Default)]
pub struct YcOutput(Arc<Mutex<Shared>>);
impl YcOutput {
    pub fn get(&self) -> Option<YcPicture> {
        self.0.lock().unwrap().output
    }
    pub fn set(&self, picture: Option<YcPicture>) {
        self.0.lock().unwrap().output = picture;
    }
    /// RIFP: a run started (true) or ended (false).
    pub fn set_busy(&self, busy: bool) {
        self.0.lock().unwrap().rifp_busy = busy;
    }
    /// RIFP: the RESZ job waiting for the run that just ended, if any.
    pub fn take_deferred(&self) -> Option<ReszJob> {
        self.0.lock().unwrap().deferred.take()
    }
}

/// Where a scaler of the RESZ family keeps its address words (see the module notes); the size
/// words (+0x14..+0x20) are common.
#[derive(Debug)]
pub struct Layout {
    /// The block's name in log lines.
    pub block: &'static str,
    /// The device name ([`Device::name`]), the manifest window kind.
    pub kind: &'static str,
    /// Source address bits 34..32; bits 31..0 follow at +4, the second plane at +8 / +0xc and
    /// the pitch at +0x10.
    pub source: u64,
    /// The same for the destination.
    pub output: u64,
    /// Whether a zero source address reads the RIFP still output (RCIZ inline).
    pub inline: bool,
}

/// RESZ 0xffd0c000.
pub static RESZ: Layout = Layout {
    block: "RESZ",
    kind: "imaging-resz",
    source: 0xd0,
    output: 0xe4,
    inline: true,
};

/// RZL2 0xff984000.
pub static RZL2: Layout = Layout {
    block: "RZL2",
    kind: "imaging-rzl2",
    source: 0xcc,
    output: 0xe0,
    inline: false,
};

impl Layout {
    fn words(&self) -> impl Iterator<Item = u64> + '_ {
        [0x10, 0x14, 0x18, 0x1c, 0x20, 0x24, 0x28]
            .into_iter()
            .chain([self.source, self.output].into_iter().flat_map(|o| (0..5).map(move |i| o + 4 * i)))
    }
}

/// The words of one RESZ (or RZL2) start.
#[derive(Clone, Debug)]
pub struct ReszJob {
    base: u64,
    layout: &'static Layout,
    words: std::collections::BTreeMap<u64, u32>,
}

impl ReszJob {
    /// Scale this job's picture; `inline` replaces a zero source address.
    pub fn scale(&self, cx: &mut Cx<'_>, inline: Option<YcPicture>) -> Result<(), String> {
        let word = |o: u64| self.words.get(&o).copied().unwrap_or(0);
        let address = |high: u64| u64::from(word(high + 4)) | (u64::from(word(high) & 7) << 32);
        let size = |w: u32| ((w & 0xffff) as usize + 1, (w >> 16) as usize + 1);
        let (src, dst) = (self.layout.source, self.layout.output);
        let (mut source, mut pitch) = (address(src), word(src + 0x10) as usize);
        let (output, out_pitch) = (address(dst), word(dst + 0x10) as usize);
        let name = self.layout.block;
        let from_rifp = source == 0;
        if from_rifp {
            let picture = inline.ok_or_else(|| {
                if self.layout.inline {
                    format!("{name} start with source address 0 (RCIZ inline) but no RIFP still output")
                } else {
                    format!("{name} start with source address 0")
                }
            })?;
            (source, pitch) = (picture.address, picture.pitch as usize);
        }
        let (width, lines) = size(word(0x14));
        // Pre-decimation shifts (+0x24 / +0x28 bits 16..17); the crop counts decimated
        // pixels and lines.
        let shift = (
            ((word(0x24) >> 16) & 3) as usize,
            ((word(0x28) >> 16) & 3) as usize,
        );
        let decimated = (width.div_ceil(1 << shift.0), lines.div_ceil(1 << shift.1));
        let origin = ((word(0x18) & 0xffff) as usize, (word(0x18) >> 16) as usize);
        let crop = match word(0x1c) {
            0 => (
                decimated.0 - origin.0.min(decimated.0),
                decimated.1 - origin.1.min(decimated.1),
            ),
            w => size(w),
        };
        let out = size(word(0x20));
        let geometry = format!(
            "{name} {:#x}: source {source:#x}{} pitch {pitch:#x} size {width}x{lines}, crop {crop:?} \
             at {origin:?}{}, output {out:?} at {output:#x} pitch {out_pitch:#x} (+0x10 {:#x})",
            self.base,
            if from_rifp { " (RIFP output)" } else { "" },
            if shift == (0, 0) {
                String::new()
            } else {
                format!(" after decimation by ({}, {})", 1 << shift.0, 1 << shift.1)
            },
            word(0x10)
        );
        // The X-T5 4.31 still ResizeToRecordSize run programs a second source/destination plane
        // (RESZ +0xd8/+0xdc, +0xec/+0xf0) next to the packed 4:2:2 main plane that the JPEG main
        // encode reads (0x32e1e000, 2 bytes per pixel). Its format is not decoded: scale plane 0
        // only and name the second plane in the log line.
        let planes = [src + 8, src + 0xc, dst + 8, dst + 0xc];
        let geometry = if planes.iter().any(|&o| word(o) != 0) {
            format!(
                "{geometry}; second plane source {:#x}:{:#x} destination {:#x}:{:#x} not modelled",
                word(planes[0]),
                word(planes[1]),
                word(planes[2]),
                word(planes[3])
            )
        } else {
            geometry
        };
        // The setters (0x13bd86c, 0x13c06ac) write only bits 0..17 and a shift up to 2.
        let steps = [word(0x24), word(0x28)];
        if steps.iter().any(|w| w >> 18 != 0 || (w >> 16) & 3 == 3) {
            return Err(format!(
                "{geometry}: undecoded step words +0x24 {:#x} / +0x28 {:#x}",
                steps[0], steps[1]
            ));
        }
        // Only the source pixels the crop reaches must fit the pitch: the X-T5 playback full-size
        // resize programs a 7760-pixel source with a 15488-byte pitch and a 7740-pixel crop.
        let used = ((origin.0 + crop.0) << shift.0).min(width).next_multiple_of(2);
        if output == 0
            || crop.0 == 0
            || crop.1 == 0
            || origin.0 + crop.0 > decimated.0
            || origin.1 + crop.1 > decimated.1
            || out.0 % 2 != 0
            || pitch < 2 * used
            || out_pitch < 2 * out.0
        {
            return Err(format!("{geometry} is outside the modelled layout"));
        }
        let bytes = cx.ram_bytes(source, pitch * (lines - 1) + 2 * used)?;
        cx.log(&geometry);
        scale_422(&bytes, pitch, shift, origin, crop, out, |y, line| {
            cx.ram_write(output + (y * out_pitch) as u64, line)
        })
    }
}

fn pixels_lines(word: u32) -> (usize, usize) {
    ((word & 0xffff) as usize, (word >> 16) as usize)
}

/// Nearest-neighbour scale of packed 4:2:2 `source` (pitch `pitch`) from the crop at `origin`
/// of `crop` (pixels, lines) to `out` (pixels, lines), written line by line to `write`. The
/// origin and crop count source pixels and lines decimated by `1 << shift`.
fn scale_422(
    source: &[u8],
    pitch: usize,
    shift: (usize, usize),
    origin: (usize, usize),
    crop: (usize, usize),
    out: (usize, usize),
    mut write: impl FnMut(usize, &[u8]) -> Result<(), String>,
) -> Result<(), String> {
    let mut line = vec![0u8; out.0 * 2];
    for y in 0..out.1 {
        let row = &source[((origin.1 + y * crop.1 / out.1) << shift.1) * pitch..];
        for pair in 0..out.0 / 2 {
            let x = [0, 1].map(|i| (origin.0 + (2 * pair + i) * crop.0 / out.0) << shift.0);
            line[pair * 4] = row[x[0] / 2 * 4 + x[0] % 2];
            line[pair * 4 + 1] = row[x[1] / 2 * 4 + x[1] % 2];
            line[pair * 4 + 2] = row[x[0] / 2 * 4 + 2];
            line[pair * 4 + 3] = row[x[0] / 2 * 4 + 3];
        }
        write(y, &line)?;
    }
    Ok(())
}

/// One RIFP still run (see the module notes) from the words `word` gives; `raw` is the
/// still channel's last RAW. Returns the picture it wrote.
pub fn rifp_run(
    cx: &mut Cx<'_>,
    word: impl Fn(u64) -> u32,
    raw: Option<StillRaw>,
) -> Result<YcPicture, String> {
    let address = |low, high| u64::from(word(low)) | (u64::from(word(high) & 7) << 32);
    let (input, pitch) = (address(0x0c, 0x10), word(0x1c) as usize);
    let (output, out_pitch) = (address(0x40, 0x44), (word(0x50) & 0xfffff) as usize);
    let (width, lines) = pixels_lines(word(0x20));
    let develop = raw.filter(|r| {
        r.pitch as usize == pitch
            && input >= r.address
            && input < r.address + r.pitch * u64::from(r.lines)
    });
    // An input in the still RAW takes its layout; any other input with a pitch of 1.5 up to
    // (not including) 2 bytes a sample would be packed 12-bit RAW (see the module notes).
    let packed12 = develop.map_or(2 * pitch >= 3 * width && pitch < 2 * width, |r| r.packed12);
    let origin = pixels_lines(word(0x24));
    let crop = match pixels_lines(word(0x28)) {
        (0, _) | (_, 0) => (width.saturating_sub(origin.0), lines.saturating_sub(origin.1)),
        crop => crop,
    };
    let out = match pixels_lines(word(0x2c)) {
        (0, _) | (_, 0) => crop,
        out => out,
    };
    let geometry = || {
        format!(
            "RIFP still run: input {input:#x} pitch {pitch:#x} size {width}x{lines} \
             (+0x20 {:#x}), crop {crop:?} at {origin:?}, output {out:?} at {output:#x} \
             pitch {out_pitch:#x} (+0x08 mode {:#x})",
            word(0x20),
            word(0x08)
        )
    };
    if width == 0
        || lines == 0
        || crop.0 == 0
        || crop.1 == 0
        || origin.0 + crop.0 > width
        || origin.1 + crop.1 > lines
        || out.0 < 2
        || out.0 % 2 != 0
        || out_pitch < 2 * out.0
        || (!packed12 && pitch < 2 * width)
    {
        return Err(format!("{} is outside the modelled layout", geometry()));
    }
    let line = if packed12 { xtrans::packed12_bytes(width) } else { 2 * width };
    if packed12 && pitch < line {
        return Err(format!("{} is outside the modelled layout", geometry()));
    }
    let bytes = cx.ram_bytes(input, pitch * (lines - 1) + line)?;
    if packed12 && develop.is_none() {
        return Err(format!(
            "{}: a packed 12-bit input pitch outside the still channel's packed RAW is not modelled",
            geometry()
        ));
    }
    let write = |cx: &mut Cx<'_>, y: usize, line: &[u8]| {
        cx.ram_write(output + (y * out_pitch) as u64, line)
    };
    if let Some(r) = develop {
        let offset = input - r.address;
        let column = offset % r.pitch;
        if packed12 && column % 3 != 0 {
            return Err(format!("{}: packed 12-bit input starts inside a sample pair", geometry()));
        }
        let sample = if packed12 { column / 3 * 2 } else { column / 2 };
        let phase = ((offset / r.pitch % 6) as usize, (sample % 6) as usize);
        let raw = if packed12 {
            xtrans::Raw::packed12(&bytes, pitch, width, lines, phase)
        } else {
            xtrans::Raw::new(&bytes, pitch, width, lines, phase)
        }
        .map_err(|e| format!("{}: {e}", geometry()))?;
        cx.log(&format!(
            "{} develops {} RAW at site {phase:?}",
            geometry(),
            if packed12 { "packed 12-bit" } else { "16-bit" }
        ));
        for y in 0..out.1 {
            let line = xtrans::develop_422_line(&raw, origin.1 + y * crop.1 / out.1, out.0, |x| {
                origin.0 + x * crop.0 / out.0
            });
            write(cx, y, &line)?;
        }
    } else {
        cx.log(&format!("{} copies packed 4:2:2", geometry()));
        scale_422(&bytes, pitch, (0, 0), origin, crop, out, |y, line| write(cx, y, line))?;
    }
    Ok(YcPicture {
        address: output,
        pitch: out_pitch as u64,
        width: out.0 as u32,
        lines: out.1 as u32,
    })
}

/// RESZ 0xffd0c000 or RZL2 0xff984000 ([`Layout`]): a completion stub that, with the still
/// path, scales at each start (see the module notes). Without it (`rifp` `None`) it is the stub
/// alone.
pub struct Resz {
    base: u64,
    layout: &'static Layout,
    stub: CompletionStub,
    rifp: Option<YcOutput>,
}

impl Resz {
    /// RESZ ([`RESZ`]).
    pub fn new(base: u64, stub: CompletionStub, rifp: Option<YcOutput>) -> Self {
        Self::with_layout(&RESZ, base, stub, rifp)
    }
    pub fn with_layout(
        layout: &'static Layout,
        base: u64,
        stub: CompletionStub,
        rifp: Option<YcOutput>,
    ) -> Self {
        Self {
            base,
            layout,
            stub,
            rifp,
        }
    }
    /// A start: scale now, or, with an inline layout and source address 0 while a RIFP run is
    /// in progress, when that run ends.
    fn start(&self, cx: &mut Cx<'_>, rifp: &YcOutput) -> Result<(), String> {
        let job = ReszJob {
            base: self.base,
            layout: self.layout,
            words: self.layout.words().map(|o| (o, self.stub.word(o))).collect(),
        };
        if !self.layout.inline {
            return job.scale(cx, None);
        }
        let source = self.layout.source;
        let mut shared = rifp.0.lock().unwrap();
        if job.words[&source] & 7 == 0 && job.words[&(source + 4)] == 0 && shared.rifp_busy {
            cx.log(&format!(
                "{} {:#x}: inline start waits for the RIFP run",
                self.layout.block, self.base
            ));
            shared.deferred = Some(job);
            return Ok(());
        }
        let output = shared.output;
        drop(shared);
        job.scale(cx, output)
    }
}

impl Device for Resz {
    fn name(&self) -> &'static str {
        self.layout.kind
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        self.stub.read(cx, address, size)
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        // The stub ignores a start while a completion is due; so does the scaling.
        let start = address == self.base + 4
            && size == 4
            && value & 1 != 0
            && self.stub.next_deadline(cx.now()).is_none();
        self.stub.write(cx, address, size, value);
        if start
            && let Some(rifp) = &self.rifp
            && self.stub.next_deadline(cx.now()).is_some()
            && let Err(e) = self.start(cx, rifp)
        {
            cx.fail(e);
        }
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        self.stub.advance(cx);
    }
    fn event(&mut self, cx: &mut Cx<'_>, token: u64) {
        self.stub.event(cx, token);
    }
    fn next_deadline(&self, now: u64) -> Option<u64> {
        self.stub.next_deadline(now)
    }
    fn checkpoint(&self) -> Result<Checkpoint, String> {
        self.stub.checkpoint()
    }
    fn validate_checkpoint(&self, saved: &Checkpoint) -> Result<(), String> {
        self.stub.validate_checkpoint(saved)
    }
    fn restore_checkpoint(&mut self, saved: &Checkpoint) -> Result<(), String> {
        self.stub.restore_checkpoint(saved)
    }
    fn fingerprint(&self) -> Result<Value, String> {
        self.stub.fingerprint()
    }
    fn save(&self) -> Value {
        self.stub.save()
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        self.stub.restore(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::test_host::recording::RecordingHost;

    const RAM: u64 = 0x1000_0000;

    /// A packed 4:2:2 picture whose bytes encode their position.
    fn picture(width: usize, lines: usize, pitch: usize) -> Vec<u8> {
        let mut bytes = vec![0xee; pitch * lines];
        for y in 0..lines {
            for x in 0..width {
                let at = y * pitch + x / 2 * 4;
                bytes[at + x % 2] = (y * 16 + x) as u8;
                bytes[at + 2] = 0x80 | (x / 2) as u8;
                bytes[at + 3] = 0x40 | y as u8;
            }
        }
        bytes
    }

    #[test]
    fn scaling_takes_the_nearest_pixel_and_the_first_pixel_pair_chroma() {
        let source = picture(8, 4, 20);
        let mut out = Vec::new();
        scale_422(&source, 20, (0, 0), (0, 0), (8, 4), (4, 2), |y, line| {
            out.push((y, line.to_vec()));
            Ok(())
        })
        .unwrap();
        assert_eq!(
            out,
            [
                (0, vec![0, 2, 0x80, 0x40, 4, 6, 0x82, 0x40]),
                (1, vec![32, 34, 0x80, 0x42, 36, 38, 0x82, 0x42]),
            ]
        );
    }

    fn rifp_words(input: u64, output: u64) -> impl Fn(u64) -> u32 {
        move |o| match o {
            0x0c => input as u32,
            0x10 => (input >> 32) as u32,
            0x1c => 64,
            0x20 => 12 << 16 | 24,
            0x40 => output as u32,
            0x44 => (output >> 32) as u32,
            0x50 => 48,
            _ => 0,
        }
    }

    /// RAW written by the still channel's law at site (0, 0); a flat colour develops to
    /// its JFIF YCbCr, and an input in that RAW is developed, any other copied.
    #[test]
    fn a_rifp_run_develops_an_input_in_the_still_raw_and_copies_any_other() {
        let colour = [40u8, 180, 90];
        let mut ram = vec![0u8; 0x4000];
        for y in 0..12 {
            xtrans::mosaic_line(&colour.repeat(24), y, &mut ram[y * 64..y * 64 + 48]);
        }
        let mut host = RecordingHost {
            ram_base: RAM,
            ram,
            ..Default::default()
        };
        let raw = StillRaw {
            address: RAM,
            pitch: 64,
            width: 24,
            lines: 12,
            packed12: false,
        };
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        let picture = rifp_run(&mut cx, rifp_words(RAM, RAM + 0x1000), Some(raw)).unwrap();
        assert_eq!(
            picture,
            YcPicture {
                address: RAM + 0x1000,
                pitch: 48,
                width: 24,
                lines: 12
            }
        );
        let [y, cb, cr] = xtrans::ycbcr_jfif(colour).map(|v| v as u8);
        for line in 0..12 {
            let at = 0x1000 + line * 48;
            assert_eq!(&host.ram[at..at + 48], [y, y, cb, cr].repeat(12).as_slice());
        }
        // The developed picture as input (not in the RAW): a straight copy.
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        let words = rifp_words(RAM + 0x1000, RAM + 0x2000);
        let copy = |o| if o == 0x1c { 48 } else { words(o) };
        rifp_run(&mut cx, copy, Some(raw)).unwrap();
        assert_eq!(host.ram[0x2000..0x2000 + 48 * 12], host.ram[0x1000..0x1000 + 48 * 12]);
        // A pitch too short for the input refuses.
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        let short = |o| if o == 0x1c { 32 } else { words(o) };
        assert!(rifp_run(&mut cx, short, Some(raw)).is_err());
    }

    /// Packed 12-bit RAW (pitch 40 for 24 samples, 1.5 bytes a sample plus padding): a flat
    /// colour develops to its JFIF YCbCr, from the RAW origin and from a column stripe that
    /// starts on a sample pair; a stripe inside a pair and a packed input outside the RAW refuse.
    #[test]
    fn a_rifp_run_develops_packed_12_bit_raw() {
        let colour = [40u8, 180, 90];
        let (width, pitch) = (24, 40);
        let mut ram = vec![0u8; 0x4000];
        let mut samples = vec![0u8; 2 * width];
        for y in 0..12 {
            xtrans::mosaic_line(&colour.repeat(width), y, &mut samples);
            xtrans::pack12_line(&samples, &mut ram[y * pitch..y * pitch + 36]);
        }
        let mut host = RecordingHost {
            ram_base: RAM,
            ram,
            ..Default::default()
        };
        let raw = StillRaw {
            address: RAM,
            pitch: pitch as u64,
            width: width as u32,
            lines: 12,
            packed12: true,
        };
        let [y, cb, cr] = xtrans::ycbcr_jfif(colour).map(|v| v as u8);
        let words = rifp_words(RAM, RAM + 0x1000);
        let packed = |o| if o == 0x1c { pitch as u32 } else { words(o) };
        rifp_run(&mut Cx::new(&mut host, 0, 0, 0), packed, Some(raw)).unwrap();
        for line in 0..12 {
            let at = 0x1000 + line * 48;
            assert_eq!(&host.ram[at..at + 48], [y, y, cb, cr].repeat(12).as_slice());
        }
        // A 12-sample stripe from sample 6 (byte 9).
        let words = rifp_words(RAM + 9, RAM + 0x2000);
        let stripe = |o| match o {
            0x1c => pitch as u32,
            0x20 => 12 << 16 | 12,
            o => words(o),
        };
        rifp_run(&mut Cx::new(&mut host, 0, 0, 0), stripe, Some(raw)).unwrap();
        assert_eq!(&host.ram[0x2000..0x2000 + 24], [y, y, cb, cr].repeat(6).as_slice());
        let words = rifp_words(RAM + 10, RAM + 0x2000);
        let inside = |o| match o {
            0x1c => pitch as u32,
            0x20 => 12 << 16 | 12,
            o => words(o),
        };
        assert!(rifp_run(&mut Cx::new(&mut host, 0, 0, 0), inside, Some(raw)).is_err());
        let words = rifp_words(RAM + 0x3000, RAM + 0x2000);
        let outside = |o| if o == 0x1c { pitch as u32 } else { words(o) };
        assert!(rifp_run(&mut Cx::new(&mut host, 0, 0, 0), outside, Some(raw)).is_err());
    }

    fn resz() -> Resz {
        let options = toml::from_str(
            "irq = 203\n\
             registers = [{ offset = 0x00, mode = \"constant\", value = 0x1 }]\n\
             starts = [{ offset = 0x04, mask = 0x1 }]\n\
             delay_us = 200\n\
             completion = [{ offset = 0x10c, set = 0x1 }, { offset = 0x04, clear = 0x1 }]\n\
             ack = { offset = 0x110, mode = \"w1c\", status_offset = 0x10c, bits = 0x3 }",
        )
        .unwrap();
        Resz::new(
            0xffd0c000,
            CompletionStub::new(0xffd0c000, 0x1000, options, 1000).unwrap(),
            Some(YcOutput::default()),
        )
    }

    #[test]
    fn a_resz_start_scales_then_completes_as_the_stub() {
        let mut resz = resz();
        let mut host = RecordingHost {
            ram_base: RAM,
            ram: vec![0; 0x2000],
            ..Default::default()
        };
        host.ram[..80].copy_from_slice(&picture(8, 4, 20));
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        for (o, v) in [
            (0x14, 3 << 16 | 7),
            (0x20, 1 << 16 | 3),
            (0xd4, RAM as u32),
            (0xe0, 20),
            (0xe8, RAM as u32 + 0x1000),
            (0xf4, 16),
            (0x04, 1),
        ] {
            resz.write(&mut cx, 0xffd0c000 + o, 4, u64::from(v));
        }
        assert!(cx.refusal().is_none());
        assert_eq!(&host.ram[0x1000..0x1008], [0, 2, 0x80, 0x40, 4, 6, 0x82, 0x40]);
        assert_eq!(&host.ram[0x1010..0x1018], [32, 34, 0x80, 0x42, 36, 38, 0x82, 0x42]);
        host.now = resz.next_deadline(0).unwrap();
        resz.advance(&mut Cx::new(&mut host, 0, 0, 0));
        assert_eq!(resz.read(&mut Cx::new(&mut host, 0, 0, 0), 0xffd0c10c, 4) & 1, 1);
        assert!(host.irqs.contains(&(203, true)));
    }

    #[test]
    fn a_resz_start_without_source_reads_the_rifp_output() {
        let mut resz = resz();
        let mut host = RecordingHost {
            ram_base: RAM,
            ram: vec![0; 0x2000],
            ..Default::default()
        };
        host.ram[..80].copy_from_slice(&picture(8, 4, 20));
        // No RIFP output yet: the start refuses.
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        for (o, v) in [(0x14, 3 << 16 | 7), (0x20, 3 << 16 | 7), (0xe8, RAM as u32 + 0x1000), (0xf4, 16)] {
            resz.write(&mut cx, 0xffd0c000 + o, 4, u64::from(v));
        }
        resz.write(&mut cx, 0xffd0c004, 4, 1);
        assert!(cx.refusal().is_some());
        let mut resz = resz_with(YcPicture {
            address: RAM,
            pitch: 20,
            width: 8,
            lines: 4,
        });
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        for (o, v) in [(0x14, 3 << 16 | 7), (0x20, 3 << 16 | 7), (0xe8, RAM as u32 + 0x1000), (0xf4, 16), (0x04, 1)] {
            resz.write(&mut cx, 0xffd0c000 + o, 4, u64::from(v));
        }
        assert!(cx.refusal().is_none());
        for y in 0..4 {
            assert_eq!(host.ram[0x1000 + y * 16..][..16], host.ram[y * 20..][..16]);
        }
    }

    /// The chain on the devices: the still RAW, a queued RIFP run that develops it, and an
    /// inline RESZ start (source 0) during that run, which scales the RIFP output when the run
    /// ends.
    #[test]
    fn an_inline_resz_start_scales_the_rifp_output_when_the_run_ends() {
        use super::super::preview::{Io, Preview, StillPath};
        let colour = [200u8, 60, 120];
        let mut ram = vec![0u8; 0x4000];
        for y in 0..12 {
            xtrans::mosaic_line(&colour.repeat(24), y, &mut ram[y * 64..y * 64 + 48]);
        }
        let mut host = RecordingHost {
            ram_base: RAM,
            ram,
            ..Default::default()
        };
        let sensor = fujimod_emulator_sensor_input::sensor::Sensor::new(None, (0, 0));
        sensor.0.lock().unwrap().still_raw = Some(StillRaw {
            address: RAM,
            pitch: 64,
            width: 24,
            lines: 12,
            packed12: false,
        });
        let shared = YcOutput::default();
        let mut io = Io::default();
        let mut rifp = Preview::new(
            true,
            io.clone(),
            &toml::from_str("completion_us=100\nsource_luma=128").unwrap(),
        )
        .unwrap()
        .with_still_path(StillPath {
            sensor,
            output: shared.clone(),
        });
        let mut resz = resz();
        resz.rifp = Some(shared.clone());
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        io.write(&mut cx, 0xffd0104c, 4, 1);
        let words = rifp_words(RAM, RAM + 0x1000);
        for o in [0x0c, 0x10, 0x1c, 0x20, 0x40, 0x44, 0x50] {
            rifp.write(&mut cx, 0xffd02000 + o, 4, u64::from(words(o)));
        }
        rifp.write(&mut cx, 0xffd02004, 4, 1);
        for (o, v) in [(0x14, 11 << 16 | 23), (0x20, 5 << 16 | 11), (0xe8, RAM as u32 + 0x2000), (0xf4, 32), (0x04, 1)] {
            resz.write(&mut cx, 0xffd0c000 + o, 4, u64::from(v));
        }
        assert!(cx.refusal().is_none());
        assert!(host.ram[0x2000..0x2000 + 24].iter().all(|b| *b == 0));
        host.now = rifp.next_deadline(0).unwrap();
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        rifp.advance(&mut cx);
        assert!(cx.refusal().is_none());
        let [y, cb, cr] = xtrans::ycbcr_jfif(colour).map(|v| v as u8);
        for line in 0..6 {
            let at = 0x2000 + line * 32;
            assert_eq!(&host.ram[at..at + 24], [y, y, cb, cr].repeat(6).as_slice());
        }
        assert_eq!(shared.get().map(|p| p.address), Some(RAM + 0x1000));
    }

    /// RZL2: the RESZ layout with the address words 4 bytes lower and no inline source. Two
    /// starts as CreateInnerYC programs them: every other line of the main picture to the
    /// preview, then the preview to the thumbnail.
    #[test]
    fn rzl2_scales_with_its_address_words_and_refuses_a_zero_source() {
        let options = toml::from_str(
            "irq = 205\n\
             registers = [{ offset = 0x00, mode = \"constant\", value = 0x1 }]\n\
             starts = [{ offset = 0x04, mask = 0x1 }]\n\
             delay_us = 200\n\
             completion = [{ offset = 0x104, set = 0x1 }, { offset = 0x04, clear = 0x1 }]\n\
             ack = { offset = 0x108, mode = \"w1c\", status_offset = 0x104, bits = 0x7 }",
        )
        .unwrap();
        let base = 0xff984000;
        let mut rzl2 = Resz::with_layout(
            &RZL2,
            base,
            CompletionStub::new(base, 0x1000, options, 1000).unwrap(),
            Some(YcOutput::default()),
        );
        assert_eq!(rzl2.name(), "imaging-rzl2");
        let mut host = RecordingHost {
            ram_base: RAM,
            ram: vec![0; 0x2000],
            ..Default::default()
        };
        host.ram[..80].copy_from_slice(&picture(8, 4, 20));
        // Pass 1: lines 0 and 2 (pitch doubled, 2 lines) of the 8x4 picture to 4x2.
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        for (o, v) in [
            (0x14, 1 << 16 | 7),
            (0x1c, 1 << 16 | 7),
            (0x20, 1 << 16 | 3),
            (0xd0, RAM as u32),
            (0xdc, 40),
            (0xe4, RAM as u32 + 0x1000),
            (0xf0, 16),
            (0x04, 1),
        ] {
            rzl2.write(&mut cx, base + o, 4, u64::from(v));
        }
        assert!(cx.refusal().is_none());
        assert_eq!(&host.ram[0x1000..0x1008], [0, 2, 0x80, 0x40, 4, 6, 0x82, 0x40]);
        assert_eq!(&host.ram[0x1010..0x1018], [32, 34, 0x80, 0x42, 36, 38, 0x82, 0x42]);
        host.now = rzl2.next_deadline(0).unwrap();
        rzl2.advance(&mut Cx::new(&mut host, 0, 0, 0));
        assert_eq!(rzl2.read(&mut Cx::new(&mut host, 0, 0, 0), base + 0x104, 4) & 1, 1);
        assert!(host.irqs.contains(&(205, true)));
        rzl2.write(&mut Cx::new(&mut host, 0, 0, 0), base + 0x108, 4, 7);
        // Pass 2: the 4x2 preview to 2x1.
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        for (o, v) in [
            (0x14, 1 << 16 | 3),
            (0x1c, 1 << 16 | 3),
            (0x20, 1),
            (0xd0, RAM as u32 + 0x1000),
            (0xdc, 16),
            (0xe4, RAM as u32 + 0x1800),
            (0xf0, 4),
            (0x04, 1),
        ] {
            rzl2.write(&mut cx, base + o, 4, u64::from(v));
        }
        assert!(cx.refusal().is_none());
        assert_eq!(&host.ram[0x1800..0x1804], [0, 4, 0x80, 0x40]);
        // A zero source address refuses, although a RIFP output exists.
        let mut rzl2 = Resz::with_layout(
            &RZL2,
            base,
            CompletionStub::new(base, 0x1000, toml::from_str("starts = [{ offset = 0x04, mask = 0x1 }]\ndelay_us = 200").unwrap(), 1000).unwrap(),
            Some(YcOutput::default()),
        );
        rzl2.rifp.as_ref().unwrap().set(Some(YcPicture {
            address: RAM,
            pitch: 20,
            width: 8,
            lines: 4,
        }));
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        for (o, v) in [(0x14, 3 << 16 | 7), (0x20, 3 << 16 | 7), (0xe4, RAM as u32 + 0x1000), (0xf0, 16), (0x04, 1)] {
            rzl2.write(&mut cx, base + o, 4, u64::from(v));
        }
        assert!(cx.refusal().is_some());
    }

    /// Colour bars through the X-T5 4.31 preview and playback scalers (run45 words): RZL2
    /// decimates the picture by 4 (+0x24 / +0x28 = 0x21000) and crops the whole decimated
    /// picture to the 640x480 preview; RESZ scales its 640x426 letterbox band to the 960x640
    /// panel. Reading the crop in source pixels showed only the first quarter of each axis
    /// (2 of the 8 bars, no lower part).
    #[test]
    fn decimated_crop_keeps_every_colour_bar_from_capture_to_panel() {
        const BARS: usize = 8;
        let bar = |b: usize| [16 + 28 * b as u8, 0x80 + b as u8, 0x40 + b as u8];
        let (width, lines) = (2560, 1920);
        let (preview, panel) = (RAM + 0xa0_0000, RAM + 0xc0_0000);
        let mut ram = vec![0u8; 0xe0_0000];
        for y in 0..lines {
            for pair in 0..width / 2 {
                // The lower quarter is a ramp: Y follows the line.
                let [luma, cb, cr] = if y < lines * 3 / 4 {
                    bar(pair * 2 * BARS / width)
                } else {
                    [(y / 8) as u8, 0x80, 0x80]
                };
                ram[y * width * 2 + pair * 4..][..4].copy_from_slice(&[luma, luma, cb, cr]);
            }
        }
        let mut host = RecordingHost {
            ram_base: RAM,
            ram,
            ..Default::default()
        };
        let stub = |irq: u32| {
            let options = toml::from_str(&format!(
                "irq = {irq}\nstarts = [{{ offset = 0x04, mask = 0x1 }}]\ndelay_us = 200"
            ))
            .unwrap();
            move |base| CompletionStub::new(base, 0x1000, options, 1000).unwrap()
        };
        let base = 0xff984000;
        let mut rzl2 = Resz::with_layout(&RZL2, base, stub(205)(base), Some(YcOutput::default()));
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        for (o, v) in [
            (0x14, 1919 << 16 | 2559),
            (0x1c, 479 << 16 | 639),
            (0x20, 479 << 16 | 639),
            (0x24, 0x21000),
            (0x28, 0x21000),
            (0xd0, RAM as u32),
            (0xdc, 2560 * 2),
            (0xe4, preview as u32),
            (0xf0, 0x500),
            (0x04, 1),
        ] {
            rzl2.write(&mut cx, base + o, 4, u64::from(v));
        }
        assert!(cx.refusal().is_none(), "{:?}", cx.refusal());
        let mut resz = Resz::new(0xffd0c000, stub(203)(0xffd0c000), Some(YcOutput::default()));
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        for (o, v) in [
            (0x14, 431 << 16 | 639),
            (0x1c, 425 << 16 | 639),
            (0x20, 639 << 16 | 959),
            (0x24, 2729),
            (0x28, 2724),
            (0xd4, (preview + 27 * 0x500) as u32),
            (0xe0, 0x500),
            (0xe8, panel as u32),
            (0xf4, 0x780),
            (0x04, 1),
        ] {
            resz.write(&mut cx, 0xffd0c000 + o, 4, u64::from(v));
        }
        assert!(cx.refusal().is_none(), "{:?}", cx.refusal());
        let pixel = |x: usize, y: usize| {
            let at = (panel - RAM) as usize + y * 0x780 + x / 2 * 4;
            let p = &host.ram[at..at + 4];
            [p[x % 2], p[2], p[3]]
        };
        // Preview lines 27..453 are source lines 108..1812: bars down to panel line ~531,
        // the ramp below.
        for b in 0..BARS {
            assert_eq!(pixel(b * 120 + 60, 100), bar(b), "bar {b}");
            assert_eq!(pixel(b * 120 + 60, 500), bar(b), "bar {b} low");
        }
        let ramp = pixel(480, 600)[0];
        assert!(usize::from(ramp) >= 1440 / 8 && pixel(480, 639)[0] > ramp, "ramp {ramp}");
        // An undecoded shift (3) refuses.
        let mut rzl2 = Resz::with_layout(&RZL2, base, stub(205)(base), Some(YcOutput::default()));
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        for (o, v) in [
            (0x14, 1919 << 16 | 2559),
            (0x20, 479 << 16 | 639),
            (0x24, 0x31000),
            (0xd0, RAM as u32),
            (0xdc, 2560 * 2),
            (0xe4, preview as u32),
            (0xf0, 0x500),
            (0x04, 1),
        ] {
            rzl2.write(&mut cx, base + o, 4, u64::from(v));
        }
        let refusal = cx.refusal().unwrap();
        assert!(refusal.contains("undecoded step words"), "{refusal}");
    }

    fn resz_with(picture: YcPicture) -> Resz {
        let resz = resz();
        resz.rifp.as_ref().unwrap().set(Some(picture));
        resz
    }
}
