use super::*;
use crate::test_host::TestHost;
struct Model {
    gpio: Gpio,
    host: TestHost,
}
impl Model {
    fn gpio_pad_state(&self, port: u32, pin: u32) -> Result<PadState, String> {
        if port >= 18 || pin >= 16 {
            return Err("GPIO pad selector is out of range".into());
        }
        self.gpio.pads().pad_state(Pin {
            port: port as u8,
            pin: pin as u8,
        })
    }
}
fn model() -> Model {
    Model {
        gpio: Gpio::new(
            Options {
                input_levels: [0xffff; 16],
                output_levels: vec![],
            },
            Wiring { irqs: vec![] },
        )
        .unwrap(),
        host: TestHost::new(),
    }
}
fn write(m: &mut Model, port: u32, offset: u32, value: u32) {
    crate::test_host::write(
        &mut m.gpio,
        &mut m.host,
        BASE + u64::from(port) * 64 + u64::from(offset),
        value,
    );
}
fn read(m: &mut Model, port: u32, offset: u32) -> u64 {
    crate::test_host::read(
        &mut m.gpio,
        &mut m.host,
        BASE + u64::from(port) * 64 + u64::from(offset),
    )
}
#[test]
fn output_latch_is_not_a_driven_pad_until_all_fields_are_known() {
    let mut m = model();
    write(&mut m, 2, 0, 0x40004000);
    let latch_only = m.gpio_pad_state(2, 14).unwrap();
    assert_eq!(latch_only.latch, Some(1));
    assert_eq!(latch_only.direction, None);
    assert_eq!(latch_only.driven_for_mux(0).unwrap(), None);
    write(&mut m, 2, 4, 0x40004000);
    write(&mut m, 2, 8, 0x40004000);
    assert_eq!(
        m.gpio_pad_state(2, 14).unwrap().driven_for_mux(0).unwrap(),
        None
    );
    write(&mut m, 2, 0x10, 0x30000000);
    assert_eq!(
        m.gpio_pad_state(2, 14).unwrap().driven_for_mux(0).unwrap(),
        Some(1)
    );
    assert_eq!(
        m.gpio_pad_state(2, 14).unwrap().driven_for_mux(1).unwrap(),
        None
    );
    write(&mut m, 2, 4, 0x40000000);
    assert_eq!(
        m.gpio_pad_state(2, 14).unwrap().driven_for_mux(0).unwrap(),
        None
    );
    write(&mut m, 2, 4, 0x40004000);
    write(&mut m, 2, 8, 0x40000000);
    assert_eq!(
        m.gpio_pad_state(2, 14).unwrap().driven_for_mux(0).unwrap(),
        None
    );
}
#[test]
fn masked_register_reads_accumulate_interleaved_fields_and_ignore_unmasked_data() {
    let mut m = model();
    for offset in [0, 4, 8, 0xc, 0x10] {
        write(&mut m, 13, offset, 0x00010001);
        write(&mut m, 2, offset, 0x00080008);
        write(&mut m, 13, offset, 0x00040007);
        assert_eq!(read(&mut m, 13, offset), 5);
        assert_eq!(read(&mut m, 2, offset), 8);
        write(&mut m, 13, offset, 0x00010000);
        assert_eq!(read(&mut m, 13, offset), 4);
        write(&mut m, 13, offset, 0x0000ffff);
        assert_eq!(read(&mut m, 13, offset), 4);
    }
    assert_eq!(m.gpio.pads.state.lock().unwrap().known[13], 5);
    assert_eq!(
        m.gpio.pads.state.lock().unwrap().configuration_known[13][0],
        5
    );
    // A partially written two-bit mux never becomes a known function.
    assert_eq!(m.gpio_pad_state(13, 0).unwrap().mux, None);
    write(&mut m, 13, 0xc, 0x00020002);
    assert_eq!(m.gpio_pad_state(13, 0).unwrap().mux, Some(2));
}
#[test]
fn declared_mux_and_enabled_input_do_not_drive_even_when_the_latch_is_high() {
    let mut m = model();
    for (offset, value) in [
        (0, 0x00800080),
        (4, 0x00800000),
        (8, 0x00800080),
        (0xc, 0xc0004000),
    ] {
        write(&mut m, 0, offset, value);
    }
    let pad = m.gpio_pad_state(0, 7).unwrap();
    assert_eq!(
        pad,
        PadState {
            latch: Some(1),
            direction: Some(0),
            enabled: Some(1),
            mux: Some(1)
        }
    );
    assert_eq!(pad.driven_for_mux(1).unwrap(), None);
    assert!(pad.input_enabled_for_mux(1).unwrap());
    assert!(!pad.input_enabled_for_mux(0).unwrap());
    let before = m.gpio.fingerprint().unwrap();
    for _ in 0..4 {
        assert_eq!(m.gpio_pad_state(0, 7).unwrap(), pad);
    }
    assert_eq!(m.gpio.fingerprint().unwrap(), before);
    let saved = m.gpio.checkpoint().unwrap();
    let mut cloned = model();
    cloned.gpio.restore_checkpoint(&saved).unwrap();
    assert_eq!(cloned.gpio_pad_state(0, 7).unwrap(), pad);
    assert!(m.gpio_pad_state(18, 0).is_err());
    assert!(m.gpio_pad_state(0, 16).is_err());
    assert!(pad.driven_for_mux(4).is_err());
    assert!(pad.input_enabled_for_mux(4).is_err());
}

#[test]
fn connector_interrupt_sense_uses_only_selected_fields_and_physical_levels() {
    // Connector interrupt sense uses selected fields and physical input levels.
    for previous in [false, true] {
        for high in [false, true] {
            for (sense, active) in [
                (1, !previous && high),
                (2, previous && !high),
                (3, previous != high),
            ] {
                assert_eq!(
                    interrupt_signal(true, sense, previous, high).unwrap(),
                    (false, active)
                );
            }
            assert_eq!(
                interrupt_signal(false, 1, previous, high).unwrap(),
                (true, high)
            );
            assert_eq!(
                interrupt_signal(false, 2, previous, high).unwrap(),
                (true, !high)
            );
            for sense in [0, 3, 4, u32::MAX] {
                assert!(interrupt_signal(false, sense, previous, high).is_err());
            }
            for sense in [0, 4, u32::MAX] {
                assert!(interrupt_signal(true, sense, previous, high).is_err());
            }
        }
    }
}
