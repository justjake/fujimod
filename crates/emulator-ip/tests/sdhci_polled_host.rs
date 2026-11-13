//! A polled SDHCI host: no signal enables, status enables only. The
//! X-Processor 4 boot ROM writes +0x34 and never +0x38; the X-T3 fast SD
//! card read sends CMD6 through the data port and resets single lines.
use fujimod_emulator_devices::Device;
use fujimod_emulator_devices::parts::card_contacts::Contacts;
use fujimod_emulator_devices::parts::sd_card::faults::{DataDirection, DataFault};
use fujimod_emulator_devices::parts::sd_card::{Card, Options as CardOptions};
use fujimod_emulator_devices::test_host::ram::Ram;
use fujimod_emulator_ip::sdhci::host::{InitializationFault, Options, Protocol, SdHost, SlotName};

const BASE: u64 = 0x12000000;

fn polled_host(high_speed: bool) -> (SdHost, Ram) {
    let mut card = Card::new(CardOptions {
        sectors: 0x400000,
        max_dirty_sectors: 64,
    })
    .unwrap();
    for sector in 0..16u8 {
        card.write_bytes(u64::from(sector) * 512, &[sector; 512]).unwrap();
    }
    let host = SdHost::new_polled(
        Options {
            base: BASE,
            protocol: Protocol::Sdhc,
            transfer_ticks: 5,
            clock_ticks: 2,
            rca: 7,
            ram_bytes: 0x10000,
            name: SlotName::MemoryCard,
            initialization_fault: InitializationFault::None,
            high_speed,
            ram_banks: vec![],
        },
        card,
        Contacts::new(true, false),
    )
    .unwrap();
    (host, Ram::new(0x10000))
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

fn due(host: &mut SdHost, ram: &mut Ram) {
    ram.now = host.next_deadline(ram.now).unwrap();
    let mut cx = ram.cx();
    host.advance(&mut cx);
    assert_eq!(cx.refusal(), None);
}

fn command(host: &mut SdHost, ram: &mut Ram, word: u32, argument: u32, blocks: u32) {
    write(host, ram, 0x30, 4, 0xffffffff);
    write(host, ram, 4, 4, u64::from(blocks));
    write(host, ram, 8, 4, u64::from(argument));
    write(host, ram, 12, 4, u64::from(word));
}

/// The boot ROM's host state after SD card initialization: 4-bit bus, power,
/// clock enabled, every status enabled and no signal enabled.
fn transfer_state(host: &mut SdHost, ram: &mut Ram) {
    write(host, ram, 0x2c, 2, 0x105);
    ram.now += 2;
    assert_eq!(read(host, ram, 0x2c, 2) & 7, 7);
    write(host, ram, 0x28, 4, 0xf02);
    write(host, ram, 0x34, 4, 0xffffffff);
    for (word, argument) in [
        (0, 0),
        (0x081a0000, 0x1aa),
        (0x371a0000, 0),
        (0x29020000, 0x40100000),
        (0x02090000, 0),
        (0x031a0000, 0),
        (0x071b0000, 7 << 16),
    ] {
        command(host, ram, word, argument, 0);
        due(host, ram);
        assert_eq!(read(host, ram, 0x30, 2) & 0x8001, 1);
    }
    assert_eq!(read(host, ram, 0x38, 4), 0);
}

fn switch(host: &mut SdHost, ram: &mut Ram, argument: u32) -> [u8; 64] {
    command(host, ram, 0x063a0010, argument, 1 << 16 | 64);
    due(host, ram);
    assert_eq!(read(host, ram, 0x30, 2), 1);
    due(host, ram);
    assert_eq!(read(host, ram, 0x30, 2) & 0x20, 0x20);
    write(host, ram, 0x30, 4, 0x20);
    let mut status = [0; 64];
    for word in status.chunks_mut(4) {
        word.copy_from_slice(&(read(host, ram, 0x20, 4) as u32).to_le_bytes());
    }
    due(host, ram);
    assert_eq!(read(host, ram, 0x30, 2) & 2, 2);
    assert_eq!(read(host, ram, 0x24, 4) & 3, 0);
    status
}

#[test]
fn polled_cmd6_reads_its_switch_status_through_the_data_port() {
    let (mut host, mut ram) = polled_host(true);
    transfer_state(&mut host, &mut ram);
    let selected = switch(&mut host, &mut ram, 0x80fffff1);
    assert_eq!(selected[16] & 15, 1);
    let restored = switch(&mut host, &mut ram, 0x80fffff0);
    assert_eq!(restored[16] & 15, 0);

    let (mut host, mut ram) = polled_host(false);
    transfer_state(&mut host, &mut ram);
    let refused = switch(&mut host, &mut ram, 0x80fffff1);
    assert_eq!(refused[16] & 15, 15);
}

#[test]
fn polled_multi_block_reads_use_status_enables_without_signal_enables() {
    let (mut host, mut ram) = polled_host(true);
    transfer_state(&mut host, &mut ram);
    // SDMA across a 4 KiB boundary resumes on the polled boundary status.
    write(&mut host, &mut ram, 0, 4, 0x800);
    command(&mut host, &mut ram, 0x123a0037, 2, 8 << 16 | 0x0200);
    due(&mut host, &mut ram);
    assert_eq!(read(&mut host, &mut ram, 0x30, 2) & 8, 8);
    write(&mut host, &mut ram, 0x30, 4, 8);
    let resume = read(&mut host, &mut ram, 0, 4);
    write(&mut host, &mut ram, 0, 4, resume);
    due(&mut host, &mut ram);
    assert_eq!(read(&mut host, &mut ram, 0x30, 2) & 2, 2);
    for sector in 0..8 {
        assert_eq!(ram.bytes[0x800 + sector * 512..0xa00 + sector * 512], [sector as u8 + 2; 512]);
    }
    // PIO multi-block read.
    command(&mut host, &mut ram, 0x123a0036, 5, 2 << 16 | 0x7200);
    for sector in [5u8, 6] {
        while read(&mut host, &mut ram, 0x30, 2) & 0x20 == 0 {
            due(&mut host, &mut ram);
        }
        write(&mut host, &mut ram, 0x30, 4, 0x20);
        for _ in 0..128 {
            assert_eq!(read(&mut host, &mut ram, 0x20, 4) as u32, u32::from_le_bytes([sector; 4]));
        }
    }
    due(&mut host, &mut ram);
    assert_eq!(read(&mut host, &mut ram, 0x30, 2) & 2, 2);
}

#[test]
fn line_resets_end_a_failed_transfer_and_keep_the_host_configured() {
    let (mut host, mut ram) = polled_host(true);
    transfer_state(&mut host, &mut ram);
    host.inject_data_fault(DataFault {
        direction: DataDirection::Read,
        sector: Some(4),
        after_sectors: 0,
        occurrences: 1,
    })
    .unwrap();
    write(&mut host, &mut ram, 0, 4, 0x1000);
    command(&mut host, &mut ram, 0x123a0037, 2, 8 << 16 | 0x7200);
    due(&mut host, &mut ram);
    assert_eq!(read(&mut host, &mut ram, 0x30, 2) & 0x8000, 0x8000);
    assert_eq!(read(&mut host, &mut ram, 0x32, 2), 0x20);
    write(&mut host, &mut ram, 0x30, 4, 0xffffffff);
    write(&mut host, &mut ram, 0x2f, 1, 6);
    assert_eq!(read(&mut host, &mut ram, 0x2f, 1), 0);
    assert_eq!(read(&mut host, &mut ram, 0x24, 4) & 3, 0);
    // The clock, bus and status enables survive line resets.
    assert_eq!(read(&mut host, &mut ram, 0x2c, 2) & 7, 7);
    assert_eq!(read(&mut host, &mut ram, 0x28, 4), 0xf02);
    command(&mut host, &mut ram, 0x0cdb0000, 0, 0);
    due(&mut host, &mut ram);
    assert_eq!(read(&mut host, &mut ram, 0x30, 2) & 0x8001, 1);
    write(&mut host, &mut ram, 0, 4, 0x1000);
    command(&mut host, &mut ram, 0x123a0037, 2, 8 << 16 | 0x7200);
    due(&mut host, &mut ram);
    assert_eq!(read(&mut host, &mut ram, 0x30, 2) & 0x8002, 2);
    assert_eq!(ram.bytes[0x1000 + 2 * 512..0x1000 + 3 * 512], [4; 512]);
    // Software Reset For All still clears the host; other bits refuse.
    write(&mut host, &mut ram, 0x2f, 1, 1);
    assert_eq!(read(&mut host, &mut ram, 0x28, 4), 0);
    let mut cx = ram.cx();
    host.write(&mut cx, BASE + 0x2f, 1, 8);
    assert!(cx.refusal().is_some());
}
