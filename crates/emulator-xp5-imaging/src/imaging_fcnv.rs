//! FCNV colour-space converter of the X-T5 4.31 at 0xffd26000 (kind `imaging-fcnv`).
//!
//! Static decode (driver 0x13c0ef8..0x13c2fb8, state 0x2027370; INTID 217):
//!
//! * +0x08 mode: `A << 4 | B`. B 0/1 read YCbCr 4:2:2 (B 0: 2 bytes a pixel, the width a
//!   multiple of 8; B 1: 8 bytes for 6 pixels), B 2/3 read RGB. A 0 writes RGB888, A 1 four
//!   bytes a pixel (the fourth is +0x21c), A 2 48-bit RGB; A 3/4 are the YCbCr outputs.
//! * +0x20/+0x24, +0x28/+0x2c: input addresses 0 and 1 (bits 31..0 / 34..32); +0x30/+0x34,
//!   +0x38/+0x3c: output addresses 2 and 3. Pitches (19 bits): input +0x44 / +0x40, output
//!   +0x4c / +0x48. +0x50: lines << 16 | pixels.
//! * +0x204..+0x218: a 3 x 3 matrix of signed 13-bit Q10 coefficients, row by row (c0 / c1 in
//!   +0x204, c2 in +0x208, c3 / c4 in +0x20c, c5 in +0x210, c6 / c7 in +0x214, c8 in +0x218).
//!   For YCbCr input the columns are Y, Cb - 128, Cr - 128 and the rows R, G, B (table
//!   0x1c09228: 1, 0, 1.402 / 1, -0.344, -0.714 / 1, 1.772, 0).
//! * +0x004 bit 0 starts; the ISR 0x13c1af4 acknowledges with +0x310.
//!
//! The user reached in playback is the GUI image draw 0x147de3c (mode 0x10: YCbCr 4:2:2 to
//! four bytes a pixel, input addresses 0 and 1 equal, alpha 0xff), for the RAW CONVERSION
//! thumbnail. Modelled: B 0 with A 0 or 1. The byte orders (Y0 Cb Y1 Cr in, R G B [alpha]
//! out) are inferred, not traced; other modes are logged and write nothing. The completion,
//! interrupt and checkpoint are the completion stub's.
use fujimod_emulator_devices::checkpoint::Checkpoint;
use fujimod_emulator_devices::{Cx, Device};
use fujimod_emulator_xp5_io::completion_stub::CompletionStub;
use serde_json::Value;

pub struct Fcnv {
    stub: CompletionStub,
}

/// Signed 13-bit field of `word` at `shift`.
fn coefficient(word: u32, shift: u32) -> i32 {
    (((word >> shift) & 0x1fff) as i32) << 19 >> 19
}

impl Fcnv {
    pub fn new(stub: CompletionStub) -> Self {
        Self { stub }
    }
    fn matrix(&self) -> [i32; 9] {
        let w = |o: u64| self.stub.word(o);
        [
            coefficient(w(0x204), 0),
            coefficient(w(0x204), 16),
            coefficient(w(0x208), 0),
            coefficient(w(0x20c), 0),
            coefficient(w(0x20c), 16),
            coefficient(w(0x210), 0),
            coefficient(w(0x214), 0),
            coefficient(w(0x214), 16),
            coefficient(w(0x218), 0),
        ]
    }
    fn convert(&self, cx: &mut Cx<'_>) -> Result<String, String> {
        let word = |o: u64| self.stub.word(o);
        let address = |low: u64| u64::from(word(low)) | (u64::from(word(low + 4) & 7) << 32);
        let mode = word(0x08);
        let (a, b) = ((mode >> 4) & 7, mode & 3);
        let (width, lines) = (u64::from(word(0x50) & 0xffff), u64::from(word(0x50) >> 16));
        let (source, output) = (address(0x20), address(0x30));
        let (in_pitch, out_pitch) = (u64::from(word(0x44) & 0x7ffff), u64::from(word(0x4c) & 0x7ffff));
        let geometry = format!(
            "FCNV: mode {mode:#x} {width}x{lines} {source:#x} (pitch {in_pitch:#x}) -> {output:#x} (pitch {out_pitch:#x})"
        );
        if b != 0 || a > 1 {
            return Err(format!("{geometry}: only YCbCr 4:2:2 (B 0) to RGB888 / four bytes (A 0 / 1) is modelled"));
        }
        let out_bytes = if a == 0 { 3 } else { 4 };
        if source == 0 || output == 0 || width == 0 || width % 2 != 0 || lines == 0
            || in_pitch < width * 2 || out_pitch < width * out_bytes
        {
            return Err(format!("{geometry} is outside the modelled layout"));
        }
        let m = self.matrix();
        let alpha = (word(0x21c) & 0xff) as u8;
        let clip = |v: i32| ((v + 512) >> 10).clamp(0, 255) as u8;
        for y in 0..lines {
            let line = cx.ram_bytes(source + y * in_pitch, (width * 2) as usize)?;
            let mut out = Vec::with_capacity((width * out_bytes) as usize);
            for pair in line.chunks_exact(4) {
                let (cb, cr) = (pair[1] as i32 - 128, pair[3] as i32 - 128);
                for luma in [pair[0] as i32, pair[2] as i32] {
                    for row in 0..3 {
                        out.push(clip(m[3 * row] * luma + m[3 * row + 1] * cb + m[3 * row + 2] * cr));
                    }
                    if a == 1 {
                        out.push(alpha);
                    }
                }
            }
            cx.ram_write(output + y * out_pitch, &out)?;
        }
        Ok(geometry)
    }
}

impl Device for Fcnv {
    fn name(&self) -> &'static str {
        "imaging-fcnv"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        self.stub.read(cx, address, size)
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        // A start while a completion is due is ignored by the stub, and here.
        let start = address & 0xfff == 4 && size == 4 && value & 1 != 0 && self.stub.next_deadline(cx.now()).is_none();
        self.stub.write(cx, address, size, value);
        if start && self.stub.next_deadline(cx.now()).is_some() {
            match self.convert(cx) {
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

    fn stub() -> CompletionStub {
        let options: fujimod_emulator_xp5_io::completion_stub::Options = toml::from_str(
            "registers = [{ offset = 0x000, mode = \"constant\", value = 0x1 }, \
             { offset = 0x308, mode = \"constant\", value = 0x10001 }]\n\
             irq = 217\n\
             starts = [{ offset = 0x04, mask = 0x1 }]\n\
             delay_us = 200\n\
             completion = [{ offset = 0x310, set = 0x1 }, { offset = 0x04, clear = 0x1 }]\n\
             ack = { offset = 0x310, mode = \"w1c\" }\n\
             irq_mask_offset = 0x30c\n\
             irq_mask_bit = 0\n\
             irq_mask_set_masks = false\n",
        )
        .unwrap();
        CompletionStub::new(0xffd2_6000, 0x1000, options, 1000).unwrap()
    }

    /// The firmware's YCbCr -> RGB matrix (Q10) turns white, red and a mid grey into RGBA.
    #[test]
    fn ycbcr_422_becomes_rgba_with_the_programmed_matrix() {
        let mut ram = vec![0u8; 0x1000];
        // Y0 Cb Y1 Cr: white, white; then red (Y 76, Cb 85, Cr 255) twice; grey.
        ram[..12].copy_from_slice(&[255, 128, 255, 128, 76, 85, 76, 255, 128, 128, 128, 128]);
        let mut host = RecordingHost { ram_base: RAM, ram, ..Default::default() };
        let mut fcnv = Fcnv::new(stub());
        let q10 = |v: f64| ((v * 1024.0).round() as i64 as u64) & 0x1fff;
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        for (offset, value) in [
            (0x08, 0x10),
            (0x20, RAM),
            (0x28, RAM),
            (0x30, RAM + 0x400),
            (0x44, 16),
            (0x40, 16),
            (0x4c, 32),
            (0x48, 32),
            (0x50, 1 << 16 | 6),
            (0x204, q10(1.0)),
            (0x208, q10(1.402)),
            (0x20c, q10(1.0) | q10(-0.344) << 16),
            (0x210, q10(-0.714)),
            (0x214, q10(1.0) | q10(1.772) << 16),
            (0x218, 0),
            (0x21c, 0xff),
            (0x04, 1),
        ] {
            fcnv.write(&mut cx, 0xffd2_6000 + offset, 4, value);
        }
        let out = &host.ram[0x400..0x400 + 24];
        assert_eq!(&out[..8], &[255, 255, 255, 255, 255, 255, 255, 255]);
        assert!(out[8] > 240 && out[9] < 16 && out[10] < 16 && out[11] == 255, "{:?}", &out[8..12]);
        assert_eq!(&out[16..24], &[128, 128, 128, 255, 128, 128, 128, 255]);
        // A mode that is not modelled writes nothing.
        let mut fcnv = Fcnv::new(stub());
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        for (offset, value) in [(0x08, 0x21), (0x30, RAM + 0x800), (0x04, 1)] {
            fcnv.write(&mut cx, 0xffd2_6000 + offset, 4, value);
        }
        assert!(host.ram[0x800..0x900].iter().all(|&b| b == 0));
    }
}
