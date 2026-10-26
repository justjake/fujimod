//! Native calls to the same session operations the browser `Camera` exposes.
use fujimod_emulator_cpu::ExecutionPolicy;
use fujimod_emulator_javascript::{Session, lifecycle::BootInput, results::CardFile};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

fn value<T: Serialize>(result: T) -> Value {
    serde_json::to_value(result).unwrap()
}
fn options<T: for<'de> Deserialize<'de>>(o: &Value, key: &str) -> T {
    serde_json::from_value(o[key].clone()).unwrap()
}
/// One browser operation by name, with its JSON result and binary output.
fn command(
    s: &mut Session,
    action: &str,
    o: Value,
    upload: &[u8],
) -> Result<(Value, Vec<u8>), String> {
    let text = |key: &str| o[key].as_str().unwrap().to_owned();
    let none = Vec::new();
    Ok(match action {
        "fingerprint" => (value(s.fingerprint()?), none),
        "state" => (value(s.session_state()?), none),
        "checkpoint" => (value(s.checkpoint()?), none),
        "restore" => (value(s.restore_checkpoint(&text("id"))?), none),
        "undo" => (value(s.undo()?), none),
        "rollback" => (value(s.rollback(text("tick").parse().unwrap())?), none),
        "replay" => (value(s.replay(upload)?), none),
        "history" => {
            let log = s.history_log()?;
            (json!({"entries":log.entries}), log.log)
        }
        "export_snapshot" => {
            let saved = s.export_snapshot()?;
            (
                json!({"tick":saved.tick,"instructions":saved.instructions}),
                saved.snapshot,
            )
        }
        "error_report" => {
            let report = s.error_report()?;
            (
                json!({"report":report.report}),
                report.snapshot.unwrap_or_default(),
            )
        }
        "save" => {
            let saved = s.save_durable()?;
            (
                json!({"positions":saved.positions,"poweredOn":saved.powered_on}),
                saved.durable,
            )
        }
        "restore_durable" => (value(s.load_durable(upload)?), none),
        "power" => (value(s.set_power(o["on"].as_bool().unwrap())?), none),
        "power_cycle" => (
            value(s.cycle_power(&options::<Vec<BootInput>>(&o, "inputs"))?),
            none,
        ),
        "restart_card" => (
            value(s.replace_card(options::<Vec<CardFile>>(&o, "files"), upload, &[])?),
            none,
        ),
        "press" => (value(s.press(text("id"), None)?), none),
        "release" => (value(s.release(text("id"))?), none),
        "set_position" => (value(s.set_position(text("id"), text("position"))?), none),
        "rotate" => (
            value(s.rotate(&text("id"), o["steps"].as_i64().unwrap() as i32)?),
            none,
        ),
        "card_read" => (json!({}), s.card_read(&text("path"), None, None)?),
        "memory_read" => (
            json!({}),
            s.memory_read(
                text("address").parse().unwrap(),
                o["size"].as_u64().unwrap() as usize,
            )?,
        ),
        "watch_pcs" => {
            s.watch_pcs(&options::<Vec<u64>>(&o, "addresses"))?;
            (json!({}), none)
        }
        "profile" => (value(s.profile(None)?), none),
        "device_workers" => (
            value(s.device_workers(o["count"].as_u64().map(|c| c as usize))?),
            none,
        ),
        "usb_status" => (value(s.usb_status(&text("node"))?), none),
        "usb_reset" => {
            s.usb_reset(&text("node"))?;
            (json!({}), none)
        }
        "usb_setup" => {
            s.usb_setup(&text("node"), upload)?;
            (json!({}), none)
        }
        "usb_out" => {
            s.usb_out(&text("node"), o["endpoint"].as_u64().unwrap() as u8, upload)?;
            (json!({}), none)
        }
        "usb_in" => (
            json!({}),
            s.usb_in(
                &text("node"),
                o["endpoint"].as_u64().unwrap() as u8,
                o["size"].as_u64().unwrap() as usize,
            )?,
        ),
        _ => panic!("unknown test action {action}"),
    })
}
struct Status {
    ticks: u64,
    instructions: u64,
    failed: bool,
}
fn status_of(s: &Session) -> Status {
    Status {
        ticks: s.core.board.now(),
        instructions: s.core.board.instruction_counts().iter().sum(),
        failed: s.failed,
    }
}
fn create(scanner: bool) -> Session {
    create_with(scanner, |_| {})
}
fn create_with(scanner: bool, configure: impl FnOnce(&mut Value)) -> Session {
    create_policy(scanner, ExecutionPolicy::Exact, configure)
}
fn create_policy(
    scanner: bool,
    policy: ExecutionPolicy,
    configure: impl FnOnce(&mut Value),
) -> Session {
    // The optional guest program continuously samples ADC1 and writes one
    // stable byte, standing in for a stock scanner in this synthetic fixture.
    // Host inspection never writes its acknowledgment.
    let words: &[u32] = if scanner {
        &[
            0xe30a0000, 0xe34f0ff8, // r0 = ADC base
            0xe3a01008, 0xe5801004, // continuous input mode
            0xe3a01002, 0xe5801008, // select channel 1
            0xe3081005, 0xe5801000, // START, continuous read-through mode
            0xe5902034, 0xe3520c01, // ldr result[1]; cmp sample, #256
            0xe3a02000, 0x33a02001, // stable = sample < 256
            0xe3003101, 0xe3403001, 0xe5c32000, 0xeafffff7,
        ]
    } else {
        &[0xeafffffe]
    };
    let firmware: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    let mapping = json!({"address":0x10000,"physical":0x10000,"size":0x4000,"permissions":3});
    let mut config = json!({"schema":1,"firmwareId":"generated","bootSource":"internal","initialPositions":{},"graph":{
        "schema":1,"firmwareId":"generated","bootSource":"internal","quantum":17,
        "execution":{"processors":[{"model":"cortex-r5f","released":true,"registers":[["Pc",0x10000]],"mprofile":null,"rprofile":{"cp14_identity":0x4003,"tcm":{"atcm":{"bytes":0,"reset":0},"btcm":{"bytes":0,"reset":0}}}}],
            "ram":[{"base":0x10000,"size":0x4000}],"mappings":[{"core":0,"address":0x10000,"physical":0x10000,"size":0x4000,"permissions":7}],"dma_mappings":[mapping],"decoders":[],"instructions_per_tick":[24,1],"peripherals":[{"core":0,"address":0xfffb3000u64,"physical":0xfffb3000u64,"size":4096}]},
        "devices":{"schema":1,"boot_entry":"loader","wires":{},"nodes":[{"name":"irq","boot_entries":["loader"],"windows":[{"base":0xfffb3000u64,"size":4096}],"dependencies":[],"model":{"kind":"exr2-interrupt-controller","options":{"arbitration":"unique"},"core":0,"base":0xfffb3000u64,"routes":null,"software":[]}}]},
        "firmware":{"size":firmware.len(),"sha256":fujimod_firmware::sha256(&firmware),"format":"raw","mappings":[{"name":"loop","offset":0,"size":firmware.len(),"base":0x10000,"encoding":"raw","executable":true}]}
    }});
    config["graph"]["execution"]["peripherals"]
        .as_array_mut()
        .unwrap()
        .push(json!({"core":0,"address":0xfff8a000u64,"physical":0xfff8a000u64,"size":4096}));
    config["graph"]["devices"]["nodes"].as_array_mut().unwrap().push(json!({"name":"adc","boot_entries":["loader"],"windows":[{"base":0xfff8a000u64,"size":4096}],"dependencies":[],"model":{"kind":"adc","options":{"conversion_ticks":4,"samples":[0,0,0,0,0,0,0,0,0,0,0,0]}}}));
    config["graph"]["controls"] = json!({"controls":[{"id":"lever","label":"Lever","kind":"selector","positions":["off","on"],"initial":"off","unavailable":null}],"circuits":[{"controls":["lever"],"cases":[{"positions":["off"],"signals":[{"kind":"analog","node":"adc","channel":0,"value":10}]},{"positions":["on"],"signals":[{"kind":"analog","node":"adc","channel":0,"value":900}]}]}]});
    if scanner {
        config["graph"]["controls"] = json!({"controls":[{"id":"button","label":"Button","kind":"button","positions":["released","pressed"],"initial":"released","unavailable":null}],"circuits":[{"controls":["button"],"cases":[{"positions":["released"],"signals":[{"kind":"analog","node":"adc","channel":1,"value":1023}]},{"positions":["pressed"],"signals":[{"kind":"analog","node":"adc","channel":1,"value":100}]}]}]});
        config["graph"]["contactDriver"] = json!({"poll_ticks":10,"max_phase_ticks":1000,
            "catalog":[{"id":"button","label":"Button","kind":"button","supported":true}],
            "actuators":[{"id":"button","momentary":true,"states":[
                {"position":"released","ready":[{"address":0x10100,"bytes":[0]}],"expected":[{"address":0x10101,"bytes":[0]}]},
                {"position":"pressed","ready":[{"address":0x10100,"bytes":[0]}],"expected":[{"address":0x10101,"bytes":[1]}]}]}]});
    }
    let format = json!({"sectors":0x800000,"max_dirty_sectors":131072,"partition_start":2048,"sectors_per_cluster":64,"label":"TEST"});
    config["graph"]["execution"]["peripherals"]
        .as_array_mut()
        .unwrap()
        .push(json!({"core":0,"address":0xfff70000u64,"physical":0xfff70000u64,"size":4096}));
    config["graph"]["devices"]["nodes"].as_array_mut().unwrap().push(json!({"name":"sd","boot_entries":["loader"],"windows":[{"base":0xfff70000u64,"size":0x108}],"dependencies":[],"model":{"kind":"sd-host","options":{"protocol":"sdhc","transfer_ticks":2,"clock_ticks":2,"rca":4660,"ram_bytes":0x14000},"card":{"kind":"formatted","options":format,"files":"sd_files"},"present":true,"protected":false,"irq":{"Core":{"core":0,"line":80}}}}));
    config["graph"]["card"] = json!({"node":"sd","format":format});
    config["graph"]["scene"]["files"] = json!({"sd_files":{"KEEP.TXT":"file"}});
    config["graph"]["assets"] = json!({"file":{"kind":"bytes","bytes":[1,2,3]}});
    configure(&mut config);
    let config = serde_json::to_vec(&config).unwrap();
    Session::create_camera(&config, &firmware, policy).unwrap()
}

fn load_snapshot(bytes: &[u8]) -> Session {
    Session::load_snapshot(bytes).unwrap()
}

#[test]
fn portable_archive_reconstructs_a_destroyed_session_and_continues_exactly() {
    let mut source = create(false);
    command(
        &mut source,
        "watch_pcs",
        json!({"addresses":[0x10000]}),
        &[],
    )
    .unwrap();
    command(
        &mut source,
        "set_position",
        json!({"id":"lever","position":"on"}),
        &[],
    )
    .unwrap();
    source.run(11).unwrap();
    let before = command(&mut source, "fingerprint", json!({}), &[])
        .unwrap()
        .0;
    let (metadata, bytes) = command(&mut source, "export_snapshot", json!({}), &[]).unwrap();
    assert_eq!(metadata["tick"], "11");
    assert_eq!(
        command(&mut source, "fingerprint", json!({}), &[])
            .unwrap()
            .0,
        before
    );
    source.run(53).unwrap();
    let expected = command(&mut source, "fingerprint", json!({}), &[])
        .unwrap()
        .0;
    source.core.board.finish().unwrap();
    let mut loaded = load_snapshot(&bytes);
    assert_eq!(
        command(&mut loaded, "fingerprint", json!({}), &[])
            .unwrap()
            .0,
        before
    );
    assert_eq!(
        command(&mut loaded, "state", json!({}), &[]).unwrap().0["positions"]["lever"],
        "on"
    );
    loaded.run(53).unwrap();
    assert_eq!(
        command(&mut loaded, "fingerprint", json!({}), &[])
            .unwrap()
            .0,
        expected
    );
    loaded.core.board.finish().unwrap();
    let mut corrupt = bytes;
    corrupt.truncate(corrupt.len() - 1);
    assert!(Session::load_snapshot(&corrupt).is_err());
}

#[test]
fn error_report_exports_a_healthy_retained_point_without_changing_the_failed_session() {
    let mut source = create_with(false, |c| {
        c["graph"]["execution"]["processors"][0]["registers"] = json!([["Pc", 0xfffd0000u32]]);
    });
    command(&mut source, "checkpoint", json!({}), &[]).unwrap();
    let original_error = source.run(1).unwrap_err();
    let failed = command(&mut source, "fingerprint", json!({}), &[])
        .unwrap()
        .0;
    let (report, bytes) = command(&mut source, "error_report", json!({}), &[]).unwrap();
    assert!(!bytes.is_empty());
    assert_eq!(report["report"]["snapshot"]["tick"], "0");
    assert_ne!(report["report"]["snapshot"]["source"], "current");
    assert_eq!(
        command(&mut source, "fingerprint", json!({}), &[])
            .unwrap()
            .0,
        failed
    );
    source.core.board.finish().unwrap();
    let mut loaded = load_snapshot(&bytes);
    let status = status_of(&loaded);
    assert!(!status.failed);
    assert_eq!(status.ticks, 0);
    assert_eq!(loaded.run(1).unwrap_err(), original_error);
    loaded.core.board.finish().unwrap();
}

#[test]
fn powered_off_archive_preserves_power_controls_and_its_cold_reset_image() {
    let mut source = create(false);
    command(
        &mut source,
        "set_position",
        json!({"id":"lever","position":"on"}),
        &[],
    )
    .unwrap();
    command(&mut source, "power", json!({"on":false}), &[]).unwrap();
    let (_, bytes) = command(&mut source, "export_snapshot", json!({}), &[]).unwrap();
    source.core.board.finish().unwrap();
    let mut loaded = load_snapshot(&bytes);
    let state = command(&mut loaded, "state", json!({}), &[]).unwrap().0;
    assert_eq!(state["poweredOn"], false);
    assert_eq!(state["positions"]["lever"], "on");
    loaded.run(5).unwrap();
    let status = status_of(&loaded);
    assert_eq!(status.ticks, 0);
    command(&mut loaded, "power", json!({"on":true}), &[]).unwrap();
    loaded.run(5).unwrap();
    let status = status_of(&loaded);
    assert_eq!(status.ticks, 5);
    loaded.core.board.finish().unwrap();
}

#[test]
fn cbor_history_preserves_u64_runs_and_restores_failed_replay_at_ratio_24() {
    #[derive(Deserialize, Serialize)]
    struct Log {
        schema: u32,
        owner: u32,
        generation: u64,
        entries: Vec<fujimod_emulator_session::history::Entry>,
    }
    let mut h = create(false);
    let cold = command(&mut h, "fingerprint", json!({}), &[]).unwrap().0;
    assert_eq!(cold["sha256"].as_str().unwrap().len(), 64);
    assert_eq!(
        command(&mut h, "fingerprint", json!({}), &[]).unwrap().0,
        cold
    );
    let checkpoint = command(&mut h, "checkpoint", json!({}), &[]).unwrap().0;
    assert_eq!(
        command(&mut h, "fingerprint", json!({}), &[]).unwrap().0,
        cold
    );
    assert_eq!(checkpoint["tick"], "0");
    h.run(5).unwrap();
    let at_five = command(&mut h, "error_report", json!({}), &[]).unwrap().0["report"].clone();
    let five_fingerprint = command(&mut h, "fingerprint", json!({}), &[]).unwrap().0;
    assert_ne!(five_fingerprint, cold);
    let (summary, bytes) = command(&mut h, "history", json!({}), &[]).unwrap();
    assert_eq!(summary["entries"], 1);
    let mut log: Log = ciborium::from_reader(bytes.as_slice()).unwrap();
    assert!(matches!(
        log.entries[0].input,
        fujimod_emulator_session::history::Input::Run {
            instructions: u64::MAX,
            ..
        }
    ));
    assert_eq!(
        command(&mut h, "undo", json!({}), &[]).unwrap().0,
        json!({"tick":"0","instructions":["0"]})
    );
    let before = command(&mut h, "error_report", json!({}), &[]).unwrap().0["report"].clone();
    assert_eq!(
        command(&mut h, "fingerprint", json!({}), &[]).unwrap().0,
        cold
    );
    log.entries[0].after.instructions[0] += 1;
    let mut bad = Vec::new();
    ciborium::into_writer(&log, &mut bad).unwrap();
    assert!(
        command(&mut h, "replay", json!({}), &bad)
            .unwrap_err()
            .contains("differs")
    );
    assert_eq!(
        command(&mut h, "error_report", json!({}), &[]).unwrap().0["report"].clone(),
        before
    );
    assert_eq!(
        command(&mut h, "fingerprint", json!({}), &[]).unwrap().0,
        cold
    );
    assert_eq!(
        command(&mut h, "replay", json!({}), &bytes).unwrap().0,
        json!({"tick":"5","instructions":["120"]})
    );
    assert_eq!(
        command(&mut h, "error_report", json!({}), &[]).unwrap().0["report"].clone(),
        at_five
    );
    assert_eq!(
        command(&mut h, "fingerprint", json!({}), &[]).unwrap().0,
        five_fingerprint
    );
    command(&mut h, "restore", json!({"id":checkpoint["id"]}), &[]).unwrap();
    assert_eq!(
        command(&mut h, "fingerprint", json!({}), &[]).unwrap().0,
        cold
    );
    h.run(9).unwrap();
    assert_eq!(
        command(&mut h, "rollback", json!({"tick":"5"}), &[])
            .unwrap()
            .0,
        json!({"tick":"5","instructions":["120"]})
    );
    assert_eq!(
        command(&mut h, "error_report", json!({}), &[]).unwrap().0["report"].clone(),
        at_five
    );
    assert_eq!(
        command(&mut h, "fingerprint", json!({}), &[]).unwrap().0,
        five_fingerprint
    );
    let mut other = create(false);
    command(&mut other, "checkpoint", json!({}), &[]).unwrap();
    assert!(
        command(&mut other, "restore", json!({"id":checkpoint["id"]}), &[])
            .unwrap_err()
            .contains("checkpoint ID")
    );
    assert!(
        command(&mut other, "replay", json!({}), &bytes)
            .unwrap_err()
            .contains("another session")
    );
    other.core.board.finish().unwrap();
    h.core.board.finish().unwrap();
}

#[test]
fn late_pc_witnesses_are_observational_and_capture_only_future_entries() {
    let mut h = create(false);
    h.run(5).unwrap();
    let before = command(&mut h, "fingerprint", json!({}), &[]).unwrap().0;
    let history_before = command(&mut h, "history", json!({}), &[]).unwrap();
    command(&mut h, "watch_pcs", json!({"addresses":[0x10000]}), &[]).unwrap();
    assert_eq!(
        command(&mut h, "fingerprint", json!({}), &[]).unwrap().0,
        before
    );
    assert_eq!(
        command(&mut h, "history", json!({}), &[]).unwrap(),
        history_before
    );
    assert_eq!(
        command(&mut h, "profile", json!({}), &[]).unwrap().0["pcEntries"],
        json!([])
    );
    h.run(6).unwrap();
    let entries = command(&mut h, "profile", json!({}), &[]).unwrap().0["pcEntries"].clone();
    assert_eq!(entries.as_array().unwrap().len(), 1);
    assert!(entries[0]["tick"].as_u64().unwrap() >= 5);
    assert_eq!(entries[0]["pc"], 0x10000);
    h.run(7).unwrap();
    assert_eq!(
        command(&mut h, "profile", json!({}), &[]).unwrap().0["pcEntries"],
        entries
    );
    assert!(command(&mut h, "watch_pcs", json!({"addresses":[0x10000]}), &[]).is_err());
    h.core.board.finish().unwrap();
}

#[test]
fn positions_drive_external_levels_and_restore_with_the_machine() {
    let mut h = create(false);
    let initial = command(&mut h, "checkpoint", json!({}), &[]).unwrap().0;
    let before = command(&mut h, "fingerprint", json!({}), &[]).unwrap().0;
    let read = command(
        &mut h,
        "memory_read",
        json!({"address":"65536","size":4}),
        &[],
    )
    .unwrap()
    .1;
    assert_eq!(read, [0xfe, 0xff, 0xff, 0xea]);
    assert!(
        command(
            &mut h,
            "memory_read",
            json!({"address":0xfff8a000u64.to_string(),"size":4}),
            &[]
        )
        .is_err()
    );
    assert_eq!(
        command(&mut h, "fingerprint", json!({}), &[]).unwrap().0,
        before
    );
    for (position, value) in [("on", 900), ("off", 10)] {
        let state = command(
            &mut h,
            "set_position",
            json!({"id":"lever","position":position}),
            &[],
        )
        .unwrap()
        .0;
        assert_eq!(state["positions"]["lever"], position);
        let report = command(&mut h, "error_report", json!({}), &[]).unwrap().0;
        let adc = report["report"]["bus"]["devices"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["kind"] == "exr2-adc")
            .unwrap();
        assert_eq!(adc["state"]["external"][0], value);
    }
    let after = command(&mut h, "fingerprint", json!({}), &[]).unwrap().0;
    let log = command(&mut h, "history", json!({}), &[]).unwrap().1;
    command(&mut h, "restore", json!({"id":initial["id"]}), &[]).unwrap();
    assert_eq!(
        command(&mut h, "fingerprint", json!({}), &[]).unwrap().0,
        before
    );
    command(&mut h, "replay", json!({}), &log).unwrap();
    assert_eq!(
        command(&mut h, "fingerprint", json!({}), &[]).unwrap().0,
        after
    );
    h.core.board.finish().unwrap();
}

#[test]
fn queued_physical_taps_wait_for_guest_acknowledgments_and_restore_in_flight() {
    let mut h = create(true);
    command(&mut h, "checkpoint", json!({}), &[]).unwrap();
    for _ in 0..2 {
        command(&mut h, "press", json!({"id":"button"}), &[]).unwrap();
        command(&mut h, "release", json!({"id":"button"}), &[]).unwrap();
    }
    let before = command(&mut h, "fingerprint", json!({}), &[]).unwrap().0;
    for _ in 0..10 {
        let state = command(&mut h, "state", json!({}), &[]).unwrap().0;
        assert_eq!(state["positions"]["button"], "pressed");
        assert_eq!(state["inputQueue"]["pending"], 3);
        assert_eq!(state["inputs"][0]["supported"], true);
    }
    assert_eq!(
        command(&mut h, "fingerprint", json!({}), &[]).unwrap().0,
        before
    );
    assert!(
        command(
            &mut h,
            "set_position",
            json!({"id":"button","position":"released"}),
            &[]
        )
        .is_err()
    );
    assert_eq!(
        command(&mut h, "fingerprint", json!({}), &[]).unwrap().0,
        before
    );
    let saved = command(&mut h, "checkpoint", json!({}), &[]).unwrap().0;
    h.run(100).unwrap();
    let after = command(&mut h, "fingerprint", json!({}), &[]).unwrap().0;
    let state = command(&mut h, "state", json!({}), &[]).unwrap().0;
    assert_eq!(state["inputQueue"]["completed"], 4);
    assert_eq!(state["inputQueue"]["active"], Value::Null);
    assert_eq!(state["positions"]["button"], "released");
    command(&mut h, "restore", json!({"id":saved["id"]}), &[]).unwrap();
    assert_eq!(
        command(&mut h, "fingerprint", json!({}), &[]).unwrap().0,
        before
    );
    h.run(100).unwrap();
    assert_eq!(
        command(&mut h, "fingerprint", json!({}), &[]).unwrap().0,
        after
    );
    h.core.board.finish().unwrap();
}

#[test]
fn full_contact_queue_reserves_release_and_refuses_without_losing_accepted_taps() {
    let mut h = create(true);
    for _ in 0..32 {
        command(&mut h, "press", json!({"id":"button"}), &[]).unwrap();
        command(&mut h, "release", json!({"id":"button"}), &[]).unwrap();
    }
    let before = command(&mut h, "fingerprint", json!({}), &[]).unwrap().0;
    assert!(
        command(&mut h, "press", json!({"id":"button"}), &[])
            .unwrap_err()
            .contains("queue is full")
    );
    assert_eq!(
        command(&mut h, "fingerprint", json!({}), &[]).unwrap().0,
        before
    );
    h.run(900).unwrap();
    let state = command(&mut h, "state", json!({}), &[]).unwrap().0;
    assert_eq!(state["inputQueue"]["completed"], 64);
    assert_eq!(state["positions"]["button"], "released");
    h.core.board.finish().unwrap();
}

#[test]
fn durable_storage_cold_lifecycle_and_held_boot_contacts_use_the_same_c_abi() {
    let mut h = create(true);
    let initial = command(&mut h, "checkpoint", json!({}), &[]).unwrap().0;
    let saved = command(&mut h, "save", json!({}), &[]).unwrap();
    assert!(!saved.1.is_empty());
    command(
        &mut h,
        "restart_card",
        json!({"files":[{"path":"NEW.BIN","offset":0,"length":4}]}),
        &[9, 8, 7, 6],
    )
    .unwrap();
    assert_eq!(
        command(&mut h, "card_read", json!({"path":"KEEP.TXT"}), &[])
            .unwrap()
            .1,
        vec![1, 2, 3]
    );
    assert_eq!(
        command(&mut h, "card_read", json!({"path":"NEW.BIN"}), &[])
            .unwrap()
            .1,
        vec![9, 8, 7, 6]
    );
    assert!(command(&mut h, "restore", json!({"id":initial["id"]}), &[]).is_err());
    command(
        &mut h,
        "power_cycle",
        json!({"inputs":[{"id":"button"}]}),
        &[],
    )
    .unwrap();
    let state = command(&mut h, "state", json!({}), &[]).unwrap().0;
    assert_eq!(state["positions"]["button"], "pressed");
    assert_eq!(state["inputQueue"]["pending"], 0);
    let status = status_of(&h);
    assert_eq!((status.ticks, status.instructions), (0, 0));
    h.run(200).unwrap();
    assert_eq!(
        command(
            &mut h,
            "memory_read",
            json!({"address":"65793","size":1}),
            &[]
        )
        .unwrap()
        .1,
        vec![1]
    );
    command(&mut h, "release", json!({"id":"button"}), &[]).unwrap();
    h.run(400).unwrap();
    assert_eq!(
        command(
            &mut h,
            "memory_read",
            json!({"address":"65793","size":1}),
            &[]
        )
        .unwrap()
        .1,
        vec![0]
    );
    command(&mut h, "power", json!({"on":false}), &[]).unwrap();
    assert_eq!(
        command(&mut h, "state", json!({}), &[]).unwrap().0["poweredOn"],
        false
    );
    h.run(9999).unwrap();
    let status = status_of(&h);
    assert_eq!(status.instructions, 0);
    assert_eq!(
        command(
            &mut h,
            "memory_read",
            json!({"address":"65793","size":1}),
            &[]
        )
        .unwrap()
        .1,
        vec![0]
    );
    let stable = command(&mut h, "fingerprint", json!({}), &[]).unwrap().0;
    assert!(
        command(
            &mut h,
            "power_cycle",
            json!({"inputs":[{"id":"missing"}]}),
            &[]
        )
        .is_err()
    );
    assert_eq!(
        command(&mut h, "fingerprint", json!({}), &[]).unwrap().0,
        stable
    );
    let off = command(&mut h, "save", json!({}), &[]).unwrap().1;
    let mut other = create(true);
    command(&mut other, "restore_durable", json!({}), &off).unwrap();
    assert_eq!(
        command(&mut other, "state", json!({}), &[]).unwrap().0["poweredOn"],
        false
    );
    assert_eq!(
        command(&mut other, "card_read", json!({"path":"NEW.BIN"}), &[])
            .unwrap()
            .1,
        vec![9, 8, 7, 6]
    );
    command(&mut other, "power", json!({"on":true}), &[]).unwrap();
    assert_eq!(
        command(&mut other, "state", json!({}), &[]).unwrap().0["poweredOn"],
        true
    );
    let before = command(&mut other, "fingerprint", json!({}), &[])
        .unwrap()
        .0;
    let mut corrupt = off.clone();
    corrupt.push(0);
    assert!(command(&mut other, "restore_durable", json!({}), &corrupt).is_err());
    assert_eq!(
        command(&mut other, "fingerprint", json!({}), &[])
            .unwrap()
            .0,
        before
    );
    command(&mut other, "restore_durable", json!({}), &saved.1).unwrap();
    assert!(command(&mut other, "card_read", json!({"path":"NEW.BIN"}), &[]).is_err());
    h.core.board.finish().unwrap();
    other.core.board.finish().unwrap();
}

#[test]
fn rotary_admission_is_atomic_and_in_flight_phases_replay_through_the_command_api() {
    let mut h = create_with(false, |config| {
        let graph = &mut config["graph"];
        graph["execution"]["peripherals"]
            .as_array_mut()
            .unwrap()
            .push(json!({"core":0,"address":0xfff89000u64,"physical":0xfff89000u64,"size":4096}));
        let nodes = graph["devices"]["nodes"].as_array_mut().unwrap();
        nodes.push(json!({"name":"pads","boot_entries":["loader"],"windows":[{"base":0xfff89000u64,"size":1152}],"dependencies":[],"model":{"kind":"gpio","options":{"input_levels":[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],"output_levels":[]},"irqs":[]}}));
        nodes.push(json!({"name":"encoder","boot_entries":["loader"],"windows":[],"dependencies":["pads"],"model":{"kind":"encoder","gpio":"pads","counter":null,"options":{"pins":[{"port":3,"pin":6},{"port":3,"pin":7}],"phase_ticks":24,"rest_phase":0}}}));
        graph["contactDriver"] = json!({"poll_ticks":10,"max_phase_ticks":1000,
            "catalog":[{"id":"lever","label":"Lever","kind":"selector","supported":true},{"id":"ring","label":"Ring","kind":"rotary","supported":true,"relative":true}],
            "actuators":[{"id":"lever","momentary":false,"states":[{"position":"off","ready":[],"expected":[]},{"position":"on","ready":[],"expected":[]}]}],
            "interfaces":[{"kind":"rotary","id":"ring","node":"encoder","cycles_per_step":-1}]});
    });
    command(&mut h, "checkpoint", json!({}), &[]).unwrap();
    let cold = command(&mut h, "fingerprint", json!({}), &[]).unwrap().0;
    let entries = command(&mut h, "history", json!({}), &[]).unwrap().0;
    assert!(
        command(&mut h, "rotate", json!({"id":"ring","steps":385}), &[])
            .unwrap_err()
            .contains("bound")
    );
    assert_eq!(
        command(&mut h, "fingerprint", json!({}), &[]).unwrap().0,
        cold
    );
    assert_eq!(
        command(&mut h, "history", json!({}), &[]).unwrap().0,
        entries
    );
    command(&mut h, "rotate", json!({"id":"ring","steps":2}), &[]).unwrap();
    let queued = command(&mut h, "fingerprint", json!({}), &[]).unwrap().0;
    h.run(100).unwrap();
    let middle = command(&mut h, "fingerprint", json!({}), &[]).unwrap().0;
    assert_ne!(middle, cold);
    let (_, log) = command(&mut h, "history", json!({}), &[]).unwrap();
    command(&mut h, "undo", json!({}), &[]).unwrap();
    assert_eq!(
        command(&mut h, "fingerprint", json!({}), &[]).unwrap().0,
        queued
    );
    command(&mut h, "undo", json!({}), &[]).unwrap();
    assert_eq!(
        command(&mut h, "fingerprint", json!({}), &[]).unwrap().0,
        cold
    );
    command(&mut h, "replay", json!({}), &log).unwrap();
    assert_eq!(
        command(&mut h, "fingerprint", json!({}), &[]).unwrap().0,
        middle
    );
    h.core.board.finish().unwrap();
}

#[test]
fn raw_usb_commands_preserve_opaque_packets_and_never_run_the_guest() {
    let mut handle =
        create_with(false, |config| {
            let graph = &mut config["graph"];
            graph["devices"]["wires"]["cable"] = json!({"logic-level":true});
            graph["devices"]["nodes"].as_array_mut().unwrap().push(json!({
            "name":"usb", "boot_entries":["loader"],
            "windows":[{"base":0xfff54000u64,"size":4096},{"base":0xfff55000u64,"size":4096}],
            "dependencies":["irq"], "model":{"kind":"usb", "cable":"cable",
                "options":{"endpoints":{"transfer_ticks":4,"packet_bytes":64,"dma_limit":1024,
                    "out_endpoints":[[0,0],[1,1]],"in_endpoints":[[128,0],[130,1],[131,2]]}},
                "endpoints":{"irq":{"Core":{"core":0,"line":84}},
                    "dma_irq":{"Core":{"core":0,"line":85}},"ram_bytes":0x14000}}
        }));
            for base in [0xfff54000u64, 0xfff55000] {
                graph["execution"]["peripherals"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!({
                        "core":0,"address":base,"physical":base,"size":4096
                    }));
            }
        });
    let usb = |h: &mut Session| value(h.usb_status("usb").unwrap());
    let before = usb(&mut handle);
    assert_eq!(before, usb(&mut handle));
    assert_eq!(before["connected"], true);
    command(&mut handle, "usb_reset", json!({"node":"usb"}), &[]).unwrap();
    command(
        &mut handle,
        "usb_setup",
        json!({"node":"usb"}),
        &[0x80, 6, 0, 1, 0, 0, 18, 0],
    )
    .unwrap();
    command(
        &mut handle,
        "usb_out",
        json!({"node":"usb","endpoint":1}),
        &[0xde, 0xad, 0xbe, 0xef],
    )
    .unwrap();
    assert_eq!(usb(&mut handle)["endpoint"]["setup_length"], 8);
    assert_eq!(usb(&mut handle)["endpoint"]["rx_length"][1], 4);
    assert!(
        command(
            &mut handle,
            "usb_in",
            json!({"node":"usb","endpoint":130,"size":64}),
            &[]
        )
        .unwrap_err()
        .contains("no packet")
    );
    assert!(
        command(
            &mut handle,
            "usb_out",
            json!({"node":"usb","endpoint":1}),
            &[1]
        )
        .is_err()
    );
    assert!(command(&mut handle, "usb_setup", json!({"node":"usb"}), &[1]).is_err());
    let state = status_of(&handle);
    assert_eq!(state.ticks, 0);
    assert_eq!(state.instructions, 0);
    assert!(!state.failed);
    handle.core.board.finish().unwrap();
}

#[test]
fn device_workers_are_host_preferences_without_history_or_fingerprint_effects() {
    let mut handle = create(false);
    assert_eq!(
        command(&mut handle, "device_workers", json!({}), &[])
            .unwrap()
            .0,
        json!({"count":0,"effective":0})
    );
    let before = command(&mut handle, "fingerprint", json!({}), &[]).unwrap();
    assert_eq!(
        command(&mut handle, "device_workers", json!({"count":2}), &[])
            .unwrap()
            .0,
        json!({"count":2,"effective":0})
    );
    assert!(command(&mut handle, "device_workers", json!({"count":1}), &[]).is_err());
    assert_eq!(
        command(&mut handle, "device_workers", json!({"count":0}), &[])
            .unwrap()
            .0,
        json!({"count":0,"effective":0})
    );
    assert_eq!(
        command(&mut handle, "fingerprint", json!({}), &[]).unwrap(),
        before
    );
    handle.core.board.finish().unwrap();
}

#[test]
fn interactive_device_workers_default_inline_and_allow_explicit_stopped_selection() {
    let mut handle = create_policy(false, ExecutionPolicy::Interactive, |_| {});
    assert_eq!(
        command(&mut handle, "device_workers", json!({}), &[])
            .unwrap()
            .0,
        json!({"count":0,"effective":0})
    );
    let before = command(&mut handle, "fingerprint", json!({}), &[]).unwrap();
    assert_eq!(
        command(&mut handle, "device_workers", json!({"count":2}), &[])
            .unwrap()
            .0,
        json!({"count":2,"effective":2})
    );
    assert_eq!(
        command(&mut handle, "fingerprint", json!({}), &[]).unwrap(),
        before
    );
    handle.run(5).unwrap();
    assert_eq!(
        command(&mut handle, "device_workers", json!({}), &[])
            .unwrap()
            .0,
        json!({"count":2,"effective":2})
    );
    assert_eq!(
        command(&mut handle, "device_workers", json!({"count":0}), &[])
            .unwrap()
            .0,
        json!({"count":0,"effective":0})
    );
    handle.core.board.finish().unwrap();
}
