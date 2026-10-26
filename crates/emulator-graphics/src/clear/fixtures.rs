//! Independently authored clear command lists for the decoder and GPU tests.
pub fn write(words: &mut Vec<u32>, index: u32, lanes: u32, value: u32) {
    words.extend([0x40000000 | lanes << 16 | index, value]);
}
pub fn align(mut words: Vec<u32>) -> Vec<u32> {
    words.resize(words.len().next_multiple_of(16), 0);
    words
}
pub fn flush(words: &mut Vec<u32>) {
    for (index, lanes, value) in [
        (0x28, 15, 0x12345678),
        (0x31, 1, 0xf0),
        (0x18, 15, 0x12345678),
    ] {
        write(words, index, lanes, value);
    }
}
pub fn fill_state() -> Vec<u32> {
    let mut words = Vec::new();
    // Independently authored supported register state; no initializer packet
    // or firmware-produced stream is needed by the decoder.
    for (index, value) in [
        (0x140, 0x50000),
        (0x190, 0),
        (0x1c0, 0),
        (0x14d, 0),
        (0x143, 0),
        (0x153, 0),
        (0x144, 0),
        (0x154, 0),
        (0x147, 1),
        (0x157, 1),
        (0x178, 1),
        (0x16c, 0x40001),
        (0x187, 0),
        (0x1a2, 0),
        (0x1a3, 8 << 16 | 8),
        (0x1a7, 0x100),
    ] {
        write(&mut words, index, 15, value);
    }
    words
}
pub fn lists() -> (Vec<u32>, Vec<u32>) {
    let mut path = Vec::new();
    for (index, value) in [
        (0x7b, 8 << 16 | 8),
        (0x60, 0x100),
        (0x81, 0x1000101),
        (0x7c, 1),
        (0x8c, 1),
        (0x80, 16 << 16),
        (0x8e, 8 << 16),
        (0x8b, 4096 + 5 * 256 - 8),
    ] {
        write(&mut path, index, 15, value);
    }
    for index in 0..9 {
        write(
            &mut path,
            0x40 + index,
            7,
            if [0, 4, 8].contains(&index) {
                0x3f0000
            } else {
                0
            },
        );
    }
    for index in 0..5 {
        write(&mut path, 0x84 + index, 15, 4096 + index * 256);
    }
    for (code, x, y) in [
        (0x28, 1f32, 1f32),
        (0x29, 1., 5.),
        (0x28, 5., 5.),
        (0x29, 5., 1.),
    ] {
        path.extend([code << 24, x.to_bits(), y.to_bits()]);
    }
    write(&mut path, 0x8d, 7, 0x1002a);
    let mut fill = fill_state();
    for (index, value) in [
        (0x115, 0),
        (0x184, 8),
        (0x1a0, 0),
        (0x1a1, 0),
        (0x1a5, 0),
        (0x1aa, 0),
        (0x1a4, 256),
        (0x185, 0x80402010),
        (0xa0, 16 << 16 | 0x10),
        (0xa2, 7),
    ] {
        write(&mut fill, index, 15, value);
    }
    for index in 0..5 {
        write(&mut fill, 0xa4 + index, 15, 4096 + index * 256);
    }
    write(&mut fill, 0xad, 7, 0x1002a);
    write(&mut fill, 0x1a9, 1, 3);
    flush(&mut fill);
    (align(path), align(fill))
}
