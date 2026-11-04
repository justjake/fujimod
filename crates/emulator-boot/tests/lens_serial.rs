use fujimod_emulator_devices::{Cx, Host, Record};
use fujimod_emulator_exr2_plan::{Plan, RefuseAdapters};

struct Clock(u64);
impl Host for Clock {
    fn now(&self) -> u64 {
        self.0
    }
    fn schedule(&mut self, _: usize, _: u64, _: u64) {}
    fn set_irq(&mut self, _: u32, _: bool) {}
    fn raise_irq(&mut self, _: usize, _: u32) -> Result<(), String> {
        Ok(())
    }
    fn ram_read(&self, _: u64, _: &mut [u8]) -> Result<(), String> {
        Err("no RAM".into())
    }
    fn ram_write(&mut self, _: u64, _: &[u8]) -> Result<(), String> {
        Err("no RAM".into())
    }
    fn record(&mut self, _: Record) {}
}

#[test]
fn spi2_menu_status_read_keeps_the_lens_deselected_and_delivers_the_reply() {
    // MENU entry regression, 2026-11-04: configuration 6 sends a status-read
    // command with MCU select low and lens select high, then drains one reply.
    const GPIO: u64 = 0xfff89000;
    const SPI2: u64 = 0xfff84080;
    let document = fujimod_emulator_boot::prepare("x70-110-rec", &[])
        .unwrap()
        .to_json()["devices"]
        .clone();
    let body: serde_json::Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../catalog/bodies/x70/body.json"
    )))
    .unwrap();
    fn status_route(value: &serde_json::Value) -> Option<&serde_json::Value> {
        match value {
            serde_json::Value::Object(object) => {
                if object.get("name").and_then(serde_json::Value::as_str) == Some("lens_serial") {
                    return value.get("model")?.get("routes")?.get(1);
                }
                object.values().find_map(status_route)
            }
            serde_json::Value::Array(values) => values.iter().find_map(status_route),
            _ => None,
        }
    }
    assert_eq!(
        status_route(&document),
        status_route(&body),
        "catalog and boot status routes must agree"
    );
    assert!(status_route(&document).is_some());
    let shift = |configuration: u64, mcu_selected: bool, lens_selected: bool, status: u8| {
        let mut document = document.clone();
        document["nodes"].as_array_mut().unwrap().retain(|node| {
            ["irq0", "irq1", "serial_gpio", "pulse", "lens_serial"]
                .contains(&node["name"].as_str().unwrap())
        });
        fujimod_emulator_exr2_board::register_board_builder();
        fujimod_emulator_models::register();
        let mut built = Plan::parse(&serde_json::to_vec(&document).unwrap())
            .unwrap()
            .build(
                fujimod_emulator_exr2_resources::Resources::default(),
                0,
                &mut RefuseAdapters,
            )
            .unwrap();
        let gpio = built.node_ids["serial_gpio"];
        let spi = built.node_ids["lens_serial"];
        built.graph.nodes[spi]
            .device
            .serial_status_mut()
            .unwrap()
            .status_input(1, 0x60, status)
            .unwrap();
        let mut host = Clock(0);
        for (id, address, value) in [
            (
                gpio,
                GPIO + 13 * 0x40,
                1 << 25 | u64::from(!lens_selected) << 9,
            ),
            (
                gpio,
                GPIO + 12 * 0x40,
                1 << 19 | u64::from(!mcu_selected) << 3,
            ),
            (spi, SPI2, 0xe0),
            (spi, SPI2 + 4, configuration),
            (spi, SPI2 + 0x1c, 0x6000),
        ] {
            let mut cx = Cx::new(&mut host, 0, 0, id);
            built.graph.nodes[id]
                .device
                .write(&mut cx, address, 4, value);
            if let Some(error) = cx.take_refusal() {
                return Err(error);
            }
        }
        host.0 = 4;
        let mut cx = Cx::new(&mut host, 0, 0, spi);
        let complete = built.graph.nodes[spi].device.read(&mut cx, SPI2 + 0xc, 4);
        if let Some(error) = cx.take_refusal() {
            return Err(error);
        }
        assert_eq!(complete & 0x300, 0x300);
        let count = built.graph.nodes[spi].device.read(&mut cx, SPI2 + 0x10, 4);
        if configuration == 6 {
            assert_eq!(count & 31, 1);
            assert_eq!(
                built.graph.nodes[spi].device.read(&mut cx, SPI2 + 0x18, 4),
                u64::from(status)
            );
            assert_eq!(
                built.graph.nodes[spi].device.read(&mut cx, SPI2 + 0x10, 4) & 31,
                0
            );
        } else {
            assert_eq!(count & 31, 0);
        }
        Ok(())
    };
    assert_eq!(shift(4, false, true, 0), Ok(()));
    for status in [0, 1] {
        assert_eq!(shift(6, true, false, status), Ok(()));
    }
    for (configuration, mcu, lens) in [
        (5, true, false),
        (6, false, true),
        (6, true, true),
        (6, false, false),
    ] {
        assert!(
            shift(configuration, mcu, lens, 0)
                .unwrap_err()
                .contains("no attached SPI route")
        );
    }
}
