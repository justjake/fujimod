//! The XP4 JPEG encoder's register contract (X-T3 5.11 driver).
//!
//! The driver has one register accessor per word, each rejecting values
//! outside its fields. The words the driver writes and the bits each
//! accessor can set:
//!
//! | Offset | Bits |
//! |---|---|
//! | +0x00 | 8 reset, 0 start |
//! | +0x0c | 23..16, 2..0 |
//! | +0x24 | 23..16, 4..0 (status, written back to clear) |
//! | +0x28 | 23..16, 4 (interrupt mask) |
//! | +0x40 | 1..0 |
//! | +0x44 | height << 16, width |
//! | +0x48 | 7..0 |
//! | +0x80 | 15..0 restart interval |
//! | +0x84 | 8 table loading, 0 |
//! | +0x8c | table words |
//! | +0x100 | source |
//! | +0x10c | destination |
//! | +0x110 | limit |
//! | +0x118 | 1..0 |
//! | +0x11c | 5..0 |
//! | +0x120 | 1..0 |
//! | +0x12c | y << 16, x (a multiple of 8) |
//! | +0x130 | pitch (a multiple of 8) |
//!
//! The source and destination writers first write the buffer's memory type
//! (bits 1..0) to 0xfff4a744; the still job always passes type 0. The
//! words read are +0x00, the status +0x24 and the stream size +0x94; the
//! other getters have no caller.
//!
//! Sequences: the reset writes 1 to +0x118, waits for status
//! bit 16, writes the status back, writes 0x100 to +0x00 and waits for bit 8
//! to read clear. The programmer writes +0x0c, the mask +0x28 =
//! 0x00fd0000 (bit 4 clear), +0x40, +0x130, +0x12c, +0x44, +0x48, +0x80, the
//! buffers, +0x11c and +0x120, then +0x84 = flag << 8, the 32 table words
//! when the descriptor's table flag is set, and +0x84 = flag. The start
//! registers the handler on interrupt line 135 (level) and sets +0x00
//! bit 0. The handler reads the status, writes it back and takes
//! bit 4 as a finished run, then reads the stream size. The handler is the
//! only completion path, so the driver's mask value leaves the finished-run
//! interrupt unmasked: bit 4 of +0x28 masks it.
use fujimod_emulator_ip::jpeg::{
    CONTROL, Contract, DESTINATION, LIMIT, PITCH, RESTART, SIZE, SOURCE, STATUS, STREAM, TABLES,
    WINDOW_ORIGIN,
};

const INTERRUPT_MASK: u64 = 0x28;
const TABLE_LOADING: u64 = 0x84;

/// The X-T3 5.11 driver's words; `memory_type` is the address of the
/// encoder's memory type word.
pub fn contract(memory_type: u64) -> Contract {
    Contract {
        writable: vec![
            (CONTROL, 0x101),
            (0x0c, 0x00ff_0007),
            (STATUS, 0x00ff_001f),
            (INTERRUPT_MASK, 0x00ff_0010),
            (0x40, 0x3),
            (SIZE, u32::MAX),
            (0x48, 0xff),
            (RESTART, 0xffff),
            (TABLE_LOADING, 0x101),
            (TABLES, u32::MAX),
            (SOURCE, u32::MAX),
            (DESTINATION, u32::MAX),
            (LIMIT, u32::MAX),
            (0x118, 0x3),
            (0x11c, 0x3f),
            (0x120, 0x3),
            (WINDOW_ORIGIN, 0xffff_fff8),
            (PITCH, 0xfff8),
        ],
        readable: vec![CONTROL, STATUS, STREAM],
        table_gate: (TABLE_LOADING, 1 << 8),
        interrupt_mask: INTERRUPT_MASK,
        memory_type: Some(memory_type),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::test_host::recording::RecordingHost;
    use fujimod_emulator_devices::{Cx, Device};
    use fujimod_emulator_image_codecs::jpeg::decode;
    use fujimod_emulator_ip::jpeg::{DONE, Encoder, READY, RESET, Registers, START};

    const BASE: u64 = 0xfffa0000;
    const MEMORY_TYPE: u64 = 0xfff4a744;
    const IRQ: u32 = 135;

    fn encoder() -> Encoder {
        Encoder::new(
            "xp4-jpeg-encoder",
            BASE,
            IRQ,
            10,
            Registers::Declared(contract(MEMORY_TYPE)),
        )
        .unwrap()
    }

    fn host() -> RecordingHost {
        RecordingHost {
            ram: vec![0; 1 << 20],
            ..RecordingHost::default()
        }
    }

    fn write(e: &mut Encoder, h: &mut RecordingHost, address: u64, value: u32) {
        let mut cx = Cx::new(h, 0, 0, 0);
        e.write(&mut cx, address, 4, value.into());
        assert!(cx.refusal().is_none(), "{address:#x} {value:#x}");
    }

    fn read(e: &mut Encoder, h: &mut RecordingHost, address: u64) -> u32 {
        let mut cx = Cx::new(h, 0, 0, 0);
        let value = e.read(&mut cx, address, 4) as u32;
        assert!(cx.refusal().is_none(), "{address:#x}");
        value
    }

    fn advance(e: &mut Encoder, h: &mut RecordingHost, now: u64) {
        h.now = now;
        let mut cx = Cx::new(h, 0, 0, 0);
        e.advance(&mut cx);
        assert!(cx.refusal().is_none());
    }

    fn refuses(encoder: &mut Encoder, address: u64, value: u32) -> bool {
        let mut h = host();
        let mut cx = Cx::new(&mut h, 0, 0, 0);
        encoder.write(&mut cx, address, 4, value.into());
        cx.refusal().is_some()
    }

    /// The driver's reset and programmer for a window of
    /// `size` at `origin` in a picture `pitch` pixels wide, with tables.
    fn program(e: &mut Encoder, h: &mut RecordingHost, size: u32, origin: u32, pitch: u32) {
        write(e, h, BASE + 0x118, 1);
        assert_eq!(read(e, h, BASE + STATUS) & READY, READY);
        let status = read(e, h, BASE + STATUS);
        write(e, h, BASE + STATUS, status);
        write(e, h, BASE, RESET);
        assert_eq!(read(e, h, BASE), 0);
        write(e, h, BASE + 0x0c, 0x000a_0003);
        write(e, h, BASE + INTERRUPT_MASK, 0x00fd_0000);
        write(e, h, BASE + 0x40, 1);
        write(e, h, BASE + PITCH, pitch);
        write(e, h, BASE + WINDOW_ORIGIN, origin);
        write(e, h, BASE + SIZE, size);
        write(e, h, BASE + 0x48, 0x11);
        write(e, h, BASE + RESTART, 0);
        write(e, h, MEMORY_TYPE, 0);
        write(e, h, BASE + SOURCE, 0x1000);
        write(e, h, MEMORY_TYPE, 0);
        write(e, h, BASE + DESTINATION, 0x20000);
        write(e, h, BASE + LIMIT, 0x8000);
        write(e, h, BASE + 0x11c, 0x10);
        write(e, h, BASE + 0x120, 0);
        write(e, h, BASE + TABLE_LOADING, 0x100);
        for _ in 0..32 {
            write(e, h, BASE + TABLES, 0x0101_0101);
        }
        write(e, h, BASE + TABLE_LOADING, 1);
    }

    #[test]
    fn the_driver_sequence_encodes_the_window_and_interrupts() {
        // A 32x24 picture; the run encodes its 16x16 window at (8, 4).
        let (pitch, x, y) = (32usize, 8usize, 4usize);
        // Packed 4:2:2 (Y0 Y1 Cb Cr): Y 200 inside the window, 20 outside.
        let picture: Vec<u8> = (0..pitch * 24 / 2)
            .flat_map(|i| {
                let (px, py) = (i * 2 % pitch, i * 2 / pitch);
                let inside = (x..x + 16).contains(&px) && (y..y + 16).contains(&py);
                let luma = if inside { 200 } else { 20 };
                [luma, luma, 128, 128]
            })
            .collect();
        let mut e = encoder();
        let mut h = host();
        h.ram[0x1000..0x1000 + picture.len()].copy_from_slice(&picture);
        program(&mut e, &mut h, 16 << 16 | 16, (y << 16 | x) as u32, pitch as u32);
        write(&mut e, &mut h, BASE, START);
        assert_eq!(e.next_deadline(0), Some(10));
        advance(&mut e, &mut h, 10);
        let status = read(&mut e, &mut h, BASE + STATUS);
        assert_eq!(status, READY | DONE);
        assert_eq!(h.irqs.last(), Some(&(IRQ, true)));
        let size = read(&mut e, &mut h, BASE + STREAM) as usize;
        let stream = h.bytes(0x20000, size).unwrap();
        assert_eq!(&stream[..2], &[0xff, 0xd8]);
        assert_eq!(&stream[size - 2..], &[0xff, 0xd9]);
        let sof = stream.windows(2).position(|w| w == [0xff, 0xc0]).unwrap();
        let height = u16::from_be_bytes([stream[sof + 5], stream[sof + 6]]);
        let width = u16::from_be_bytes([stream[sof + 7], stream[sof + 8]]);
        assert_eq!((width, height), (16, 16));
        let (ycc, w, hgt) = decode(&stream, 64, 64).unwrap();
        assert_eq!((w, hgt), (16, 16));
        assert!(ycc.chunks(3).all(|p| (i32::from(p[0]) - 200).abs() <= 4));
        // The handler writes the status back, which clears the interrupt.
        write(&mut e, &mut h, BASE + STATUS, status);
        assert_eq!(read(&mut e, &mut h, BASE + STATUS), READY);
        assert!(!h.irq);
        let saved = e.save();
        let mut other = encoder();
        other.restore(&saved).unwrap();
        assert_eq!(read(&mut other, &mut h, BASE + STREAM) as usize, size);
    }

    #[test]
    fn the_mask_holds_the_finished_run_interrupt() {
        let mut e = encoder();
        let mut h = host();
        h.ram[0x1000..0x1000 + 16 * 16 * 2].fill(128);
        program(&mut e, &mut h, 16 << 16 | 16, 0, 16);
        write(&mut e, &mut h, BASE + INTERRUPT_MASK, DONE);
        write(&mut e, &mut h, BASE, START);
        advance(&mut e, &mut h, 10);
        assert_eq!(read(&mut e, &mut h, BASE + STATUS), READY | DONE);
        assert!(!h.irq);
        write(&mut e, &mut h, BASE + INTERRUPT_MASK, 0);
        assert_eq!(h.irqs.last(), Some(&(IRQ, true)));
    }

    #[test]
    fn undeclared_words_bits_and_tables_refuse() {
        let mut e = encoder();
        // Words the driver never writes, bits outside a word's fields, a
        // memory type other than 0 and a misaligned origin.
        assert!(refuses(&mut e, BASE + 0x04, 1));
        assert!(refuses(&mut e, BASE + STREAM, 1));
        assert!(refuses(&mut e, BASE + 0x40, 4));
        assert!(refuses(&mut e, BASE + WINDOW_ORIGIN, 4));
        assert!(refuses(&mut e, BASE + PITCH, 6240 + 4));
        assert!(refuses(&mut e, MEMORY_TYPE, 1));
        assert!(refuses(&mut e, MEMORY_TYPE - 4, 0));
        // Table words outside loading, or closing it early.
        assert!(refuses(&mut e, BASE + TABLES, 1));
        assert!(!refuses(&mut e, BASE + TABLE_LOADING, 0x100));
        assert!(!refuses(&mut e, BASE + TABLES, 1));
        assert!(refuses(&mut e, BASE + TABLE_LOADING, 0));
        // Reads of words the driver never reads.
        let mut h = host();
        let mut cx = Cx::new(&mut h, 0, 0, 0);
        e.read(&mut cx, BASE + SIZE, 4);
        assert!(cx.refusal().is_some());
        assert!(!refuses(&mut e, BASE + WINDOW_ORIGIN, 4160 << 16 | 1040));
        assert!(!refuses(&mut e, BASE + PITCH, 6240));
    }

    #[test]
    fn a_run_without_loaded_tables_fails() {
        let mut e = encoder();
        let mut h = host();
        write(&mut e, &mut h, BASE + SIZE, 16 << 16 | 16);
        write(&mut e, &mut h, BASE + TABLE_LOADING, 0);
        write(&mut e, &mut h, BASE, START);
        h.now = 10;
        let mut cx = Cx::new(&mut h, 0, 0, 0);
        e.advance(&mut cx);
        assert!(cx.refusal().is_some());
    }
}
