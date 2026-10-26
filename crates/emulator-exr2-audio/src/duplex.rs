//! One serial-audio DMA channel with explicit external input and output endpoints.
//!
//! Programmed channel configuration selects the direction. Inactive direction
//! changes preserve the channel registers, completion state and audio banks;
//! an active transfer cannot change direction. Each direction retains its
//! existing descriptor validation, byte movement, clock and IRQ ordering.
use super::{receive, transmit};
use fujimod_emulator_devices::exr2::irq_routes::IrqRoutes;
use fujimod_emulator_devices::{Cx, Device, checkpoint::Checkpoint};
use fujimod_emulator_exr2_serial::dma::audio::BASE;
use fujimod_emulator_parts_io::audio_sink::{AudioSink, Options as SinkOptions};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub receive: receive::Options,
    pub transmit: transmit::Options,
    pub sinks: [bool; 2],
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Direction {
    Receive,
    Transmit,
}

pub struct Duplex {
    options: Options,
    receive: receive::Receiver,
    transmit: transmit::Transmitter,
    direction: Direction,
}
impl Duplex {
    pub fn new(options: Options, routes: IrqRoutes) -> Result<Self, String> {
        if options.receive.ram_bytes != options.transmit.ram_bytes
            || options.receive.max_transfer_bytes != options.transmit.max_transfer_bytes
        {
            return Err("audio channel directions require the same DMA bounds".into());
        }
        Ok(Self {
            options,
            receive: receive::Receiver::new(options.receive, routes.clone())?,
            transmit: transmit::Transmitter::new(
                options.transmit,
                options
                    .sinks
                    .map(|connected| AudioSink::new(SinkOptions { connected })),
                routes,
            )?,
            direction: Direction::Transmit,
        })
    }
    fn device(&self) -> &dyn Device {
        match self.direction {
            Direction::Receive => &self.receive,
            Direction::Transmit => &self.transmit,
        }
    }
    fn device_mut(&mut self) -> &mut dyn Device {
        match self.direction {
            Direction::Receive => &mut self.receive,
            Direction::Transmit => &mut self.transmit,
        }
    }
    fn active(&self) -> bool {
        match self.direction {
            Direction::Receive => self.receive.dma_active(),
            Direction::Transmit => self.transmit.dma_active(),
        }
    }
    fn decode(&self, value: &Value) -> Result<(Direction, Value), String> {
        let (direction, state): (Direction, Value) =
            serde_json::from_value(value.clone()).map_err(|error| error.to_string())?;
        match direction {
            Direction::Receive => self.receive.validate_state(&state)?,
            Direction::Transmit => self.transmit.validate_state(&state)?,
        }
        Ok((direction, state))
    }
}
impl Device for Duplex {
    fn name(&self) -> &'static str {
        "exr2-serial-audio-duplex"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        self.device_mut().read(cx, address, size)
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        if address == BASE + 0x2c && size == 4 {
            let direction = if u32::try_from(value).is_ok_and(receive::Receiver::configuration) {
                Some(Direction::Receive)
            } else if u32::try_from(value).is_ok_and(transmit::Transmitter::configuration) {
                Some(Direction::Transmit)
            } else {
                None
            };
            if let Some(direction) = direction {
                if direction != self.direction {
                    if self.active() {
                        cx.fail("active serial-audio DMA cannot change direction");
                        return;
                    }
                    // Both engines expose the same inactive channel register
                    // state. Transfer that state once, replacing only the
                    // configuration word written by this hardware operation.
                    let mut state = self.device().save();
                    state["dma"]["registers"]["44"] = json!(value);
                    let result = match direction {
                        Direction::Receive => self.receive.restore(&state),
                        Direction::Transmit => self.transmit.restore(&state),
                    };
                    if let Err(error) = result {
                        cx.fail(error);
                        return;
                    }
                    self.direction = direction;
                }
            }
        }
        self.device_mut().write(cx, address, size, value);
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        self.device_mut().advance(cx);
    }
    fn next_deadline(&self, now: u64) -> Option<u64> {
        self.device().next_deadline(now)
    }
    fn pollable(&self, address: u64, size: u8) -> bool {
        self.device().pollable(address, size)
    }
    fn save(&self) -> Value {
        json!((self.direction, self.device().save()))
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let (direction, state) = self.decode(value)?;
        match direction {
            Direction::Receive => self.receive.restore(&state)?,
            Direction::Transmit => self.transmit.restore(&state)?,
        }
        self.direction = direction;
        Ok(())
    }
    fn fingerprint(&self) -> Result<Value, String> {
        Ok(json!({"options":self.options,"state":self.save()}))
    }
    fn checkpoint(&self) -> Result<Checkpoint, String> {
        Checkpoint::encode(self.name(), &(self.options, self.save()))
    }
    fn validate_checkpoint(&self, saved: &Checkpoint) -> Result<(), String> {
        let (options, state): (Options, Value) = saved.decode(self.name())?;
        if options != self.options {
            return Err("audio duplex snapshot configuration differs".into());
        }
        self.decode(&state).map(|_| ())
    }
    fn restore_checkpoint(&mut self, saved: &Checkpoint) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        let (_, state): (Options, Value) = saved.decode(self.name())?;
        self.restore(&state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::test_host::{TestHost, advance, read, write};
    fn setup() -> (Duplex, TestHost) {
        let routes = IrqRoutes::default();
        routes.set(0x5c, Some(0)).unwrap();
        let device = Duplex::new(
            Options {
                receive: receive::Options {
                    ram_bytes: 0x100000,
                    max_transfer_bytes: 1024,
                    ticks_per_second: 12_000_000,
                    sample_rate: 48000,
                    sample: -1234,
                },
                transmit: transmit::Options {
                    ram_bytes: 0x100000,
                    max_transfer_bytes: 1024,
                    ticks_per_byte: 2,
                    minimum_ticks: 3,
                },
                sinks: [true, false],
            },
            routes,
        )
        .unwrap();
        let mut host = TestHost::new();
        host.ram[0x1000..0x1004].copy_from_slice(&[1, 2, 3, 4]);
        (device, host)
    }
    fn start(device: &mut Duplex, host: &mut TestHost, receive: bool) {
        write(
            device,
            host,
            0xfffa0008,
            if receive { 0x10002 } else { 0x20001 },
        );
        for (offset, value) in [
            (0, if receive { 0xfffa0020 } else { 0x1000 }),
            (4, if receive { 0x2000 } else { 0xfffa0010 }),
            (8, if receive { 8 } else { 4 }),
            (0x2c, if receive { 0x122011 } else { 0x221011 }),
            (0x28, 1),
        ] {
            write(device, host, BASE + offset, value);
        }
    }
    #[test]
    fn transmit_receive_transmit_preserves_endpoints_bytes_and_completion_order() {
        let (mut device, mut host) = setup();
        start(&mut device, &mut host, false);
        advance(&mut device, &mut host, 0);
        host.ram[0x1000..0x1004].fill(99);
        advance(&mut device, &mut host, 8);
        assert_eq!(
            device.device().save()["audio"]["sinks"][0]["bytes_received"],
            4
        );
        start(&mut device, &mut host, true);
        advance(&mut device, &mut host, 8);
        advance(&mut device, &mut host, 508);
        assert_eq!(
            &host.ram[0x2000..0x2008],
            &(-1234i16).to_le_bytes().repeat(4)
        );
        assert_eq!(host.effects, ["irq", "dma", "irq"]);
        start(&mut device, &mut host, false);
        advance(&mut device, &mut host, 508);
        advance(&mut device, &mut host, 516);
        assert_eq!(
            device.device().save()["audio"]["sinks"][0]["bytes_received"],
            8
        );
        assert_eq!(host.irqs, [(0, 0x5c), (0, 0x5c), (0, 0x5c)]);
        assert_eq!(read(&mut device, &mut host, BASE + 0x24), 0x40);
    }
    #[test]
    fn checkpoint_resumes_each_direction_and_validates_external_endpoints() {
        for receive in [false, true] {
            let (mut device, mut host) = setup();
            start(&mut device, &mut host, receive);
            advance(&mut device, &mut host, 0);
            let saved = device.checkpoint().unwrap();
            let before = host.clone();
            let due = device.next_deadline(0).unwrap();
            advance(&mut device, &mut host, due);
            let expected = device.fingerprint().unwrap();
            let expected_host = host.clone();
            device.restore_checkpoint(&saved).unwrap();
            host = before;
            advance(&mut device, &mut host, due);
            assert_eq!(device.fingerprint().unwrap(), expected);
            assert_eq!(host.ram, expected_host.ram);
            assert_eq!(host.irqs, expected_host.irqs);
            let mut other = setup().0;
            other.options.sinks[0] = false;
            assert!(other.validate_checkpoint(&saved).is_err());
        }
    }
    #[test]
    fn active_direction_changes_and_unknown_configurations_refuse_without_mutation() {
        for receive in [false, true] {
            let (mut device, mut host) = setup();
            start(&mut device, &mut host, receive);
            advance(&mut device, &mut host, 0);
            let before = device.fingerprint().unwrap();
            let mut context = host.cx();
            device.write(
                &mut context,
                BASE + 0x2c,
                4,
                if receive { 0x221011 } else { 0x122011 },
            );
            assert!(context.refusal().is_some());
            assert_eq!(device.fingerprint().unwrap(), before);
            let mut context = host.cx();
            device.write(&mut context, BASE + 0x2c, 4, 0xdeadbeef);
            assert!(context.refusal().is_some());
            assert_eq!(device.fingerprint().unwrap(), before);
            assert!(host.irqs.is_empty());
        }
    }
}
