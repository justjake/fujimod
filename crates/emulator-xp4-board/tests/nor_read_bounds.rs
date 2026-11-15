//! Source coverage applies to ordinary board execution without a driver guard.
use fujimod_emulator_xp4_board::{Definition, Session};
use serde_json::{Value, json};

const FLASH: u64 = 0xf0000000;
const SERIAL: u64 = 0xfffd8000;
const DMA: u64 = 0xfffe0000;
const OUTPUT: usize = 0x4000;

fn definition(strict: bool) -> Value {
    json!({
        "ram_size": 65536, "cores": 1, "discovery": false, "cbar": 0xffff0000u32,
        "storage_jedec": 0x20, "storage_geometry": [4096, 256, 8, 2],
        "storage_irq": 37, "storage_dma_routes": (207..223).collect::<Vec<_>>(),
        "storage_strict_backing": strict,
        "initial_registers": [[[{"Arm32": {"R": 15}}, 0x1000]]]
    })
}

fn session(strict: bool, bytes: Vec<u8>) -> Session {
    Session::new(serde_json::from_value::<Definition>(definition(strict)).unwrap(),
                 Some(bytes), vec![]).unwrap()
}

fn mov32(code: &mut Vec<u32>, register: u32, value: u32) {
    for (base, half) in [(0xe3000000, value & 0xffff), (0xe3400000, value >> 16)] {
        code.push(base | register << 12 | (half & 0xf000) << 4 | half & 0xfff);
    }
}

fn store(code: &mut Vec<u32>, address: u64, value: u32) {
    mov32(code, 0, address as u32);
    mov32(code, 1, value);
    code.push(0xe5801000);
}

fn run(session: &Session, code: &[u32]) {
    let bytes: Vec<_> = code.iter().flat_map(|word| word.to_le_bytes()).collect();
    session.write(0x1000, &bytes).unwrap();
    // DMA writes may yield for translated-code invalidation. Resume the same
    // bounded session; no guard is used to supply the source bound.
    for _ in 0..8 {
        let state = session.state(false);
        if state.failure.is_some() || state.instructions >= code.len() as u64 {
            break;
        }
        let remaining = 1000u64.saturating_sub(state.instructions);
        assert!(remaining > 0, "native caller instruction bound reached");
        let _ = session.run(0, remaining as usize);
    }
}

fn device(session: &Session, name: &str) -> Value {
    serde_json::from_str(&session.device_state_json()[name]).unwrap()
}

#[test]
fn ordinary_cpu_reads_refuse_crossing_the_asset_and_default_to_erased_bytes_when_omitted() {
    for (strict, offset, expected) in [
        (true, 24, Some([24, 25, 26, 27])),
        (true, 28, None),
        (true, 32, None),
        (false, 28, Some([28, 29, 255, 255])),
        (false, 32, Some([255; 4])),
    ] {
        let session = session(strict, (0..30).collect());
        session.write(OUTPUT, &[0xa5; 8]).unwrap();
        let mut code = vec![];
        mov32(&mut code, 0, (FLASH + offset) as u32);
        code.push(0xe5902000); // ldr r2,[r0]
        mov32(&mut code, 1, OUTPUT as u32);
        code.extend([0xe5812000, 0xe320f003]); // str; wfi
        run(&session, &code);
        let mut actual = [0; 8];
        session.read(OUTPUT, &mut actual).unwrap();
        match expected {
            Some(bytes) => {
                assert!(session.state(false).failure.is_none());
                assert_eq!(&actual[..4], &bytes);
                assert_eq!(&actual[4..], &[0xa5; 4]);
            }
            None => {
                assert!(session.state(false).failure.unwrap().contains("outside supplied backing"));
                assert_eq!(actual, [0xa5; 8]);
            }
        }
    }
}

#[test]
fn ordinary_dma_read_checks_entire_source_before_destination_copy() {
    for (strict, offset, success) in [(true, 0, true), (true, 16, false), (false, 16, true)] {
        let session = session(strict, (0..32).collect());
        session.write(OUTPUT, &[0xa5; 64]).unwrap();
        let mut code = vec![];
        for (address, value) in [
            (DMA + 0x14, (FLASH + offset) as u32), (DMA + 0x18, OUTPUT as u32),
            (DMA + 0x10, 31), (DMA + 0x1c, 0x30f04), (DMA + 0x20, 0x30f00),
            (DMA + 0x24, 0), (DMA + 0x28, 0x10201011), (0xfff4a500, 0x12c2a),
            (SERIAL + 0x20, 0x707), (SERIAL + 0x68, (FLASH + offset) as u32),
            (SERIAL + 0x6c, 32), (SERIAL + 0x60, 1),
        ] {
            store(&mut code, address, value);
        }
        code.extend([0xe1a00000; 128]);
        code.push(0xe320f003);
        run(&session, &code);
        let mut actual = [0; 64];
        session.read(OUTPUT, &mut actual).unwrap();
        let channel = &device(&session, "xp4-dma")["channels"][0];
        let serial = device(&session, "xp4-serial-flash");
        if success {
            assert!(session.state(false).failure.is_none());
            let expected: Vec<u8> = (offset..offset + 32)
                .map(|n| if n < 32 { n as u8 } else { 255 }).collect();
            assert_eq!(&actual[..32], expected.as_slice());
            assert_eq!(&actual[32..], &[0xa5; 32]);
            assert_eq!(channel["completed"], 1);
            assert_eq!(channel["submitted"], false);
        } else {
            assert!(session.state(false).failure.unwrap().contains("outside supplied backing"));
            assert_eq!(actual, [0xa5; 64]);
            assert_eq!(channel["completed"], 0);
            assert!(serial["stream"].is_null());
            assert!(serial["next"].is_null());
        }
    }
}

#[test]
fn strict_backing_requires_explicit_contents_and_a_nor_owner() {
    assert!(Session::new(serde_json::from_value::<Definition>(definition(true)).unwrap(),
                         None, vec![]).is_err());
    let mut no_owner = definition(true);
    no_owner["storage_jedec"] = Value::Null;
    assert!(Session::new(serde_json::from_value::<Definition>(no_owner).unwrap(),
                         Some(vec![]), vec![]).is_err());
    // An explicit empty source is valid for identity/status-only callers.
    let session = session(true, vec![]);
    let mut code = vec![];
    store(&mut code, SERIAL + 0x90, 0x9f800001);
    code.extend([0xe1a00000; 16]);
    mov32(&mut code, 0, (SERIAL + 0xa0) as u32);
    code.push(0xe5902000);
    mov32(&mut code, 1, OUTPUT as u32);
    code.extend([0xe5812000, 0xe320f003]);
    run(&session, &code);
    assert!(session.state(false).failure.is_none());
    let mut identity = [0; 4];
    session.read(OUTPUT, &mut identity).unwrap();
    assert_eq!(u32::from_le_bytes(identity), 0x20);
}

fn erased_definition() -> Value {
    let mut selected = definition(true);
    selected["storage_erased_ranges"] = json!([
        {"offset": 96, "length": 32}, {"offset": 30, "length": 2}
    ]);
    selected
}

#[test]
fn explicit_erased_ranges_admit_cpu_reads_and_keep_holes_inaccessible() {
    for (offset, expected) in [(28, Some([28, 29, 255, 255])),
                                (32, None), (92, None),
                                (96, Some([255; 4])), (124, Some([255; 4])), (128, None)] {
        let session = Session::new(serde_json::from_value(erased_definition()).unwrap(),
                                   Some((0..30).collect()), vec![]).unwrap();
        session.write(OUTPUT, &[0xa5; 8]).unwrap();
        let mut code = vec![];
        mov32(&mut code, 0, (FLASH + offset) as u32);
        code.push(0xe5902000);
        mov32(&mut code, 1, OUTPUT as u32);
        code.extend([0xe5812000, 0xe320f003]);
        run(&session, &code);
        let mut actual = [0; 8];
        session.read(OUTPUT, &mut actual).unwrap();
        if let Some(bytes) = expected {
            assert!(session.state(false).failure.is_none());
            assert_eq!(&actual[..4], &bytes);
        } else {
            assert!(session.state(false).failure.unwrap().contains("outside supplied backing"));
            assert_eq!(&actual[..4], &[0xa5; 4]);
        }
        assert_eq!(&actual[4..], &[0xa5; 4]);
        assert_eq!(device(&session, "xp4-serial-flash")["nor"]["read_backing"]["erased_ranges"],
                   json!([{"offset":30,"length":2},{"offset":96,"length":32}]));
    }
}

#[test]
fn declared_erased_input_is_copied_by_original_dma_routing_without_filling_gaps() {
    for (offset, success) in [(96, true), (16, false)] {
        let session = Session::new(serde_json::from_value(erased_definition()).unwrap(),
                                   Some((0..30).collect()), vec![]).unwrap();
        session.write(OUTPUT, &[0xa5; 64]).unwrap();
        let mut code = vec![];
        for (address, value) in [
            (DMA + 0x14, (FLASH + offset) as u32), (DMA + 0x18, OUTPUT as u32),
            (DMA + 0x10, 31), (DMA + 0x1c, 0x30f04), (DMA + 0x20, 0x30f00),
            (DMA + 0x24, 0), (DMA + 0x28, 0x10201011), (0xfff4a500, 0x12c2a),
            (SERIAL + 0x20, 0x707), (SERIAL + 0x68, (FLASH + offset) as u32),
            (SERIAL + 0x6c, 32), (SERIAL + 0x60, 1),
        ] { store(&mut code, address, value); }
        code.extend([0xe1a00000; 128]);
        code.push(0xe320f003);
        run(&session, &code);
        let mut actual = [0; 64];
        session.read(OUTPUT, &mut actual).unwrap();
        if success {
            assert!(session.state(false).failure.is_none());
            assert_eq!(&actual[..32], &[255; 32]);
            assert_eq!(&actual[32..], &[0xa5; 32]);
        } else {
            assert!(session.state(false).failure.unwrap().contains("outside supplied backing"));
            assert_eq!(actual, [0xa5; 64]);
            assert!(device(&session, "xp4-serial-flash")["stream"].is_null());
        }
    }
}

#[test]
fn erased_input_requires_strict_explicit_nor_contents_and_valid_ranges() {
    let selected = erased_definition();
    assert!(Session::new(serde_json::from_value(selected.clone()).unwrap(), None, vec![]).is_err());
    for (field, value) in [("storage_strict_backing", json!(false)),
                           ("storage_jedec", Value::Null),
                           ("storage_erased_ranges", json!([{"offset":29,"length":1}])),
                           ("storage_erased_ranges", json!([{"offset":32,"length":0}])),
                           ("storage_erased_ranges", json!([{"offset":0x8000000,"length":1}]))] {
        let mut bad = selected.clone();
        bad[field] = value;
        assert!(Session::new(serde_json::from_value(bad).unwrap(), Some(vec![0;30]), vec![]).is_err());
    }
    let mut empty = definition(false);
    empty["storage_erased_ranges"] = json!([]);
    assert!(Session::new(serde_json::from_value(empty).unwrap(), Some(vec![0;30]), vec![]).is_ok());
}
