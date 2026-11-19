//! Modelled movie data path of the X-T5 4.31 (opt-in, `XT5_MOVIE_PATH=real`): the frame that
//! the MOVDC movie channel develops reaches the H.264 encoder input ("aEncYC") through
//! RIFP (inline YC) -> RESZ (two-plane 4:2:0) -> TNR.
//!
//! # Where the encoder input comes from
//!
//! Static (recmvbk op worker 0x2db3ce4, op programs 0x333d0d8, op jump table 0x333c690):
//!
//! * op 6 (msg 0x12b6, 0x2db4df8): `0x2db341c(rec, 6, job)` picks the destination, an aEncYC
//!   buffer (type 0x16) when job+0x28 is set, otherwise a type 0x15 YC buffer, and moves its
//!   chroma plane to the luma plane's address + 0x800 modulo 0x1000. It then runs the RECC
//!   route 0x2d1e7f0(rec, job+0x78 source, destination, ...): 0x2d13118 builds the RIFP and
//!   RCIZ parameters and the recf queue 0x2d3a980 programs RIFP 0xffd02000 and RESZ 0xffd0c000
//!   (queue mode, destination words only) through the register-list DMA 0xffd01000.
//! * op 10 (msg 0x129a, 0x2db522c, "recmvbkTimeNoiseReduction"): `0x2db341c(rec, 10, job)`
//!   allocates the aEncYC destination (type 0x16, or 0x19 when rec+0x84e0 == 1) and calls
//!   0x2d138e8(rec, reference = job+0x58 or the current picture, current = job+0x78,
//!   destination, 0, mode, ...). That fills the TNR parameter block 0x3d86c68: +0x38 / +0x40
//!   current Y / C, +0x48 / +0x50 reference Y / C, +0x68 / +0x70 destination Y / C, pitches
//!   +0x14 / +0x18, +0x1c / +0x20, +0x2c / +0x30, u16 lines +0x34 and u16 pixels +0x36 (half
//!   the descriptor's +0x24 line bytes), and 0x2d3e1cc starts TNR. The job's current picture
//!   becomes the destination; op 0x16 (0x12a3, 1F_ENC_START) encodes it.
//! * TNR driver 0x2c73414..0x2c757f0: start 0x2c74bcc writes eight address pairs {lo, hi
//!   bits 2..0} +0x20/+0x24 .. +0x58/+0x5c, then +0x30c = 1 (IRQ enable) and +0x04 = 1;
//!   set 0x2c74f68 writes +0x08 (mode), the eight 17-bit pitches +0x60..+0x7c and +0x80 =
//!   lines << 16 | pixels (0x2c73a28).
//!
//! Run93 (FHD, XT5_MOVIE_PATH=real, 150 frames) end state, one RIFP, RESZ and TNR run per
//! frame, MOVDC channel 2 151 frames:
//!
//! | stage | words |
//! |---|---|
//! | MOVDC 0xff972000 | +0x08 0x101 (two-plane 4:2:0), Y 0x9df80000, C 0x1_cfc54000, pitch 0xc00, 1920x1080 |
//! | RIFP 0xffd02000 | +0x08 0x10006012 (bits 1..0 = 2), input Y +0x0c 0x9d9a7000, C +0x14/+0x18 0x1_cf67b000, pitch +0x1c 0xc00, +0x20 0x045a0780, +0x28 0x77f, +0x2c 0x437, output +0x40 0xa5401000 / +0x48 0x1_d70d5800, +0x134..+0x13c 0x05000000 |
//! | RESZ 0xffd0c000 | +0x10 0x10000121, 1920x1080 -> 1920x1080, source words 0 (inline), destination 0xa5401000 / 0x1_d70d5800 pitch 0x1600 |
//! | TNR 0xffd24000 | +0x08 0x110011, current 0xa5401000 / 0x1_d70d5800, reference 0x6f7f3000 / 0x6fdcc000, output 0x703a5000 / 0x7097e000, pitches 0x1600, +0x80 0x04380780 |
//! | H.264 0xffd15000 | IMG_WADR Y 0x703a5000, C 0x7097e000, IMG_LINE_OFS 0x1600 |
//!
//! So TNR writes the encoder input, from the RESZ output, which RESZ scales from the RIFP
//! inline stream of the MOVDC picture (the RIFP input pitch and planes are the MOVDC ones; the
//! RIFP input is the MOVDC frame before the last).
//!
//! # Model
//!
//! * RIFP ([`rifp_inline_picture`]): a queued run with mode 2 (+0x08 bits 1..0) and an input
//!   second plane is a two-plane 4:2:0 YC input. At its start the picture of
//!   (+0x28 + 1) x (+0x2c + 1) pixels from crop origin +0x24 of the input planes becomes the
//!   inline picture for RESZ. RIFP itself writes nothing: its output words equal the RESZ
//!   destination, and the RESZ pitch (0x1600) differs from the RIFP +0x50 (0xc00), so the
//!   RIFP output is taken to be the inline link (+0x134..+0x13c) only. The colour processing
//!   is not applied (the MOVDC model already writes the scene as YC).
//! * RESZ ([`super::still::ReszJob::scale_two_plane`]): see [`super::still`].
//! * TNR ([`Tnr`]): copies the current picture to the output; the reference is not blended
//!   (no temporal filter).
//!
//! INFERRED, not decoded: the +0x28 / +0x2c minus-one sizes of RIFP mode 2 (the still runs
//! also program width - 1 / height - 1 there), the RESZ class meaning and the TNR register
//! roles (from the parameter block order and the run93 addresses).
use super::still::YcPicture;
use fujimod_emulator_devices::checkpoint::Checkpoint;
use fujimod_emulator_devices::{Cx, Device};
use fujimod_emulator_xp5_io::completion_stub::CompletionStub;
use serde_json::Value;

/// RIFP: the two-plane 4:2:0 inline picture of a movie run (see the module notes) from the
/// RIFP words `word`, `None` when the run is not a mode 2 run with an input second plane.
pub fn rifp_inline_picture(word: impl Fn(u64) -> u32) -> Option<Result<YcPicture, String>> {
    let address = |low: u64| u64::from(word(low)) | (u64::from(word(low + 4) & 7) << 32);
    let (luma, chroma) = (address(0x0c), address(0x14));
    if word(0x08) & 3 != 2 || chroma == 0 {
        return None;
    }
    let pitch = u64::from(word(0x1c));
    let input = (u64::from(word(0x20) & 0xffff), u64::from(word(0x20) >> 16));
    let origin = (u64::from(word(0x24) & 0xffff), u64::from(word(0x24) >> 16));
    let size = (u64::from(word(0x28) & 0xffff) + 1, u64::from(word(0x2c) & 0xffff) + 1);
    let geometry = || {
        format!(
            "RIFP movie run: input {luma:#x} / {chroma:#x} pitch {pitch:#x} (+0x20 {:#x}), crop \
             {size:?} at {origin:?} (+0x08 mode {:#x})",
            word(0x20),
            word(0x08)
        )
    };
    if luma == 0
        || origin.0 % 2 != 0
        || origin.1 % 2 != 0
        || size.0 % 2 != 0
        || origin.0 + size.0 > input.0
        || origin.1 + size.1 > input.1
        || pitch < origin.0 + size.0
    {
        return Some(Err(format!("{} is outside the modelled layout", geometry())));
    }
    Some(Ok(YcPicture {
        address: luma + origin.1 * pitch + origin.0,
        pitch,
        width: size.0 as u32,
        lines: size.1 as u32,
        chroma: Some(chroma + origin.1 / 2 * pitch + origin.0),
    }))
}

/// TNR 0xffd24000 (kind `imaging-tnr`): the completion stub of 57-develop-stubs-3.toml
/// (same options) that, with the movie path ([`Tnr::with_movie_path`]), copies the current
/// two-plane 4:2:0 picture to the output at each start (see the module notes). Registers:
///
/// * +0x20 / +0x24 and +0x28 / +0x2c: current Y and C (address bits 31..0 / 34..32);
/// * +0x30 / +0x34 and +0x38 / +0x3c: reference Y and C (the previous output; not read);
/// * +0x50 / +0x54 and +0x58 / +0x5c: output Y and C;
/// * +0x60 / +0x64 current, +0x68 / +0x6c reference, +0x78 / +0x7c output Y / C pitches;
/// * +0x80: lines << 16 | pixels.
///
/// The checkpoint is the stub's.
pub struct Tnr {
    stub: CompletionStub,
    copy: bool,
}

impl Tnr {
    pub fn new(stub: CompletionStub) -> Self {
        Self { stub, copy: false }
    }
    pub fn with_movie_path(mut self) -> Self {
        self.copy = true;
        self
    }
    fn copy(&self, cx: &mut Cx<'_>) -> Result<String, String> {
        let word = |o: u64| self.stub.word(o);
        let address = |low: u64| u64::from(word(low)) | (u64::from(word(low + 4) & 7) << 32);
        let pitch = |o: u64| u64::from(word(o) & 0x1ffff);
        let (luma, chroma) = (address(0x20), address(0x28));
        let (out_luma, out_chroma) = (address(0x50), address(0x58));
        let (width, lines) = (u64::from(word(0x80) & 0xffff), u64::from(word(0x80) >> 16));
        let pitches = [pitch(0x60), pitch(0x64), pitch(0x78), pitch(0x7c)];
        let geometry = format!(
            "TNR: {width}x{lines} current {luma:#x} / {chroma:#x} -> output {out_luma:#x} / \
             {out_chroma:#x}, pitches {:#x} {:#x} -> {:#x} {:#x} (+0x08 mode {:#x}; reference \
             {:#x} / {:#x} not blended)",
            pitches[0],
            pitches[1],
            pitches[2],
            pitches[3],
            word(0x08),
            address(0x30),
            address(0x38)
        );
        if [luma, chroma, out_luma, out_chroma].contains(&0)
            || width == 0
            || width % 2 != 0
            || lines == 0
            || pitches.iter().any(|&p| p < width)
        {
            return Err(format!("{geometry} is outside the modelled layout"));
        }
        let planes = [
            (luma, pitches[0], out_luma, pitches[2], lines),
            (chroma, pitches[1], out_chroma, pitches[3], lines.div_ceil(2)),
        ];
        for (source, source_pitch, output, output_pitch, rows) in planes {
            let bytes = cx.ram_bytes(source, (source_pitch * (rows - 1) + width) as usize)?;
            for y in 0..rows {
                let at = (y * source_pitch) as usize;
                cx.ram_write(output + y * output_pitch, &bytes[at..at + width as usize])?;
            }
        }
        Ok(geometry)
    }
}

impl Device for Tnr {
    fn name(&self) -> &'static str {
        "imaging-tnr"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        self.stub.read(cx, address, size)
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        // As for RESZ: a start while a completion is due is ignored by the stub, and here.
        let start = self.copy
            && address & 0xfff == 4
            && size == 4
            && value & 1 != 0
            && self.stub.next_deadline(cx.now()).is_none();
        self.stub.write(cx, address, size, value);
        if start && self.stub.next_deadline(cx.now()).is_some() {
            match self.copy(cx) {
                Ok(line) => cx.log(&line),
                Err(e) => cx.log(&format!("{e}; nothing written")),
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

    #[test]
    fn a_mode_2_run_with_a_second_plane_is_the_inline_picture() {
        let words = |o: u64| match o {
            0x08 => 0x1000_6012,
            0x0c => 0x9d9a_7000,
            0x14 => 0xcf67_b000,
            0x18 => 1,
            0x1c => 0xc00,
            0x20 => 0x045a_0780,
            0x28 => 0x77f,
            0x2c => 0x437,
            _ => 0,
        };
        assert_eq!(
            rifp_inline_picture(words).unwrap().unwrap(),
            YcPicture {
                address: 0x9d9a_7000,
                pitch: 0xc00,
                width: 1920,
                lines: 1080,
                chroma: Some(0x1_cf67_b000),
            }
        );
        // The still ConvertToYC mode (bits 1..0 = 0) and a run without a second plane are not.
        assert!(rifp_inline_picture(|o| if o == 0x08 { 0x1122_0000 } else { words(o) }).is_none());
        assert!(rifp_inline_picture(|o| if o == 0x14 || o == 0x18 { 0 } else { words(o) }).is_none());
        // A crop beyond the input refuses.
        assert!(rifp_inline_picture(|o| if o == 0x2c { 0x45a } else { words(o) }).unwrap().is_err());
    }

    fn tnr_stub() -> CompletionStub {
        let options: fujimod_emulator_xp5_io::completion_stub::Options = toml::from_str(
            "registers = [{ offset = 0x000, mode = \"constant\", value = 0x1 }]\n\
             irq = 218\n\
             starts = [{ offset = 0x04, mask = 0x1 }]\n\
             delay_us = 200\n\
             completion = [{ offset = 0x314, set = 0x1 }, { offset = 0x04, clear = 0x1 }]\n\
             ack = { offset = 0x310, mode = \"w1c\", status_offset = 0x314, bits = 0x1 }\n\
             irq_mask_offset = 0x30c\n\
             irq_mask_bit = 0\n\
             irq_mask_set_masks = false\n",
        )
        .unwrap();
        CompletionStub::new(0xffd2_4000, 0x1000, options, 1000).unwrap()
    }

    /// TNR copies the current Y and C planes (4 x 4, pitch 8) to the output (pitch 16) and
    /// leaves the bytes past each line alone; without the movie path it writes nothing.
    #[test]
    fn tnr_copies_the_current_picture_to_the_output() {
        let mut ram = vec![0xeeu8; 0x1000];
        for (i, byte) in ram[..32].iter_mut().enumerate() {
            *byte = i as u8;
        }
        for (i, byte) in ram[0x100..0x110].iter_mut().enumerate() {
            *byte = 0x80 | i as u8;
        }
        let mut host = RecordingHost {
            ram_base: RAM,
            ram,
            ..Default::default()
        };
        let program = |tnr: &mut Tnr, host: &mut RecordingHost| {
            let mut cx = Cx::new(host, 0, 0, 0);
            for (offset, value) in [
                (0x20, RAM),
                (0x28, RAM + 0x100),
                (0x50, RAM + 0x400),
                (0x58, RAM + 0x800),
                (0x60, 8),
                (0x64, 8),
                (0x78, 16),
                (0x7c, 16),
                (0x80, 4 << 16 | 4),
                (0x04, 1),
            ] {
                tnr.write(&mut cx, 0xffd2_4000 + offset, 4, value & 0xffff_ffff);
            }
        };
        program(&mut Tnr::new(tnr_stub()), &mut host);
        assert!(host.ram[0x400..0xa00].iter().all(|&b| b == 0xee));
        program(&mut Tnr::new(tnr_stub()).with_movie_path(), &mut host);
        for y in 0..4 {
            let at = 0x400 + y * 16;
            assert_eq!(host.ram[at..at + 4], host.ram[y * 8..y * 8 + 4]);
            assert_eq!(host.ram[at + 4], 0xee);
        }
        for y in 0..2 {
            let at = 0x800 + y * 16;
            assert_eq!(host.ram[at..at + 4], host.ram[0x100 + y * 8..0x100 + y * 8 + 4]);
        }
        assert_eq!(host.ram[0x820], 0xee);
    }
}
