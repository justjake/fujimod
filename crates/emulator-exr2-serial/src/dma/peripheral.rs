//! EXR II direct peripheral DMA, driven by request lines and data ports.
//!
//! Register layout and supported byte-wide TX/RX modes are explicit model
//! configuration.
//! This mechanism does not identify an attached part or implement its protocol.
//! Memory, linked, repeated, strided and other peripheral modes refuse.
use super::routing::{Routing, SharedRouting};
use fujimod_emulator_devices::bus::InterruptLine;
use fujimod_emulator_devices::dma::{Direction, SharedPort};
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

const CHANNELS: usize = 8;
const TX: u32 = 0x03200168;
const RX: u32 = 0x02100160;

#[derive(Clone)]
pub struct Connection {
    pub address: u64,
    pub direction: Direction,
    pub port: SharedPort,
}
#[derive(Clone, Default, Serialize, Deserialize)]
struct Channel {
    registers: [u32; 16],
    active: bool,
    transferred: u32,
    irq_published: bool,
}
impl Channel {
    fn direction(&self) -> Result<Direction, String> {
        match self.registers[11] & !7 {
            TX => Ok(Direction::Transmit),
            RX => Ok(Direction::Receive),
            _ => Err("EXR II DMA peripheral mode is unmodeled".into()),
        }
    }
    fn complete(&self) -> bool {
        self.registers[9] == 0x40
    }
}

pub struct PeripheralChannels {
    base: u64,
    channels: [Channel; CHANNELS],
    routing: SharedRouting,
    connections: BTreeMap<u8, Connection>,
    interrupts: [Vec<InterruptLine>; CHANNELS],
}
impl PeripheralChannels {
    fn validate_state(
        &self,
        channels: &[Channel; CHANNELS],
        routing: &Routing,
    ) -> Result<(), String> {
        if routing.words.iter().any(|v| v & !0x1f1f1f1f != 0)
            || channels.iter().enumerate().any(|(index, c)| {
                c.active != routing.active[index]
                    || (c.active && (c.registers[9] != 5 || c.registers[2] == 0))
                    || (!c.active && !matches!(c.registers[9], 0 | 0x40))
                    || c.irq_published != c.complete()
                    || c.transferred > 0x100000
                    || (c.active && c.registers[2] as u64 + c.transferred as u64 > 0x100000)
            })
        {
            return Err("invalid EXR II DMA checkpoint state".into());
        }
        // Validate active mode/port wiring against the saved route before any mutation.
        for (index, c) in channels.iter().enumerate().filter(|(_, c)| c.active) {
            if c.registers[11] & 7 != index as u32 {
                return Err("EXR II DMA checkpoint channel differs".into());
            }
            let p = self
                .connections
                .get(&routing.selector(index))
                .ok_or("EXR II DMA checkpoint port is absent")?;
            let direction = c.direction()?;
            if p.direction != direction
                || p.address
                    != c.registers[if direction == Direction::Transmit {
                        1
                    } else {
                        0
                    }] as u64
                || [3, 4, 5, 12, 13, 14]
                    .iter()
                    .any(|word| c.registers[*word] != 0)
                || c.registers[2] > 0x100000
            {
                return Err("EXR II DMA checkpoint transfer is unsupported".into());
            }
        }
        Ok(())
    }

    pub fn new(
        base: u64,
        routing: SharedRouting,
        connections: BTreeMap<u8, Connection>,
        interrupts: [Vec<InterruptLine>; CHANNELS],
    ) -> Result<Self, String> {
        if base & 0x3f != 0
            || base.checked_add(0x200).is_none()
            || connections.keys().any(|v| *v >= 32)
        {
            return Err("invalid EXR II peripheral DMA wiring".into());
        }
        Ok(Self {
            base,
            channels: std::array::from_fn(|_| Channel::default()),
            routing,
            connections,
            interrupts,
        })
    }
    fn register(&self, address: u64, size: u8) -> Option<(usize, usize)> {
        let offset = address.checked_sub(self.base)?;
        if size != 4 || offset >= 0x200 || offset & 3 != 0 {
            return None;
        }
        let word = (offset % 0x40) as usize / 4;
        matches!(word, 0..=5 | 9..=14).then_some((offset as usize / 0x40, word))
    }
    fn connection(&self, channel: usize, c: &Channel) -> Result<&Connection, String> {
        let selector = self
            .routing
            .lock()
            .map_err(|_| "EXR II DMA routing poisoned")?
            .selector(channel);
        let p = self
            .connections
            .get(&selector)
            .ok_or("EXR II DMA request has no attached port")?;
        let direction = c.direction()?;
        let address = c.registers[if direction == Direction::Transmit {
            1
        } else {
            0
        }] as u64;
        if p.direction != direction || address != p.address {
            return Err("EXR II DMA port address/direction differs from its request wire".into());
        }
        Ok(p)
    }
    fn validate(&self, channel: usize, c: &Channel) -> Result<(), String> {
        if c.registers[11] & 7 != channel as u32
            || !matches!(c.registers[2], 1..=0x100000)
            || [3, 4, 5, 12, 13, 14].iter().any(|i| c.registers[*i] != 0)
        {
            return Err(
                "EXR II DMA count, channel, alternate, repeat or linked mode is unmodeled".into(),
            );
        }
        self.connection(channel, c)?;
        Ok(())
    }
    fn request(&self, channel: usize, c: &Channel) -> Result<Option<u64>, String> {
        if !c.active {
            return Ok(None);
        }
        let p = self.connection(channel, c)?;
        Ok(p.port
            .lock()
            .map_err(|_| "EXR II DMA port poisoned")?
            .request(p.direction, false))
    }
    fn publish(&mut self, cx: &mut Cx<'_>, channel: usize) -> Result<(), String> {
        let high = self.channels[channel].complete();
        if high == self.channels[channel].irq_published {
            return Ok(());
        }
        for line in &self.interrupts[channel] {
            line.drive(cx, high)?;
        }
        self.channels[channel].irq_published = high;
        Ok(())
    }
    fn write_register(&mut self, channel: usize, word: usize, value: u32) -> Result<(), String> {
        if word == 9 {
            return Err("EXR II DMA status is read-only".into());
        }
        if word == 10 && value == 8 {
            let c = &mut self.channels[channel];
            c.active = false;
            c.registers[9] = 0;
            c.registers[10] = 8;
            c.transferred = 0;
            self.routing.lock().unwrap().active[channel] = false;
            return Ok(());
        }
        if self.channels[channel].active {
            return Err("EXR II DMA channel changed while active".into());
        }
        if word != 10 {
            self.channels[channel].registers[word] = value;
            return Ok(());
        }
        if value != 1 {
            return Err("EXR II DMA activation control is unmodeled".into());
        }
        self.validate(channel, &self.channels[channel])?;
        let c = &mut self.channels[channel];
        c.registers[9] = 5;
        c.registers[10] = value;
        c.active = true;
        c.transferred = 0;
        self.routing.lock().unwrap().active[channel] = true;
        Ok(())
    }
    fn beat(&mut self, cx: &mut Cx<'_>, channel: usize) -> Result<(), String> {
        let c = &self.channels[channel];
        let p = self.connection(channel, c)?;
        let direction = p.direction;
        let word = if direction == Direction::Transmit {
            0
        } else {
            1
        };
        let address = c.registers[word] as u64;
        let mut port = p.port.lock().map_err(|_| "EXR II DMA port poisoned")?;
        if direction == Direction::Transmit {
            let mut byte = [0];
            cx.ram_read(address, &mut byte)?;
            port.transmit(cx.now(), 1, byte[0] as u64)?;
        } else {
            let value = port.receive(cx.now(), 1)?;
            if value > 255 {
                return Err("EXR II DMA byte port returned a wider value".into());
            }
            cx.ram_write(address, &[value as u8])?;
        }
        drop(port);
        let c = &mut self.channels[channel];
        c.transferred += 1;
        c.registers[word] = c.registers[word].wrapping_add(1);
        c.registers[2] -= 1;
        if c.registers[2] == 0 {
            c.active = false;
            c.registers[9] = 0x40;
            self.routing.lock().unwrap().active[channel] = false;
        }
        Ok(())
    }
    fn wiring(&self) -> Value {
        let ports: Vec<_> = self
            .connections
            .iter()
            .map(|(request, p)| (*request, p.address, p.direction))
            .collect();
        json!({"base":self.base,"ports":ports,"interrupts":self.interrupts})
    }
}
impl Device for PeripheralChannels {
    fn name(&self) -> &'static str {
        "exr2-peripheral-dma"
    }
    fn checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        fujimod_emulator_devices::checkpoint::Checkpoint::encode(
            self.name(),
            &(
                self.wiring(),
                &self.channels,
                &*self.routing.lock().unwrap(),
            ),
        )
    }
    fn validate_checkpoint(&self, saved: &fujimod_emulator_devices::checkpoint::Checkpoint) -> Result<(), String> {
        let (wiring, channels, routing): (Value, [Channel; CHANNELS], Routing) =
            saved.decode(self.name())?;
        if wiring != self.wiring() || !saved.children().is_empty() || !saved.blobs().is_empty() {
            return Err("request DMA checkpoint wiring differs".into());
        }
        self.validate_state(&channels, &routing)
    }
    fn restore_checkpoint(&mut self, saved: &fujimod_emulator_devices::checkpoint::Checkpoint) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        let (_, channels, routing): (Value, [Channel; CHANNELS], Routing) =
            saved.decode(self.name())?;
        self.channels = channels;
        *self.routing.lock().unwrap() = routing;
        Ok(())
    }
    fn fingerprint(&self) -> Result<Value, String> {
        Ok(self.save())
    }

    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let Some((channel, word)) = self.register(address, size) else {
            cx.unmodeled();
            return 0;
        };
        self.channels[channel].registers[word] as u64
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let Some((channel, word)) = self.register(address, size) else {
            cx.unmodeled();
            return;
        };
        if value > u32::MAX as u64 {
            cx.unmodeled();
            return;
        }
        if let Err(error) = self.write_register(channel, word, value as u32) {
            cx.fail(error);
            return;
        }
        if let Err(error) = self.publish(cx, channel) {
            cx.fail(error);
        }
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        for channel in 0..CHANNELS {
            match self.request(channel, &self.channels[channel]) {
                Ok(Some(tick)) if tick <= cx.now() => {
                    if let Err(error) = self.beat(cx, channel) {
                        cx.fail(error);
                        return;
                    }
                }
                Err(error) => {
                    cx.fail(error);
                    return;
                }
                _ => {}
            }
            if let Err(error) = self.publish(cx, channel) {
                cx.fail(error);
                return;
            }
        }
    }
    fn next_deadline(&self, now: u64) -> Option<u64> {
        self.channels
            .iter()
            .enumerate()
            .filter_map(|(channel, c)| self.request(channel, c).unwrap_or(Some(now)))
            .map(|tick| tick.max(now))
            .min()
    }
    fn save(&self) -> Value {
        json!({"wiring":self.wiring(),"channels":self.channels,"routing":*self.routing.lock().unwrap()})
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        if value["wiring"] != self.wiring() {
            return Err("EXR II DMA checkpoint wiring differs".into());
        }
        let channels: [Channel; CHANNELS] =
            serde_json::from_value(value["channels"].clone()).map_err(|e| e.to_string())?;
        let routing: Routing =
            serde_json::from_value(value["routing"].clone()).map_err(|e| e.to_string())?;
        self.validate_state(&channels, &routing)?;
        self.channels = channels;
        *self.routing.lock().unwrap() = routing;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
