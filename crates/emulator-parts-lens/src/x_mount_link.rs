//! Bounded X-mount link carried by physical byte-wide SPI frames.
//!
//! Supported exchanges are sync, link parameters, identification, status,
//! ring reads, and declared read-only catalogs. Bounded command20 writes latch on sync.
//! Unknown commands refuse. GPIO/power behavior, optical stabilization and
//! boot qualification are outside this part.
use fujimod_emulator_parts_io::serial::{Frame, Wire};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::VecDeque;

const SYNC: u16 = 0xff;
const PARAMETERS: u16 = 0x18;
const IDENTIFICATION: u16 = 0x14;
const STATUS: u16 = 0x20;
const APERTURE_RING: u16 = 0x30;
const FOCUS_RING: u16 = 0x32;
const DETAIL_STATUS: u16 = 0x26;
const READ_CATALOG: u16 = 0xc0;
const CATALOG_BLOCK_BYTES: usize = 2048;
const CONTROL20: u16 = 0x80;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Packet(u32);
impl Packet {
    fn encode(payload: u16, family: u16, ack: bool) -> Self {
        let mut word = ((payload as u32) << 16) | ((family as u32) << 6);
        word |= u32::from(ack) << 15;
        let check = ([27, 22, 17, 12, 7]
            .iter()
            .map(|s| (word >> s) & 31)
            .sum::<u32>()
            + ((word >> 6) & 1))
            & 31;
        Self(word | (check << 1))
    }
    fn decode(bytes: [u8; 4]) -> Result<Self, String> {
        let packet = Self(u32::from_be_bytes(bytes));
        if packet.0 & 1 != 0
            || Self::encode(packet.payload(), packet.family(), packet.ack()) != packet
        {
            return Err(format!(
                "X-mount packet checksum/framing is invalid: {:08x}",
                packet.0
            ));
        }
        if packet.ack() && (packet.0 >> 27 != 1 || packet.payload() & 0x00ef != 0) {
            return Err(format!(
                "X-mount acknowledgment status is unmodeled: {:08x}",
                packet.0
            ));
        }
        Ok(packet)
    }
    fn payload(self) -> u16 {
        (self.0 >> 16) as u16
    }
    fn family(self) -> u16 {
        ((self.0 & 0x7fc0) >> 6) as u16
    }
    fn ack(self) -> bool {
        self.0 & 0x8000 != 0
    }
    fn bytes(self) -> [u8; 4] {
        self.0.to_be_bytes()
    }
}

/// A read-only external data bank selected by command30/tag0.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReadCatalog {
    pub selector: u8,
    /// Five raw metadata bits above the eleven-bit block count.
    pub attributes: u8,
    pub data: Vec<u8>,
}
impl ReadCatalog {
    fn word(&self) -> u16 {
        ((self.attributes as u16) << 11) | (self.data.len() / CATALOG_BLOCK_BYTES) as u16
    }
}
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
enum BulkSource {
    Identification,
    Catalog { selector: u8 },
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Configuration {
    /// Supported incoming link-parameter value; no unconstrained echo law.
    pub peer_parameters: u16,
    pub parameters: u16,
    /// The externally supplied 128-byte identification record.
    pub identification: Vec<u8>,
    /// Raw tag-0 command-08 status input. Its individual flags are not inferred.
    pub status: u16,
    /// Raw command0C tag0/tag2 and command09 tag2 inputs, respectively.
    pub aperture_ring: u16,
    pub focus_ring: u16,
    pub detail_status: u16,
    pub read_catalogs: Vec<ReadCatalog>,
}
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Message {
    packet: Packet,
    sequence: u8,
    catalog: Option<u8>,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
enum Phase {
    #[default]
    Header,
    Acknowledgment {
        received: Packet,
        sent: Message,
    },
    Bulk {
        source: BulkSource,
        block: usize,
    },
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    input: Vec<u8>,
    replies: VecDeque<Message>,
    outgoing: Option<Message>,
    phase: Phase,
    transmit_counter: u8,
    receive_counter: u8,
    frames: u64,
    last_tick: Option<u64>,
    control20: Option<u16>,
    staged_control20: Option<u16>,
}

pub struct Link {
    configuration: Configuration,
    state: State,
}
impl Link {
    pub fn new(configuration: Configuration) -> Result<Self, String> {
        if configuration.identification.len() != 128 {
            return Err("X-mount identification record must contain 128 bytes".into());
        }
        for (index, catalog) in configuration.read_catalogs.iter().enumerate() {
            if catalog.selector > 15
                || catalog.attributes > 31
                || catalog.data.is_empty()
                || catalog.data.len() % CATALOG_BLOCK_BYTES != 0
                || catalog.data.len() / CATALOG_BLOCK_BYTES > 0x7ff
                || configuration.read_catalogs[..index]
                    .iter()
                    .any(|c| c.selector == catalog.selector)
            {
                return Err("invalid X-mount read catalog geometry/selector/metadata".into());
            }
        }
        Ok(Self {
            configuration,
            state: State::default(),
        })
    }
    /// Explicit external reset; serial-controller FIFO reset is a separate wire.
    pub fn reset(&mut self) {
        self.state = State::default();
    }
    pub fn frame_count(&self) -> u64 {
        self.state.frames
    }
    fn frame(frame: Frame) -> Result<(), String> {
        if frame.bits != 8 || frame.mode != 3 || frame.word > 255 {
            return Err("X-mount link requires byte-wide SPI mode 3".into());
        }
        Ok(())
    }
    fn response(&self, packet: Packet) -> Result<Option<u16>, String> {
        if packet.ack() {
            return Err("X-mount expected a normal header".into());
        }
        match (packet.family(), packet.payload()) {
            (0, 0) => Ok(None),
            (SYNC, 0) | (IDENTIFICATION, 0) => Ok(Some(0)),
            (STATUS, 0) => Ok(Some(self.configuration.status)),
            (APERTURE_RING, 0) => Ok(Some(self.configuration.aperture_ring)),
            (FOCUS_RING, 0) => Ok(Some(self.configuration.focus_ring)),
            (DETAIL_STATUS, 0) => Ok(Some(self.configuration.detail_status)),
            (CONTROL20, value @ 0..=1) => Ok(Some(value)),
            (READ_CATALOG, selector) => self
                .configuration
                .read_catalogs
                .iter()
                .find(|c| c.selector as u16 == selector)
                .map(|c| Some(c.word()))
                .ok_or_else(|| format!("X-mount read catalog {selector} is not supplied")),
            (PARAMETERS, value) if value == self.configuration.peer_parameters => {
                Ok(Some(self.configuration.parameters))
            }
            _ => Err(format!("X-mount request is unmodeled: {:08x}", packet.0)),
        }
    }
    fn valid_message(&self, message: Message) -> bool {
        if message.sequence > 7 || message.packet.ack() {
            return false;
        }
        if Packet::decode(message.packet.bytes()).is_err() {
            return false;
        }
        if let Some(selector) = message.catalog {
            return message.packet.family() == READ_CATALOG
                && self
                    .configuration
                    .read_catalogs
                    .iter()
                    .any(|c| c.selector == selector && c.word() == message.packet.payload());
        }
        match (message.packet.family(), message.packet.payload()) {
            (0, 0) | (SYNC, 0) | (IDENTIFICATION, 0) => true,
            (PARAMETERS, payload) => payload == self.configuration.parameters,
            (STATUS, payload) => payload == self.configuration.status,
            (APERTURE_RING, payload) => payload == self.configuration.aperture_ring,
            (FOCUS_RING, payload) => payload == self.configuration.focus_ring,
            (DETAIL_STATUS, payload) => payload == self.configuration.detail_status,
            (CONTROL20, 0..=1) => true,
            _ => false,
        }
    }
    fn deliver(&mut self, packet: Packet, ack: Packet) -> Result<(), String> {
        if packet.0 == 0 {
            return Ok(());
        }
        // A tag belongs to the peer's queued normal header. It must match
        // the ordinal of this received message, not a count of SPI clocks.
        if (ack.payload() >> 8) & 7 != self.state.receive_counter as u16 {
            return Err("X-mount acknowledgment has an unexpected message counter".into());
        }
        if let Some(payload) = self.response(packet)? {
            self.state.replies.push_back(Message {
                packet: Packet::encode(payload, packet.family(), false),
                sequence: self.state.transmit_counter,
                catalog: (packet.family() == READ_CATALOG).then_some(packet.payload() as u8),
            });
            self.state.transmit_counter = (self.state.transmit_counter + 1) & 7;
        }
        match packet.family() {
            CONTROL20 => self.state.staged_control20 = Some(packet.payload()),
            SYNC => {
                if let Some(value) = self.state.staged_control20.take() {
                    self.state.control20 = Some(value);
                }
            }
            _ => {}
        }
        self.state.receive_counter = (self.state.receive_counter + 1) & 7;
        Ok(())
    }
    fn bulk_view(&self, source: BulkSource) -> Result<(&[u8], usize), String> {
        match source {
            BulkSource::Identification => Ok((&self.configuration.identification, 128)),
            BulkSource::Catalog { selector } => self
                .configuration
                .read_catalogs
                .iter()
                .find(|c| c.selector == selector)
                .map(|c| (c.data.as_slice(), CATALOG_BLOCK_BYTES))
                .ok_or_else(|| "X-mount bulk source is not supplied".into()),
        }
    }
    fn bulk_headers(block: usize, count: usize) -> ([u8; 2], [u8; 2]) {
        let input = if block < count - 1 {
            (block + 1) as u16
        } else if block == count - 1 {
            0xffff
        } else {
            0xff00
        };
        let output = if block < count { block as u16 } else { 0xffff };
        (input.to_be_bytes(), output.to_be_bytes())
    }
    fn accept(&mut self, frame: Frame, now: u64) -> Result<u16, String> {
        Self::frame(frame)?;
        if self.state.last_tick.is_some_and(|last| now < last) {
            return Err("X-mount serial clock moved backward".into());
        }
        let next_frames = self
            .state
            .frames
            .checked_add(1)
            .ok_or("X-mount frame counter overflow")?;
        let index = self.state.input.len();
        let received = match self.state.phase.clone() {
            Phase::Header => {
                if index == 0 {
                    self.state.outgoing = Some(self.state.replies.pop_front().unwrap_or(Message {
                        packet: Packet(0),
                        sequence: 0,
                        catalog: None,
                    }));
                }
                let sent = self.state.outgoing.unwrap();
                let byte = sent.packet.bytes()[index];
                self.state.input.push(frame.word as u8);
                if self.state.input.len() == 4 {
                    let packet = Packet::decode(self.state.input.as_slice().try_into().unwrap())?;
                    self.response(packet)?;
                    self.state.phase = Phase::Acknowledgment {
                        received: packet,
                        sent,
                    };
                    self.state.input.clear();
                    self.state.outgoing = None;
                }
                byte
            }
            Phase::Acknowledgment { received, sent } => {
                let output = Packet::encode(
                    0x0800 | ((sent.sequence as u16) << 8),
                    received.family(),
                    true,
                );
                let byte = output.bytes()[index];
                self.state.input.push(frame.word as u8);
                if self.state.input.len() == 4 {
                    let ack = Packet::decode(self.state.input.as_slice().try_into().unwrap())?;
                    if !ack.ack() || ack.family() != sent.packet.family() {
                        return Err("X-mount acknowledgment has no matching sent header".into());
                    }
                    self.deliver(received, ack)?;
                    self.state.input.clear();
                    let source = if sent.packet.family() == IDENTIFICATION {
                        Some(BulkSource::Identification)
                    } else {
                        sent.catalog
                            .map(|selector| BulkSource::Catalog { selector })
                    };
                    self.state.phase = if let Some(source) = source {
                        if received.0 != 0 || !self.state.replies.is_empty() {
                            return Err("X-mount bulk handoff has pending packets".into());
                        }
                        Phase::Bulk { source, block: 0 }
                    } else {
                        Phase::Header
                    };
                }
                byte
            }
            Phase::Bulk { source, block } => {
                let (data, width) = self.bulk_view(source)?;
                let count = data.len() / width;
                let (input, output) = Self::bulk_headers(block, count);
                let payload =
                    &data[block.min(count - 1) * width..(block.min(count - 1) + 1) * width];
                let byte = match index {
                    0..=1 => output[index],
                    i if i < width + 2 => payload[i - 2],
                    _ => output
                        .iter()
                        .chain(payload)
                        .fold(0u8, |sum, b| sum.wrapping_add(*b)),
                };
                let wanted = if index < 2 {
                    input[index]
                } else if index < width + 2 {
                    0
                } else {
                    0x5a
                };
                if frame.word != wanted as u16 {
                    return Err(format!("X-mount bulk request byte {index} is unmodeled"));
                }
                self.state.input.push(frame.word as u8);
                if self.state.input.len() == width + 3 {
                    self.state.input.clear();
                    self.state.phase = if block == count {
                        Phase::Header
                    } else {
                        Phase::Bulk {
                            source,
                            block: block + 1,
                        }
                    };
                }
                byte
            }
        };
        if self.state.replies.len() > 4 {
            return Err("X-mount startup peer did not drain its replies".into());
        }
        self.state.frames = next_frames;
        self.state.last_tick = Some(now);
        Ok(received as u16)
    }
}
impl Link {
    fn validate_state(&self, candidate: &State) -> Result<(), String> {
        if candidate.transmit_counter > 7
            || candidate.receive_counter > 7
            || candidate.control20.is_some_and(|v| v > 1)
            || candidate.staged_control20.is_some_and(|v| v > 1)
            || candidate.last_tick.is_some() != (candidate.frames != 0)
            || candidate.replies.len() > 4
            || candidate
                .replies
                .iter()
                .any(|message| !self.valid_message(*message))
        {
            return Err("invalid X-mount link checkpoint".into());
        }
        match candidate.phase {
            Phase::Header => {
                if candidate.input.len() > 3
                    || candidate.outgoing.is_some() != !candidate.input.is_empty()
                    || candidate
                        .outgoing
                        .is_some_and(|message| !self.valid_message(message))
                {
                    return Err("invalid X-mount partial header".into());
                }
            }
            Phase::Acknowledgment { received, sent } => {
                if candidate.input.len() > 3
                    || candidate.outgoing.is_some()
                    || Packet::decode(received.bytes()).is_err()
                    || self.response(received).is_err()
                    || !self.valid_message(sent)
                {
                    return Err("invalid X-mount partial acknowledgment".into());
                }
            }
            Phase::Bulk { source, block } => {
                let (data, width) = self.bulk_view(source)?;
                let count = data.len() / width;
                if block > count
                    || candidate.input.len() >= width + 3
                    || candidate.outgoing.is_some()
                    || !candidate.replies.is_empty()
                {
                    return Err("invalid X-mount partial bulk transfer".into());
                }
                let (header, _) = Self::bulk_headers(block, count);
                for (index, byte) in candidate.input.iter().enumerate() {
                    if *byte != if index < 2 { header[index] } else { 0 } {
                        return Err("invalid X-mount partial bulk request".into());
                    }
                }
            }
        }
        Ok(())
    }
}
impl Wire for Link {
    fn checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        fujimod_emulator_devices::checkpoint::Checkpoint::encode(
            "x-mount-startup-link",
            &(&self.configuration, &self.state),
        )
    }
    fn validate_checkpoint(&self, saved: &fujimod_emulator_devices::checkpoint::Checkpoint) -> Result<(), String> {
        let (configuration, state): (Configuration, State) =
            saved.decode("x-mount-startup-link")?;
        if configuration != self.configuration
            || !saved.children().is_empty()
            || !saved.blobs().is_empty()
        {
            return Err("X-mount startup checkpoint configuration differs".into());
        }
        self.validate_state(&state)
    }
    fn restore_checkpoint(&mut self, saved: &fujimod_emulator_devices::checkpoint::Checkpoint) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        let (_, state): (Configuration, State) = saved.decode("x-mount-startup-link")?;
        self.state = state;
        Ok(())
    }
    fn validate_pending(
        &self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
        token: u64,
        frame: Frame,
    ) -> Result<(), String> {
        Self::frame(frame)?;
        self.validate_checkpoint(saved)?;
        let (_, state): (Configuration, State) = saved.decode("x-mount-startup-link")?;
        if token != state.frames {
            return Err("X-mount pending serial token differs".into());
        }
        Ok(())
    }

    fn begin(&mut self, frame: Frame) -> Result<u64, String> {
        Self::frame(frame)?;
        Ok(self.state.frames)
    }
    fn finish(&mut self, token: u64, frame: Frame, now: u64) -> Result<u16, String> {
        if token != self.state.frames {
            return Err("X-mount serial frame token differs".into());
        }
        let before = self.state.clone();
        match self.accept(frame, now) {
            Ok(value) => Ok(value),
            Err(error) => {
                self.state = before;
                Err(error)
            }
        }
    }
    fn save(&self) -> Value {
        json!({"kind":"x-mount-startup-link","configuration":self.configuration,"state":self.state})
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let candidate: State =
            serde_json::from_value(value["state"].clone()).map_err(|e| e.to_string())?;
        if value["kind"] != "x-mount-startup-link"
            || value["configuration"] != json!(self.configuration)
        {
            return Err("X-mount startup checkpoint configuration differs".into());
        }
        self.validate_state(&candidate)?;
        self.state = candidate;
        Ok(())
    }
}
#[cfg(test)]
mod tests;
