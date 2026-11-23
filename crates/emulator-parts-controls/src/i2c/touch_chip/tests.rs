use super::*;

const STARTUP: [(u16, u8); 30] = [
    (0x1c02, 0x00), (0x1c03, 0x1a), (0x1c07, 0x00), (0x1c09, 0xc0), (0x1c0b, 0x01),
    (0x1c1b, 0x05), (0x1c1c, 0x14), (0x1c1d, 0x00), (0x1c1e, 0x03), (0x1c1f, 0x14),
    (0x1c23, 0x00), (0x1c24, 0x1e), (0x1c29, 0x1e), (0x1c2e, 0x9c), (0x1c2f, 0xff),
    (0x1c32, 0xce), (0x1c33, 0xff), (0x1c38, 0x1c), (0x1c3c, 0x11), (0x1c3d, 0x01),
    (0x1c3e, 0x06), (0x1c3f, 0x06), (0x1c99, 0x23), (0x1cc0, 0x32), (0x1cc1, 0x00),
    (0x1cc2, 0x64), (0x1cc3, 0x00), (0x1cc4, 0x50), (0x1cc5, 0x00), (0x1cd0, 0xa0),
];

fn chip() -> TouchChip {
    let mut writable: Vec<u16> = STARTUP.iter().map(|(r, _)| *r).collect();
    writable.extend([0x1c00, 0x1c08, 0x1c09]);
    TouchChip::new(Profile {
        address: 0x55,
        report_register: 0x0000,
        heartbeat_register: 0x1c06,
        mode_register: 0x1c00,
        restart_register: 0x1c09,
        restart_value: 0xc1,
        writable_registers: writable,
        contact_ids: [1, 2],
        input_ready_irq: 153,
        ticks_per_second: 1000,
    })
    .unwrap()
}

fn write_register(c: &mut TouchChip, register: u16, data: &[u8]) -> Result<(), String> {
    assert!(c.address(0xaa));
    let result = register.to_be_bytes().iter().chain(data).try_for_each(|b| c.write(*b));
    c.stop();
    result
}

fn read_register(c: &mut TouchChip, register: u16, len: usize) -> Result<Vec<u8>, String> {
    write_register(c, register, &[])?;
    assert!(c.address(0xab));
    let bytes = (0..len).map(|_| c.read()).collect();
    c.stop();
    bytes
}

fn start(c: &mut TouchChip) {
    for (register, value) in STARTUP {
        write_register(c, register, &[value]).unwrap();
    }
    write_register(c, 0x1c00, &[2]).unwrap();
}

#[test]
fn startup_writes_are_accepted_and_the_initial_ready_edge_comes_once() {
    let mut c = chip();
    assert!(c.advance(0).unwrap());
    start(&mut c);
    assert!(!c.advance(1).unwrap());
    assert_eq!(c.register(0x1c2e), Some(0x9c));
    assert_eq!(c.register(0x1c00), Some(2));
    assert!(!c.address(0xa0), "other addresses are not acknowledged");
    assert_eq!(c.profile().input_ready_irq, 153);
}

#[test]
fn tap_produces_slot_one_report_bytes_and_ready_edges() {
    let mut c = chip();
    c.advance(0).unwrap();
    start(&mut c);
    c.press(10, 0x123, 0x456).unwrap();
    assert!(c.advance(10).unwrap());
    assert_eq!(
        read_register(&mut c, 0, 11).unwrap(),
        [1, 2, 1, 0x23, 0x56, 0x41, 128, 0, 0, 0, 0]
    );
    c.release(40, 0x123, 0x456).unwrap();
    let mut frames = Vec::new();
    for now in [40, 50, 60] {
        if c.advance(now).unwrap() {
            frames.push(read_register(&mut c, 0, 11).unwrap());
        }
    }
    assert_eq!(frames.first().map(|f| (f[0], f[1])), Some((1, 1)));
    assert_eq!(frames.last().map(|f| (f[0], f[1], f[2])), Some((0, 0, 0)));
    assert_eq!(c.next_deadline(70), None);
}

#[test]
fn move_changes_coordinates_without_changing_the_slot() {
    let mut c = chip();
    c.advance(0).unwrap();
    start(&mut c);
    c.press(10, 100, 200).unwrap();
    assert!(c.advance(10).unwrap());
    read_register(&mut c, 0, 11).unwrap();
    c.move_to(30, 4095, 17).unwrap();
    assert!(c.advance(30).unwrap());
    let frame = read_register(&mut c, 0, 11).unwrap();
    assert_eq!((frame[1], frame[2], frame[3], frame[4], frame[5]), (3, 1, 0xff, 17, 0x0f));
}

#[test]
fn reporting_waits_for_the_mode_register() {
    let mut c = chip();
    c.advance(0).unwrap();
    c.press(5, 1, 2).unwrap();
    assert!(!c.advance(5).unwrap());
    write_register(&mut c, 0x1c00, &[2]).unwrap();
    assert!(c.advance(6).unwrap());
    write_register(&mut c, 0x1c00, &[0]).unwrap();
    assert!(!c.enabled);
}

#[test]
fn heartbeat_changes_between_reads() {
    let mut c = chip();
    let first = read_register(&mut c, 0x1c06, 1).unwrap();
    let second = read_register(&mut c, 0x1c06, 1).unwrap();
    assert_ne!(first, second);
}

#[test]
fn unknown_registers_refuse_without_effect() {
    let mut c = chip();
    start(&mut c);
    let before = serde_json::to_value(&c).unwrap();
    assert!(write_register(&mut c, 0x1c04, &[1]).is_err());
    assert!(write_register(&mut c, 0x0000, &[1]).is_err());
    assert!(write_register(&mut c, 0x1c06, &[1]).is_err());
    assert!(read_register(&mut c, 0x1c02, 1).is_err());
    assert!(read_register(&mut c, 0x0000, 12).is_err());
    assert!(c.write(0).is_err(), "no transaction is open");
    assert!(c.read().is_err());
    let mut after = serde_json::to_value(&c).unwrap();
    // Failed transactions leave the pointer and phase where the attempt put them.
    after["pointer"] = before["pointer"].clone();
    after["phase"] = before["phase"].clone();
    assert_eq!(before["registers"], after["registers"]);
    assert_eq!(before["heartbeat"], after["heartbeat"]);
}

#[test]
fn restart_register_answers_with_a_new_ready_edge() {
    let mut c = chip();
    assert!(c.advance(0).unwrap());
    assert!(!c.advance(1).unwrap());
    write_register(&mut c, 0x1c09, &[0xc0]).unwrap();
    assert!(!c.advance(2).unwrap());
    write_register(&mut c, 0x1c09, &[0xc1]).unwrap();
    assert!(c.advance(2).unwrap());
    assert!(!c.advance(3).unwrap());
}
