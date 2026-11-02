use super::*;
use fujimod_catalog_bulk::Upload;
use fujimod_usb::{
    ff80::{SessionState, framing::RequestKind},
    testing::{Exchange as E, Script},
};
use futures_lite::future::block_on;

fn envelope(kind: RequestKind, size: u32, id: u16) -> [u8; 12] {
    let mut raw = [0; 12];
    raw[0] = kind as u8;
    raw[6..10].copy_from_slice(&size.to_be_bytes());
    raw[10..12].copy_from_slice(&id.to_be_bytes());
    raw
}
fn exchange(request: Vec<u8>, reply: Vec<u8>) -> Vec<E> {
    vec![
        E::Control(envelope(
            RequestKind::WriteCommand,
            request.len() as u32,
            0x1234,
        )),
        E::Write(request),
        E::Control(envelope(
            RequestKind::ReadCommand,
            reply.len() as u32,
            0x1234,
        )),
        E::Read {
            maximum: reply.len(),
            data: reply,
        },
    ]
}
fn read(address: u32, data: &[u8]) -> Vec<E> {
    let request: Vec<_> = [0x200001, 0, 0, 0, 2, 0, 0, address, data.len() as u32]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect();
    let mut reply = request.clone();
    reply.extend(data);
    exchange(request, reply)
}
fn write(address: u32, data: &[u8]) -> Vec<E> {
    let mut request: Vec<_> = [0x200001, 0, 0, 0, 3, 0, 0, address, data.len() as u32]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect();
    let reply = request.clone();
    request.extend(data);
    exchange(request, reply)
}
fn session(commands: Vec<E>) -> (Session, Script) {
    let mut steps = vec![
        E::Control(envelope(RequestKind::Open, 8, 0)),
        E::Read {
            maximum: 8,
            data: vec![0x34, 0x12, 0, 0, 0, 0, 1, 0],
        },
    ];
    steps.extend(commands);
    let script = Script::new(steps);
    (block_on(script.begin_session(65536, None)).unwrap(), script)
}
fn upload() -> Upload {
    Upload {
        pointer: 0x1000,
        pointer_bytes: 4,
        configuration_checks: vec![],
        capacity_address: 0x1004,
        piece_bytes: 128,
        address_alignment: 32,
        code_checks: vec![fujimod_catalog_bulk::CodeCheck {
            name: "upload".into(),
            address: 0x9000,
            size: 4,
            sha256: fujimod_usb::operations::checks::sha256_bytes(b"code")
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect(),
        }],
    }
}
fn limits() -> DebugMemoryLimits {
    DebugMemoryLimits {
        read: NonZeroU32::new(256).unwrap(),
        write: NonZeroU32::new(4).unwrap(),
    }
}

#[test]
fn e0_upload_preserves_padding_ownership_and_restores_destination_on_success_or_completed_error() {
    block_on(async {
        for refusal in [false, true] {
            let original = 0x8000u32.to_le_bytes();
            let selected = 0x4000u32.to_le_bytes();
            let capacity = 128u32.to_le_bytes();
            let mut steps = read(0x9000, b"code");
            // The transport discovers current state; OwnedUpload guards it again before publication.
            steps.extend(read(0x1000, &original));
            steps.extend(read(0x1004, &capacity));
            steps.extend(read(0x1000, &original));
            steps.extend(read(0x1004, &capacity));
            steps.extend(write(0x1000, &selected));
            steps.extend(read(0x1000, &selected));
            let mut setup = vec![0; 32];
            setup[..4].copy_from_slice(&0x760001u32.to_le_bytes());
            setup[20..24].copy_from_slice(&128u32.to_le_bytes());
            steps.extend(exchange(setup.clone(), setup.clone()));
            let mut payload = vec![0x5a; 70];
            payload.resize(128, 0);
            steps.extend([
                E::Control(envelope(RequestKind::WriteData, 128, 0x1234)),
                E::Write(payload),
            ]);
            if refusal {
                setup[4..8].copy_from_slice(&9u32.to_le_bytes());
                setup[8..12].copy_from_slice(&3u32.to_le_bytes());
            }
            steps.extend([
                E::Control(envelope(RequestKind::EmptyResponse, 32, 0x1234)),
                E::Read {
                    maximum: 32,
                    data: setup,
                },
            ]);
            steps.extend(read(0x1000, &selected));
            steps.extend(write(0x1000, &original));
            steps.extend(read(0x1000, &original));
            let (mut session, script) = session(steps);
            let upload = upload();
            let mut io = Transport::new(None, limits(), &session, Some(&upload));
            let result = io.write_owned(&mut session, 0x4000, &[0x5a; 70], 128).await;
            if refusal {
                assert!(matches!(result, Err(Error::DeviceStatus { .. })));
            } else {
                assert!(result.unwrap());
            }
            assert_eq!(session.state(), SessionState::Ready);
            assert_eq!(script.remaining(), 0);
        }
    });
}
#[test]
fn tiny_writes_or_padding_outside_the_owned_buffer_choose_cpu_without_usb_setup() {
    block_on(async {
        let (mut session, script) = session(vec![]);
        let upload = upload();
        let mut io = Transport::new(None, limits(), &session, Some(&upload));
        assert!(
            !io.write_owned(&mut session, 0x4000, &[1; 3], 128)
                .await
                .unwrap()
        );
        assert!(
            !io.write_owned(&mut session, 0x4000, &[1; 70], 70)
                .await
                .unwrap()
        );
        assert!(
            !io.write_owned(&mut session, 0x4001, &[1; 70], 128)
                .await
                .unwrap()
        );
        let mut wide_cpu = limits();
        wide_cpu.write = NonZeroU32::new(256).unwrap();
        let mut io = Transport::new(None, wide_cpu, &session, Some(&upload));
        assert!(
            !io.write_owned(&mut session, 0x4000, &[1; 70], 128)
                .await
                .unwrap(),
            "one CPU command beats upload setup"
        );
        assert_eq!(script.remaining(), 0);
    });
}

#[test]
fn wide_upload_guards_and_restores_the_complete_64_bit_pointer() {
    block_on(async {
        let original = 0x1_0000_8000u64.to_le_bytes();
        let selected = 0x4000u64.to_le_bytes();
        let mut steps = read(0x9000, b"code");
        for _ in 0..2 {
            steps.extend(read(0x1000, &original));
            steps.extend(read(0x1008, &128u32.to_le_bytes()));
        }
        steps.extend(write(0x1000, &selected[..4]));
        steps.extend(write(0x1004, &selected[4..]));
        steps.extend(read(0x1000, &selected));
        let mut setup = vec![0; 32];
        setup[..4].copy_from_slice(&0x760001u32.to_le_bytes());
        setup[20..24].copy_from_slice(&128u32.to_le_bytes());
        steps.extend(exchange(setup.clone(), setup.clone()));
        let mut payload = vec![0x5a; 70];
        payload.resize(128, 0);
        steps.extend([
            E::Control(envelope(RequestKind::WriteData, 128, 0x1234)),
            E::Write(payload),
            E::Control(envelope(RequestKind::EmptyResponse, 32, 0x1234)),
            E::Read {
                maximum: 32,
                data: setup,
            },
        ]);
        steps.extend(read(0x1000, &selected));
        steps.extend(write(0x1000, &original[..4]));
        steps.extend(write(0x1004, &original[4..]));
        steps.extend(read(0x1000, &original));
        let (mut session, script) = session(steps);
        let mut upload = upload();
        upload.pointer_bytes = 8;
        upload.capacity_address = 0x1008;
        let mut io = Transport::new(None, limits(), &session, Some(&upload));
        assert!(
            io.write_owned(&mut session, 0x4000, &[0x5a; 70], 128)
                .await
                .unwrap()
        );
        assert_eq!(script.remaining(), 0);
    });
}

#[test]
fn setup_with_configuration_effects_is_skipped_when_its_ram_only_precondition_is_absent() {
    use fujimod_usb::ff80::commands::configuration::{
        ConfigurationLocation, ConfigurationView, ReadConfiguration,
    };
    let request = ReadConfiguration::new(
        ConfigurationLocation {
            view: ConfigurationView::Ram,
            page: 0,
            offset: 0x1700,
        },
        NonZeroU32::MIN,
        NonZeroU32::new(256).unwrap(),
    )
    .unwrap()
    .encode();
    let mut reply = request[..36].to_vec();
    reply.push(1);
    let mut steps = read(0x9000, b"code");
    steps.extend(exchange(request, reply));
    let (mut session, script) = session(steps);
    let mut upload = upload();
    upload.configuration_checks = vec![fujimod_catalog_bulk::ConfigurationCheck {
        offset: 0x1700,
        value: 0,
    }];
    let mut io = Transport::new(None, limits(), &session, Some(&upload));
    assert!(!block_on(io.write_owned(&mut session, 0x4000, &[0x5a; 70], 128)).unwrap());
    assert_eq!(
        script.remaining(),
        0,
        "no pointer or configuration writes and no E0"
    );
}
