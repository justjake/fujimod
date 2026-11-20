//! MIX 0xffd22000 (X-T5 4.31, logical IRQ 0x19 = INTID 213) in the HDR still: the block that
//! writes the HDR composite that the last RIFP run develops into the JPEG input.
//!
//! # Evidence
//!
//! Driver 0x2c46c20..0x2c494c4 (see 57-develop-stubs-3.toml for open, start, ISR and stop). The
//! address setter 0x2c47184 copies six groups of the parameter block into the registers:
//!
//! | group | parameter block | registers |
//! |---|---|---|
//! | input 0 | +0x08 / +0x0c, +0x10 / +0x14, pitch +0x50 | +0x0c / +0x10, +0x14 / +0x18, pitch +0x1c |
//! | input 1 | +0x18 / +0x1c, +0x20 / +0x24, pitch +0x54 | +0x20 / +0x24, +0x28 / +0x2c, pitch +0x30 |
//! | input 2 | +0x28 / +0x2c, +0x30 / +0x34, pitch +0x58 | +0x34 / +0x38, +0x3c / +0x40, pitch +0x44 |
//! | input 3 | +0x38 / +0x3c, +0x40 / +0x44, pitch +0x5c | +0x48 / +0x4c, +0x50 / +0x54, pitch +0x58 |
//! | map | +0x48 / +0x4c, pitch +0x60, u16 lines +0x7c, u16 pixels +0x7e | +0x5c / +0x60, pitch +0x64, size +0x84 |
//! | output | +0x68 / +0x6c, +0x70 / +0x74, pitch +0x78 | +0x70 / +0x74, +0x78 / +0x7c, pitch +0x80 |
//!
//! Each address is a pair {bits 31..0, bits 34..32}; the second pair of a group is the second
//! plane (chroma) of a two-plane picture and is 0 for a packed one. The size word is
//! lines << 16 | pixels. The mode word +0x08 selects one of six parameter groups (0x2c480c4);
//! its bit meanings are not decoded.
//!
//! X-T5 4.31 HDR run (drive dial HDR, `--watch-writes` on the MIX page, scratch/up/runw.out),
//! all pictures packed 4:2:2 (second planes 0), 7728x5152:
//!
//! | step | +0x08 | input 0 | input 1 | input 2 | map | output |
//! |---|---|---|---|---|---|---|
//! | 0x64..0x69 (3 passes, each after a MAP run) | 0x3100020 | frame, pitch 0x3c80 | frame, 0x3c80 | - | MAP output, 0x1e40 | 8-bit map, 0x1e40 |
//! | (same passes) | 0x20 | the same two frames | | - | the map just written | frame, 0x3c80 |
//! | 0x6b HdrComposite | 0x32 | pass 3 output | pass 1 output | a RESZ output | HDRCR map 0x1_c0c00000, 0x1e40 | 0x1_c34f5880, 0x3d60 |
//!
//! The HdrComposite output is 64 lines and 64 pixels into a 0x3d60-pitch buffer at
//! 0x1_c3400000; the final RIFP run (+0x08 0x10002002) reads it from there.
//! The single still (drive dial S) runs MIX once with mode 0x25, two-plane input 0 = output
//! (in place); that mode is not modelled here and writes nothing, as before.
//!
//! # Live view with the drive dial at ADV (advanced filter)
//!
//! Each live-view frame runs MIX once and then the advanced-filter scaler 0xff983000 (see
//! `imaging_still.rs`), in place on the panel picture (strict run from a drive dial S start,
//! `--set drive=ADV`, end state of the MIX and scaler pages):
//!
//! | +0x08 | size +0x84 | input 0 | input 1 | map | output |
//! |---|---|---|---|---|---|
//! | 0x2000022 | 968x640 | panel picture 0x20307000, pitch 0xa00 | 0x2a4c2000, pitch 0x780 | 0x2a71a000, pitch 0x500 | 0x20a0f000, pitch 0xa00 |
//!
//! The scaler then reads 960x640 at 0x20a0f000 and writes the panel picture 0x20307000 again.
//! Input 1 is narrower than the picture (pitch 0x780 = 960 pixels), so it is not a second
//! frame of the same layout; what it and the map hold is not decoded.
//!
//! # Model (GUESSES, marked as such)
//!
//! * mode 0x20 (motion-corrected blend of two frames by a map): GUESS: the output is input 0
//!   (the frames are taken to be already aligned and the scene static, so the blend of the
//!   two is the first one). The map is not read.
//! * mode 0x32 (HDR composite of three frames by the HDRCR map): GUESS: the output is the
//!   per-byte average of inputs 0, 1 and 2 (Y, Cb and Cr alike, rounded). The tone mapping
//!   and the per-pixel weights of the map are not modelled.
//! * mode 0x2000022 (the ADV live-view blend, with or without the still path): GUESS: the
//!   output is input 0, as for mode 0x20. Input 1 and the map are not read, so the filter's
//!   effect on the preview is not modelled; the panel picture passes through unchanged.
//! * mode 0x3100020 (an 8-bit blend map from two frames and the MAP motion map): nothing is
//!   written (the only reader the model has, mode 0x20, ignores the map).
//! * other modes: nothing is written, as with the plain stub.
//!
//! A modelled mode with a second plane, a zero address, an odd or zero size, a pitch shorter
//! than a line, or (mode 0x20) an input 2 or input 3 address refuses: the run stops.
use fujimod_emulator_devices::checkpoint::Checkpoint;
use fujimod_emulator_devices::{Cx, Device};
use fujimod_emulator_xp5_io::completion_stub::CompletionStub;
use serde_json::Value;

/// MIX base on the X-T5 4.31.
pub const MIX_BASE: u64 = 0xffd2_2000;

const MODE_BLEND_MAP: u32 = 0x0310_0020;
const MODE_BLEND: u32 = 0x20;
const MODE_HDR_COMPOSITE: u32 = 0x32;
/// The live-view blend of the drive dial's ADV position (module notes).
const MODE_FILTER_BLEND: u32 = 0x0200_0022;

/// One picture group: address, second-plane address, pitch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Group {
    address: u64,
    second: u64,
    pitch: u64,
}

/// What a start does.
#[derive(Debug, PartialEq, Eq)]
enum Job {
    /// Copy `pitch`-spaced rows of packed 4:2:2 pictures: the output is the rounded per-byte
    /// average of `inputs` (one input is a copy).
    Average {
        inputs: Vec<Group>,
        output: Group,
        width: u64,
        lines: u64,
    },
    /// Nothing is written; the line says why.
    Nothing(String),
}

/// MIX 0xffd22000: the completion stub of 57-develop-stubs-3.toml (same options) that, with
/// the still path ([`Mix::with_still_path`]), writes the HDR modes at each start, and always
/// the ADV live-view blend (see the module notes). The device name and checkpoint are the stub's, so snapshots taken with the
/// plain stub stay valid.
pub struct Mix {
    stub: CompletionStub,
    model: bool,
}

impl Mix {
    pub fn new(stub: CompletionStub) -> Self {
        Self { stub, model: false }
    }
    pub fn with_still_path(mut self) -> Self {
        self.model = true;
        self
    }

    fn group(&self, offset: u64, pitch: u64) -> Result<Group, String> {
        Ok(Group {
            address: self.address(offset)?,
            second: self.address(offset + 8)?,
            pitch: u64::from(self.stub.word(pitch)),
        })
    }
    fn address(&self, low: u64) -> Result<u64, String> {
        let high = self.stub.word(low + 4);
        if high > 7 {
            return Err(format!("MIX: address high word +{:#x} = {high:#x} is not bits 34..32", low + 4));
        }
        Ok(u64::from(self.stub.word(low)) | u64::from(high) << 32)
    }

    /// The job of the start programmed now.
    fn job(&self) -> Result<Job, String> {
        let mode = self.stub.word(0x08);
        let size = self.stub.word(0x84);
        let (width, lines) = (u64::from(size & 0xffff), u64::from(size >> 16));
        let inputs = [
            self.group(0x0c, 0x1c)?,
            self.group(0x20, 0x30)?,
            self.group(0x34, 0x44)?,
            self.group(0x48, 0x58)?,
        ];
        let map = Group {
            address: self.address(0x5c)?,
            second: 0,
            pitch: u64::from(self.stub.word(0x64)),
        };
        let output = self.group(0x70, 0x80)?;
        if mode == MODE_FILTER_BLEND {
            let row = width * 2;
            let geometry = format!(
                "MIX mode {mode:#x} {width}x{lines}: input {:#x} pitch {:#x} (input 1 {:#x} pitch {:#x}, \
                 map {:#x} pitch {:#x} not read) -> output {:#x} pitch {:#x}",
                inputs[0].address,
                inputs[0].pitch,
                inputs[1].address,
                inputs[1].pitch,
                map.address,
                map.pitch,
                output.address,
                output.pitch
            );
            let bad = width == 0
                || width % 2 != 0
                || lines == 0
                || output.address == 0
                || output.second != 0
                || output.pitch < row
                || inputs[0].address == 0
                || inputs[0].pitch < row
                || inputs.iter().any(|g| g.second != 0)
                || inputs[2..].iter().any(|g| g.address != 0);
            if bad {
                return Err(format!("{geometry} is outside the decoded packed 4:2:2 layout"));
            }
            // GUESS (module notes): the blend is input 0.
            return Ok(Job::Average {
                inputs: inputs[..1].to_vec(),
                output,
                width,
                lines,
            });
        }
        let used = match mode {
            MODE_BLEND => 2,
            MODE_HDR_COMPOSITE => 3,
            MODE_BLEND_MAP => {
                return Ok(Job::Nothing(format!(
                    "MIX mode {mode:#x} (blend map) {width}x{lines}: inputs {:#x} / {:#x}, map {:#x} -> \
                     map {:#x} pitch {:#x} not modelled; nothing written",
                    inputs[0].address, inputs[1].address, map.address, output.address, output.pitch
                )));
            }
            _ => {
                return Ok(Job::Nothing(format!(
                    "MIX mode {mode:#x} {width}x{lines}: output {:#x} not modelled; nothing written",
                    output.address
                )));
            }
        };
        let geometry = format!(
            "MIX mode {mode:#x} {width}x{lines}: inputs {} map {:#x} pitch {:#x} -> output {:#x} pitch {:#x}",
            inputs[..used]
                .iter()
                .map(|g| format!("{:#x} pitch {:#x}", g.address, g.pitch))
                .collect::<Vec<_>>()
                .join(", "),
            map.address,
            map.pitch,
            output.address,
            output.pitch
        );
        let row = width * 2;
        let bad = width == 0
            || width % 2 != 0
            || lines == 0
            || output.address == 0
            || output.second != 0
            || output.pitch < row
            || inputs[..used]
                .iter()
                .any(|g| g.address == 0 || g.second != 0 || g.pitch < row)
            || inputs[used..].iter().any(|g| g.address != 0 || g.second != 0);
        if bad {
            return Err(format!("{geometry} is outside the decoded packed 4:2:2 layout"));
        }
        // GUESS (module notes): the blend is input 0, the composite the average of all three.
        let averaged = if mode == MODE_BLEND { 1 } else { used };
        Ok(Job::Average {
            inputs: inputs[..averaged].to_vec(),
            output,
            width,
            lines,
        })
    }

    fn run(&self, cx: &mut Cx<'_>) -> Result<String, String> {
        let (inputs, output, width, lines) = match self.job()? {
            Job::Nothing(line) => return Ok(line),
            Job::Average {
                inputs,
                output,
                width,
                lines,
            } => (inputs, output, width, lines),
        };
        let row = (width * 2) as usize;
        let sources = inputs
            .iter()
            .map(|g| cx.ram_bytes(g.address, (g.pitch * (lines - 1)) as usize + row))
            .collect::<Result<Vec<_>, _>>()?;
        let count = sources.len() as u32;
        let mut line = vec![0u8; row];
        for y in 0..lines {
            for (x, byte) in line.iter_mut().enumerate() {
                let sum: u32 = sources
                    .iter()
                    .zip(&inputs)
                    .map(|(bytes, g)| u32::from(bytes[(y * g.pitch) as usize + x]))
                    .sum();
                *byte = ((sum + count / 2) / count) as u8;
            }
            cx.ram_write(output.address + y * output.pitch, &line)?;
        }
        Ok(format!(
            "MIX mode {:#x}: {} {width}x{lines} packed 4:2:2 from {} -> {:#x} pitch {:#x} (guess: {})",
            self.stub.word(0x08),
            if count == 1 { "copies" } else { "averages" },
            inputs
                .iter()
                .map(|g| format!("{:#x} pitch {:#x}", g.address, g.pitch))
                .collect::<Vec<_>>()
                .join(", "),
            output.address,
            output.pitch,
            if self.stub.word(0x08) == MODE_FILTER_BLEND {
                "advanced-filter preview as input 0; input 1 and map not read"
            } else if count == 1 {
                "aligned static frames, the blend is input 0; map not read"
            } else {
                "HDR composite as the plain average; HDRCR map not read"
            }
        ))
    }
}

impl Device for Mix {
    fn name(&self) -> &'static str {
        self.stub.name()
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        self.stub.read(cx, address, size)
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        // As for TNR and RESZ: a start while a completion is due is ignored by the stub, and here.
        // The ADV live-view blend runs with or without the still path.
        let start = (self.model || self.stub.word(0x08) == MODE_FILTER_BLEND)
            && address & 0xfff == 4
            && size == 4
            && value & 1 != 0
            && self.stub.next_deadline(cx.now()).is_none();
        self.stub.write(cx, address, size, value);
        if start && self.stub.next_deadline(cx.now()).is_some() {
            match self.run(cx) {
                Ok(line) => cx.log(&line),
                Err(e) => cx.fail(format!("fujimod-emulator: MIX {e}")),
            }
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

    fn mix_stub() -> CompletionStub {
        let options: fujimod_emulator_xp5_io::completion_stub::Options = toml::from_str(
            "registers = [\n\
               { offset = 0x000, mode = \"constant\", value = 0x1 },\n\
               { offset = 0x208, mode = \"constant\", value = 0x10001 },\n\
             ]\n\
             irq = 213\n\
             starts = [{ offset = 0x04, mask = 0x1 }]\n\
             delay_us = 200\n\
             completion = [{ offset = 0x210, set = 0x1 }, { offset = 0x04, clear = 0x1 }]\n\
             ack = { offset = 0x210, mode = \"w1c\" }\n\
             irq_mask_offset = 0x20c\n\
             irq_mask_bit = 0\n\
             irq_mask_set_masks = false\n",
        )
        .unwrap();
        CompletionStub::new(MIX_BASE, 0x1000, options, 1000).unwrap()
    }

    fn host() -> RecordingHost {
        let mut ram = vec![0xeeu8; 0x2000];
        // Three 4x2 packed 4:2:2 inputs (8 bytes a line), pitch 16, at 0x000, 0x100 and 0x200.
        for (input, base) in [(0u8, 0x000), (1, 0x100), (2, 0x200)] {
            for y in 0..2 {
                for x in 0..8 {
                    ram[base + y * 16 + x] = 30 * input + 10 * y as u8 + x as u8;
                }
            }
        }
        RecordingHost {
            ram_base: RAM,
            ram,
            ..Default::default()
        }
    }

    /// Program `words` (offset, value), then start; returns the refusal.
    fn program(mix: &mut Mix, host: &mut RecordingHost, words: &[(u64, u64)]) -> Option<String> {
        let mut cx = Cx::new(host, 0, 0, 0);
        for &(offset, value) in words {
            mix.write(&mut cx, MIX_BASE + offset, 4, value);
        }
        mix.write(&mut cx, MIX_BASE + 4, 4, 1);
        cx.refusal().map(str::to_owned)
    }

    fn composite_words() -> Vec<(u64, u64)> {
        vec![
            (0x08, 0x32),
            (0x0c, RAM),
            (0x1c, 16),
            (0x20, RAM + 0x100),
            (0x30, 16),
            (0x34, RAM + 0x200),
            (0x44, 16),
            (0x5c, RAM + 0x800),
            (0x64, 8),
            (0x84, 2 << 16 | 4),
            (0x70, RAM + 0x1000),
            (0x80, 32),
        ]
    }

    /// The ADV live-view blend (mode 0x2000022) copies input 0 to the output without the still
    /// path too, although input 1 is narrower than the picture; input 1 and the map are not
    /// read. A second plane refuses.
    #[test]
    fn the_filter_blend_copies_input_0_without_the_still_path() {
        let words = vec![
            (0x08, 0x0200_0022),
            (0x0c, RAM),
            (0x1c, 16),
            // Pitch 6 is less than a 4-pixel line (8 bytes).
            (0x20, RAM + 0x100),
            (0x30, 6),
            (0x5c, RAM + 0x800),
            (0x64, 4),
            (0x84, 2 << 16 | 4),
            (0x70, RAM + 0x1000),
            (0x80, 32),
        ];
        let mut host = host();
        assert_eq!(program(&mut Mix::new(mix_stub()), &mut host, &words), None);
        for y in 0..2 {
            for x in 0..8 {
                assert_eq!(host.ram[0x1000 + y * 32 + x], 10 * y as u8 + x as u8);
            }
            assert_eq!(host.ram[0x1000 + y * 32 + 8], 0xee);
        }
        let mut second_plane = words.clone();
        second_plane.push((0x78, RAM + 0x1800));
        assert!(program(&mut Mix::new(mix_stub()), &mut host, &second_plane).is_some());
    }

    /// The HDR composite (mode 0x32) writes the rounded average of the three inputs at the
    /// output pitch and leaves the bytes past each line alone; without the still path it
    /// writes nothing.
    #[test]
    fn hdr_composite_averages_three_inputs() {
        let mut host = host();
        assert_eq!(program(&mut Mix::new(mix_stub()), &mut host, &composite_words()), None);
        assert!(host.ram[0x1000..0x1040].iter().all(|&b| b == 0xee));
        let refusal = program(
            &mut Mix::new(mix_stub()).with_still_path(),
            &mut host,
            &composite_words(),
        );
        assert_eq!(refusal, None);
        for y in 0..2 {
            for x in 0..8 {
                // (v + (v + 30) + (v + 60)) / 3 = v + 30.
                assert_eq!(host.ram[0x1000 + y * 32 + x], 30 + 10 * y as u8 + x as u8);
            }
            assert_eq!(host.ram[0x1000 + y * 32 + 8], 0xee);
        }
    }

    /// The blend (mode 0x20) copies input 0 to the output.
    #[test]
    fn blend_copies_input_0() {
        let mut host = host();
        let words = [
            (0x08, 0x20),
            (0x0c, RAM + 0x100),
            (0x1c, 16),
            (0x20, RAM + 0x200),
            (0x30, 16),
            (0x5c, RAM + 0x800),
            (0x64, 8),
            (0x84, 2 << 16 | 4),
            (0x70, RAM + 0x1000),
            (0x80, 16),
        ];
        let mut mix = Mix::new(mix_stub()).with_still_path();
        assert_eq!(program(&mut mix, &mut host, &words), None);
        assert_eq!(host.ram[0x1000..0x1008], host.ram[0x100..0x108]);
        assert_eq!(host.ram[0x1010..0x1018], host.ram[0x110..0x118]);
        assert_eq!(host.ram[0x1008], 0xee);
    }

    /// The blend map (mode 0x3100020) and the single still's in-place two-plane mode 0x25
    /// write nothing and do not refuse.
    #[test]
    fn unmodelled_modes_write_nothing() {
        for mode in [0x0310_0020, 0x25] {
            let mut host = host();
            let mut words = composite_words();
            words[0] = (0x08, mode);
            words.push((0x14, RAM + 0x400));
            let mut mix = Mix::new(mix_stub()).with_still_path();
            assert_eq!(program(&mut mix, &mut host, &words), None);
            assert!(host.ram[0x1000..0x1040].iter().all(|&b| b == 0xee));
        }
    }

    /// Undecoded combinations of a modelled mode refuse and write nothing.
    #[test]
    fn undecoded_layouts_refuse() {
        let cases: [&[(u64, u64)]; 5] = [
            // A second plane.
            &[(0x14, RAM + 0x400)],
            // A pitch shorter than a line (4 pixels = 8 bytes).
            &[(0x30, 6)],
            // A zero input address.
            &[(0x34, 0)],
            // An odd width.
            &[(0x84, 2 << 16 | 3)],
            // An input 3 address in a three-input mode.
            &[(0x48, RAM + 0x300)],
        ];
        for extra in cases {
            let mut host = host();
            let mut words = composite_words();
            words.extend_from_slice(extra);
            let mut mix = Mix::new(mix_stub()).with_still_path();
            let refusal = program(&mut mix, &mut host, &words).expect("refuses");
            assert!(refusal.contains("MIX"), "{refusal}");
            assert!(host.ram[0x1000..0x1040].iter().all(|&b| b == 0xee));
        }
        // An address high word beyond bits 34..32 refuses too.
        let mut host = host();
        let mut words = composite_words();
        words.push((0x74, 8));
        let mut mix = Mix::new(mix_stub()).with_still_path();
        assert!(program(&mut mix, &mut host, &words).is_some());
    }
}
