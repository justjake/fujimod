use super::*;
use fujimod_emulator_devices::parts::pins::Pin;
use fujimod_emulator_devices::{
    Record,
    bus::{Cx, Host},
    exr2::gpio::{Gpio, InitialOutput},
    test_host::ram::Ram,
};
use fujimod_emulator_exr2_plan::serial::{FixedLowByteInput, PinLevel};
fn model(connected: bool) -> (SerialState, Pads) {
    let mut levels = [0; 16];
    levels[11] = 0;
    let gpio = Gpio::new(
        fujimod_emulator_devices::exr2::gpio::Options {
            input_levels: levels,
            output_levels: vec![InitialOutput {
                pin: Pin { port: 11, pin: 2 },
                high: false,
            }],
        },
        fujimod_emulator_devices::exr2::gpio::Wiring { irqs: vec![] },
    )
    .unwrap();
    let pads = gpio.pads();
    let input = FixedLowByteInput {
        required: [0xc0, 0x30, 0],
        selection: PinLevel {
            pin: Pin { port: 11, pin: 2 },
            high: false,
        },
        presence: PinLevel {
            pin: Pin { port: 11, pin: 2 },
            high: false,
        },
    };
    let model = SerialState::new(
        Options {
            transfer_ticks: 4,
            max_program_bytes: 0x10000,
            spi0: connected.then_some(input),
        },
        Wiring { gpio: pads.clone() },
    )
    .unwrap();
    (model, pads)
}
fn write(model: &mut SerialState, host: &mut Ram, address: u64, value: u64) -> Result<(), String> {
    let mut cx = host.cx();
    model.write(&mut cx, block(address), address, 4, value);
    cx.take_refusal().map_or(Ok(()), Err)
}
fn read(model: &mut SerialState, host: &mut Ram, address: u64) -> Result<u64, String> {
    let mut cx = host.cx();
    let value = model.read(&mut cx, block(address), address, 4);
    cx.take_refusal().map_or(Ok(value), Err)
}
fn block(address: u64) -> Block {
    if (SPI..SPI + 0x20).contains(&address) {
        Block::Spi0
    } else if (UART..UART + 0x10).contains(&address) {
        Block::Uart
    } else if (DMA..DMA + 0x40).contains(&address) {
        Block::Dma
    } else {
        Block::Routing
    }
}
fn configure_spi(model: &mut SerialState, host: &mut Ram) {
    for (offset, value) in [(0, 0xc0), (4, 0x30), (8, 0)] {
        write(model, host, SPI + offset, value).unwrap();
    }
}
#[test]
fn fixed_low_byte_has_one_consumable_reply_at_the_exact_deadline() {
    let (mut model, _) = model(true);
    let mut host = Ram::new(0x10000);
    configure_spi(&mut model, &mut host);
    write(&mut model, &mut host, SPI + 0x1c, 0x55).unwrap();
    assert_eq!(model.next_deadline(), Some(4));
    let checkpoint = model.snapshot();
    let before = model.save();
    assert_eq!(before, model.save());
    host.now = 3;
    model.advance(&mut host.cx());
    assert_eq!(before, model.save());
    assert_eq!(read(&mut model, &mut host, SPI).unwrap() & 1, 1);
    host.now = 4;
    model.advance(&mut host.cx());
    assert_eq!(model.next_deadline(), None);
    assert_eq!(read(&mut model, &mut host, SPI + 0x10).unwrap(), 1);
    assert_eq!(read(&mut model, &mut host, SPI + 0x18).unwrap(), 0);
    assert!(read(&mut model, &mut host, SPI + 0x18).is_err());
    assert_eq!(model.shifter.completed, 1);
    assert_eq!(model.shifter.consumed, 1);
    let complete = model.save();
    model.restore(checkpoint.clone());
    host.now = 3;
    model.advance(&mut host.cx());
    assert_eq!(model.save(), before);
    host.now = 4;
    model.advance(&mut host.cx());
    assert_eq!(read(&mut model, &mut host, SPI + 0x18).unwrap(), 0);
    assert_eq!(model.save(), complete);
    assert!(host.effects.is_empty());
}
#[test]
fn absent_input_pad_and_unread_fifo_refuse_without_starting_a_shift() {
    let (mut model, mut pads) = model(true);
    let mut host = Ram::new(0x10000);
    configure_spi(&mut model, &mut host);
    pads.input(&mut host.cx(), Pin { port: 11, pin: 2 }, true)
        .unwrap();
    let before = model.save();
    assert!(write(&mut model, &mut host, SPI + 0x1c, 0x23).is_err());
    assert_eq!(model.save(), before);
    pads.input(&mut host.cx(), Pin { port: 11, pin: 2 }, false)
        .unwrap();
    write(&mut model, &mut host, SPI + 0x1c, 0xaa).unwrap();
    host.now = 4;
    model.advance(&mut host.cx());
    let before = model.save();
    assert!(write(&mut model, &mut host, SPI + 0x1c, 0x23).is_err());
    assert_eq!(model.save(), before);
    write(&mut model, &mut host, SPI + 8, 0x10000000).unwrap();
    assert_eq!(model.next_deadline(), None);
    assert!(model.shifter.rx.is_none());
}
fn arm_uart(model: &mut SerialState, host: &mut Ram) {
    for (address, value) in [
        (0xfff98500, 0x12 << 16),
        (DMA, 0x400),
        (DMA + 4, UART + 4),
        (DMA + 8, 3),
        (DMA + 0x2c, 0x0022242a),
        (DMA + 0x28, 1),
    ] {
        write(model, host, address, value).unwrap();
    }
}
#[test]
fn uart_dma_captures_opaque_bytes_and_cancellation_has_no_external_effect() {
    let (mut model, _) = model(false);
    let mut host = Ram::new(0x10000);
    host.bytes[0x400..0x403].copy_from_slice(&[0x13, 0x55, 0xa2]);
    arm_uart(&mut model, &mut host);
    assert_eq!(model.next_deadline(), Some(4));
    let checkpoint = model.snapshot();
    host.bytes[0x400..0x403].fill(0);
    let before = model.save();
    host.now = 3;
    model.advance(&mut host.cx());
    assert_eq!(before, model.save());
    assert_eq!(read(&mut model, &mut host, UART + 12).unwrap(), 0);
    host.now = 4;
    model.advance(&mut host.cx());
    assert_eq!(read(&mut model, &mut host, UART + 12).unwrap(), 1);
    assert_eq!(read(&mut model, &mut host, DMA + 0x24).unwrap(), 0x40);
    assert_eq!(model.transmitted, vec![vec![0x13, 0x55, 0xa2]]);
    let complete = model.save();
    model.restore(checkpoint.clone());
    host.now = 3;
    model.advance(&mut host.cx());
    assert!(model.transmitted.is_empty());
    host.now = 4;
    model.advance(&mut host.cx());
    assert_eq!(model.save(), complete);
    arm_uart(&mut model, &mut host);
    write(&mut model, &mut host, DMA + 0x28, 8).unwrap();
    assert_eq!(model.next_deadline(), None);
    host.now = 20;
    model.advance(&mut host.cx());
    assert_eq!(model.transmitted.len(), 1);
    assert!(host.effects.is_empty());
    assert!(read(&mut model, &mut host, UART + 4).is_err());
}
#[test]
fn uart_direct_transmit_refuses_after_retaining_the_written_word() {
    let (mut model, _) = model(false);
    let mut host = Ram::new(0x10000);
    let before = model.save();
    assert!(write(&mut model, &mut host, UART + 4, 0x5a).is_err());
    assert_ne!(model.save(), before);
    assert_eq!(model.reg(UART + 4), 0x5a);
    assert!(host.effects.is_empty());
}
#[test]
fn unresolved_spi_is_only_shifted_by_the_explicit_discovery_entry() {
    let (model, _) = model(false);
    let mut host = Ram::new(0x10000);
    let core = Arc::new(Mutex::new(model));
    let mut spi = SerialDevice {
        core: core.clone(),
        block: Block::Spi0,
    };
    assert!(spi.access_contract(SPI + 0x1c, 4, Some(0x1234)).is_some());
    let before = core.lock().unwrap().save();
    assert!(write(&mut core.lock().unwrap(), &mut host, SPI + 0x1c, 0x1234).is_err());
    assert_eq!(core.lock().unwrap().save(), before);
    assert_eq!(
        spi.discovery_access(&mut host.cx(), SPI + 0x1c, 4, Some(0x1234)),
        Some(0)
    );
    assert!(
        core.lock().unwrap().save()["byte"]["transport_only"]
            .as_bool()
            .unwrap()
    );
    host.now = 4;
    core.lock().unwrap().advance(&mut host.cx());
    assert_eq!(
        read(&mut core.lock().unwrap(), &mut host, SPI + 0x18).unwrap(),
        0
    );
}

#[test]
fn status_acknowledges_low_bits_and_retains_unadmitted_high_configuration() {
    let (mut model, _) = model(true);
    let mut host = Ram::new(0x10000);
    configure_spi(&mut model, &mut host);
    write(&mut model, &mut host, SPI + 0x1c, 0x23).unwrap();
    host.now = 4;
    model.advance(&mut host.cx());
    read(&mut model, &mut host, SPI + 0x18).unwrap();
    write(&mut model, &mut host, SPI + 12, 0xabcd0011).unwrap();
    assert_eq!(read(&mut model, &mut host, SPI + 12).unwrap(), 0xabcd0300);
    assert!(
        write(&mut model, &mut host, SPI + 0x1c, 0xaa)
            .unwrap_err()
            .contains("unsupported SPI0")
    );
    assert_eq!(model.next_deadline(), None);
    write(&mut model, &mut host, SPI + 12, 0x300).unwrap();
    assert_eq!(read(&mut model, &mut host, SPI + 12).unwrap(), 0);
    write(&mut model, &mut host, SPI + 0x1c, 0xaa).unwrap();
    assert_eq!(model.next_deadline(), Some(8));
}

#[test]
fn checkpoint_replays_pending_spi_and_opaque_dma_at_the_same_boundary() {
    let (mut model, pads) = model(true);
    let mut host = Ram::new(0x10000);
    configure_spi(&mut model, &mut host);
    host.bytes[0x400..0x403].copy_from_slice(&[0x13, 0x55, 0xa2]);
    write(&mut model, &mut host, SPI + 0x1c, 0x55).unwrap();
    arm_uart(&mut model, &mut host);
    let before = model.save();
    let order = (
        model.byte.as_ref().unwrap().order,
        model.program.as_ref().unwrap().order,
    );
    assert!(order.0 < order.1);
    let saved = model.snapshot();
    assert_eq!(model.save(), before);
    assert_eq!(model.next_deadline(), Some(4));
    host.effects.clear();
    host.now = 4;
    model.advance(&mut host.cx());
    let completed = model.save();
    assert_eq!(model.transmitted, [vec![0x13, 0x55, 0xa2]]);
    assert_eq!(read(&mut model, &mut host, SPI + 0x18).unwrap(), 0);
    model.restore(saved.clone());
    assert_eq!(
        (
            model.byte.as_ref().unwrap().order,
            model.program.as_ref().unwrap().order
        ),
        order
    );
    assert_eq!(model.save(), before);
    // RAM is no longer the captured transfer source; restored DMA retains its bytes.
    host.bytes[0x400..0x403].fill(0);
    host.effects.clear();
    host.now = 3;
    model.advance(&mut host.cx());
    assert_eq!(model.save(), before);
    assert!(host.effects.is_empty());
    host.now = 4;
    model.advance(&mut host.cx());
    assert_eq!(model.save(), completed);
    assert!(host.effects.is_empty());
    assert_eq!(read(&mut model, &mut host, SPI + 0x18).unwrap(), 0);
    assert_eq!(model.next_deadline(), None);
    assert!(!pads.input_level(Pin { port: 11, pin: 2 }).unwrap());
}

struct TraceHost {
    now: u64,
    bytes: Vec<u8>,
    records: Vec<String>,
}
impl Host for TraceHost {
    fn now(&self) -> u64 {
        self.now
    }
    fn schedule(&mut self, _: usize, _: u64, _: u64) {}
    fn set_irq(&mut self, _: u32, _: bool) {}
    fn raise_irq(&mut self, _: usize, _: u32) -> Result<(), String> {
        Ok(())
    }
    fn ram_read(&self, address: u64, out: &mut [u8]) -> Result<(), String> {
        let start = address as usize & 0x3fff_ffff;
        let input = self
            .bytes
            .get(start..start + out.len())
            .ok_or("test DMA read out of RAM")?;
        out.copy_from_slice(input);
        Ok(())
    }
    fn ram_write(&mut self, address: u64, bytes: &[u8]) -> Result<(), String> {
        let start = address as usize & 0x3fff_ffff;
        self.bytes
            .get_mut(start..start + bytes.len())
            .ok_or("test DMA write out of RAM")?
            .copy_from_slice(bytes);
        Ok(())
    }
    fn record(&mut self, record: Record) {
        self.records.push(record.name.into_owned());
    }
}
impl TraceHost {
    fn cx(&mut self) -> Cx<'_> {
        Cx::new(self, 0, 0, 0)
    }
}

fn simultaneous_completions(spi_first: bool) -> (Vec<String>, u32, u32, u32, Vec<Vec<u8>>) {
    let (mut core, _) = model(true);
    core.put(SPI, 0xc0);
    core.put(SPI + 4, 0x30);
    core.put(SPI + 8, 0);
    core.put(0xfff98500, 0x12 << 16);
    core.put(DMA, 0x400);
    core.put(DMA + 4, (UART + 4) as u32);
    core.put(DMA + 8, 3);
    core.put(DMA + 0x2c, 0x0022242a);
    let mut host = TraceHost {
        now: 0,
        bytes: vec![0; 0x1000],
        records: vec![],
    };
    host.bytes[0x400..0x403].copy_from_slice(&[0x13, 0x55, 0xa2]);
    if spi_first {
        core.start_byte(&mut host.cx(), 0x55, false).unwrap();
        core.start_program(&mut host.cx()).unwrap();
    } else {
        core.start_program(&mut host.cx()).unwrap();
        core.start_byte(&mut host.cx(), 0x55, false).unwrap();
    }
    assert_eq!(core.next_deadline(), Some(4));
    let core = Arc::new(Mutex::new(core));
    let mut scheduler = SerialDevice {
        core,
        block: Block::Coordinator,
    };
    host.now = 4;
    scheduler.advance(&mut host.cx());
    let core = scheduler.core.lock().unwrap();
    let records = host
        .records
        .into_iter()
        .filter(|name| name.ends_with("complete"))
        .collect();
    (
        records,
        core.reg(SPI + 12),
        core.reg(UART + 12),
        core.reg(DMA + 0x24),
        core.transmitted.clone(),
    )
}

#[test]
fn boot_serial_blocks_preserve_start_order_for_equal_tick_spi_and_dma_completions() {
    assert_eq!(
        simultaneous_completions(true),
        (
            vec![
                "SPI0 byte shift complete".to_owned(),
                "Boot serial DMA complete".to_owned(),
            ],
            0x300,
            1,
            0x40,
            vec![vec![0x13, 0x55, 0xa2]],
        )
    );
    assert_eq!(
        simultaneous_completions(false),
        (
            vec![
                "Boot serial DMA complete".to_owned(),
                "SPI0 byte shift complete".to_owned(),
            ],
            0x300,
            1,
            0x40,
            vec![vec![0x13, 0x55, 0xa2]],
        )
    );
}

#[test]
fn boot_serial_blocks_assign_each_physical_aperture_to_one_block() {
    let (_, pads) = model(true);
    let devices = BootSerialBlocks::new(
        Options {
            transfer_ticks: 4,
            max_program_bytes: 0x10000,
            spi0: None,
        },
        Wiring { gpio: pads },
    )
    .unwrap()
    .devices();
    let layout = devices
        .iter()
        .map(|(mapping, device)| {
            (
                mapping.map(|mapping| (mapping.base, mapping.size)),
                device.name(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        layout,
        [
            (Some((SPI, 0x20)), "spi0-byte-shifter"),
            (Some((UART, 0x10)), "boot-uart"),
            (Some((DMA, 0x40)), "boot-peripheral-dma"),
            (Some((0xfff98500, 8)), "boot-dma-request-selector"),
            (None, "serial-event-coordinator"),
        ]
    );
}
