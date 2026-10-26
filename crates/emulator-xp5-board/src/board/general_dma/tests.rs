use super::*;
use fujimod_emulator_devices::Cx;
use fujimod_emulator_xp5_io::gpio::Gpio;
use fujimod_emulator_devices::xp5::pins::Pins;
use fujimod_emulator_devices::test_host::recording::RecordingHost;
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
                json!({"source_hz":48000000,"ticks_per_second":1000000000,"receive_request":2,"transmit_request":3,
                "wire":{"kind":"gpio-selected","endpoints":[
                    {"select":{"port":11,"pin":5,"active_high":false},"part":{"kind":"bis-configuration-registers","revision":[3,12],"idle_high":false}},
                    {"select":{"port":11,"pin":3,"active_high":false},"part":{"kind":"lsm6ds-configuration-registers"}}
                ]}}),
            ),
            window(
                0xff930000,
                0x8000,
                "general-dma",
                json!({"companion_irq_base":83}),
            ),
            window(0xff90e500, 0x20, "dma-request-routing", json!({})),
            window(0xff90e748, 0x40, "dma-address-high", json!({})),
        ];
        register_all(&mut graph, &windows, Some(4), pins).unwrap();
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
    assert_eq!(wire["parts"][1]["state"]["registers"][0x12], 0x44);
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
    f.graph.nodes[id].device.write(&mut cx, SPI + 8, 4, 0x9e); // STATUS_REG is outside the modeled configuration subset.
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
