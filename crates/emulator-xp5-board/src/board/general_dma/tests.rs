use super::*;
use fujimod_emulator_devices::Cx;
use fujimod_emulator_devices::test_host::recording::RecordingHost;
use fujimod_emulator_devices::xp5::pins::Pins;
use fujimod_emulator_xp5_io::gpio::Gpio;
use serde_json::json;
const GPIO: u64 = 0xff907000;
const SPI: u64 = 0xff911000;
const DMA: u64 = 0xff935000;
fn window(base: u64, size: u64, kind: &str, options: serde_json::Value) -> DeviceWindow {
    DeviceWindow {
        base,
        size,
        kind: kind.into(),
        options: serde_json::from_value(options).unwrap(),
    }
}
struct Fixture {
    graph: DeviceGraph,
    host: RecordingHost,
}
impl Fixture {
    fn new() -> Self {
        Self::with_dma(json!({"companion_irq_base":83}))
    }
    fn with_dma(dma: serde_json::Value) -> Self {
        let pins = Pins::shared();
        let mut graph = DeviceGraph::new();
        graph
            .register(
                GPIO,
                0x1000,
                Box::new(Gpio::new(GPIO, pins.clone(), false, vec![]).unwrap()),
            )
            .unwrap();
        let windows = vec![
            window(GPIO, 0x1000, "gpio", json!({})),
            window(
                SPI,
                0x1000,
                "pl022",
                json!({"source_hz":48000000,"ticks_per_second":1000000000,"receive_request":2,"transmit_request":3,"companion_irq_line":99,
                "wire":{"kind":"gpio-selected","endpoints":[
                    {"select":{"port":11,"pin":5,"active_high":false},"part":{"kind":"bis-configuration-registers","revision":[3,12],"samples":[0,0],"idle_high":false}},
                    {"select":{"port":11,"pin":3,"active_high":false},"part":{"kind":"lsm6ds-configuration-registers","temperature":0}}
                ]}}),
            ),
            window(0xff930000, 0x8000, "general-dma", dma),
            window(0xff90e500, 0x20, "dma-request-routing", json!({})),
            window(0xff90e748, 0x40, "dma-address-high", json!({})),
        ];
        register_all(
            &mut graph,
            &windows,
            Some(4),
            pins,
            Default::default(),
            None,
        )
        .unwrap();
        let mut f = Self {
            graph,
            host: RecordingHost {
                ram: vec![0xcc; 0x2000],
                ..Default::default()
            },
        };
        f.write(GPIO + 0x2c0, 0x00280028);
        for (offset, value) in [(0, 0xc7), (0x10, 6), (0x24, 3), (4, 0xa)] {
            f.write(SPI + offset, value);
        }
        f.write(0xff90e514, 0x01010203);
        f.write(DMA, 0x10000000);
        f
    }
    fn write(&mut self, address: u64, value: u64) {
        let id = self.graph.at(address).unwrap();
        let mut cx = Cx::new(&mut self.host, 4, 0xabcd, id);
        self.graph.nodes[id]
            .device
            .write(&mut cx, address, 4, value);
        assert_eq!(cx.refusal(), None, "{address:#x} <- {value:#x}");
    }
    fn until(&mut self, target: u64) {
        for _ in 0..10000 {
            let next = self
                .graph
                .nodes
                .iter()
                .filter_map(|n| n.device.next_deadline(self.host.now))
                .min();
            let Some(at) = next.filter(|&at| at <= target) else {
                self.host.now = target;
                return;
            };
            assert!(at >= self.host.now);
            self.host.now = at;
            for (id, node) in self.graph.nodes.iter_mut().enumerate() {
                if node.device.next_deadline(at).is_some_and(|t| t <= at) {
                    let mut cx = Cx::new(&mut self.host, 0, 0, id);
                    node.device.advance(&mut cx);
                    assert_eq!(cx.refusal(), None);
                }
            }
        }
        panic!("device deadline did not settle");
    }
    fn transfer(&mut self, select: u32, bytes: &[u8]) -> Vec<u8> {
        self.host.ram[0x1000..0x1000 + bytes.len()].copy_from_slice(bytes);
        self.write(GPIO + 0x2c0, 0x00280028 & !(1 << select));
        for (lane, source, destination, ss, ds) in
            [(0, 0x1000, SPI + 8, 0, 4), (1, SPI + 8, 0x1100, 4, 0)]
        {
            let base = DMA + lane * 0x30;
            for (offset, value) in [
                (0x30, 0),
                (0x10, bytes.len() as u64 - 1),
                (0x14, source),
                (0x18, destination),
                (0x1c, ss),
                (0x20, ds),
                (0x24, 0),
            ] {
                self.write(base + offset, value);
            }
        }
        self.write(DMA + 0x58, 0x10309011);
        self.write(DMA + 0x28, 0x10209011);
        self.until(self.host.now + bytes.len() as u64 * 1000);
        self.write(GPIO + 0x2c0, 0x00280028);
        self.host.ram[0x1100..0x1100 + bytes.len()].to_vec()
    }
}
#[test]
fn a53_software_dma_completion_is_per_channel_and_acknowledged() {
    let mut f = Fixture::with_dma(json!({"companion_irq_base":83,"a53_irq_base":237}));
    let bank = 0xff930000;
    let lane = bank + 0x30; // Channel 1, INTID 238.
    let source = [0x12, 0x34, 0x56, 0x78];
    f.host.ram[0x100..0x104].copy_from_slice(&source);
    f.write(0xff90e500, 0x01003f00); // Channel 1 software request selector.
    f.write(bank, 0x10000000);
    for (offset, value) in [
        (0x10, 3),
        (0x14, 0x100),
        (0x18, 0x200),
        (0x1c, 0),
        (0x20, 0),
        (0x24, 0),
    ] {
        f.write(lane + offset, value);
    }
    // Configuring a channel without a start must not copy or assert an interrupt.
    f.until(100);
    assert_eq!(&f.host.ram[0x200..0x204], &[0xcc; 4]);
    assert!(f.host.irqs.is_empty());
    assert!(f.host.core_irqs.is_empty());

    f.write(lane + 0x28, 0x11100001); // Enable, software start, normal interrupt.
    f.until(100);
    assert_eq!(&f.host.ram[0x200..0x204], &source);
    assert_eq!(f.host.irqs, [(238, true)]);
    assert_eq!(f.host.core_irqs, [(4, 84, true)]);
    let id = f.graph.at(lane + 0x30).unwrap();
    let mut cx = Cx::new(&mut f.host, 0, 0, id);
    assert_eq!(f.graph.nodes[id].device.read(&mut cx, lane + 0x30, 4), 8);
    assert_eq!(cx.refusal(), None);

    // The model's interrupt status is clear-by-zero, not write-one-to-clear.
    f.write(lane + 0x30, 0);
    assert_eq!(f.host.irqs, [(238, true), (238, false)]);
    assert_eq!(f.host.core_irqs, [(4, 84, true), (4, 84, false)]);
    let mut cx = Cx::new(&mut f.host, 0, 0, id);
    assert_eq!(f.graph.nodes[id].device.read(&mut cx, lane + 0x30, 4), 0);
    assert_eq!(cx.refusal(), None);
}

#[test]
fn companion_zero_base_routes_each_full_duplex_dma_completion_independently() {
    let mut f = Fixture::with_dma(json!({"companion_irq_base":0,"a53_irq_base":237}));
    let bank = 0xff936000; // Channels 12 and 13.
    f.write(0xff90e518, 0x01010203);
    f.write(bank, 0x10000000);
    f.write(GPIO + 0x2c0, 0x00280008); // Select the synthetic configuration endpoint.
    f.host.ram[0x1000..0x1002].copy_from_slice(&[0xa1, 0]);
    for (lane, source, destination, ss, ds) in
        [(0, 0x1000, SPI + 8, 0, 4), (1, SPI + 8, 0x1100, 4, 0)]
    {
        let base = bank + lane * 0x30;
        for (offset, value) in [
            (0x10, 1),
            (0x14, source),
            (0x18, destination),
            (0x1c, ss),
            (0x20, ds),
            (0x24, 0),
        ] {
            f.write(base + offset, value);
        }
    }
    // Configured but not started: neither lane can assert a completion.
    f.until(100);
    assert_eq!(&f.host.ram[0x1100..0x1102], &[0xcc; 2]);
    assert!(f.host.core_irqs.is_empty());
    assert!(f.host.irqs.is_empty());

    // Receive waits for the clocked SPI endpoint; it cannot complete alone.
    f.write(bank + 0x58, 0x10309011);
    f.until(200);
    assert!(f.host.core_irqs.is_empty());
    assert!(f.host.irqs.is_empty());
    f.write(bank + 0x28, 0x10209011);
    // The transmit FIFO drains before the last received frame is clocked.
    for _ in 0..100 {
        if !f.host.core_irqs.is_empty() {
            break;
        }
        f.until(f.host.now + 50);
    }
    assert_eq!(f.host.core_irqs, [(4, 12, true)]);
    assert_eq!(f.host.irqs, [(249, true)]);
    assert_eq!(f.host.ram[0x1101], 0xcc);
    f.until(2200);
    assert_eq!(&f.host.ram[0x1100..0x1102], &[0, 3]);
    assert_eq!(f.host.core_irqs, [(4, 12, true), (4, 13, true)]);
    assert_eq!(f.host.irqs, [(249, true), (250, true)]);

    // Clear each lane separately; the other lane's outputs remain asserted.
    f.write(bank + 0x30, 0);
    assert_eq!(
        f.host.core_irqs,
        [(4, 12, true), (4, 13, true), (4, 12, false)]
    );
    assert_eq!(f.host.irqs, [(249, true), (250, true), (249, false)]);
    f.write(bank + 0x60, 0);
    assert_eq!(
        f.host.core_irqs,
        [(4, 12, true), (4, 13, true), (4, 12, false), (4, 13, false)]
    );
    assert_eq!(
        f.host.irqs,
        [(249, true), (250, true), (249, false), (250, false)]
    );
}

#[test]
fn graph_connects_both_request_edges_and_clocked_data_to_actual_companion_irq_lines() {
    let mut f = Fixture::new();
    let serial = f.graph.at(SPI).unwrap();
    let pair = f.graph.at(DMA).unwrap();
    let pins = f.graph.at(GPIO).unwrap();
    assert!(f.graph.nodes[serial].native_dependencies.contains(&pair));
    assert!(f.graph.nodes[serial].native_dependencies.contains(&pins));
    assert!(f.graph.nodes[pair].native_dependencies.contains(&serial));
    assert!(
        f.graph.nodes[pair]
            .native_dependencies
            .contains(&f.graph.at(0xff90e514).unwrap())
    );
    assert_eq!(f.transfer(5, &[9]), [0]);
    assert_eq!(f.host.core_irqs, [(4, 93, true), (4, 94, true)]);
    assert_eq!(f.transfer(5, &[1, 0x5a]), [0, 0]);
    assert_eq!(f.transfer(5, &[0xa1, 0]), [0, 3]);
    assert_eq!(f.transfer(5, &[0xa2, 0]), [0, 12]);
    let state = f.graph.nodes[serial].device.save();
    assert_eq!(
        state["wire"]["parts"][0]["state"]["state"]["registers"][0],
        0x5a
    );
}
#[test]
fn selected_sensor_reset_readback_and_config_writes_execute_over_the_same_dma_ports() {
    let mut f = Fixture::new();
    assert_eq!(f.transfer(3, &[0x12, 5]), [0, 0]);
    assert_eq!(f.transfer(3, &[0x92, 0]), [0, 4]);
    assert_eq!(f.transfer(3, &[0x12, 0x44]), [0, 0]);
    assert_eq!(f.transfer(3, &[0x73, 0x23, 0x45, 0x67]), [0, 0, 0, 0]);
    assert_eq!(f.transfer(3, &[0xf3, 0, 0, 0]), [0, 0x23, 0x45, 0x67]);
    let serial = f.graph.at(SPI).unwrap();
    let before = f.graph.nodes[serial].device.save();
    for _ in 0..10 {
        assert_eq!(f.graph.nodes[serial].device.save(), before);
    }
    let wire = &before["wire"];
    assert_eq!(wire["parts"][1]["state"]["state"]["registers"][0x12], 0x44);
}

#[test]
fn pending_selected_wire_and_dma_restore_without_transfers_or_interrupts() {
    let mut f = Fixture::new();
    f.host.ram[0x1000..0x1002].copy_from_slice(&[1, 0x6b]);
    f.write(GPIO + 0x2c0, 0x00280008);
    for (lane, source, destination, ss, ds) in
        [(0, 0x1000, SPI + 8, 0, 4), (1, SPI + 8, 0x1100, 4, 0)]
    {
        let base = DMA + lane * 0x30;
        for (offset, value) in [
            (0x10, 1),
            (0x14, source),
            (0x18, destination),
            (0x1c, ss),
            (0x20, ds),
            (0x24, 0),
        ] {
            f.write(base + offset, value);
        }
    }
    f.write(DMA + 0x58, 0x10309011);
    f.write(DMA + 0x28, 0x10209011);
    f.until(1500);
    let states: Vec<_> = f.graph.nodes.iter().map(|n| n.device.save()).collect();
    let mut fresh = Fixture::new();
    fresh.host.ram = f.host.ram.clone();
    fresh.host.now = f.host.now;
    for (node, state) in fresh.graph.nodes.iter_mut().zip(&states) {
        node.device.restore(state).unwrap();
    }
    assert!(fresh.host.core_irqs.is_empty());
    assert_eq!(
        fresh
            .graph
            .nodes
            .iter()
            .map(|n| n.device.save())
            .collect::<Vec<_>>(),
        states
    );
    f.host.core_irqs.clear();
    f.until(2000);
    fresh.until(2000);
    assert_eq!(fresh.host.ram, f.host.ram);
    assert_eq!(fresh.host.core_irqs, f.host.core_irqs);
    assert_eq!(
        fresh
            .graph
            .nodes
            .iter()
            .map(|n| n.device.save())
            .collect::<Vec<_>>(),
        f.graph
            .nodes
            .iter()
            .map(|n| n.device.save())
            .collect::<Vec<_>>()
    );
}

#[test]
fn unknown_sensor_data_and_changed_chip_select_refuse_without_rx_completion() {
    let mut f = Fixture::new();
    let id = f.graph.at(SPI).unwrap();
    f.write(GPIO + 0x2c0, 0x00280020); // Sensor selected, configuration part idle.
    let mut cx = Cx::new(&mut f.host, 4, 0xfeed, id);
    f.graph.nodes[id].device.write(&mut cx, SPI + 8, 4, 0xb0); // Register 0x30 is outside the modeled subset.
    assert!(cx.is_unmodeled());
    assert_eq!(f.graph.nodes[id].device.next_deadline(f.host.now), None);
    f.write(GPIO + 0x2c0, 0x00280008);
    f.write(SPI + 8, 1);
    f.write(GPIO + 0x2c0, 0x00280028); // Drop selection before the frame completes.
    f.host.now += 1000;
    let mut cx = Cx::new(&mut f.host, 4, 0, id);
    f.graph.nodes[id].device.advance(&mut cx);
    assert!(cx.is_unmodeled());
    assert!(f.host.core_irqs.is_empty());
    assert_eq!(f.graph.nodes[id].device.save()["state"]["rx"], json!([]));
}

fn ssp5_windows(options: serde_json::Value) -> Vec<DeviceWindow> {
    vec![
        window(GPIO, 0x1000, "gpio", json!({})),
        window(0xff915000, 0x1000, "pl022", options),
        window(
            0xff930000,
            0x8000,
            "general-dma",
            json!({"companion_irq_base":83}),
        ),
        window(0xff90e500, 0x20, "dma-request-routing", json!({})),
        window(0xff90e748, 0x40, "dma-address-high", json!({})),
    ]
}

#[test]
fn ssp_instance_without_declared_dma_requests_or_irq_polls_and_refuses_dma() {
    let wire = json!({"kind":"gpio-selected","endpoints":[
        {"select":{"port":12,"pin":8,"active_high":false},"part":{"kind":"bis-configuration-registers","revision":[0,0],"samples":[0,0],"idle_high":false}}
    ]});
    let pins = Pins::shared();
    let mut graph = DeviceGraph::new();
    graph
        .register(
            GPIO,
            0x1000,
            Box::new(Gpio::new(GPIO, pins.clone(), false, vec![]).unwrap()),
        )
        .unwrap();
    let windows = ssp5_windows(
        json!({"source_hz":48000000,"ticks_per_second":1000000000,"wire":wire.clone()}),
    );
    register_all(
        &mut graph,
        &windows,
        Some(4),
        pins.clone(),
        Default::default(),
        None,
    )
    .unwrap();
    let mut f = Fixture {
        graph,
        host: RecordingHost {
            ram: vec![0; 0x2000],
            ..Default::default()
        },
    };
    // The companion's receive-FIFO drain reads the status word (bit 2, receive not empty).
    let id = f.graph.at(0xff91500c).unwrap();
    let mut cx = Cx::new(&mut f.host, 4, 0x386c, id);
    let status = f.graph.nodes[id].device.read(&mut cx, 0xff91500c, 2);
    assert_eq!(cx.refusal(), None);
    assert_eq!(status & 4, 0);
    // No request number reaches this instance, so a DMA channel aimed at it refuses.
    for (offset, value) in [(0, 0xc7), (0x10, 6), (0x24, 3), (4, 0xa)] {
        f.write(0xff915000 + offset, value);
    }
    f.write(0xff90e514, 0x01010203);
    f.write(DMA, 0x10000000);
    for (offset, value) in [
        (0x10, 0),
        (0x14, 0x1000),
        (0x18, 0xff915008),
        (0x1c, 0),
        (0x20, 4),
        (0x24, 0),
    ] {
        f.write(DMA + offset, value);
    }
    let id = f.graph.at(DMA).unwrap();
    let mut cx = Cx::new(&mut f.host, 4, 0, id);
    f.graph.nodes[id]
        .device
        .write(&mut cx, DMA + 0x28, 4, 0x10209011);
    assert!(
        cx.refusal()
            .is_some_and(|r| r.contains("no connected peripheral; request=3")),
        "{:?}",
        cx.refusal()
    );
    assert!(f.host.core_irqs.is_empty());

    for options in [
        json!({"source_hz":48000000,"ticks_per_second":1000000000,"receive_request":2,"wire":wire.clone()}),
        json!({"source_hz":48000000,"ticks_per_second":1000000000,"receive_request":2,"transmit_request":2,"wire":wire.clone()}),
    ] {
        let pins = Pins::shared();
        let error = register_all(
            &mut DeviceGraph::new(),
            &ssp5_windows(options),
            Some(4),
            pins,
            Default::default(),
            None,
        );
        assert_eq!(
            error.err().as_deref(),
            Some("invalid XP5 serial DMA request wiring")
        );
    }
    let mut outside =
        ssp5_windows(json!({"source_hz":48000000,"ticks_per_second":1000000000,"wire":wire}));
    outside[1].base = 0xff91c000;
    let error = register_all(
        &mut DeviceGraph::new(),
        &outside,
        Some(4),
        Pins::shared(),
        Default::default(),
        None,
    );
    assert_eq!(error.err().as_deref(), Some("invalid XP5 PL022 window"));
}

#[test]
fn ssp_instance_with_an_unconnected_interrupt_output_unmasks_without_driving_a_line() {
    let wire = json!({"kind":"gpio-selected","endpoints":[
        {"select":{"port":12,"pin":8,"active_high":false},"part":{"kind":"bis-configuration-registers","revision":[0,0],"samples":[0,0],"idle_high":false}}
    ]});
    let options = |extra: serde_json::Value| {
        let mut options =
            json!({"source_hz":48000000,"ticks_per_second":1000000000,"wire":wire.clone()});
        options
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        options
    };
    for (extra, refused) in [
        (json!({"companion_irq_unconnected":true}), false),
        (json!({}), true),
    ] {
        let pins = Pins::shared();
        let mut graph = DeviceGraph::new();
        graph
            .register(
                GPIO,
                0x1000,
                Box::new(Gpio::new(GPIO, pins.clone(), false, vec![]).unwrap()),
            )
            .unwrap();
        register_all(
            &mut graph,
            &ssp5_windows(options(extra)),
            Some(4),
            pins,
            Default::default(),
            None,
        )
        .unwrap();
        let mut f = Fixture {
            graph,
            host: RecordingHost {
                ram: vec![0; 0x2000],
                ..Default::default()
            },
        };
        for (offset, value) in [(0, 0xc7), (0x10, 6), (0x24, 3), (4, 0xa)] {
            f.write(0xff915000 + offset, value);
        }
        // The companion's SSP setup unmasks all four interrupts with the transmit FIFO
        // empty, so the transmit interrupt goes high.
        let id = f.graph.at(0xff915014).unwrap();
        let mut cx = Cx::new(&mut f.host, 4, 0x20084, id);
        f.graph.nodes[id].device.write(&mut cx, 0xff915014, 4, 0xf);
        assert_eq!(cx.is_unmodeled(), refused);
        assert!(f.host.core_irqs.is_empty() && f.host.irqs.is_empty());
    }
    let both = options(json!({"companion_irq_line":99,"companion_irq_unconnected":true}));
    let error = register_all(
        &mut DeviceGraph::new(),
        &ssp5_windows(both),
        Some(4),
        Pins::shared(),
        Default::default(),
        None,
    );
    assert_eq!(
        error.err().as_deref(),
        Some("PL022 interrupt output is both connected and unconnected")
    );
}

#[test]
fn a53_irq_base_adds_one_gic_spi_per_channel_beside_the_companion_line() {
    let f = Fixture::new();
    let bank0 = f.graph.at(0xff930000).unwrap();
    assert_eq!(
        f.graph.nodes[bank0].device.save()["interrupts"],
        json!([[{"Core":{"core":4,"line":83}}],[{"Core":{"core":4,"line":84}}]])
    );
    let mut f = Fixture::with_dma(json!({"companion_irq_base":83,"a53_irq_base":237}));
    // X-T5 4.31 channel 1 (bank 0, lane 1) completes the DDR PHY training copy on INTID 238.
    let bank0 = f.graph.at(0xff930000).unwrap();
    assert_eq!(
        f.graph.nodes[bank0].device.save()["interrupts"],
        json!([
            [{"Core":{"core":4,"line":83}},{"Shared":237}],
            [{"Core":{"core":4,"line":84}},{"Shared":238}]
        ])
    );
    let bank7 = f.graph.at(0xff937000).unwrap();
    assert_eq!(
        f.graph.nodes[bank7].device.save()["interrupts"][1],
        json!([{"Core":{"core":4,"line":98}},{"Shared":252}])
    );
    // Bank 5 is channels 10 and 11: both completions reach both controllers.
    assert_eq!(f.transfer(5, &[9]), [0]);
    assert_eq!(f.host.core_irqs, [(4, 93, true), (4, 94, true)]);
    assert_eq!(f.host.irqs, [(247, true), (248, true)]);
}

#[test]
fn a53_irq_base_outside_the_gic_spi_range_refuses() {
    for base in [31, 1005] {
        let windows = vec![
            window(
                0xff930000,
                0x8000,
                "general-dma",
                json!({"companion_irq_base":83,"a53_irq_base":base}),
            ),
            window(0xff90e500, 0x20, "dma-request-routing", json!({})),
            window(0xff90e748, 0x40, "dma-address-high", json!({})),
        ];
        let mut graph = DeviceGraph::new();
        assert!(
            register_all(
                &mut graph,
                &windows,
                Some(4),
                Pins::shared(),
                Default::default(),
                None
            )
            .is_err()
        );
    }
}

#[test]
fn the_xt5_lens_fragment_registers_the_lens_bus_connector() {
    let fragment: toml::Table = toml::from_str(include_str!(
        "../../../../../catalog/firmware/xt5/4.31/xp5-boot/manifest.d/45-lens.toml"
    ))
    .unwrap();
    let device = fragment["device"].as_array().unwrap()[0]
        .as_table()
        .unwrap();
    let lens = DeviceWindow {
        base: device["base"].as_integer().unwrap() as u64,
        size: device["size"].as_integer().unwrap() as u64,
        kind: device["kind"].as_str().unwrap().into(),
        options: device["options"].as_table().unwrap().clone(),
    };
    let pins = Pins::shared();
    let mut graph = DeviceGraph::new();
    graph
        .register(
            GPIO,
            0x1000,
            Box::new(Gpio::new(GPIO, pins.clone(), false, vec![]).unwrap()),
        )
        .unwrap();
    let windows = vec![
        window(GPIO, 0x1000, "gpio", json!({})),
        lens,
        window(
            0xff930000,
            0x8000,
            "general-dma",
            json!({"companion_irq_base":83,"a53_irq_base":237}),
        ),
        window(0xff90e500, 0x20, "dma-request-routing", json!({})),
        window(0xff90e748, 0x40, "dma-address-high", json!({})),
    ];
    register_all(
        &mut graph,
        &windows,
        Some(4),
        pins.clone(),
        Default::default(),
        None,
    )
    .unwrap();
    let id = graph.at(0xff910000).unwrap();
    assert_eq!(graph.nodes[id].device.name(), "xp5-x-mount-lens");
    let attached = std::env::var("XT5_LENS").map_or(true, |v| v != "none");
    assert_eq!(pins.lock().unwrap().level(0, 11) == 0, attached);
}
#[test]
fn standalone_views_work_without_dma_or_companion_and_keep_checkpoint_state() {
    let mut graph = DeviceGraph::new();
    let windows = vec![
        window(0xff90e748, 0x40, "dma-address-high", json!({})),
        window(0xff90e500, 0x20, "dma-request-routing", json!({})),
    ];
    register_all(
        &mut graph,
        &windows,
        None,
        Pins::shared(),
        Default::default(),
        None,
    )
    .unwrap();
    let mut host = RecordingHost::default();
    let id = graph.at(0xff90e74c).unwrap();
    let mut cx = Cx::new(&mut host, 4, 0, id);
    let view = &mut graph.nodes[id].device;
    view.write(&mut cx, 0xff90e74c, 4, 0x12340001);
    assert_eq!(view.read(&mut cx, 0xff90e74c, 4), 0x12340001);
    assert_eq!(cx.refusal(), None);
    let checkpoint = view.checkpoint().unwrap();
    view.write(&mut cx, 0xff90e74c, 4, 0);
    view.restore_checkpoint(&checkpoint).unwrap();
    assert_eq!(view.read(&mut cx, 0xff90e74c, 4), 0x12340001);
    for (address, width) in [
        (0xff90e74c, 1),
        (0xff90e74c, 2),
        (0xff90e74d, 4),
        (0xff90e788, 4),
    ] {
        let mut cx = Cx::new(&mut host, width, 0, id);
        view.write(&mut cx, address, width as u8, 0);
        assert!(cx.is_unmodeled());
        let mut cx = Cx::new(&mut host, width, 0, id);
        view.read(&mut cx, address, width as u8);
        assert!(cx.is_unmodeled());
    }
    let route_id = graph.at(0xff90e500).unwrap();
    let mut cx = Cx::new(&mut host, 4, 0, route_id);
    let route = &mut graph.nodes[route_id].device;
    route.write(&mut cx, 0xff90e500, 4, 0x10012);
    let checkpoint = route.checkpoint().unwrap();
    route.write(&mut cx, 0xff90e500, 4, 0x10000);
    route.restore_checkpoint(&checkpoint).unwrap();
    assert_eq!(route.read(&mut cx, 0xff90e500, 4), 0x12);
    assert_eq!(cx.refusal(), None);
    assert!(graph.at(0xff930000).is_err());
    assert!(host.core_irqs.is_empty());
}

#[test]
fn dma_still_requires_companion_and_both_views() {
    let dma = window(
        0xff930000,
        0x8000,
        "general-dma",
        json!({"companion_irq_base":83}),
    );
    assert!(
        register_all(
            &mut DeviceGraph::new(),
            std::slice::from_ref(&dma),
            None,
            Pins::shared(),
            Default::default(),
            None
        )
        .is_err()
    );
    assert!(
        register_all(
            &mut DeviceGraph::new(),
            &[dma],
            Some(4),
            Pins::shared(),
            Default::default(),
            None
        )
        .is_err()
    );
}
