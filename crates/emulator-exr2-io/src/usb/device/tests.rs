use super::*;
use fujimod_emulator_devices::{bus::InterruptLine, test_host::ram::{Effect, Ram}};
const CORE: u64 = super::super::CORE as u64;
const PHY: u64 = super::super::PHY as u64;
fn model(endpoints: bool) -> (UsbController, Level) {
    let cable = Level::new(false);
    let options = endpoints.then_some(usb_endpoint::EndpointOptions {
        transfer_ticks: 4,
        packet_bytes: 64,
        dma_limit: 1024,
        out_endpoints: vec![(0, 0), (1, 1)],
        in_endpoints: vec![(0x80, 0), (0x82, 1), (0x83, 2)],
    });
    let wiring = endpoints.then_some(usb_endpoint::Wiring {
        irq: InterruptLine::Core { core: 0, line: 84 },
        dma_irq: InterruptLine::Core { core: 0, line: 85 },
        ram_bytes: 0x10000,
    });
    (
        UsbController::new(
            Options { endpoints: options },
            Wiring {
                cable: cable.clone(),
                endpoints: wiring,
            },
        )
        .unwrap(),
        cable,
    )
}
fn write(m: &mut UsbController, h: &mut Ram, a: u64, s: u8, v: u64) {
    let mut cx = h.cx();
    m.write(&mut cx, a, s, v);
    assert_eq!(cx.refusal(), None, "{a:x}");
}
fn read(m: &mut UsbController, h: &mut Ram, a: u64, s: u8) -> u64 {
    let mut cx = h.cx();
    let value = m.read(&mut cx, a, s);
    assert_eq!(cx.refusal(), None, "{a:x}");
    value
}
#[test]
fn startup_uses_cable_level_without_inspection_effects_or_invented_interrupts() {
    let (mut model, cable) = model(false);
    let mut host = Ram::new(0x10000);
    let before = model.save();
    assert_eq!(before, model.save());
    let fingerprint = model.fingerprint().unwrap();
    assert_eq!(fingerprint, model.fingerprint().unwrap());
    cable.drive(true);
    assert_ne!(fingerprint, model.fingerprint().unwrap());
    assert!(model.save()["cable"].as_bool().unwrap());
    assert_eq!(model.save()["startup"], before["startup"]);
    assert_eq!(read(&mut model, &mut host, PHY + 0x104, 4), 1 << 31);
    for (a, s, v) in [
        (PHY + 0x100, 4, 1),
        (PHY + 0x104, 4, 0x1ecbc7),
        (CORE + 0x90, 2, 1),
        (CORE + 0xac, 2, 0x3fff),
        (CORE + 0x94, 2, 0xc0),
        (CORE + 0x98, 2, 1),
        (CORE + 8, 2, 2),
        (PHY + 0x108, 4, 0x10000),
        (PHY + 0x108, 4, 0),
    ] {
        write(&mut model, &mut host, a, s, v);
    }
    assert_eq!(read(&mut model, &mut host, PHY + 0x104, 4), 0x801ecbc7);
    assert_eq!(model.save()["startup"]["reset_pulses"], 1);
    cable.drive(false);
    assert_eq!(read(&mut model, &mut host, PHY + 0x104, 4), 0x1ecbc7);
    assert!(host.effects.is_empty());
    assert_eq!(model.next_deadline(host.now), None);
}
#[test]
fn endpoint_controls_take_precedence_and_dma_keeps_exact_deadline() {
    let (mut model, _) = model(true);
    let mut host = Ram::new(0x10000);
    // These are endpoint mask controls, even before PHY startup configuration.
    for o in [0x40, 0x44, 0x48] {
        write(&mut model, &mut host, CORE + o, 2, 0xffff);
    }
    host.bytes[0x1000..0x1006].copy_from_slice(&[1, 2, 3, 4, 5, 6]);
    for (a, s, v) in [
        (PHY + 0x804, 4, 1),
        (PHY + 0x820, 4, 0x90401000),
        (PHY + 0x824, 4, 0x1000),
        (PHY + 0x828, 4, 8),
        (CORE + 0x70, 2, 2),
        (CORE + 0x1124, 4, 6),
        (CORE + 0x110c, 4, 1),
    ] {
        write(&mut model, &mut host, a, s, v);
    }
    assert_eq!(model.next_deadline(0), Some(4));
    let before = model.save();
    assert_eq!(before, model.save());
    let fingerprint = model.fingerprint().unwrap();
    assert_eq!(fingerprint, model.fingerprint().unwrap());
    assert_eq!(before, model.save());
    assert_eq!(before["endpoints"]["active"]["exact"], 6);
    assert_eq!(before["endpoints"]["scratch"][0], 1);
    host.now = 3;
    model.advance(&mut host.cx());
    assert_eq!(model.save(), before);
    assert_eq!(model.fingerprint().unwrap(), fingerprint);
    assert!(
        model
            .endpoint_mut()
            .unwrap()
            .host_in(&mut host.cx(), 0x82, 64)
            .is_err()
    );
    host.now = 4;
    model.advance(&mut host.cx());
    assert_eq!(model.next_deadline(4), None);
    assert_ne!(model.fingerprint().unwrap(), fingerprint);
    assert_eq!(
        model
            .endpoint_mut()
            .unwrap()
            .host_in(&mut host.cx(), 0x82, 64)
            .unwrap(),
        [1, 2, 3, 4, 5, 6]
    );
    assert!(host.effects.contains(&Effect::Irq(85, true)));
    assert_eq!(model.save()["startup"]["configured"], false);
}
#[test]
fn unproven_startup_command_and_width_refuse_without_mutation() {
    let (mut model, _) = model(false);
    let mut host = Ram::new(0x10000);
    for (a, s, v) in [
        (PHY + 0x104, 4, 0x1ecbc7),
        (PHY + 0x100, 4, 0),
        (PHY + 0x108, 4, 1),
        (CORE + 0x90, 4, 1),
    ] {
        let before = model.save();
        let mut cx = host.cx();
        model.write(&mut cx, a, s, v);
        assert!(cx.refusal().is_some());
        assert_eq!(model.save(), before);
    }
}
