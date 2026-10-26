//! Device host for tests in this crate and the crates built on it.
pub mod bus;
pub mod ram;
pub mod recording;
use crate::{Cx, Device, Host, Record};

#[derive(Clone, Default)]
pub struct TestHost {
    pub now: u64,
    pub ram: Vec<u8>,
    pub effects: Vec<&'static str>,
    pub irqs: Vec<(usize, u32)>,
    pub levels: std::collections::BTreeMap<(usize, u32), bool>,
}
impl TestHost {
    pub fn new() -> Self {
        Self {
            ram: vec![0; 1024 * 1024],
            ..Self::default()
        }
    }
    pub fn cx(&mut self) -> Cx<'_> {
        Cx::new(self, 0, 0, 0)
    }
}
impl Host for TestHost {
    fn now(&self) -> u64 {
        self.now
    }
    fn schedule(&mut self, _: usize, _: u64, _: u64) {}
    fn set_irq(&mut self, _: u32, _: bool) {}
    fn raise_irq(&mut self, c: usize, l: u32) -> Result<(), String> {
        self.effects.push("irq");
        self.irqs.push((c, l));
        Ok(())
    }
    fn set_core_irq(&mut self, core: usize, line: u32, high: bool) -> Result<(), String> {
        let previous = self.levels.insert((core, line), high).unwrap_or(false);
        if previous != high {
            self.effects.push(if high { "irq" } else { "irq-low" });
            if high {
                self.irqs.push((core, line));
            }
        }
        Ok(())
    }
    fn ram_read(&self, a: u64, out: &mut [u8]) -> Result<(), String> {
        let a = (a & 0x3fffffff) as usize;
        let r = self.ram.get(a..a + out.len()).ok_or("unmapped read")?;
        out.copy_from_slice(r);
        Ok(())
    }
    fn ram_write(&mut self, a: u64, b: &[u8]) -> Result<(), String> {
        let a = (a & 0x3fffffff) as usize;
        self.ram
            .get_mut(a..a + b.len())
            .ok_or("unmapped write")?
            .copy_from_slice(b);
        self.effects.push("dma");
        Ok(())
    }
    fn record(&mut self, _: Record) {}
}
pub fn write(d: &mut dyn Device, h: &mut TestHost, a: u64, v: u32) {
    let mut cx = h.cx();
    d.write(&mut cx, a, 4, v as u64);
    assert_eq!(cx.refusal(), None, "{a:#x}={v:#x}");
}
pub fn read(d: &mut dyn Device, h: &mut TestHost, a: u64) -> u64 {
    let mut cx = h.cx();
    let v = d.read(&mut cx, a, 4);
    assert_eq!(cx.refusal(), None);
    v
}
pub fn advance(d: &mut dyn Device, h: &mut TestHost, now: u64) {
    h.now = now;
    let mut cx = h.cx();
    d.advance(&mut cx);
    assert_eq!(cx.refusal(), None);
}

/// Compare due DMA bytes, IRQ order and complete private state after an exact
/// pending-operation restore, including the owned input/pixel staging buffers.
pub fn advance_from_checkpoint(d: &mut dyn Device, h: &mut TestHost, now: u64) {
    let before_host = h.clone();
    let before_state = d.fingerprint().unwrap();
    let deadline = d.next_deadline(h.now);
    let saved = d.checkpoint().unwrap();
    let repeated = d.checkpoint().unwrap();
    assert_eq!(saved.blobs().len(), repeated.blobs().len());
    for (a, b) in saved.blobs().iter().zip(repeated.blobs()) {
        assert!(std::sync::Arc::ptr_eq(a, b));
    }
    if !saved.blobs().is_empty() {
        let invalid = saved.clone().with_blobs(Vec::new());
        assert!(d.validate_checkpoint(&invalid).is_err());
        assert!(d.restore_checkpoint(&invalid).is_err());
        assert_eq!(d.fingerprint().unwrap(), before_state);
        if let Some(index) = saved.blobs().iter().position(|blob| !blob.is_empty()) {
            let mut blobs = saved.blobs().to_vec();
            blobs[index] = blobs[index][..blobs[index].len() - 1].into();
            let invalid = saved.clone().with_blobs(blobs);
            assert!(d.validate_checkpoint(&invalid).is_err());
            assert!(d.restore_checkpoint(&invalid).is_err());
            assert_eq!(d.fingerprint().unwrap(), before_state);
        }
    }
    let envelope = crate::checkpoint::Checkpoint::encode("pending-device", &saved).unwrap();
    let decoded: crate::checkpoint::Checkpoint = envelope.decode("pending-device").unwrap();
    assert_eq!(decoded.blobs(), saved.blobs());
    d.validate_checkpoint(&decoded).unwrap();
    d.restore_checkpoint(&decoded).unwrap();
    assert_eq!(d.fingerprint().unwrap(), before_state);
    advance(d, h, now);
    let after_host = h.clone();
    let after_state = d.fingerprint().unwrap();
    d.validate_checkpoint(&saved).unwrap();
    d.restore_checkpoint(&saved).unwrap();
    *h = before_host;
    assert_eq!(d.fingerprint().unwrap(), before_state);
    assert_eq!(d.next_deadline(h.now), deadline);
    advance(d, h, now);
    assert_eq!(h.ram, after_host.ram);
    assert_eq!(h.effects, after_host.effects);
    assert_eq!(h.irqs, after_host.irqs);
    assert_eq!(d.fingerprint().unwrap(), after_state);
}

/// Film registers for a neutral matrix, linear tone and unit chroma.
pub fn film_words() -> Vec<u32> {
    let mut r = vec![0; 1024];
    for o in (0x304..0x330).step_by(4) {
        r[o / 4] = 512 | 512 << 16;
    }
    r[0x400 / 4] = 1024;
    r[0x408 / 4] = 1024 << 16;
    r[0x414 / 4] = 1024;
    r[0x500 / 4] = 0x55555555;
    r[0x504 / 4] = 5;
    for i in 0..28 {
        r[(0x508 + i / 2 * 4) / 4] |= ((i * 2048 / 3).min(16383) as u32) << (i % 2 * 16);
    }
    r[0x604 / 4] = 1024;
    r[0x608 / 4] = 1024;
    r[0x640 / 4] = 0xc0804000;
    for o in (0x644..0x658).step_by(4) {
        r[o / 4] = 256 << 16;
    }
    r[0x200 / 4] = 0x1000100;
    r[0x204 / 4] = 256;
    r
}

/// A 6x6 color filter pattern for image input and still processor tests.
pub fn cfa() -> Vec<u8> {
    (0..36).map(|i| (i % 3) as u8).collect()
}
