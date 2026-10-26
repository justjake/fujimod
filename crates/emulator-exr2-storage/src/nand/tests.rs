use super::*;
use fujimod_emulator_devices::{Host, Record, test_host::ram::Ram};
use std::{collections::BTreeMap, sync::Arc};
struct TestHost {
    ram: Ram,
    irqs: Vec<(usize, u32)>,
}
impl TestHost {
    fn new() -> Self {
        Self {
            ram: Ram::new(0x8000),
            irqs: vec![],
        }
    }
    fn cx(&mut self) -> Cx<'_> {
        Cx::new(self, 0, 0, 0)
    }
}
impl Host for TestHost {
    fn now(&self) -> u64 {
        self.ram.now
    }
    fn schedule(&mut self, _: usize, _: u64, _: u64) {}
    fn set_irq(&mut self, _: u32, _: bool) {}
    fn raise_irq(&mut self, c: usize, l: u32) -> Result<(), String> {
        self.irqs.push((c, l));
        Ok(())
    }
    fn ram_read(&self, a: u64, b: &mut [u8]) -> Result<(), String> {
        self.ram.ram_read(a, b)
    }
    fn ram_write(&mut self, a: u64, b: &[u8]) -> Result<(), String> {
        self.ram.ram_write(a, b)
    }
    fn record(&mut self, _: Record) {}
}
fn model(small: bool) -> NandController {
    NandController::new(
        Options { erase_ticks: 64 },
        Nand::new(
            fujimod_emulator_devices::parts::nand::Options {
                bytes: 0x40000,
                page_bytes: if small { 512 } else { 2048 },
                pages_per_block: if small { 32 } else { 64 },
                spare_bytes: 512,
            },
            Arc::from(vec![255; 4096]),
            vec![],
            BTreeMap::new(),
        )
        .unwrap(),
    )
    .unwrap()
}
fn write(m: &mut NandController, h: &mut TestHost, o: u64, v: u32) {
    let mut cx = h.cx();
    m.write(&mut cx, BASE + o, 4, v as u64);
    assert_eq!(cx.refusal(), None);
}
fn read(m: &mut NandController, h: &mut TestHost, o: u64) -> u32 {
    let mut cx = h.cx();
    let v = m.read(&mut cx, BASE + o, 4);
    assert_eq!(cx.refusal(), None);
    v as u32
}
fn tick(m: &mut NandController, h: &mut TestHost, t: u64) {
    h.ram.now = t;
    let mut cx = h.cx();
    m.advance(&mut cx);
    assert_eq!(cx.refusal(), None);
}
#[test]
fn read_dma_commits_bytes_before_status_irq_and_never_on_early_ticks_or_host_save() {
    let mut m = model(false);
    let mut h = TestHost::new();
    for (o, v) in [
        (0, 1),
        (0x54, 8),
        (0x1c, 0x300004),
        (0x884, 0x10001),
        (0x8a0, 0x401000),
        (0x8a4, 0x1000),
        (0x8a8, 0x100000),
        (8, 0x8013b3),
    ] {
        write(&mut m, &mut h, o, v);
    }
    assert_eq!(m.next_deadline(0), Some(4));
    assert_eq!(read(&mut m, &mut h, 4) & 4, 4);
    let before = m.save();
    assert_eq!(m.save(), before);
    tick(&mut m, &mut h, 3);
    assert_eq!(m.save(), before);
    assert!(h.irqs.is_empty());
    assert_eq!(h.ram.bytes[0x1000], 0);
    tick(&mut m, &mut h, 4);
    assert_eq!(&h.ram.bytes[0x1000..0x1800], &[255; 2048]);
    assert_eq!(read(&mut m, &mut h, 0x890), 0);
    assert_eq!(read(&mut m, &mut h, 0x54), 9);
    assert_eq!(h.irqs, [(0, 80)]);
    tick(&mut m, &mut h, 20);
    assert_eq!(h.irqs.len(), 1);
}
#[test]
fn program_erase_and_retention_preserve_committed_data_across_fresh_controllers() {
    let mut m = model(false);
    let mut h = TestHost::new();
    h.ram.bytes[..2048].fill(0x85);
    for (o, v) in [
        (0, 1),
        (0x18, 0x8093),
        (0x1c, 0x80107004),
        (0x5c, 0x1010),
        (0x804, 0x10001),
        (0x820, 0x80301000),
        (0x824, 0),
        (0x828, 0x100000),
        (8, 0x802362),
    ] {
        write(&mut m, &mut h, o, v);
    }
    let mut bytes = [0; 2048];
    m.medium.read(0, &mut bytes).unwrap();
    assert_eq!(bytes, [255; 2048]);
    tick(&mut m, &mut h, 4);
    m.medium.read(0, &mut bytes).unwrap();
    assert_eq!(bytes, [0x85; 2048]);
    let mut cold = model(false);
    cold.retain(m.medium().clone()).unwrap();
    cold.medium.read(0, &mut bytes).unwrap();
    assert_eq!(bytes, [0x85; 2048]);
    // A zero-to-one request has the admitted hardware status failure while
    // retaining NAND's one-to-zero law; the immutable initial image is intact.
    h.ram.bytes[..2048].fill(255);
    for (o, v) in [(0x824, 0), (0x828, 0x100000), (8, 0x802362)] {
        write(&mut m, &mut h, o, v);
    }
    tick(&mut m, &mut h, 8);
    assert_eq!(read(&mut m, &mut h, 0x28), 1 << 24);
    m.medium.read(0, &mut bytes).unwrap();
    assert_eq!(bytes, [0x85; 2048]);
    for (o, v) in [(0x1c, 0x60d07000), (8, 0x800006)] {
        write(&mut m, &mut h, o, v);
    }
    tick(&mut m, &mut h, 71);
    m.medium.read(0, &mut bytes).unwrap();
    assert_eq!(bytes, [0x85; 2048]);
    tick(&mut m, &mut h, 72);
    m.medium.read(0, &mut bytes).unwrap();
    assert_eq!(bytes, [255; 2048]);
}
#[test]
fn pio_cache_requires_four_matching_sectors_and_abort_discards_only_unconfirmed_data() {
    let mut m = model(false);
    let mut h = TestHost::new();
    for (o, v) in [(0, 1), (0x18, 0x8093), (0x5c, 0x1010)] {
        write(&mut m, &mut h, o, v);
    }
    for sector in 0..4 {
        m.aperture.fill(0x40 + sector as u8);
        for (o, v) in [
            (0x20, sector * 0x210),
            (0x1c, [0x80000001, 1, 1, 0x107001][sector as usize]),
            (8, [0x882062, 0x882004, 0x882004, 0x802064][sector as usize]),
        ] {
            write(&mut m, &mut h, o, v);
        }
        let due = m.next_deadline(0).unwrap();
        tick(&mut m, &mut h, due);
        if sector < 3 {
            let mut bytes = [0; 2048];
            m.medium.read(0, &mut bytes).unwrap();
            assert_eq!(bytes, [255; 2048]);
        }
    }
    let mut bytes = [0; 2048];
    m.medium.read(0, &mut bytes).unwrap();
    for s in 0..4 {
        assert_eq!(&bytes[s * 512..(s + 1) * 512], &[0x40 + s as u8; 512]);
    }
    m.aperture.fill(0);
    for (o, v) in [
        (0x20, 0),
        (0x24, 1),
        (0x1c, 0x80000001),
        (8, 0x882062),
        (8, 0),
    ] {
        write(&mut m, &mut h, o, v);
    }
    tick(&mut m, &mut h, 100);
    m.medium.read(2048, &mut bytes).unwrap();
    assert_eq!(bytes, [255; 2048]);
    assert!(!m.busy());
}
#[test]
fn small_page_staged_pio_dma_and_program_use_the_attached_geometry() {
    let mut m = model(true);
    let mut h = TestHost::new();
    for (o, v) in [(0, 4), (0x1c, 0), (0x20, 0), (0x24, 0), (8, 0x800073)] {
        write(&mut m, &mut h, o, v);
    }
    tick(&mut m, &mut h, 2);
    assert_eq!(m.aperture, [255; 512]);
    for (o, v) in [
        (0x58, 512),
        (0x884, 0x10001),
        (0x8a0, 0x401000),
        (0x8a4, 0x1000),
        (0x8a8, 0x100000),
        (8, 0x800980),
    ] {
        write(&mut m, &mut h, o, v);
    }
    tick(&mut m, &mut h, 4);
    assert_eq!(&h.ram.bytes[0x1000..0x1200], &[255; 512]);
    h.ram.bytes[..512].fill(0x33);
    for (o, v) in [
        (0x18, 0x8093),
        (0x1c, 0x80107001),
        (0x60, 0),
        (0x5c, 0x1010),
        (0x804, 0x10001),
        (0x820, 0x80301000),
        (0x824, 0),
        (0x828, 0x100000),
        (8, 0x802342),
    ] {
        write(&mut m, &mut h, o, v);
    }
    tick(&mut m, &mut h, 6);
    let mut bytes = [0; 512];
    m.medium.read(0, &mut bytes).unwrap();
    assert_eq!(bytes, [0x33; 512]);
    let mut cx = h.cx();
    m.write(&mut cx, BASE, 4, 1);
    assert!(cx.refusal().unwrap().contains("geometry"));
}

#[test]
fn small_page_erase_uses_packed_row_and_preserves_neighbor_blocks_until_the_deadline() {
    // A packed row selects one 32-page block; neighboring blocks change only
    // when the scheduled erase completes.
    for row in [64, 128] {
        let mut m = model(true);
        let mut h = TestHost::new();
        for page in row - 1..=row + 32 {
            m.medium.program(page, &[0x11; 512], &[0x22; 512]).unwrap();
        }
        for (o, v) in [
            (0, 4),
            (0x18, 0x8093),
            (0x1c, 0x60d07000),
            (0x20, (row << 8) & 0xffff00),
            (0x24, row >> 16),
            (0x60, row),
            (8, 0x802046),
        ] {
            write(&mut m, &mut h, o, v);
        }
        assert_eq!(m.next_deadline(0), Some(64));
        let saved = m.checkpoint().unwrap();
        let mut restored = model(true);
        restored.restore_checkpoint(&saved).unwrap();
        let before = m.fingerprint().unwrap();
        assert_eq!(restored.fingerprint().unwrap(), before);
        tick(&mut m, &mut h, 63);
        assert_eq!(m.fingerprint().unwrap(), before);
        assert!(m.busy());
        tick(&mut m, &mut h, 64);
        let mut replay_host = TestHost::new();
        tick(&mut restored, &mut replay_host, 64);
        assert_eq!(restored.fingerprint().unwrap(), m.fingerprint().unwrap());
        assert!(!m.busy());
        let mut data = [0; 512];
        let mut spare = [0; 512];
        for page in row - 1..=row + 32 {
            m.medium.read(u64::from(page) * 512, &mut data).unwrap();
            m.medium.read_spare(page, &mut spare).unwrap();
            let erased = (row..row + 32).contains(&page);
            assert_eq!(data, [if erased { 255 } else { 0x11 }; 512]);
            assert_eq!(spare, [if erased { 255 } else { 0x22 }; 512]);
        }
        assert!(h.irqs.is_empty());
    }
}

#[test]
fn small_page_erase_refuses_misaligned_inconsistent_and_out_of_range_rows() {
    for (row, packed) in [(65, 65 << 8), (64, (64 << 8) | 1), (512, 512 << 8)] {
        let mut m = model(true);
        let mut h = TestHost::new();
        for (o, v) in [
            (0, 4),
            (0x18, 0x8093),
            (0x1c, 0x60d07000),
            (0x20, packed),
            (0x24, row >> 16),
            (0x60, row),
        ] {
            write(&mut m, &mut h, o, v);
        }
        let before = m.fingerprint().unwrap();
        let mut cx = h.cx();
        m.write(&mut cx, BASE + 8, 4, 0x802046);
        assert!(cx.refusal().unwrap().contains("NAND erase"));
        assert_eq!(m.next_deadline(0), None);
        // A refused command is recorded as a register write but cannot erase.
        assert_eq!(m.medium.fingerprint(), before["medium"]);
    }
}

#[test]
fn small_page_erase_decodes_packed_row_and_commits_only_its_32_page_block() {
    // Small-page erase carries a packed row and a 32-page block boundary.
    let mut m = model(true);
    let mut h = TestHost::new();
    for row in [31, 32, 63, 64] {
        m.medium.program(row, &[0xa5; 512], &[0x12; 16]).unwrap();
    }
    for (o, v) in [
        (0, 4),
        (0x1c, 0x60d07000),
        (0x18, 0x8093),
        (0x20, 32 << 8),
        (0x24, 0),
        (8, 0x802046),
    ] {
        write(&mut m, &mut h, o, v);
    }
    assert_eq!(m.next_deadline(0), Some(64));
    let mut data = [0; 512];
    tick(&mut m, &mut h, 63);
    m.medium.read(32 * 512, &mut data).unwrap();
    assert_eq!(data, [0xa5; 512]);
    tick(&mut m, &mut h, 64);
    for row in [32, 63] {
        m.medium.read(row * 512, &mut data).unwrap();
        assert_eq!(data, [255; 512]);
        m.medium.read_spare(row as u32, &mut data).unwrap();
        assert_eq!(data, [255; 512]);
    }
    for row in [31, 64] {
        m.medium.read(row * 512, &mut data).unwrap();
        assert_eq!(data, [0xa5; 512]);
    }
    // A changed packed low byte or unaligned block row refuses before busy.
    for packed in [(32 << 8) | 1, 33 << 8] {
        write(&mut m, &mut h, 0x20, packed);
        let mut cx = h.cx();
        m.write(&mut cx, BASE + 8, 4, 0x802046);
        assert!(cx.refusal().is_some());
        assert_eq!(m.next_deadline(64), None);
    }
}
