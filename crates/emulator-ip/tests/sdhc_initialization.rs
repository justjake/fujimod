use fujimod_emulator_devices::Device;
use fujimod_emulator_devices::bus::InterruptLine;
use fujimod_emulator_devices::parts::card_contacts::Contacts;
use fujimod_emulator_devices::parts::sd_card::{Card, Options as CardOptions};
use fujimod_emulator_devices::test_host::ram::{Effect, Ram};
use fujimod_emulator_ip::sdhci::host::{InitializationFault, Options, Protocol, SdHost, Wiring};

const BASE: u64 = 0x12000000;
const IRQ: u32 = 5;

fn options(protocol: Protocol, initialization_fault: InitializationFault) -> Options {
    Options {
        base: BASE,
        protocol,
        transfer_ticks: 5,
        clock_ticks: 2,
        rca: 7,
        ram_bytes: 0x4000,
        initialization_fault,
        high_speed: false,
    }
}

fn host(protocol: Protocol, fault: InitializationFault) -> (SdHost, Contacts, Ram) {
    let contacts = Contacts::new(true, false);
    let mut card = Card::new(CardOptions {
        sectors: 0x400000,
        max_dirty_sectors: 8,
    })
    .unwrap();
    card.write_bytes(512, &[0x53; 512]).unwrap();
    let host = SdHost::new(
        options(protocol, fault),
        Wiring {
            card,
            contacts: contacts.clone(),
            irq: InterruptLine::Shared(IRQ),
        },
    )
    .unwrap();
    (host, contacts, Ram::new(0x4000))
}

fn write(host: &mut SdHost, ram: &mut Ram, offset: u64, size: u8, value: u64) {
    let mut cx = ram.cx();
    host.write(&mut cx, BASE + offset, size, value);
    assert_eq!(cx.refusal(), None);
}

fn read(host: &mut SdHost, ram: &mut Ram, offset: u64, size: u8) -> u64 {
    let mut cx = ram.cx();
    let value = host.read(&mut cx, BASE + offset, size);
    assert_eq!(cx.refusal(), None);
    value
}

fn setup(host: &mut SdHost, ram: &mut Ram, protocol: Protocol) {
    write(host, ram, 0x2c, 2, 5);
    ram.now += 2;
    assert_eq!(read(host, ram, 0x2c, 2), 7);
    if protocol == Protocol::Rom {
        write(host, ram, 0x29, 1, 15);
    } else {
        write(host, ram, 0x28, 2, 0x100);
    }
    write(host, ram, 0x34, 2, 0xffff);
    write(host, ram, 0x36, 2, 1);
    if protocol != Protocol::Rom {
        write(host, ram, 0x38, 2, 0xffff);
        write(host, ram, 0x3a, 2, 1);
    }
}

fn command(host: &mut SdHost, ram: &mut Ram, word: u32, argument: u32) {
    write(host, ram, 0x30, 2, 0xffff);
    write(host, ram, 0x32, 2, 0xffff);
    write(host, ram, 8, 2, u64::from(argument & 0xffff));
    write(host, ram, 10, 2, u64::from(argument >> 16));
    write(host, ram, 12, 4, u64::from(word));
}

fn due(host: &mut SdHost, ram: &mut Ram) {
    ram.now = host.next_deadline(ram.now).unwrap();
    let mut cx = ram.cx();
    host.advance(&mut cx);
    assert_eq!(cx.refusal(), None);
}

fn enumerate(host: &mut SdHost, ram: &mut Ram) {
    for (word, argument) in [
        (0, 0),
        (0x081a0000, 0x1aa),
        (0x371a0000, 0),
        (0x29020000, 0x40100000),
        (0x02090000, 0),
        (0x031a0000, 0),
        (0x071b0000, 7 << 16),
    ] {
        command(host, ram, word, argument);
        due(host, ram);
        assert_eq!(read(host, ram, 0x30, 2) & 1, 1);
    }
}

#[test]
fn constructor_json_defaults_to_no_fault_and_rejects_unknown_policies() {
    let mut json = serde_json::json!({
        "base": BASE, "protocol": "sdhc", "transfer_ticks": 5,
        "clock_ticks": 2, "rca": 7, "ram_bytes": 0x4000
    });
    let normal: Options = serde_json::from_value(json.clone()).unwrap();
    assert_eq!(normal.initialization_fault, InitializationFault::None);
    json["initialization_fault"] = "reject_cmd8".into();
    let faulty: Options = serde_json::from_value(json.clone()).unwrap();
    assert_eq!(faulty.initialization_fault, InitializationFault::RejectCmd8);
    json["initialization_fault"] = "unsupported".into();
    assert!(serde_json::from_value::<Options>(json).is_err());
}

#[test]
fn cmd8_timeout_preserves_detect_media_and_response_and_replays_checkpoints() {
    for protocol in [Protocol::Sdhc, Protocol::Application, Protocol::Rom] {
        let (mut host, contacts, mut ram) = host(protocol, InitializationFault::RejectCmd8);
        setup(&mut host, &mut ram, protocol);
        let revision = host.card().revision();
        if protocol == Protocol::Rom {
            write(&mut host, &mut ram, 8, 4, 0x1aa);
            write(&mut host, &mut ram, 14, 2, 0x081a);
        } else {
            command(&mut host, &mut ram, 0x081a0000, 0x1aa);
        }
        assert!(host.busy());
        let deadline = host.next_deadline(ram.now).unwrap();
        let pending = host.checkpoint().unwrap();
        ram.now = deadline - 1;
        assert_eq!(read(&mut host, &mut ram, 0x30, 2), 0);
        assert_eq!(read(&mut host, &mut ram, 0x32, 2), 0);
        due(&mut host, &mut ram);
        assert!(!host.busy());
        assert_eq!(read(&mut host, &mut ram, 0x24, 4) & 0x10003, 0x10000);
        assert_eq!(read(&mut host, &mut ram, 0x30, 2), 0x8000);
        assert_eq!(read(&mut host, &mut ram, 0x32, 2), 1);
        let response_width = if protocol == Protocol::Application {
            2
        } else {
            4
        };
        assert_eq!(read(&mut host, &mut ram, 0x10, response_width), 0);
        assert!(contacts.present());
        assert!(!contacts.protected());
        assert_eq!(host.card().sector(1).unwrap(), [0x53; 512]);
        assert_eq!(host.card().revision(), revision);
        assert_eq!(
            ram.effects.last(),
            Some(&Effect::Irq(IRQ, protocol != Protocol::Rom))
        );
        let completed = host.checkpoint().unwrap();
        let fingerprint = host.fingerprint().unwrap();
        assert_eq!(host.save()["initialization_fault"], "reject_cmd8");
        write(&mut host, &mut ram, 0x30, 2, 0x8000);
        write(&mut host, &mut ram, 0x32, 2, 1);
        assert_eq!(ram.effects.last(), Some(&Effect::Irq(IRQ, false)));
        contacts.set(false, true);
        host.restore_checkpoint(&pending).unwrap();
        assert!(contacts.present());
        assert!(!contacts.protected());
        ram.now = deadline - 1;
        due(&mut host, &mut ram);
        assert_eq!(host.fingerprint().unwrap(), fingerprint);
        host.restore_checkpoint(&completed).unwrap();
        assert_eq!(host.fingerprint().unwrap(), fingerprint);
        assert_eq!(host.next_deadline(ram.now), None);
        if protocol == Protocol::Application {
            write(&mut host, &mut ram, 0x2e, 2, 0x100);
        } else {
            write(&mut host, &mut ram, 0x2f, 1, 1);
        }
        setup(&mut host, &mut ram, protocol);
        if protocol == Protocol::Rom {
            write(&mut host, &mut ram, 8, 4, 0x1aa);
            write(&mut host, &mut ram, 14, 2, 0x081a);
        } else {
            command(&mut host, &mut ram, 0x081a0000, 0x1aa);
        }
        due(&mut host, &mut ram);
        assert_eq!(read(&mut host, &mut ram, 0x32, 2), 1);
    }
}

#[test]
fn checkpoints_reject_different_initialization_policies_without_mutating_receiver() {
    let (faulty, _, _) = host(Protocol::Sdhc, InitializationFault::RejectCmd8);
    let (mut normal, _, _) = host(Protocol::Sdhc, InitializationFault::None);
    let saved = faulty.checkpoint().unwrap();
    let before = normal.fingerprint().unwrap();
    assert_ne!(before, faulty.fingerprint().unwrap());
    assert!(normal.validate_checkpoint(&saved).is_err());
    assert!(normal.restore_checkpoint(&saved).is_err());
    assert_eq!(normal.fingerprint().unwrap(), before);
    assert!(
        faulty
            .validate_checkpoint(&normal.checkpoint().unwrap())
            .is_err()
    );
}

#[test]
fn timeout_irq_waits_for_error_status_and_signal_enables() {
    let (mut host, _, mut ram) = host(Protocol::Sdhc, InitializationFault::RejectCmd8);
    setup(&mut host, &mut ram, Protocol::Sdhc);
    write(&mut host, &mut ram, 0x34, 2, 1);
    write(&mut host, &mut ram, 0x3a, 2, 0);
    command(&mut host, &mut ram, 0x081a0000, 0x1aa);
    due(&mut host, &mut ram);
    assert_eq!(read(&mut host, &mut ram, 0x32, 2), 1);
    assert_eq!(ram.effects.last(), Some(&Effect::Irq(IRQ, false)));
    write(&mut host, &mut ram, 0x36, 2, 0);
    write(&mut host, &mut ram, 0x3a, 2, 1);
    assert_eq!(ram.effects.last(), Some(&Effect::Irq(IRQ, false)));
    write(&mut host, &mut ram, 0x36, 2, 1);
    assert_eq!(ram.effects.last(), Some(&Effect::Irq(IRQ, true)));
    write(&mut host, &mut ram, 0x32, 2, 1);
    assert_eq!(ram.effects.last(), Some(&Effect::Irq(IRQ, false)));
}

#[test]
fn removed_media_refuses_commands_and_reinsertion_restores_normal_cmd8() {
    let (mut host, contacts, mut ram) = host(Protocol::Sdhc, InitializationFault::None);
    setup(&mut host, &mut ram, Protocol::Sdhc);
    contacts.set(false, false);
    assert_eq!(read(&mut host, &mut ram, 0x24, 4) & 0x10000, 0);
    assert!(host.readable_card(None).is_err());
    write(&mut host, &mut ram, 8, 4, 0x1aa);
    let mut cx = ram.cx();
    host.write(&mut cx, BASE + 12, 4, 0x081a0000);
    assert!(cx.refusal().is_some());
    assert!(!host.busy());
    assert_eq!(host.next_deadline(ram.now), None);
    contacts.set(true, false);
    command(&mut host, &mut ram, 0x081a0000, 0x1aa);
    due(&mut host, &mut ram);
    assert_eq!(read(&mut host, &mut ram, 0x10, 4), 0x1aa);
    assert_eq!(read(&mut host, &mut ram, 0x30, 2), 1);
    assert_eq!(read(&mut host, &mut ram, 0x32, 2), 0);
    assert_eq!(
        host.readable_card(None).unwrap().sector(1).unwrap(),
        [0x53; 512]
    );
}

#[test]
fn protected_media_refuses_dma_and_pio_writes_but_still_reads() {
    let (mut host, contacts, mut ram) = host(Protocol::Sdhc, InitializationFault::None);
    setup(&mut host, &mut ram, Protocol::Sdhc);
    enumerate(&mut host, &mut ram);
    write(&mut host, &mut ram, 0x30, 2, 0xffff);
    write(&mut host, &mut ram, 0, 4, 0x1000);
    write(&mut host, &mut ram, 4, 2, 512);
    write(&mut host, &mut ram, 6, 2, 1);
    write(&mut host, &mut ram, 8, 4, 1);
    contacts.set(true, true);
    let revision = host.card().revision();
    for word in [0x193a0027, 0x193a0026] {
        let mut cx = ram.cx();
        host.write(&mut cx, BASE + 12, 4, word);
        assert!(cx.refusal().is_some());
        assert!(!host.busy());
        assert_eq!(host.card().sector(1).unwrap(), [0x53; 512]);
        assert_eq!(host.card().revision(), revision);
    }
    contacts.set(true, false);
    ram.bytes[0x1000..0x1200].fill(0x72);
    command(&mut host, &mut ram, 0x193a0027, 1);
    due(&mut host, &mut ram);
    assert_eq!(host.card().sector(1).unwrap(), [0x72; 512]);
    assert_eq!(read(&mut host, &mut ram, 0x30, 2), 3);
    assert_eq!(ram.effects.last(), Some(&Effect::Irq(IRQ, true)));
    contacts.set(true, true);
    write(&mut host, &mut ram, 6, 2, 1);
    command(&mut host, &mut ram, 0x113a0010, 1);
    due(&mut host, &mut ram);
    for _ in 0..128 {
        assert_eq!(read(&mut host, &mut ram, 0x20, 4), 0x72727272);
    }
    due(&mut host, &mut ram);
    assert!(!host.busy());
    assert_eq!(host.card().sector(1).unwrap(), [0x72; 512]);
}

#[test]
fn uhs2_interface_attempt_finds_no_uhs2_card_and_falls_back_after_reset() {
    let (mut host, _, mut ram) = host(Protocol::Sdhc, InitializationFault::None);
    // X-T20 2.11 0xc61e40, 0xc65a40 and 0xc61ce4: VDD1 3.3 V, VDD2 and the
    // UHS-II interface with mode select 7, then the internal clock.
    write(&mut host, &mut ram, 0x28, 2, 0xbfc0);
    write(&mut host, &mut ram, 0x3e, 2, 0x100);
    write(&mut host, &mut ram, 0x3e, 2, 0x107);
    write(&mut host, &mut ram, 0x2c, 2, 1);
    ram.now += 2;
    assert_eq!(read(&mut host, &mut ram, 0x2c, 2), 3);
    write(&mut host, &mut ram, 0x2c, 2, 7);
    assert_eq!(read(&mut host, &mut ram, 0x24, 4) >> 31, 0);
    let mut cx = ram.cx();
    host.write(&mut cx, BASE + 12, 4, 0);
    assert!(cx.refusal().is_some());
    let mut cx = ram.cx();
    host.hardware_reset(&mut cx).unwrap();
    assert_eq!(read(&mut host, &mut ram, 0x3e, 2), 0);
    assert_eq!(read(&mut host, &mut ram, 0x28, 2), 0);
    setup(&mut host, &mut ram, Protocol::Sdhc);
    enumerate(&mut host, &mut ram);
}
