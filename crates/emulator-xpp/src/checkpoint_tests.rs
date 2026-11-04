//! Exact checkpoints of the X-Processor Pro device models: each restores to
//! the same fingerprint, resumes its pending work identically after an
//! envelope round trip, and refuses a differently wired device.
use crate::*;
use fujimod_emulator_devices::Device;
use fujimod_emulator_devices::test_host::{
    TestHost, advance, advance_from_checkpoint, assert_exact_restore, read, write,
};
use fujimod_emulator_ip::socionext::gpio::Pins;

fn poke(d: &mut dyn Device, address: u64, value: u32) {
    let mut host = TestHost::new();
    write(d, &mut host, address, value);
}

#[test]
fn adc_resumes_a_conversion_and_keeps_external_inputs() {
    let samples: Vec<u16> = (0..12).map(|n| n * 10).collect();
    let mut adc = adc::Converter::new(0x80000, &samples, 16).unwrap();
    adc.set_input(4, 485).unwrap();
    let mut host = TestHost::new();
    write(&mut adc, &mut host, 0x80000, 1);
    advance_from_checkpoint(&mut adc, &mut host, 16);
    assert_eq!(read(&mut adc, &mut host, 0x80000 + 0x10 + 16), 485);
    let saved = assert_exact_restore(&mut adc, |d| poke(d, 0x80004, 7));
    let mut other = adc::Converter::new(0x80000, &samples, 16).unwrap();
    other.restore_checkpoint(&saved).unwrap();
    assert_eq!(read(&mut other, &mut host, 0x80000 + 0x10 + 16), 485);
    let mut invalid = saved.clone();
    invalid = invalid.with_children(vec![saved]);
    assert!(other.validate_checkpoint(&invalid).is_err());
}

#[test]
fn dram_and_power_resume_their_startup_settling() {
    let mut dram = dram::Controller::new(0x80000, 2)
        .unwrap()
        .with_startup(10)
        .unwrap();
    let mut host = TestHost::new();
    write(&mut dram, &mut host, 0x80000, 1);
    advance_from_checkpoint(&mut dram, &mut host, 10);
    let saved = assert_exact_restore(&mut dram, |d| poke(d, 0x80000 + 0x9c, 0xc30));
    // Startup words exist only on a controller that performs a startup.
    assert!(
        dram::Controller::new(0x80000, 2)
            .unwrap()
            .validate_checkpoint(&saved)
            .is_err()
    );

    let mut power = power::Control::new(0x90000)
        .unwrap()
        .with_startup(20)
        .unwrap();
    let mut host = TestHost::new();
    write(&mut power, &mut host, 0x90000 + 0x0c, 1);
    advance_from_checkpoint(&mut power, &mut host, 20);
    let saved = assert_exact_restore(&mut power, |d| poke(d, 0x90000 + 0x08, 5));
    let mut cold = power::Control::new(0x90000).unwrap();
    cold.restore_checkpoint(&saved).unwrap();
}

#[test]
fn frame_sources_resume_their_periodic_work() {
    let mut sync = sync::Generator::new(0x80000, 0x81000, vec![(0, 100), (2, 101)], 10).unwrap();
    let mut host = TestHost::new();
    write(&mut sync, &mut host, 0x81000, 0x10001);
    advance_from_checkpoint(&mut sync, &mut host, 10);
    let saved = assert_exact_restore(&mut sync, |d| poke(d, 0x80008, 3));
    let other = sync::Generator::new(0x80000, 0x81000, vec![(0, 100)], 7).unwrap();
    assert!(other.validate_checkpoint(&saved).is_err());

    let mut clock = display::FrameClock::new(vec![112, 113], 8).unwrap();
    let mut host = TestHost::new();
    advance_from_checkpoint(&mut clock, &mut host, 8);
    let saved = clock.checkpoint().unwrap();
    assert!(
        display::FrameClock::new(vec![112], 5)
            .unwrap()
            .validate_checkpoint(&saved)
            .is_err()
    );

    let mut statistics = statistics::Engine::new(0x80000, 100, 10).unwrap();
    let mut host = TestHost::new();
    write(&mut statistics, &mut host, 0x80000 + 0x04, 1);
    write(&mut statistics, &mut host, 0x80000 + 0x1e8, 1 << 28);
    advance_from_checkpoint(&mut statistics, &mut host, 10);
    let saved = assert_exact_restore(&mut statistics, |d| poke(d, 0x80008, 1));
    assert!(
        statistics::Engine::new(0x80000, 100, 3)
            .unwrap()
            .validate_checkpoint(&saved)
            .is_err()
    );

    let mut face = face::Engine::new(0x80000, 100, 10).unwrap();
    let mut host = TestHost::new();
    write(&mut face, &mut host, 0x80004, 1);
    advance_from_checkpoint(&mut face, &mut host, 10);
    assert_exact_restore(&mut face, |d| poke(d, 0x80010, 9));
}

#[test]
fn jpeg_engines_restore_tables_and_status() {
    let mut encoder = jpeg::Encoder::new(0x80000, 100, 10).unwrap();
    assert_exact_restore(&mut encoder, |d| poke(d, 0x80000 + 0x8c, 0x11223344));
    let mut decoder = jpeg::Decoder::new(0x80000, 100, 10).unwrap();
    assert_exact_restore(&mut decoder, |d| poke(d, 0x80000 + 0x118, 1));
}

#[test]
fn key_lines_and_gates_restore_shared_line_levels_in_place() {
    let pins = Pins::new([0b11, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    let mut gates = resets::Gates::new(0xfff0_a000).unwrap();
    let lines = gates.external_lines();
    let mut keys =
        keys::KeyLines::new(&[(0, 0, 0), (1, 0, 1)], 10, pins.clone(), lines.clone()).unwrap();
    let mut host = TestHost::new();
    advance_from_checkpoint(&mut keys, &mut host, 10);
    let saved = assert_exact_restore(&mut keys, |d| {
        let mut host = TestHost::new();
        advance(d, &mut host, 40);
    });
    let elsewhere = resets::Gates::new(0xfff0_b000).unwrap().external_lines();
    assert!(
        keys::KeyLines::new(&[(0, 0, 0)], 7, pins, elsewhere)
            .unwrap()
            .validate_checkpoint(&saved)
            .is_err()
    );
    let saved = assert_exact_restore(&mut gates, |d| {
        let mut host = TestHost::new();
        write(d, &mut host, 0xfff0_a000 + 0x1c, 1 << 18);
        write(d, &mut host, 0xfff0_a000 + 0x1c, 0);
        write(d, &mut host, 0xfff0_a000 + 0x08, 1);
    });
    assert!(
        resets::Gates::new(0xfff0_a000)
            .unwrap()
            .restore_checkpoint(&saved)
            .is_ok()
    );
}

#[test]
fn register_blocks_and_imaging_resume_jobs_and_readouts() {
    let mut block = blocks::Block::new(0x80000, 0x1000, vec![(0, 1)], None)
        .unwrap()
        .with_job(blocks::Job {
            start: (4, 1),
            status: Some((8, 1)),
            stop: None,
            irq: 100,
            ticks: 10,
        })
        .unwrap();
    let mut host = TestHost::new();
    write(&mut block, &mut host, 0x80004, 1);
    advance_from_checkpoint(&mut block, &mut host, 10);
    let saved = assert_exact_restore(&mut block, |d| poke(d, 0x80010, 5));
    let mut plain = blocks::Block::new(0x80000, 0x1000, vec![], None).unwrap();
    assert!(plain.validate_checkpoint(&saved).is_err());
    assert!(plain.restore_checkpoint(&saved).is_err());

    let mut control = imaging::Control::new(0x80000)
        .unwrap()
        .with_readout(imaging::Readout {
            offset: 0x80,
            irq: 100,
            ticks: 10,
            frame_period: None,
        })
        .unwrap();
    let mut host = TestHost::new();
    write(&mut control, &mut host, 0x80000 + 0x80, 1);
    advance_from_checkpoint(&mut control, &mut host, 10);
    let saved = assert_exact_restore(&mut control, |d| poke(d, 0x80000 + 0x300, 7));
    assert!(
        imaging::Control::new(0x80000)
            .unwrap()
            .validate_checkpoint(&saved)
            .is_err()
    );
}

#[test]
fn display_output_resumes_its_handshake() {
    let mut output = display::Output::new(0x80000, 0x1000, &[], &[], None)
        .unwrap()
        .with_handshake(5)
        .unwrap();
    let mut host = TestHost::new();
    write(&mut output, &mut host, 0x80000, 1 << 16);
    write(&mut output, &mut host, 0x80000, 0);
    advance_from_checkpoint(&mut output, &mut host, 5);
    let saved = assert_exact_restore(&mut output, |d| poke(d, 0x80010, 3));
    let without = display::Output::new(0x80000, 0x1000, &[], &[], None).unwrap();
    assert!(without.validate_checkpoint(&saved).is_err());
}

#[test]
fn serial_ports_restore_peers_and_refuse_other_peers() {
    let base = 0xfff2a000;
    let mut port = serial::DiscoveryPort::with_peer(base, vec![0; serial::PEER_REGISTERS])
        .unwrap()
        .with_clock(serial::Clock {
            first: 0x41,
            hold: 0x45,
            seconds: 100,
            ticks_per_second: 10,
        })
        .unwrap();
    let saved = assert_exact_restore(&mut port, |d| {
        let mut host = TestHost::new();
        write(d, &mut host, base + 0x60, 0xc500);
    });
    assert!(
        serial::DiscoveryPort::new(base)
            .unwrap()
            .validate_checkpoint(&saved)
            .is_err()
    );
    let mut frames = serial::DiscoveryPort::with_frame_peer(base, [(0, 0xfd50)].into()).unwrap();
    assert_exact_restore(&mut frames, |d| {
        let mut host = TestHost::new();
        write(d, &mut host, base + 8, 1);
        write(d, &mut host, base + 0x60, 0x11);
    });
    let mut configuration = serial::ConfigurationPort::new(base).unwrap();
    assert_exact_restore(&mut configuration, |d| poke(d, base + 0x14, 9));
}

#[test]
fn i2c_controller_restores_with_its_parts() {
    use fujimod_emulator_ip::dw_i2c::{Controller, Options};
    let options = Options {
        base: 0xfff20000,
        irq: 68,
        fifo_depth: 8,
        clock_hz: 1000,
        tick_hz: 1000,
    };
    let part = || i2c::Part {
        address: 0x36,
        select_mask: 0xff,
        width: 2,
        little_endian: true,
        registers: [(0x10, 0x1234)].into(),
    };
    let mut bus = Controller::new(options.clone(), i2c::Parts::new(vec![part()]).unwrap()).unwrap();
    let mut host = TestHost::new();
    for (offset, value) in [
        (0, 2),
        (4, 0x36),
        (0x14, 10),
        (0x18, 10),
        (0x6c, 1),
        (0x10, 0x10),
    ] {
        write(&mut bus, &mut host, 0xfff20000 + offset, value);
    }
    let due = bus.next_deadline(0).unwrap();
    advance_from_checkpoint(&mut bus, &mut host, due);
    let saved = assert_exact_restore(&mut bus, |d| poke(d, 0xfff20000 + 0x38, 3));
    let other = Controller::new(options, i2c::Parts::new(vec![]).unwrap()).unwrap();
    assert!(other.validate_checkpoint(&saved).is_err());
}
