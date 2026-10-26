use fujimod_emulator_devices::{Cx, test_host::ram::Ram};
use fujimod_emulator_exr2_plan::*;
use fujimod_emulator_exr2_resources::*;
use serde_json::{Value, json};

fn node(name: &str, base: u64, size: u64, model: Value) -> Value {
    json!({"name":name,"boot_entries":["rom","loader"],"windows":[{"base":base,"size":size}],"dependencies":[],"model":model})
}
fn document(nodes: Vec<Value>) -> Value {
    json!({"schema":1,"boot_entry":"loader","wires":{},"nodes":nodes})
}
fn parse(value: Value) -> Result<Plan, String> {
    Plan::parse(&serde_json::to_vec(&value).unwrap())
}
fn basic() -> Value {
    document(vec![node(
        "output",
        0xfff86000,
        12,
        json!({"kind":"block86000","options":{}}),
    )])
}
#[test]
fn xm1_contact_response_uses_scenario_gpio_phases_and_physical_irq_routes() {
    let mut input_levels = [0u32; 16];
    input_levels[0] = (1 << 9) | (1 << 6);
    let gpio = node(
        "pads",
        0xfff89000,
        0x480,
        json!({
            "kind":"gpio",
            "options":{
                "input_levels":input_levels,
                "output_levels":[
                    {"pin":{"port":1,"pin":1},"high":false},
                    {"pin":{"port":1,"pin":2},"high":false}
                ]
            },
            "irqs":[
                {"pin":{"port":0,"pin":9},"edge":"rising","outputs":[{"Core":{"core":0,"line":17}}]},
                {"pin":{"port":0,"pin":6},"edge":"falling","outputs":[{"Core":{"core":0,"line":17}}]}
            ]
        }),
    );
    let mut response = node(
        "contact_response",
        0,
        0,
        json!({
            "kind":"gpio-contact-response",
            "options":{
                "drive_pins":[{"port":1,"pin":1},{"port":1,"pin":2}],
                "sense_pins":[{"port":0,"pin":9},{"port":0,"pin":6}],
                "phases":[
                    {"drive":[false,false],"sense":[true,true]},
                    {"drive":[true,false],"sense":[false,false]},
                    {"drive":[true,true],"sense":[true,true]}
                ],
                "initial_phase":0,
                "settle_ticks":3000
            },
            "gpio":"pads"
        }),
    );
    response["windows"] = json!([]);
    let interrupt_controller = node(
        "irq0",
        0xfffb3000,
        0x1000,
        json!({
            "kind":"exr2-interrupt-controller",
            "options":{"arbitration":"unique"},
            "core":0,
            "base":0xfffb3000u64,
            "routes":null,
            "software":[]
        }),
    );
    let built = build(document(vec![gpio, response, interrupt_controller])).unwrap();
    let response_id = built.node_ids["contact_response"];
    assert_eq!(
        built.graph.nodes[response_id].device.name(),
        "gpio-contact-response"
    );
    assert_eq!(built.graph.nodes[response_id].native_dependencies, [0]);
    let fingerprint = built.graph.nodes[response_id].device.fingerprint().unwrap();
    assert_eq!(fingerprint["options"]["initial_phase"], 0);
    assert_eq!(
        fingerprint["options"]["phases"].as_array().unwrap().len(),
        3
    );
}
#[test]
fn timer_constructor_uses_declared_controller_outputs() {
    let value = document(vec![
        controller("irq0", 0, 0xfffb3000),
        controller("irq1", 1, 0xfffb4000),
        node(
            "timers",
            0xfff91000,
            0x40,
            json!({"kind":"exr2-system-timers","options":{"main_targets":[0,1]},"controllers":["irq0","irq1"]}),
        ),
    ]);
    let mut value = value;
    value["wires"] = json!({"routing":"irq-routes"});
    let built = build(value.clone()).unwrap();
    assert_eq!(built.graph.nodes[2].device.name(), "exr2-system-timers");
    assert_eq!(built.graph.nodes[2].native_dependencies, [0, 1]);
    value["nodes"][2]["model"]["controllers"] = json!(["irq0", "irq0"]);
    assert!(build(value).is_err());
}
#[test]
fn block98000_uses_a_declared_execution_port_and_its_real_constructor() {
    let value = document(vec![node(
        "boot",
        0xfff98000,
        0x300,
        json!({
            "kind":"block-98000","options":{"boot_source":3,"selection_reset":0},
            "wiring":{"decoder":0,"selections":[{"word":0,"choice":2},{"word":1,"choice":0}]}
        }),
    )]);
    let plan = parse(value.clone()).unwrap();
    plan.validate_memory_outputs(&[MemoryChoices {
        initial: 2,
        choices: 3,
    }])
    .unwrap();
    for ports in [
        vec![],
        vec![MemoryChoices {
            initial: 0,
            choices: 3,
        }],
        vec![MemoryChoices {
            initial: 2,
            choices: 2,
        }],
    ] {
        assert!(plan.validate_memory_outputs(&ports).is_err());
    }
    let mut built = build(value).unwrap();
    assert_eq!(built.graph.nodes[0].device.name(), "block-98000");
    let mut host = Ram::new(0);
    assert_eq!(get(&mut built, &mut host, 0xfff98000, 4), 3);
    assert_eq!(get(&mut built, &mut host, 0xfff98008, 4), 0);
}
#[test]
fn core_release_has_checked_reset_state_one_signal_writer_and_a_causal_dependency() {
    let mut value = document(vec![
        controller("irq", 1, 0xfffb4000),
        json!({
            "name":"release","boot_entries":["rom","loader"],"windows":[],"dependencies":[],
            "model":{"kind":"core-release","options":{"released":false},"core":1,"request":"release"}
        }),
    ]);
    value["wires"] = json!({"routing":"irq-routes","release":"software-pulse"});
    value["nodes"][0]["model"]["software"] = json!([{"line":120,"pulse":"release"}]);
    let plan = parse(value.clone()).unwrap();
    plan.validate_reset_outputs(&[true, false]).unwrap();
    for states in [vec![], vec![true], vec![true, true]] {
        assert!(plan.validate_reset_outputs(&states).is_err());
    }
    let built = build(value.clone()).unwrap();
    assert_eq!(built.graph.nodes[1].native_dependencies, [0]);
    assert_eq!(built.graph.nodes[1].device.name(), "core-release");
    value["nodes"][0]["model"]["software"]
        .as_array_mut()
        .unwrap()
        .push(json!({"line":119,"pulse":"release"}));
    assert!(
        build(value)
            .err()
            .unwrap()
            .contains("one controller output")
    );
}
fn build(value: Value) -> Result<BuiltBoard, String> {
    build_with_models(parse(value)?, Resources::default(), 0, &mut RefuseAdapters)
}

#[test]
fn schema_kinds_fields_ranges_and_boot_dependencies_are_checked() {
    let original = basic();
    assert!(build(original.clone()).is_ok());
    for (pointer, value) in [
        ("/schema", json!(2)),
        ("/nodes/0/model/kind", json!("imagined-device")),
        ("/nodes/0/name", json!("unsafe/name")),
        ("/nodes/0/boot_entries", json!(["rom", "rom"])),
        ("/nodes/0/windows/0/size", json!(0)),
        ("/nodes/0/windows/0/base", json!(0x1_0000_0000u64)),
        ("/nodes/0/dependencies", json!(["missing"])),
        ("/nodes/0/dependencies", json!(["output"])),
    ] {
        let mut changed = original.clone();
        *changed.pointer_mut(pointer).unwrap() = value;
        assert!(parse(changed).is_err(), "{pointer}");
    }
    let mut changed = original.clone();
    changed["nodes"][0]["model"]["options"]["profile"] = json!("old");
    assert!(parse(changed).is_err());
    let mut changed = original.clone();
    changed["ignored"] = json!(true);
    assert!(parse(changed).is_err());
    let mut changed = original;
    let first = changed["nodes"][0].clone();
    changed["nodes"].as_array_mut().unwrap().push(first);
    assert!(parse(changed).is_err());
    let mut dependency = node(
        "source",
        0xfff86000,
        12,
        json!({"kind":"block86000","options":{}}),
    );
    dependency["boot_entries"] = json!(["rom"]);
    let mut consumer = node(
        "consumer",
        0xfff87000,
        4,
        json!({"kind":"block86000","options":{}}),
    );
    consumer["dependencies"] = json!(["source"]);
    assert!(parse(document(vec![dependency, consumer])).is_err());
}

#[test]
fn legacy_core_and_lens_graph_kinds_are_not_aliases() {
    for kind in [
        "interrupt-controller",
        "timers",
        "pulse-unit",
        "lens-bridge",
        "boot-control",
    ] {
        let error = serde_json::from_value::<Model>(json!({"kind":kind}))
            .err()
            .expect("legacy graph kind must be rejected");
        assert!(
            error.to_string().contains("unknown variant"),
            "{kind}: {error}"
        );
    }
    let error = serde_json::from_value::<SerialEndpoint>(json!({"kind":"lens"}))
        .err()
        .expect("legacy lens endpoint kind must be rejected");
    assert!(error.to_string().contains("unknown variant"), "{error}");
}

fn controller(name: &str, core: usize, base: u64) -> Value {
    node(
        name,
        base,
        0x1000,
        json!({"kind":"exr2-interrupt-controller","options":{"arbitration":"unique"},"core":core,"base":base,"routes":"routing","software":[]}),
    )
}
fn put(built: &mut BuiltBoard, host: &mut Ram, address: u64, size: u8, value: u64) {
    let id = built.graph.at(address).unwrap();
    let mut cx = Cx::new(host, 0, 0, id);
    built.graph.nodes[id]
        .device
        .write(&mut cx, address, size, value);
    assert_eq!(cx.refusal(), None, "{address:#x}");
}
fn get(built: &mut BuiltBoard, host: &mut Ram, address: u64, size: u8) -> u64 {
    let id = built.graph.at(address).unwrap();
    let mut cx = Cx::new(host, 0, 0, id);
    let value = built.graph.nodes[id].device.read(&mut cx, address, size);
    assert_eq!(cx.refusal(), None, "{address:#x}");
    value
}

#[test]
fn controller_registers_publish_unique_image_routes_and_causal_connections() {
    let mut doc = document(vec![
        controller("irq0", 0, 0xfffb3000),
        controller("irq1", 1, 0xfffb4000),
        node(
            "phase",
            0xfff24000,
            0x2000,
            json!({"kind":"phase-detection","options":{"arithmetic":"disabled","data_ticks":2,"stop_ticks":2},"routes":"routing"}),
        ),
    ]);
    doc["wires"] = json!({"routing":"irq-routes"});
    let mut built = build(doc.clone()).unwrap();
    assert_eq!(built.core_interrupts.len(), 2);
    assert_eq!(
        built.graph.nodes[built.node_ids["phase"]].native_dependencies,
        vec![0, 1]
    );
    let Wire::IrqRoutes(routes) = &built.resources.wires["routing"] else {
        panic!()
    };
    let routes = routes.clone();
    assert_eq!(routes.owner(33), None);
    let mut host = Ram::new(0);
    put(&mut built, &mut host, 0xfffb3084, 4, 2);
    assert_eq!(routes.owner(33), Some(0));
    put(&mut built, &mut host, 0xfffb4084, 4, 2);
    assert_eq!(routes.owner(33), None);
    assert!(routes.latch(33).is_err());
    put(&mut built, &mut host, 0xfffb30a4, 4, 2);
    assert_eq!(routes.owner(33), Some(1));
    put(&mut built, &mut host, 0xfffb40a4, 4, 2);
    assert_eq!(routes.owner(33), None);
    doc["wires"]["routing"] = json!("parameters");
    assert!(build(doc).err().unwrap().contains("IrqRoutes"));
}

#[test]
fn loader_engines_are_native_nodes_with_exact_apertures_and_irq_wiring() {
    let mut memory = node(
        "memory",
        0xfff02000,
        0x40,
        json!({"kind":"memory-dma","options":{},"base":0xfff02000u64,"routes":"routing"}),
    );
    memory["windows"]
        .as_array_mut()
        .unwrap()
        .push(json!({"base":0xfff02300u64,"size":4}));
    let lz = node(
        "lz",
        0xfff43000,
        0x100,
        json!({"kind":"lz","options":{},"base":0xfff43000u64,"controllers":["irq0","irq1"]}),
    );
    let mut doc = document(vec![
        controller("irq0", 0, 0xfffb3000),
        controller("irq1", 1, 0xfffb4000),
        memory,
        lz,
    ]);
    doc["wires"] = json!({"routing":"irq-routes"});
    let mut built = build(doc.clone()).unwrap();
    assert_eq!(
        built.graph.nodes[built.node_ids["lz"]].device.name(),
        "exr2-lz"
    );
    assert_eq!(
        built.graph.nodes[built.node_ids["memory"]].native_dependencies,
        vec![0, 1]
    );
    assert!(built.graph.at(0xfff02040).is_err());
    let mut host = Ram::new(0);
    put(&mut built, &mut host, 0xfff43000, 4, 0);
    let id = built.node_ids["lz"];
    assert_eq!(built.graph.nodes[id].device.next_deadline(0), Some(2));
    host.now = 2;
    built.graph.nodes[id]
        .device
        .advance(&mut Cx::new(&mut host, 0, 0, id));
    assert_eq!(get(&mut built, &mut host, 0xfff43000, 4), 1);
    doc["nodes"][2]["windows"][0]["size"] = json!(0x80);
    assert!(build(doc.clone()).err().unwrap().contains("channel 0"));
    doc["nodes"][2]["windows"][0]["size"] = json!(0x40);
    doc["nodes"][3]["windows"][0]["size"] = json!(0x104);
    assert!(
        build(doc)
            .err()
            .unwrap()
            .contains("complete register window")
    );
}

#[test]
fn both_sd_hosts_own_their_clock_words_and_reject_a_competing_owner() {
    let sd = node(
        "card",
        0xfff50000,
        0x1000,
        json!({"kind":"sd-host","options":{"base":0xfff50000u32,"protocol":"application","transfer_ticks":2,"clock_ticks":3,"rca":1,"ram_bytes":0x100000},
        "card":{"kind":"blank","options":{"sectors":0x400000,"max_dirty_sectors":64}},"present":true,"protected":false,"irq":{"Core":{"core":0,"line":80}}}),
    );
    let sdio = node(
        "wifi",
        0xfff51000,
        0x1000,
        json!({"kind":"sdio-host","options":{"transfer_ticks":2,"clock_ticks":5,"rca":1,"ram_bytes":0x100000},
        "module":{"protocol":"enumeration","firmware_bytes":0},"irq":{"Core":{"core":0,"line":81}}}),
    );
    let clocks = node(
        "system_clocks",
        0xfff88000,
        0x100,
        json!({"kind":"clocks","options":{"output_settle_ticks":7,"reset_ready":true}}),
    );
    let mut built = build(document(vec![sd.clone(), sdio.clone(), clocks])).unwrap();
    assert!(built.contacts["card"].present());
    let mut host = Ram::new(0);
    put(&mut built, &mut host, 0xfff88048, 4, 1 << 28);
    for address in [0xfff5002c, 0xfff5102c] {
        put(&mut built, &mut host, address, 2, 1);
    }
    host.now = 2;
    assert_eq!(get(&mut built, &mut host, 0xfff5002c, 2), 1);
    assert_eq!(get(&mut built, &mut host, 0xfff5102c, 2), 1);
    host.now = 3;
    assert_eq!(get(&mut built, &mut host, 0xfff5002c, 2), 3);
    assert_eq!(get(&mut built, &mut host, 0xfff5102c, 2), 1);
    host.now = 5;
    assert_eq!(get(&mut built, &mut host, 0xfff5102c, 2), 3);
    assert_eq!(get(&mut built, &mut host, 0xfff88048, 4), 1 << 28);
    host.now = 7;
    assert_eq!(
        get(&mut built, &mut host, 0xfff88048, 4),
        (1 << 28) | (1 << 24)
    );
    let competing = node(
        "clocks",
        0xfff5002c,
        2,
        json!({"kind":"adapter","adapter":{"implementation":"remaining-clocks","options":{},"references":{},"wires":[]}}),
    );
    assert!(
        parse(document(vec![sd, sdio, competing]))
            .err()
            .unwrap()
            .contains("overlapping")
    );
}

#[test]
fn adapters_are_explicit_and_unavailable_implementations_refuse() {
    let adapter = node(
        "pending",
        0xfff87000,
        4,
        json!({"kind":"adapter","adapter":{"implementation":"pending-register-law","options":{},"references":{},"wires":[]}}),
    );
    let error = build(document(vec![adapter])).err().unwrap();
    assert!(error.contains("pending requires unavailable adapter pending-register-law"));
    let mut doc = basic();
    doc["nodes"][0]["boot_entries"] = json!(["rom"]);
    assert!(parse(doc).err().unwrap().contains("selects no nodes"));
}

#[test]
fn serial_lens_owns_its_part_and_uses_native_gpio_selection_and_pulse_wiring() {
    let gpio = node(
        "pads",
        0xfff89000,
        0x480,
        json!({"kind":"gpio","options":{"input_levels":vec![65535u32;16],"output_levels":[{"pin":{"port":13,"pin":9},"high":false}]},"irqs":[]}),
    );
    let pulses = node(
        "pulses",
        0xfffa9000,
        0xd00,
        json!({"kind":"exr2-pulse","options":{"counter_channels":[0,4],"timer_channels":[],"quadrature":false,"initial_phase":0,"capture_channel":null},"irqs":vec![Value::Null;12]}),
    );
    let motor = json!({"initial_position":3,"travel_limits":[-10000,10000],"ticks_per_step":10,"settle_ticks":30,"home":{"pin":{"port":0,"pin":0},"threshold":2,"side":"above"},"counters":[]});
    let serial = node(
        "lens",
        0xfff84080,
        0x40,
        json!({"kind":"serial-channel","options":{"base":0xfff84080u64,"transports":[{"control":0xe0,"configuration":4,"bits":16,"receive":false}],"selection":[{"pin":{"port":13,"pin":9},"high":false}],"transfer_ticks":4,"queue":"single"},"gpio":"pads",
        "endpoint":{"kind":"lens-unit","options":{"protocol":{"kind":"banked","clock":[{"kind":"fixed"},{"kind":"fixed"}],"positive_direction_high":false,"stop_response":"settled"},"motors":[motor.clone(),motor],"encoder":null,"zoom":null},"gpio":"pads","pulses":"pulses","completion":[{"Core":{"core":0,"line":14}},{"Core":{"core":0,"line":12}}]}}),
    );
    assert!(parse(document(vec![serial.clone(), gpio.clone(), pulses.clone()])).is_err());
    let mut built = build(document(vec![gpio, pulses, serial])).unwrap();
    assert_eq!(
        built.graph.nodes[built.node_ids["lens"]].native_dependencies,
        vec![0, 1]
    );
    let mut host = Ram::new(0);
    put(&mut built, &mut host, 0xfff84080, 4, 0xe0);
    put(&mut built, &mut host, 0xfff84084, 4, 4);
    put(&mut built, &mut host, 0xfff8409c, 4, 0x0420);
    let id = built.node_ids["lens"];
    let fingerprint = built.graph.nodes[id].device.fingerprint().unwrap();
    assert_eq!(fingerprint["part"]["mechanics"]["motors"][0]["position"], 3);
    assert_eq!(
        fingerprint["part"]["mechanics"]["motors"][0]["home_level"],
        true
    );
    assert_eq!(
        fingerprint["part"]["mechanics"]["motors"][0]["options"]["home"]["pin"],
        json!({"port":0,"pin":0})
    );
    assert_eq!(built.graph.nodes[id].device.next_deadline(0), Some(4));
    host.now = 3;
    built.graph.nodes[id].device.advance(&mut host.cx());
    assert_eq!(get(&mut built, &mut host, 0xfff8408c, 4), 0);
    host.now = 4;
    built.graph.nodes[id].device.advance(&mut host.cx());
    assert_eq!(get(&mut built, &mut host, 0xfff8408c, 4), 0x300);
    put(&mut built, &mut host, 0xfff89340, 4, 0x02000200);
    let mut cx = host.cx();
    built.graph.nodes[id]
        .device
        .write(&mut cx, 0xfff8409c, 4, 0x3e00);
    assert!(cx.refusal().is_some());
}

#[test]
fn readback_and_jpeg_peers_share_real_constructor_handles() {
    use fujimod_emulator_imaging::analysis::ImageLayout;
    use fujimod_emulator_imaging::scene::ImageFrames;
    let mut doc = document(vec![
        node(
            "parameters",
            0xfff19000,
            0x1000,
            json!({"kind":"parameters","options":{},"bank":"parameters"}),
        ),
        node(
            "input",
            0xfff10000,
            0x400,
            json!({"kind":"image-input","options":{"reset_ticks":2,"command_ticks":3,"phase_ticks":2,"command":"packed_words","command_controls":[0],"phase_ack":"enabled","clock":null,"sensor_period_ticks":null,"raw_dma_ticks":5,"readback_ticks":7,"cfa":(0..36).map(|i|i%3).collect::<Vec<_>>()},
            "phase_irqs":[{"core":0,"line":39},{"core":1,"line":41}],"raw_irq":{"core":0,"line":36},"readback_irq":{"core":0,"line":38},"sensor":null,"parameters":"parameters","memory_outputs":"readback","edges":"edges"}),
        ),
        node(
            "out0",
            0xfff12000,
            0x100,
            json!({"kind":"output-channel","options":{"stop_ticks":2,"clock":{"kind":"fixed","ticks":3},"geometry":"skip_incomplete"},"base":0xfff12000u64,"irq":44,"channel":0,"channels":"outputs","routes":"routes","frames":"image","memory_outputs":"readback"}),
        ),
        node(
            "out1",
            0xfff13000,
            0x100,
            json!({"kind":"output-channel","options":{"stop_ticks":2,"clock":{"kind":"fixed","ticks":3},"geometry":"skip_incomplete"},"base":0xfff13000u64,"irq":45,"channel":1,"channels":"outputs","routes":"routes","frames":"image","memory_outputs":"readback"}),
        ),
        node(
            "warp",
            0xfff65000,
            0x200,
            json!({"kind":"warp","options":{"completion_ticks":3,"stop_ticks":2},"irq":{"core":0,"line":74},"stream":"jpeg"}),
        ),
        node(
            "jpeg",
            0xfff62000,
            0x200,
            json!({"kind":"jpeg-encoder","options":{"completion_ticks":3,"reset_ticks":2},"irq":{"core":0,"line":77},"stream":"jpeg"}),
        ),
    ]);
    doc["wires"] = json!({"parameters":"parameters","readback":"memory-outputs","edges":"phase-edges","outputs":"output-channels","routes":"irq-routes","jpeg":"stream"});
    let mut resources = Resources::default();
    fujimod_emulator_exr2_images::images_mut(&mut resources).unwrap().images.insert(
        "image".into(),
        ImageFrames::new(ImageLayout {
            width: 2,
            height: 2,
            stride: 4,
        })
        .unwrap(),
    );
    let built =
        build_with_models(parse(doc).unwrap(), resources, 123, &mut RefuseAdapters).unwrap();
    assert_eq!(
        built.graph.nodes[built.node_ids["input"]].native_dependencies,
        vec![0, 2, 3]
    );
    assert_eq!(
        built.graph.nodes[built.node_ids["warp"]].native_dependencies,
        vec![5]
    );
    assert_eq!(
        built.graph.nodes[built.node_ids["jpeg"]].native_dependencies,
        vec![4]
    );
    assert_eq!(
        built.graph.nodes[built.node_ids["input"]]
            .device
            .next_deadline(123),
        None
    );
}

#[test]
fn graphics_uses_real_constructor_and_two_declared_controller_outputs() {
    let mut value = document(vec![
        controller("irq0", 0, 0xfffb3000),
        controller("irq1", 1, 0xfffb4000),
        node(
            "graphics",
            0xfff78000,
            0x5000,
            json!({"kind":"vector-gpu","options":{
            "ram_bytes":4096,"max_fill_bytes":4096,"max_copy_bytes":4096,
            "max_clear_bytes":4096,"max_command_capture_bytes":65536,"bytes_per_tick":16,
            "copy_profile":null,"clear_profile":null
        },"controllers":["irq0","irq1"]}),
        ),
    ]);
    value["wires"] = json!({"routing":"irq-routes"});
    let built = build(value.clone()).unwrap();
    assert_eq!(built.graph.nodes[2].device.name(), "vector-gpu");
    assert_eq!(built.graph.nodes[2].native_dependencies, [0, 1]);
    value["nodes"][2]["model"]["controllers"] = json!(["irq0", "irq0"]);
    assert!(build(value.clone()).is_err());
    value["nodes"][2]["model"]["controllers"] = json!(["irq0", "irq1"]);
    value["nodes"][2]["windows"][0]["size"] = json!(0x4000);
    assert!(build(value).is_err());
}

#[test]
fn external_image_inputs_record_the_frame_owners_that_must_be_woken() {
    use fujimod_emulator_imaging::analysis::ImageLayout;
    use fujimod_emulator_imaging::scene::{ImageFrames, StatisticsFrames, StatisticsSpan};
    let plan = document(vec![json!({
        "name":"frames", "boot_entries":["loader"], "windows":[], "dependencies":[],
        "model":{"kind":"external-frames", "options":{"period_ticks":10,"phase_source":null,"optics":null},
            "sensor_clock":null,"images":"image","statistics":"statistics","activity":null,
            "position":null,"radiometric":null,"phase_edges":null,"frame_period":null}
    })]);
    let mut resources = Resources::default();
    for name in ["image", "unconnected"] {
        fujimod_emulator_exr2_images::images_mut(&mut resources).unwrap().images.insert(
            name.into(),
            ImageFrames::new(ImageLayout {
                width: 2,
                height: 2,
                stride: 4,
            })
            .unwrap(),
        );
    }
    fujimod_emulator_exr2_images::images_mut(&mut resources).unwrap().statistics.insert(
        "statistics".into(),
        StatisticsFrames::new(vec![StatisticsSpan {
            destination: 2,
            offset: 0,
            bytes: vec![0; 4],
        }])
        .unwrap(),
    );
    let built = build_with_models(
        parse(plan.clone()).unwrap(),
        resources,
        0,
        &mut RefuseAdapters,
    )
    .unwrap();
    assert_eq!(
        built.resources.image_users("image").unwrap(),
        [built.node_ids["frames"]]
    );
    assert!(
        built
            .resources
            .image_users("unconnected")
            .unwrap()
            .is_empty()
    );
    assert!(built.resources.image_users("absent").is_err());
    assert!(
        build_with_models(
            parse(plan).unwrap(),
            built.resources,
            0,
            &mut RefuseAdapters
        )
        .is_err()
    );
}

#[test]
fn serial_register_access_wakes_the_coordinator_after_dma_consolidation() {
    // Browser cold boot stalled with an SPI0 byte due at tick 4,344,370 while
    // execution reached 30,000,000. Only DMA had a coordinator dependency.
    let gpio = node(
        "pads",
        0xfff89000,
        0x480,
        json!({"kind":"gpio",
        "options":{"input_levels":vec![0u32;16],"output_levels":[{"pin":{"port":11,"pin":2},"high":false}]},"irqs":[]}),
    );
    let mut serial = node(
        "serial",
        0,
        0,
        json!({"kind":"power","gpio":"pads",
        "options":{"transfer_ticks":4,"max_program_bytes":65536,"spi0":{
            "required":[192,48,0],"selection":{"pin":{"port":11,"pin":2},"high":false},
            "presence":{"pin":{"port":11,"pin":2},"high":false}}}}),
    );
    serial["windows"] = json!([
        {"base":0xfff84000u64,"size":32},{"base":0xfff46080u64,"size":16},
        {"base":0xfff02080u64,"size":64},{"base":0xfff98500u64,"size":8}]);
    let mut built = build(document(vec![gpio, serial])).unwrap();
    let coordinator = built
        .graph
        .nodes
        .iter()
        .position(|node| node.device.name() == "serial-event-coordinator")
        .unwrap();
    for address in [0xfff84000, 0xfff46080, 0xfff02080, 0xfff98500] {
        let source = built.graph.at(address).unwrap();
        assert!(
            built.graph.nodes[coordinator]
                .native_dependencies
                .contains(&source),
            "{address:#x}"
        );
    }
    let mut host = Ram::new(65536);
    for (offset, value) in [(0, 192), (4, 48), (8, 0), (28, 0x23)] {
        put(&mut built, &mut host, 0xfff84000 + offset, 4, value);
    }
    assert_eq!(
        built.graph.nodes[coordinator].device.next_deadline(0),
        Some(4)
    );
    host.now = 4;
    built.graph.nodes[coordinator]
        .device
        .advance(&mut Cx::new(&mut host, 0, 4, coordinator));
    assert_eq!(get(&mut built, &mut host, 0xfff84010, 4), 1);
    assert_eq!(get(&mut built, &mut host, 0xfff84018, 4), 0);
    assert_eq!(built.graph.nodes[coordinator].device.next_deadline(4), None);
}

#[test]
fn board_registers_spi_selector_and_one_dma_controller_as_independent_nodes() {
    use fujimod_emulator_exr2_plan::{Plan, RefuseAdapters};
    use fujimod_emulator_exr2_resources::Resources;
    let plan = Plan::parse(
        &serde_json::json!({
            "schema": 1,
            "boot_entry": "rom",
            "wires": {},
            "nodes": [{
                "name": "link",
                "boot_entries": ["rom"],
                "dependencies": [],
                "windows": [
                    {"base": 0xfff84080_u64, "size": 0x24_u64},
                    {"base": 0xfff98500_u64, "size": 8_u64},
                    {"base": 0xfff02080_u64, "size": 0x40_u64},
                    {"base": 0xfff020c0_u64, "size": 0x40_u64}
                ],
                "model": {
                    "kind": "serial-dma-link",
                    "options": {
                        "serial_base": 0xfff84080_u64,
                        "dma_base": 0xfff02000_u64,
                        "routing_base": 0xfff98500_u64,
                        "channels": [2, 3],
                        "transmit_request": 9,
                        "receive_request": 8,
                        "frame_ticks": 64,
                        "irq_targets": []
                    },
                    "input": {"kind": "fixed", "high": true}
                }
            }]
        })
        .to_string()
        .as_bytes(),
    )
    .unwrap();
    let mut adapters = RefuseAdapters;
    let board = build_with_models(plan, Resources::default(), 0, &mut adapters).unwrap();
    assert_eq!(board.graph.nodes.len(), 3);
    let serial = board.graph.at(0xfff84080).unwrap();
    let selector = board.graph.at(0xfff98500).unwrap();
    let controller = board.graph.at(0xfff02080).unwrap();
    assert_eq!(board.graph.nodes[serial].device.name(), "exr2-byte-serial");
    assert_eq!(
        board.graph.nodes[selector].device.name(),
        "exr2-dma-request-routing"
    );
    assert_eq!(
        board.graph.nodes[controller].device.name(),
        "exr2-dma-controller"
    );
    assert_eq!(board.graph.nodes[controller].mappings.len(), 2);
    assert_eq!(
        board.graph.nodes[controller].native_dependencies,
        [serial, selector]
    );
    assert_eq!(board.graph.nodes[serial].native_dependencies, [controller]);
    assert_eq!(board.node_ids["link"], serial);
}

#[test]
fn checked_board_graph_expands_boot_serial_into_disjoint_register_owners() {
    use fujimod_emulator_devices::graph::Mapping;
    const SPI: u64 = 0xfff84000;
    const UART: u64 = 0xfff46080;
    const DMA: u64 = 0xfff02080;
    let input = serde_json::json!({
        "schema": 1,
        "boot_entry": "loader",
        "wires": {},
        "nodes": [
            {
                "name": "gpio",
                "boot_entries": ["rom", "loader"],
                "windows": [{"base": 0xfff89000u64, "size": 0x480}],
                "dependencies": [],
                "model": {
                    "kind": "gpio",
                    "options": {"input_levels": vec![0u32; 16], "output_levels": []},
                    "irqs": []
                }
            },
            {
                "name": "boot_serial",
                "boot_entries": ["rom", "loader"],
                "windows": [
                    {"base": SPI, "size": 0x20},
                    {"base": UART, "size": 0x10},
                    {"base": DMA, "size": 0x40},
                    {"base": 0xfff98500u64, "size": 8}
                ],
                "dependencies": [],
                "model": {
                    "kind": "power",
                    "options": {"transfer_ticks": 4, "max_program_bytes": 65536, "spi0": null},
                    "gpio": "gpio"
                }
            }
        ]
    });
    let plan = Plan::parse(&serde_json::to_vec(&input).unwrap()).unwrap();
    let mut adapters = fujimod_emulator_exr2_plan::RefuseAdapters;
    let built = build_with_models(plan, Resources::default(), 0, &mut adapters).unwrap();
    let nodes = &built.graph.nodes;
    let names = nodes
        .iter()
        .map(|node| node.device.name())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        [
            "gpio",
            "spi0-byte-shifter",
            "boot-uart",
            "exr2-dma-controller",
            "boot-dma-request-selector",
            "serial-event-coordinator",
        ]
    );
    assert_eq!(built.node_ids["boot_serial"], 1);
    assert_eq!(
        nodes[1..5]
            .iter()
            .map(|node| node.mappings.as_slice())
            .collect::<Vec<_>>(),
        [
            &[Mapping {
                base: SPI,
                size: 0x20
            }][..],
            &[Mapping {
                base: UART,
                size: 0x10
            }][..],
            &[Mapping {
                base: DMA,
                size: 0x40
            }][..],
            &[Mapping {
                base: 0xfff98500,
                size: 8
            }][..],
        ]
    );
    assert!(nodes[5].mappings.is_empty());
}

fn build_with_models(
    plan: Plan,
    resources: Resources,
    start_tick: u64,
    adapter: &mut dyn AdapterFactory,
) -> Result<BuiltBoard, String> {
    fujimod_emulator_models::register();
    crate::build(plan, resources, start_tick, adapter)
}
