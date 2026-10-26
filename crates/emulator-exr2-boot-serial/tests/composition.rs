use fujimod_emulator_devices::{
    exr2::gpio::{self, Gpio},
    graph::DeviceGraph,
    parts::pins::Pin,
    test_host::ram::Ram,
};
use fujimod_emulator_exr2_boot_serial::{BootSerialBlocks, Wiring};
use fujimod_emulator_exr2_plan::serial::{FixedLowByteInput, Options, PinLevel};

#[test]
fn composed_serial_devices_complete_a_byte_through_the_shared_coordinator() {
    let gpio = Gpio::new(
        gpio::Options {
            input_levels: [0; 16],
            output_levels: vec![gpio::InitialOutput {
                pin: Pin { port: 0, pin: 0 },
                high: false,
            }],
        },
        gpio::Wiring { irqs: vec![] },
    )
    .unwrap();
    let select = PinLevel {
        pin: Pin { port: 0, pin: 0 },
        high: false,
    };
    let blocks = BootSerialBlocks::new(
        Options {
            transfer_ticks: 4,
            max_program_bytes: 1024,
            spi0: Some(FixedLowByteInput {
                required: [0xc0, 0x30, 0],
                selection: select,
                presence: select,
            }),
        },
        Wiring { gpio: gpio.pads() },
    )
    .unwrap();
    let mut graph = DeviceGraph::new();
    for (mapping, device) in blocks.devices() {
        match mapping {
            Some(mapping) => {
                graph.register(mapping.base, mapping.size, device).unwrap();
            }
            None => {
                graph.attach(device);
            }
        }
    }
    let mut host = Ram::new(4096);
    let spi = graph.at(0xfff8_4000).unwrap();
    for (offset, value) in [(0, 0xc0), (4, 0x30), (8, 0), (0x1c, 0x55)] {
        let mut cx = host.cx();
        graph.nodes[spi]
            .device
            .write(&mut cx, 0xfff8_4000 + offset, 4, value);
        assert!(cx.take_refusal().is_none());
    }
    host.now = 4;
    for node in &mut graph.nodes {
        let mut cx = host.cx();
        node.device.advance(&mut cx);
        assert!(cx.take_refusal().is_none());
    }
    let mut cx = host.cx();
    assert_eq!(graph.nodes[spi].device.read(&mut cx, 0xfff8_4010, 4), 1);
    assert_eq!(graph.nodes[spi].device.read(&mut cx, 0xfff8_4018, 4), 0);
    assert!(cx.take_refusal().is_none());
}
