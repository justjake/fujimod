//! SPI2 (FFF84080) accepts a receive transport and returns the byte a
//! register-file endpoint drives when the addressed register is read.
use super::*;
use fujimod_emulator_devices::Cx;
use fujimod_emulator_devices::parts::pins::{Pin, Pins, Selection};
use fujimod_emulator_devices::test_host::{TestHost, advance, read, write};
use fujimod_emulator_parts_io::spi::{RegisterFile, RegisterOptions};

const BASE: u64 = 0xfff84080;

/// Port 4 pin 0 is the chip select of the attached part and is active low.
struct TestPins;

impl Pins for TestPins {
    fn output_level(&self, pin: Pin) -> Result<Option<bool>, String> {
        Ok(matches!(pin, Pin { port: 4, pin: 0 }).then_some(false))
    }

    fn input(&mut self, _: &mut Cx<'_>, _: Pin, _: bool) -> Result<(), String> {
        Ok(())
    }
}

fn channel() -> SerialChannel {
    SerialChannel::new(
        Options {
            base: BASE,
            transports: vec![Transport {
                control: 0xe0,
                configuration: 6,
                bits: 16,
                receive: true,
            }],
            selection: vec![Selection {
                pin: Pin { port: 4, pin: 0 },
                high: false,
            }],
            transfer_ticks: 4,
            queue: Queue::Single,
        },
        Wiring {
            pins: Box::new(TestPins),
            endpoint: Box::new(
                RegisterFile::new(RegisterOptions {
                    registers: vec![(0x60, 0x22)],
                    readable: true,
                    status_registers: vec![0x60],
                    clock: None,
                })
                .unwrap(),
            ),
        },
    )
    .unwrap()
}

#[test]
fn spi2_reads_a_register_through_a_receive_transport() {
    let mut device = channel();
    let mut host = TestHost::new();
    // The declared receive transport is control 0xE0 with configuration 6.
    write(&mut device, &mut host, BASE, 0xe0);
    write(&mut device, &mut host, BASE + 4, 6);
    // One 16-bit frame: write bit15 clear reads address 0x60.
    write(&mut device, &mut host, BASE + 0x1c, 0x6000);
    assert_eq!(device.next_deadline(host.now), Some(4));
    advance(&mut device, &mut host, 4);
    assert_eq!(device.next_deadline(host.now), None);
    // The reply lands in the channel RX register once the frame completes.
    assert_eq!(read(&mut device, &mut host, BASE + 0x18) & 0xff, 0x22);
}
