use fujimod_emulator_devices::exr2::irq_routes::Irq;
use fujimod_emulator_imaging::exr2_isp::*;
#[test]
fn dma_bounds_clock_and_options_are_strict() {
    assert_eq!(dma::span(0x40004000, 4).unwrap(), 0x4000..0x4004);
    for (a, n) in [
        (0, 4),
        (0x3ffffffc, 8),
        (0x7ffffffc, 8),
        (0x80000000, 4),
        (u64::MAX, 4),
        (0x4000, dma::MAX_BYTES + 1),
        (0x4000, 0),
    ] {
        assert!(dma::span(a, n).is_err());
    }
    assert!(due(u64::MAX, 1).is_err());
    assert!(
        serde_json::from_str::<fujimod_emulator_imaging::exr2_isp::options::output::Options>(
            r#"{"stop_ticks":2,"clock":{"kind":"fixed","ticks":5},"geometry":"skip_incomplete","profile":"old"}"#
        )
        .is_err()
    );
}
#[test]
fn row_dma_early_calls_padding_alias_and_order() {
    let mut h = fujimod_emulator_devices::test_host::TestHost::new();
    h.ram.fill(0xa5);
    let mut t = dma::Transfer::default();
    let r = dma::Raster {
        address: 0x40008000,
        width: 2,
        height: 2,
        pitch: 8,
    };
    t.prepare(0, 10, r, 4).unwrap();
    t.pixels.copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
    h.now = 9;
    assert!(!t.write(&mut h.cx()).unwrap());
    assert!(h.effects.is_empty());
    let pending = serde_json::to_value(&t).unwrap();
    assert_eq!(
        pending["pixels"],
        serde_json::json!([1, 2, 3, 4, 5, 6, 7, 8])
    );
    // A pending transfer keeps its payload across a stopped-board save.
    let mut t: dma::Transfer = serde_json::from_value(pending).unwrap();
    h.now = 10;
    assert!(t.write(&mut h.cx()).unwrap());
    h.effects.push("status");
    dma::Transfer::interrupt(&mut h.cx(), Irq { core: 1, line: 61 });
    assert_eq!(h.effects, ["dma", "dma", "status", "irq"]);
    assert_eq!(
        &h.ram[0x8000..0x8010],
        &[
            1, 2, 3, 4, 0xa5, 0xa5, 0xa5, 0xa5, 5, 6, 7, 8, 0xa5, 0xa5, 0xa5, 0xa5
        ]
    );
    assert!(!t.write(&mut h.cx()).unwrap());
    let completed = serde_json::to_value(&t).unwrap();
    assert_eq!(completed["pixels"], serde_json::json!([]));
    assert_eq!(completed["row_bytes"], 0);
    assert!(completed["raster"].is_null());
    assert!(t.prepare(0, 1, dma::Raster { pitch: 2, ..r }, 4).is_err());
    assert!(t.prepare(u64::MAX, 1, r, 4).is_err());
    assert_eq!(dma::physical(0x1ffffffc, 8).unwrap(), 0x1ffffffc);
}

#[test]
fn xt1_statistics_span_uses_high_dram_without_truncation() {
    use fujimod_emulator_devices::{Cx, Host, Record};
    // X-T1 5.52 strict run, 56,921,270 instructions / tick 18,800,000:
    // FFF18508=7FECB2B0 selects the external 1024-byte bank. The declared
    // body aperture ends at 3FFF0000; this is not a measured RAM population.
    const ADDRESS: u64 = 0x7fecb2b0;
    assert_eq!(dma::span(ADDRESS, 1024).unwrap(), 0x3fecb2b0..0x3fecb6b0);
    assert_eq!(
        dma::span(0x3fecb2b0, 1024).unwrap(),
        dma::span(ADDRESS, 1024).unwrap()
    );
    assert_eq!(dma::span(0x7ffffffc, 4).unwrap(), 0x3ffffffc..0x40000000);

    struct Backing {
        limit: u64,
        writes: Vec<(u64, Vec<u8>)>,
    }
    impl Host for Backing {
        fn now(&self) -> u64 {
            10
        }
        fn schedule(&mut self, _: usize, _: u64, _: u64) {
            panic!("unexpected schedule")
        }
        fn set_irq(&mut self, _: u32, _: bool) {
            panic!("unexpected IRQ")
        }
        fn raise_irq(&mut self, _: usize, _: u32) -> Result<(), String> {
            panic!("unexpected IRQ")
        }
        fn ram_read(&self, _: u64, _: &mut [u8]) -> Result<(), String> {
            panic!("unexpected read")
        }
        fn ram_write(&mut self, address: u64, bytes: &[u8]) -> Result<(), String> {
            let start = dma::physical(address, bytes.len())?;
            if start + bytes.len() as u64 > self.limit {
                return Err("unmapped body RAM".into());
            }
            self.writes.push((start, bytes.to_vec()));
            Ok(())
        }
        fn record(&mut self, _: Record) {
            panic!("unexpected recording")
        }
    }
    let mut transfer = dma::Transfer::default();
    transfer
        .prepare(
            0,
            10,
            dma::Raster {
                address: ADDRESS,
                width: 256,
                height: 1,
                pitch: 1024,
            },
            1024,
        )
        .unwrap();
    transfer.pixels.fill(0x5a);
    let mut backing = Backing {
        limit: 0x20000000,
        writes: vec![],
    };
    assert_eq!(
        transfer.write(&mut Cx::new(&mut backing, 0, 0, 0)),
        Err("unmapped body RAM".into())
    );
    assert!(backing.writes.is_empty());
    assert_eq!(transfer.deadline(), Some(10));
    backing.limit = 0x3fff0000;
    assert!(transfer.write(&mut Cx::new(&mut backing, 0, 0, 0)).unwrap());
    assert_eq!(backing.writes, [(0x3fecb2b0, vec![0x5a; 1024])]);
    assert_eq!(transfer.deadline(), None);
}

#[test]
fn transfer_clock_uses_payload_size_instead_of_sensor_cadence() {
    use fujimod_emulator_imaging::exr2_isp::dma::Clock;
    let clock = Clock::Throughput { bytes_per_tick: 16 };
    assert_eq!(clock.delay(16).unwrap(), 1);
    assert_eq!(clock.delay(17).unwrap(), 2);
    assert_eq!(clock.delay(2048 * 208 * 2).unwrap(), 53248);
    assert!(Clock::Throughput { bytes_per_tick: 0 }.delay(1).is_err());
    assert!(Clock::Fixed { ticks: 0 }.delay(1).is_err());
}
