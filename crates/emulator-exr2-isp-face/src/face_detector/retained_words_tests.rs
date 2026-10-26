use super::*;
use fujimod_emulator_devices::{Host, Record};

// A retained-word bank has no outward effects, even when START-looking words
// and DMA-looking addresses are written. Every such host call fails this test.
struct NoEffects;
impl Host for NoEffects {
    fn now(&self) -> u64 {
        71
    }
    fn schedule(&mut self, _: usize, _: u64, _: u64) {
        panic!("face deadline")
    }
    fn set_irq(&mut self, _: u32, _: bool) {
        panic!("face level IRQ")
    }
    fn raise_irq(&mut self, _: usize, _: u32) -> Result<(), String> {
        panic!("face IRQ")
    }
    fn ram_read(&self, _: u64, _: &mut [u8]) -> Result<(), String> {
        panic!("face DMA read")
    }
    fn ram_write(&mut self, _: u64, _: &[u8]) -> Result<(), String> {
        panic!("face DMA write")
    }
    fn record(&mut self, _: Record) {}
}

fn read(model: &mut FaceDetector, address: u64, size: u8) -> (u64, Option<String>) {
    let mut host = NoEffects;
    let mut cx = Cx::new(&mut host, 1, 0x123456, 4);
    let value = model.read(&mut cx, address, size);
    (value, cx.take_refusal())
}
fn write(model: &mut FaceDetector, address: u64, size: u8, value: u64) -> Option<String> {
    let mut host = NoEffects;
    let mut cx = Cx::new(&mut host, 0, 0x987654, 4);
    model.write(&mut cx, address, size, value);
    cx.take_refusal()
}
fn bank() -> FaceDetector {
    FaceDetector::new(Options {
        mode: Mode::RetainedWords,
    })
}

#[test]
fn full_word_map_is_reconfigurable_after_repeated_central_requests_without_effects() {
    let mut model = bank();
    for offset in (0..0x1000).step_by(4) {
        assert_eq!(read(&mut model, BASE + offset, 4), (0, None));
    }
    for generation in 0..2u32 {
        for offset in (0..0x1000).step_by(4) {
            let word = (offset as u32).wrapping_mul(0x1020304) ^ (u32::MAX >> generation);
            assert_eq!(write(&mut model, BASE + offset, 4, u64::from(word)), None);
            assert_eq!(read(&mut model, BASE + offset, 4), (u64::from(word), None));
        }
    }
    assert!(model.state.request.is_none());
    assert_eq!(model.next_deadline(0), None);
    assert_eq!(model.next_deadline(u64::MAX), None);
    let before = model.save();
    let mut host = NoEffects;
    model.advance(&mut Cx::new(&mut host, 0, 0, 4));
    assert_eq!(model.save(), before);
}

#[test]
fn bad_widths_addresses_and_cross_mode_snapshots_refuse_without_mutation() {
    let mut model = bank();
    assert_eq!(write(&mut model, BASE + 0x14, 4, u64::from(u32::MAX)), None);
    let before = model.save();
    for (address, size, value) in [
        (BASE - 4, 4, 0),
        (BASE + 0x1000, 4, 0),
        (BASE + 1, 4, 0),
        (BASE, 1, 0),
        (BASE, 2, 0),
        (BASE, 8, 0),
        (BASE, 4, 1u64 << 32),
    ] {
        assert!(write(&mut model, address, size, value).is_some());
        assert_eq!(model.save(), before);
        if value == 0 {
            assert!(read(&mut model, address, size).1.is_some());
        }
        assert_eq!(model.save(), before);
    }
    let mut restored = bank();
    restored.restore(&before).unwrap();
    assert_eq!(restored.save(), before);
    assert!(FaceDetector::default().restore(&before).is_err());
    let mut corrupted = before;
    corrupted["dma"][0]["phase"] = serde_json::json!("configured");
    let saved = restored.save();
    assert!(restored.restore(&corrupted).is_err());
    assert_eq!(restored.save(), saved);
}
