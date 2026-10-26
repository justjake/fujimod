use fujimod_emulator_devices::exr2::irq_routes::Irq;
use crate::*;
use fujimod_emulator_devices::test_host::*;
use fujimod_emulator_devices::Device;
#[test]
fn still_isp_reconstructs_addressed_pixels_and_refuses_unequal_black() {
    let mut h = TestHost::new();
    let mut d = StillProcessor::new(
        fujimod_emulator_imaging::exr2_isp::options::still::Options {
            completion_ticks: 4,
            cfa: cfa(),
        },
        Irq { core: 0, line: 61 },
    )
    .unwrap();
    for (o, v) in [
        (0x38, 6 << 16 | 6),
        (0x44, 6),
        (0x48, 8),
        (0x50, 0x8000),
        (0x58, 0x9000),
        (0x64, 5 << 16 | 5),
    ] {
        write(&mut d, &mut h, 0xfff40000 + o, v);
    }
    write(&mut d, &mut h, 0xfff38054, 0x1000100);
    write(&mut d, &mut h, 0xfff38058, 0x1000100);
    write(&mut d, &mut h, 0xfff40004, 1);
    advance(&mut d, &mut h, 3);
    assert!(h.effects.is_empty());
    advance_from_checkpoint(&mut d, &mut h, 4);
    assert_eq!(&h.ram[0x9000..0x9004], &[0, 0, 128, 128]);
    assert_eq!(h.irqs, [(0, 61)]);
    let first = d.checkpoint().unwrap();
    assert_eq!(d.diagnostic_buffer("raw-input").unwrap(), &[0; 72]);
    assert!(d.diagnostic_buffer("dma-output").is_err());
    h.ram[0x8000..0x8048].fill(32);
    write(&mut d, &mut h, 0xfff40004, 1);
    let second = d.checkpoint().unwrap();
    assert_eq!(d.diagnostic_buffer("raw-input").unwrap(), &[32; 72]);
    assert_eq!(first.blobs()[0].as_ref(), &[0; 72]);
    assert!(!std::sync::Arc::ptr_eq(
        &first.blobs()[0],
        &second.blobs()[0]
    ));
    assert_eq!(&*first.blobs()[0], &[0; 72]);
    assert_eq!(&*second.blobs()[0], &[32; 72]);
    advance(&mut d, &mut h, 8);
    d.restore_checkpoint(&first).unwrap();
    assert!(std::sync::Arc::ptr_eq(
        &first.blobs()[0],
        &d.checkpoint().unwrap().blobs()[0]
    ));
    write(&mut d, &mut h, 0xfff38004, 1);
    let mut cx = h.cx();
    d.write(&mut cx, 0xfff40004, 4, 1);
    assert!(cx.refusal().is_some());
}

fn raw_writeback() -> (StillProcessor, TestHost) {
    let mut h = TestHost::new();
    h.ram[0x7f00..0xa000].fill(0xa5);
    for y in 0..12 {
        for x in 0..12 {
            let value = (1024 + y * 64 + x * 128) as u16;
            let address = 0x8040 + y * 32 + x * 2;
            h.ram[address..address + 2].copy_from_slice(&value.to_le_bytes());
        }
    }
    let mut d = StillProcessor::new(
        fujimod_emulator_imaging::exr2_isp::options::still::Options {
            completion_ticks: 4,
            cfa: cfa(),
        },
        Irq { core: 0, line: 61 },
    )
    .unwrap();
    // Backward RAW writeback can overlap active rows and stride padding.
    // This compact fixture moves the addressed RAW window two rows earlier.
    for (o, v) in [
        (0x38, 8 << 16 | 8),
        (0x40, 12 << 16 | 12),
        (0x44, 16),
        (0x48, 12),
        (0x4c, 16),
        (0x50, 0x8040),
        (0x58, 0x9000),
        (0x60, 0x8000),
        (0x64, 11 << 16 | 11),
    ] {
        write(&mut d, &mut h, 0xfff40000 + o, v);
    }
    write(&mut d, &mut h, 0xfff38054, 0x1000100);
    write(&mut d, &mut h, 0xfff38058, 0x1000100);
    for (index, value) in fujimod_emulator_devices::test_host::film_words()
        .into_iter()
        .enumerate()
    {
        if let Some(offset) = fujimod_emulator_imaging::film::still_offset(index * 4) {
            write(&mut d, &mut h, 0xfff44000 + offset as u64, value);
        }
    }
    (d, h)
}

#[test]
fn raw_and_color_dma_complete_together_with_overlap_padding_and_exact_restore() {
    let (mut d, mut h) = raw_writeback();
    let before = h.ram.clone();
    write(&mut d, &mut h, 0xfff40004, 3);
    assert_eq!(d.next_deadline(0), Some(4));
    let identity = d.fingerprint().unwrap();
    let _ = d.save();
    assert_eq!(d.fingerprint().unwrap(), identity);
    advance(&mut d, &mut h, 3);
    assert_eq!(h.ram, before);
    assert!(h.effects.is_empty());
    advance_from_checkpoint(&mut d, &mut h, 4);
    for y in 0..12 {
        assert_eq!(
            &h.ram[0x8000 + y * 32..0x8000 + y * 32 + 24],
            &before[0x8040 + y * 32..0x8040 + y * 32 + 24]
        );
        assert_eq!(
            &h.ram[0x8000 + y * 32 + 24..0x8020 + y * 32],
            &before[0x8000 + y * 32 + 24..0x8020 + y * 32]
        );
    }
    for y in 0..8 {
        assert_eq!(&h.ram[0x9000 + y * 24 + 16..0x9018 + y * 24], &[0xa5; 8]);
    }
    assert_ne!(&h.ram[0x9000..0x9010], &[0xa5; 16]);
    assert!(h.ram[0x9000..0x9010].iter().step_by(4).any(|&v| v > 0));
    assert_eq!(h.irqs, [(0, 61)]);
    assert_eq!(read(&mut d, &mut h, 0xfff40084), 1);
    assert_eq!(d.next_deadline(4), None);
    advance(&mut d, &mut h, 5);
    assert_eq!(h.irqs, [(0, 61)]);
}

#[test]
fn raw_writeback_refuses_unmodeled_layouts_before_dma() {
    for (offset, value) in [
        (0x60, 0x8001),
        (0x60, 0x8060),
        (0x60, 0x9000),
        (0x40, 8 << 16 | 8),
        (0x4c, 14),
        (8, 0x100),
    ] {
        let (mut d, mut h) = raw_writeback();
        write(&mut d, &mut h, 0xfff40000 + offset, value);
        let before = h.ram.clone();
        let mut cx = h.cx();
        d.write(&mut cx, 0xfff40004, 4, 3);
        assert!(cx.refusal().is_some(), "{offset:#x}={value:#x}");
        assert_eq!(d.next_deadline(0), None);
        assert_eq!(h.ram, before);
        assert!(h.effects.is_empty());
    }
}

#[test]
fn raw_writeback_supports_eight_rows_plus_64_byte_displacement() {
    let (mut d, mut h) = raw_writeback();
    let pitch = 9984;
    let source = 0x28000;
    let destination = source - (8 * pitch + 64);
    for y in 0..12 {
        for x in 0..12 {
            let value = (1024 + y * 64 + x * 128) as u16;
            let address = source + y * pitch + x * 2;
            h.ram[address..address + 2].copy_from_slice(&value.to_le_bytes());
        }
    }
    for (offset, value) in [
        (0x44, pitch / 2),
        (0x4c, pitch / 2),
        (0x50, source),
        (0x60, destination),
    ] {
        write(&mut d, &mut h, 0xfff40000 + offset, value as u32);
    }
    let before = h.ram.clone();
    write(&mut d, &mut h, 0xfff40004, 3);
    advance_from_checkpoint(&mut d, &mut h, 4);
    for y in 0..12 {
        assert_eq!(
            &h.ram[destination + y * pitch..destination + y * pitch + 24],
            &before[source + y * pitch..source + y * pitch + 24]
        );
        assert_eq!(
            &h.ram[destination + y * pitch + 24..destination + (y + 1) * pitch],
            &before[destination + y * pitch + 24..destination + (y + 1) * pitch]
        );
    }
    assert_eq!(h.irqs, [(0, 61)]);
}

#[test]
fn worker_still_retains_sample_latching_joint_dma_and_restore() {
    fujimod_emulator_devices::work::configure(2).unwrap();
    still_isp_reconstructs_addressed_pixels_and_refuses_unequal_black();
    raw_and_color_dma_complete_together_with_overlap_padding_and_exact_restore();
    raw_writeback_supports_eight_rows_plus_64_byte_displacement();
    fujimod_emulator_devices::work::configure(0).unwrap();
}
