use super::*;
use fujimod_emulator_devices::{Host, Record, exr2::irq};
struct Ram {
    now: u64,
    bytes: Vec<u8>,
    effects: Vec<(usize, u32)>,
    writes: usize,
}
impl Host for Ram {
    fn now(&self) -> u64 {
        self.now
    }
    fn schedule(&mut self, _: usize, _: u64, _: u64) {
        panic!("GPU declares exact deadlines")
    }
    fn set_irq(&mut self, _: u32, _: bool) {
        panic!("GPU uses per-core physical inputs")
    }
    fn raise_irq(&mut self, c: usize, l: u32) -> Result<(), String> {
        self.effects.push((c, l));
        Ok(())
    }
    fn ram_read(&self, a: u64, b: &mut [u8]) -> Result<(), String> {
        let data = self
            .bytes
            .get(a as usize..a as usize + b.len())
            .ok_or("DMA outside physical RAM")?;
        b.copy_from_slice(data);
        Ok(())
    }
    fn ram_write(&mut self, a: u64, b: &[u8]) -> Result<(), String> {
        self.bytes
            .get_mut(a as usize..a as usize + b.len())
            .ok_or("DMA outside physical RAM")?
            .copy_from_slice(b);
        self.writes += 1;
        Ok(())
    }
    fn record(&mut self, _: Record) {}
}
fn options() -> Options {
    Options {
        ram_bytes: 4096,
        max_fill_bytes: 4096,
        max_copy_bytes: 4096,
        max_clear_bytes: 4096,
        max_command_capture_bytes: 65536,
        bytes_per_tick: 16,
        copy_profile: None,
        clear_profile: None,
    }
}
fn setup() -> (VectorGpu, Ram, [irq::InterruptController; 2]) {
    setup_with_options(options())
}
fn setup_with_options(options: Options) -> (VectorGpu, Ram, [irq::InterruptController; 2]) {
    let ram_bytes = options.ram_bytes as usize;
    let inputs: [irq::Inputs; 2] = Default::default();
    let controllers = std::array::from_fn(|c| {
        irq::InterruptController::new(
            irq::Options {
                arbitration: irq::Arbitration::Unique,
            },
            irq::Wiring {
                base: 0xfffb3000 + c as u64 * 4096,
                inputs: inputs[c].clone(),
                enables: None,
                software: vec![],
            },
        )
        .unwrap()
    });
    let gpu = VectorGpu::new(
        options,
        Wiring {
            controllers: inputs.into_iter().enumerate().collect(),
        },
    )
    .unwrap();
    (
        gpu,
        Ram {
            now: 100,
            bytes: vec![0x81; ram_bytes],
            effects: vec![],
            writes: 0,
        },
        controllers,
    )
}
fn enable(controllers: &mut [irq::InterruptController; 2], host: &mut Ram, c: usize) {
    let mut cx = Cx::new(host, c, 0, 0);
    controllers[c].write(&mut cx, 0xfffb3084 + c as u64 * 4096, 4, 1 << (IRQ - 32));
    assert_eq!(cx.take_refusal(), None);
}
fn put(gpu: &mut VectorGpu, host: &mut Ram, logical: u64, value: u64) -> Option<String> {
    let mut cx = Cx::new(host, 0, 0x12345678, 2);
    gpu.write(&mut cx, super::super::BASE + logical * 2, 4, value);
    cx.take_refusal()
}
#[test]
fn fill_commits_at_the_due_tick_routes_by_irq_enable_and_host_reads_are_pure() {
    let (mut gpu, mut host, mut controllers) = setup();
    enable(&mut controllers, &mut host, 1);
    for (o, v) in [
        (0x10, 128),
        (0x14, 160),
        (0x18, 0x12345678),
        (0x1c, super::super::FILL_CONTROL.into()),
    ] {
        assert_eq!(put(&mut gpu, &mut host, o, v), None);
    }
    assert_eq!(gpu.next_deadline(host.now), Some(102));
    assert_eq!(host.writes, 0);
    let before = gpu.save();
    assert_eq!(gpu.save(), before);
    host.now = 101;
    gpu.advance(&mut Cx::new(&mut host, 0, 0, 2));
    assert_eq!(gpu.save(), before);
    assert_eq!(host.writes, 0);
    host.now = 102;
    gpu.advance(&mut Cx::new(&mut host, 0, 0, 2));
    assert_eq!(host.bytes[128..160], 0x12345678u32.to_le_bytes().repeat(8));
    assert_eq!(host.effects, [(1, IRQ.into())]);
    assert_eq!(host.writes, 1);
    assert_eq!(gpu.next_deadline(102), None);
    let complete = gpu.save();
    gpu.advance(&mut Cx::new(&mut host, 0, 0, 2));
    assert_eq!(gpu.save(), complete);
    assert_eq!(host.writes, 1);
}

#[test]
fn viewer_diagnostics_bound_pending_pixels_without_weakening_checkpoint_or_fingerprint() {
    let mut options = options();
    options.ram_bytes = 4 * 1024 * 1024;
    options.max_fill_bytes = 2 * 1024 * 1024;
    let (mut gpu, mut host, mut controllers) = setup_with_options(options);
    enable(&mut controllers, &mut host, 1);
    for (o, v) in [
        (0x10, 128),
        (0x14, 128 + 2 * 1024 * 1024),
        (0x18, 0x12345678),
        (0x1c, super::super::FILL_CONTROL.into()),
    ] {
        assert_eq!(put(&mut gpu, &mut host, o, v), None);
    }
    let diagnostic = gpu.save();
    assert!(serde_json::to_vec(&diagnostic).unwrap().len() < 8192);
    assert_eq!(diagnostic["engine"]["pending_completions"], 1);
    assert!(diagnostic["engine"].get("completions").is_none());
    let saved = gpu.checkpoint().unwrap();
    let payload_len = gpu.state.completions.values().next().unwrap().output[0]
        .1
        .len();
    assert_eq!(payload_len, 2 * 1024 * 1024);
    assert!(saved.bytes() < payload_len + payload_len / 100);
    let deadline = gpu.next_deadline(host.now).unwrap();
    let identity = gpu.fingerprint().unwrap();
    assert_eq!(host.writes, 0);
    let pending = gpu.state.completions.values_mut().next().unwrap();
    std::sync::Arc::make_mut(&mut pending.output[0].1)[0] ^= 1;
    assert_ne!(gpu.fingerprint().unwrap(), identity);
    gpu.restore_checkpoint(&saved).unwrap();
    assert_eq!(gpu.fingerprint().unwrap(), identity);
    assert_eq!(gpu.save(), diagnostic);
    assert_eq!(gpu.next_deadline(host.now), Some(deadline));
    host.now = deadline;
    gpu.advance(&mut Cx::new(&mut host, 0, 0, 2));
    assert_eq!(host.writes, 1);
    let expected = 0x12345678u32.to_le_bytes().repeat(payload_len / 4);
    assert_eq!(&host.bytes[128..128 + payload_len], expected);
}
#[test]
fn ambiguous_irq_unknown_commands_and_invalid_width_do_not_gain_dma_or_deadlines() {
    let (mut gpu, mut host, mut controllers) = setup();
    enable(&mut controllers, &mut host, 0);
    enable(&mut controllers, &mut host, 1);
    for (o, v) in [(0x10, 128), (0x14, 160), (0x18, 0)] {
        assert_eq!(put(&mut gpu, &mut host, o, v), None);
    }
    assert!(
        put(&mut gpu, &mut host, 0x1c, super::super::FILL_CONTROL.into())
            .unwrap()
            .contains("ambiguous")
    );
    assert_eq!(gpu.next_deadline(100), None);
    assert_eq!(host.writes, 0);
    let mut cx = Cx::new(&mut host, 0, 0, 2);
    gpu.write(&mut cx, super::super::BASE + 0x20, 2, 1);
    assert!(cx.take_refusal().is_some());
    assert_eq!(gpu.next_deadline(100), None);
    assert!(
        put(&mut gpu, &mut host, 0x1c, 1)
            .unwrap()
            .contains("fill control")
    );
    assert_eq!(host.writes, 0);
    assert!(host.effects.is_empty());
}

#[test]
fn linear_copy_keeps_all_pixel_bytes_through_dma_completion() {
    // The transfer preserves the full pixel word and does not apply the CPU
    // fallback's ROR8 conversion.
    let (mut gpu, mut host, mut controllers) = setup();
    gpu.state.copy_profile = Some(Profile::GpuCopyProfile);
    enable(&mut controllers, &mut host, 1);
    let (w, h) = (16usize, 16usize);
    let mut expected = Vec::new();
    for row in 0..h {
        for x in 0..w {
            let y = h - 1 - row;
            let pixel = [(x * 17) as u8, (y * 17) as u8, (x + y) as u8, 231];
            let at = 128 + fujimod_emulator_graphics::render::morton(w, x, y) * 4;
            host.bytes[at..at + 4].copy_from_slice(&pixel);
            expected.extend_from_slice(&pixel);
        }
    }
    for (o, v) in [
        (0xc00, 0x40000080),
        (0xc04, 0x40000800),
        (0xc08, 0x100010),
        (0xc0c, 0x100010),
        (0xc10, 1),
        (0xc18, 1),
    ] {
        assert_eq!(put(&mut gpu, &mut host, o, v), None);
    }
    let due = gpu.next_deadline(host.now).unwrap();
    assert_eq!(due, 164);
    let before = gpu.save();
    host.now = due - 1;
    gpu.advance(&mut Cx::new(&mut host, 0, 0, 2));
    assert_eq!(gpu.save(), before);
    assert_eq!(host.writes, 0);
    host.bytes[128..1152].fill(0); // Copy source was snapshotted at submission.
    host.now = due;
    gpu.advance(&mut Cx::new(&mut host, 0, 0, 2));
    assert_eq!(host.bytes[2048..3072], expected);
    assert_eq!(host.effects, [(1, IRQ.into())]);
    assert_eq!(host.writes, 1);
    assert_eq!(gpu.next_deadline(due), None);
}

#[test]
fn decoded_clear_preserves_pending_dma_deadline_restore_and_observational_reads() {
    let (path, fill) = fujimod_emulator_graphics::clear::fixtures::lists();
    let mut options = options();
    options.ram_bytes = 32768;
    options.clear_profile = Some(Profile::GpuClearProfile);
    let (mut gpu, mut host, mut controllers) = setup_with_options(options.clone());
    enable(&mut controllers, &mut host, 1);
    for (address, words) in [(8192, &path), (12288, &fill)] {
        for (index, word) in words.iter().enumerate() {
            host.bytes[address + index * 4..address + index * 4 + 4]
                .copy_from_slice(&word.to_le_bytes());
        }
    }
    for (offset, value) in [
        (0x10d4, 8192),
        (0x10dc, (path.len() / 2) as u64),
        (0x10d0, 0x10001),
    ] {
        assert_eq!(put(&mut gpu, &mut host, offset, value), None);
    }
    assert_eq!(gpu.next_deadline(host.now), None);
    for (offset, value) in [
        (0x10e4, 12288),
        (0x10ec, (fill.len() / 2) as u64),
        (0x10e0, 1),
    ] {
        assert_eq!(put(&mut gpu, &mut host, offset, value), None);
    }
    let before = gpu.save();
    let checkpoint = gpu.checkpoint().unwrap();
    let deadline = gpu.next_deadline(host.now).unwrap();
    assert_eq!(deadline, 104);
    assert_eq!(host.writes, 0);
    assert!(host.effects.is_empty());
    let _ = gpu.fingerprint().unwrap();
    assert_eq!(gpu.save(), before);
    host.now = deadline - 1;
    gpu.advance(&mut Cx::new(&mut host, 0, 0, 2));
    assert_eq!(gpu.save(), before);
    // Reconstruct a fresh device with its physical IRQ inputs, then restore
    // the pending DMA and exact completion tick without rereading the stream.
    let (mut restored, _, _controllers) = setup_with_options(options);
    restored.restore_checkpoint(&checkpoint).unwrap();
    assert_eq!(restored.save(), before);
    assert_eq!(restored.next_deadline(host.now), Some(deadline));
    host.bytes[8192..16384].fill(0);
    host.now = deadline;
    restored.advance(&mut Cx::new(&mut host, 0, 0, 2));
    assert_eq!(host.writes, 1);
    assert_eq!(host.effects, [(1, IRQ.into())]);
    for y in 0..8 {
        for x in 0..8 {
            let offset = 256 + fujimod_emulator_graphics::render::morton(8, x, y) * 4;
            let pixel = if (1..5).contains(&x) && (1..5).contains(&y) {
                &[0x80, 0x40, 0x20, 0x10]
            } else {
                &[0x81; 4]
            };
            assert_eq!(&host.bytes[offset..offset + 4], pixel);
        }
    }
    assert_eq!(restored.next_deadline(deadline), None);
}
