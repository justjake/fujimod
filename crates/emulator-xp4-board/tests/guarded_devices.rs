use fujimod_emulator_cpu::{Register, arm};
use fujimod_emulator_xp4_board::{Definition, Session};
use serde_json::{Value, json};

const SERIAL: u64 = 0xfffd8000;
const DMA: u64 = 0xfffe0000;
const MUX: u64 = 0xfff4a500;

fn session() -> Session {
    let definition: Definition = serde_json::from_value(json!({
        "ram_size": 65536, "cores": 1, "discovery": false, "cbar": 0xffff0000u32,
        "storage_jedec": 0x20ba20, "storage_geometry": [65536, 256, 100, 10],
        "storage_irq": 37, "storage_dma_routes": (207..223).collect::<Vec<_>>(),
        "initial_registers": [[[{"Arm32": {"R": 15}}, 0x1000],
                                [{"Arm32": {"R": 14}}, 0x8000]]]
    }))
    .unwrap();
    Session::new(
        definition,
        Some((0..256).map(|n| n as u8).collect()),
        vec![],
    )
    .unwrap()
}

fn policy(addresses: &[u64], write: bool, model_writable: Vec<(u64, u64)>) -> Value {
    json!({"mmio": addresses.iter().map(|&start| json!({
        "start": start, "end": start + 4, "width": 4, "read": true, "write": write
    })).collect::<Vec<_>>(), "model_readable": [], "model_writable": model_writable})
}

fn mov32(code: &mut Vec<u32>, register: u32, value: u32) {
    for (base, half) in [(0xe3000000, value & 0xffff), (0xe3400000, value >> 16)] {
        code.push(base | register << 12 | (half & 0xf000) << 4 | half & 0xfff);
    }
}

fn program(session: &Session, code: &[u32], policy: Value) {
    let bytes: Vec<_> = code.iter().flat_map(|word| word.to_le_bytes()).collect();
    session.write(0x1000, &bytes).unwrap();
    session
        .driver_guard_begin(
            vec![(0x1000, 0x1000 + bytes.len() as u64)],
            vec![(0, 65536)],
            vec![(0x4000, 0x4040)],
            0x8000,
            4096,
            0,
            Some(policy.to_string()),
        )
        .unwrap();
}

#[test]
fn modeled_register_access_requires_explicit_policy_and_checks_before_dispatch() {
    for write in [false, true] {
        let session = session();
        let mut code = vec![];
        mov32(&mut code, 0, (SERIAL + 0x68) as u32);
        mov32(&mut code, 1, 0xf0000100);
        code.extend([0xe5801000, 0xe5902000, 0xe12fff1e]);
        program(&session, &code, policy(&[SERIAL + 0x68], write, vec![]));
        session.run(0, 64).unwrap();
        let observed = session.driver_guard_state().unwrap();
        if write {
            assert!(observed.returned && observed.failure.is_none());
            assert_eq!(
                observed.mmio_accesses,
                vec![(SERIAL + 0x68, 4, true), (SERIAL + 0x68, 4, false)]
            );
            assert_eq!(
                session
                    .read_register(0, Register::Arm32(arm::Register::R(2)))
                    .unwrap(),
                0xf0000100
            );
        } else {
            assert!(observed.failure.unwrap().contains("rejected write"));
            assert!(observed.mmio_accesses.is_empty());
        }
        session.driver_guard_end();
        if !write {
            // Read the actual model register in a second admitted call: the
            // denied store must have left it at its cold zero value.
            session
                .write_register(0, Register::Arm32(arm::Register::R(15)), 0x2000)
                .unwrap();
            session
                .write_register(0, Register::Arm32(arm::Register::R(2)), 0xdeadbeef)
                .unwrap();
            // A new address avoids reusing translated code from the first call.
            let code: Vec<_> = [0xe5902000u32, 0xe12fff1e]
                .into_iter()
                .flat_map(u32::to_le_bytes)
                .collect();
            session.write(0x2000, &code).unwrap();
            session
                .driver_guard_begin(
                    vec![(0x2000, 0x2008)],
                    vec![(0, 65536)],
                    vec![],
                    0x8000,
                    32,
                    0,
                    Some(policy(&[SERIAL + 0x68], false, vec![]).to_string()),
                )
                .unwrap();
            session.run(0, 16).unwrap();
            assert_eq!(
                session
                    .read_register(0, Register::Arm32(arm::Register::R(2)))
                    .unwrap(),
                0
            );
            assert!(session.driver_guard_state().unwrap().returned);
            session.driver_guard_end();
        }
    }
    let session = session();
    assert!(
        session
            .driver_guard_begin(vec![(0x1000, 0x1004)], vec![], vec![], 0x8000, 32, 0, None)
            .unwrap_err()
            .contains("without devices")
    );
}

#[test]
fn native_admission_rejects_unmodeled_and_ram_device_policies() {
    for address in [0x2000, 0xfffd9000] {
        let session = session();
        assert!(
            session
                .driver_guard_begin(
                    vec![(0x1000, 0x1004)],
                    vec![],
                    vec![],
                    0x8000,
                    32,
                    0,
                    Some(policy(&[address], true, vec![]).to_string())
                )
                .is_err()
        );
        assert!(session.driver_guard_state().is_none());
        assert_eq!(session.state(false).instructions, 0);
    }
}

#[test]
fn guarded_storage_model_transfers_pattern_and_denies_out_of_bounds_dma_before_copy() {
    let writes = [
        (DMA + 0x14, 0xf0000000),
        (DMA + 0x18, 0x4000),
        (DMA + 0x10, 31),
        (DMA + 0x1c, 0x30f04),
        (DMA + 0x20, 0x30f00),
        (DMA + 0x24, 0),
        (DMA + 0x28, 0x10201011),
        (MUX, 0x12c2a),
        (SERIAL + 0x20, 0x707),
        (SERIAL + 0x68, 0xf0000000),
        (SERIAL + 0x6c, 32),
        (SERIAL + 0x60, 1),
    ];
    let addresses: Vec<_> = writes.iter().map(|pair| pair.0).collect();
    for admitted_length in [16, 32] {
        let session = session();
        session.write(0x4000, &[0xa5; 64]).unwrap();
        let mut code = vec![];
        for (address, value) in writes {
            mov32(&mut code, 0, address as u32);
            mov32(&mut code, 1, value);
            code.push(0xe5801000);
        }
        code.extend([0xe1a00000; 128]);
        code.push(0xe12fff1e);
        program(
            &session,
            &code,
            policy(&addresses, true, vec![(0x4000, 0x4000 + admitted_length)]),
        );
        let mut run = Ok(());
        // A modeled DMA write invalidates translated code and may yield the
        // CPU for a cache redirect. Resume through the same bounded session.
        for _ in 0..8 {
            let remaining = 1000u64.saturating_sub(session.state(false).instructions);
            assert!(remaining > 0, "guarded DMA instruction limit reached");
            run = session.run(0, remaining as usize);
            let guard = session.driver_guard_state().unwrap();
            if run.is_err()
                || guard.returned
                || guard.failure.is_some()
                || session.state(false).failure.is_some()
            {
                break;
            }
        }
        let observed = session.driver_guard_state().unwrap();
        let mut actual = [0; 64];
        session.read(0x4000, &mut actual).unwrap();
        if admitted_length == 32 {
            run.unwrap();
            assert!(
                observed.returned && observed.failure.is_none(),
                "returned={} guard={:?} runtime={:?}",
                observed.returned,
                observed.failure,
                session.state(false).failure
            );
            assert_eq!(&actual[..32], &(0..32).collect::<Vec<u8>>());
            assert_eq!(&actual[32..], &[0xa5; 32]);
            assert_eq!(observed.model_writes, vec![(0x4000, 32)]);
        } else {
            assert!(observed.failure.unwrap().contains("rejected model write"));
            assert!(observed.model_writes.is_empty());
            assert_eq!(actual, [0xa5; 64]);
        }
        session.driver_guard_end();
    }
}
