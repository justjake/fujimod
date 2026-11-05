//! MT5931 HIF command/event ABI and one open virtual access point.
//! Protocol references are in README.md. The endpoint has no RF peers unless
//! an access point is configured, and it consumes only register and FIFO
//! transactions.
use crate::wifi_module::virtual_network::{AccessPoint, Network};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

const EVENT_COMMAND_RESULT: u8 = 0x01;
const EVENT_CAPABILITY: u8 = 0x02;
const EVENT_LINK_QUALITY: u8 = 0x05;
const EVENT_BASIC_CONFIG: u8 = 0x09;
const EVENT_SCAN_DONE: u8 = 0x0f;
const EVENT_SCAN_CHANGE: u8 = 0x15;
const EVENT_TX_DONE: u8 = 0x17;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub mac_address: [u8; 6],
    #[serde(default)]
    pub access_point: Option<AccessPoint>,
    /// Explicit boot capability body. Absence refuses command 0x80.
    #[serde(default)]
    pub capability_reply: Option<[u8; 20]>,
}

impl Options {
    pub fn validate(&self) -> Result<(), String> {
        if self.mac_address == [0; 6] || self.mac_address[0] & 1 != 0 {
            return Err("MT5931 permanent MAC must be a nonzero unicast address".into());
        }
        if let Some(access_point) = &self.access_point {
            access_point.validate()?;
        }
        Ok(())
    }
}

#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Station {
    channel: Option<u8>,
    bssid: Option<[u8; 6]>,
    address: Option<[u8; 6]>,
    selector: u8,
    associated: bool,
}

#[derive(Default, Serialize, Deserialize)]
pub struct Endpoint {
    mac_address: [u8; 6],
    #[serde(default)]
    access_point: Option<AccessPoint>,
    #[serde(default)]
    capability_reply: Option<[u8; 20]>,
    control: u32,
    interrupt_enable: u32,
    interrupt_status: u32,
    interrupt_output: bool,
    tx_released: [u8; 8],
    rx0: VecDeque<Vec<u8>>,
    rx1: VecDeque<Vec<u8>>,
    station: Station,
    network: Network,
    wlan_ready: bool,
    ownership: u32,
    commands: u64,
    ticks: u64,
    last_command: (u8, u8),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Change {
    Control(u32),
    InterruptEnable(u32),
    LowPower(u32),
    ClearInterrupt(u32),
    ClearTx(usize),
    Receive,
    Response {
        class: u8,
        frame_kind: u8,
        events: Vec<Vec<u8>>,
        frames: Vec<Vec<u8>>,
    },
}

impl Endpoint {
    pub fn new(options: Option<&Options>) -> Self {
        Self {
            mac_address: options.map_or([0; 6], |options| options.mac_address),
            access_point: options.and_then(|options| options.access_point.clone()),
            capability_reply: options.and_then(|options| options.capability_reply),
            ..Self::default()
        }
    }

    pub fn fingerprint(&self) -> serde_json::Value {
        serde_json::json!({
            "mac_address": self.mac_address,
            "access_point": self.access_point,
            "capability_reply": self.capability_reply,
            "control": self.control,
            "interrupt_enable": self.interrupt_enable,
            "interrupt_status": self.interrupt_status,
            "interrupt_output": self.interrupt_output,
            "tx_released": self.tx_released,
            "rx0": self.rx0.iter().map(|packet| fujimod_emulator_devices::fingerprint::bytes(packet)).collect::<Vec<_>>(),
            "rx1": self.rx1.iter().map(|packet| fujimod_emulator_devices::fingerprint::bytes(packet)).collect::<Vec<_>>(),
            "station": {"channel": self.station.channel, "bssid": self.station.bssid,
                "address": self.station.address, "selector": self.station.selector,
                "associated": self.station.associated},
            "network": self.network.fingerprint(),
            "wlan_ready": self.wlan_ready,
            "ownership": self.ownership,
            "commands": self.commands,
            "ticks": self.ticks,
            "last_command": self.last_command,
        })
    }

    pub fn interrupt(&self) -> bool {
        self.interrupt_output && self.interrupt_status & self.interrupt_enable != 0
    }

    pub fn low_power(&self, value: u32) -> Result<Change, String> {
        if value & !0x303 != 0 {
            return Err(format!("MT5931 unsupported WHLPCR bits {value:#x}"));
        }
        Ok(Change::LowPower(value))
    }

    pub fn transfer(
        &mut self,
        address: u32,
        write: bool,
        payload: &[u8],
        quantity: usize,
    ) -> Result<(Vec<u8>, Option<Change>), String> {
        if address == 0x2c && write {
            return self.transmit(payload, quantity);
        }
        if address == 0x34 && !write {
            if self.rx1.is_empty() {
                self.poll_network();
            }
            let packet = self.rx1.front().ok_or("MT5931 RX1 FIFO underflow")?;
            if quantity != packet.len().div_ceil(4) * 4 {
                return Err(format!(
                    "MT5931 RX1 extent {quantity} does not match packet {}",
                    packet.len()
                ));
            }
            let mut data = packet.clone();
            data.resize(quantity, 0);
            return Ok((data, Some(Change::Receive)));
        }
        if address == 0x30 && !write {
            let packet = self.rx0.front().ok_or("MT5931 RX0 FIFO underflow")?;
            if quantity != packet.len().div_ceil(4) * 4 {
                return Err("MT5931 RX0 extent does not match packet".into());
            }
            let mut data = packet.clone();
            data.resize(quantity, 0);
            return Ok((data, Some(Change::Receive)));
        }
        // The stock request-flag reader samples the 0x58-byte window at 0x10.
        // The first word is WHISR; the two TX status words follow it. Stock's
        // table-driven action consumes those bits and then clears the source
        // registers through their own W1C/read-clear accesses.
        if address == 0x10 && !write && quantity == 0x58 {
            let mut block = vec![0u8; 0x58];
            block[..4].copy_from_slice(&self.interrupt_status.to_le_bytes());
            block[4..8].copy_from_slice(&self.tx_released[..4]);
            block[8..12].copy_from_slice(&self.tx_released[4..]);
            return Ok((block, None));
        }
        if quantity != 4 || write && payload.len() != 4 {
            return Err(format!(
                "MT5931 unsupported register/FIFO extent address={address:#x} write={write} bytes={quantity}"
            ));
        }
        if write {
            let value = u32::from_le_bytes(payload.try_into().unwrap());
            let change = match address {
                4 => self.low_power(value)?,
                0xc if value & !6 == 0 => Change::Control(value),
                0x14 => Change::InterruptEnable(value),
                0x10 if self.control & 2 != 0 => Change::ClearInterrupt(value),
                _ => {
                    return Err(format!(
                        "MT5931 unsupported register write {address:#x}={value:#x}"
                    ));
                }
            };
            return Ok((Vec::new(), Some(change)));
        }
        let (value, change) = match address {
            0 => (
                (0x205931 & !0x200000) | if self.wlan_ready { 0x200000 } else { 0 },
                None,
            ),
            4 => (
                0x100 | u32::from(self.interrupt_output) | self.ownership,
                None,
            ),
            0xc => (self.control, None),
            0x14 => (self.interrupt_enable, None),
            0x10 => (
                self.interrupt_status,
                (self.control & 2 == 0).then_some(Change::ClearInterrupt(self.interrupt_status)),
            ),
            0x20 | 0x24 => {
                let index = (address - 0x20) as usize;
                (
                    u32::from_le_bytes(self.tx_released[index..index + 4].try_into().unwrap()),
                    Some(Change::ClearTx(index)),
                )
            }
            0x50 => {
                let rx0 = self.rx0.front().map_or(0, Vec::len) as u32;
                let rx1 = self.rx1.front().map_or(0, Vec::len) as u32;
                (rx0 | rx1 << 16, None)
            }
            _ => return Err(format!("MT5931 unsupported register read {address:#x}")),
        };
        Ok((value.to_le_bytes().to_vec(), change))
    }

    fn poll_network(&mut self) {
        let Some(access_point) = self.access_point.clone() else {
            return;
        };
        let frames = self.network.poll(&access_point, self.ticks);
        for frame in frames {
            self.push_frame(0, &frame);
        }
    }

    fn push_frame(&mut self, kind: u8, frame: &[u8]) {
        if self.rx1.len() >= 16 {
            return;
        }
        let length = (12 + frame.len()) as u16;
        let mut packet = vec![0u8; 12 + frame.len()];
        packet[..2].copy_from_slice(&length.to_le_bytes());
        packet[2] = kind;
        packet[8] = self.station.selector;
        packet[9] = self.access_point.as_ref().map_or(0, |ap| ap.rcpi);
        packet[12..].copy_from_slice(frame);
        let padded = packet.len().div_ceil(4) * 4;
        packet.resize(padded + 4, 0);
        self.rx1.push_back(packet);
        self.interrupt_status |= 4;
    }

    fn push_event(&mut self, event: Vec<u8>) {
        if self.rx1.len() < 16 {
            self.rx1.push_back(event);
            self.interrupt_status |= 4;
        }
    }

    fn event(id: u8, sequence: u8, body: &[u8]) -> Vec<u8> {
        let mut result = Vec::with_capacity(8 + body.len());
        result.extend_from_slice(&((8 + body.len()) as u16).to_le_bytes());
        result.extend_from_slice(&[1, 0, id, sequence, 0, 0]);
        result.extend_from_slice(body);
        result
    }

    fn command_result(cid: u8, sequence: u8) -> Vec<u8> {
        Self::event(EVENT_COMMAND_RESULT, sequence, &[cid, 0, 0, 0])
    }

    fn transmit(
        &mut self,
        packet: &[u8],
        quantity: usize,
    ) -> Result<(Vec<u8>, Option<Change>), String> {
        if packet.len() != quantity || quantity < 8 {
            return Err("MT5931 truncated HIF header".into());
        }
        let length = (u16::from_le_bytes(packet[..2].try_into().unwrap()) & 0xfff) as usize;
        if length < 8 || length > quantity || quantity != length.div_ceil(4) * 4 {
            return Err(format!(
                "MT5931 unsupported HIF packet length {length}/{quantity}"
            ));
        }
        let packet_type = packet[3] >> 6;
        if packet_type == 1 {
            if packet[3] != 0x50 || packet[5] > 1 {
                return Err(format!(
                    "MT5931 unsupported command framing {:02x?}",
                    &packet[..8]
                ));
            }
            return self.command(&packet[..length], length);
        }
        if packet_type != 0 && packet_type != 3 {
            return Err(format!("MT5931 unsupported HIF packet type {packet_type}"));
        }
        if length < 16 {
            return Err("MT5931 transmit header truncated".into());
        }
        let frame = &packet[16..length];
        let class = (packet[3] >> 2) & 0xf;
        let selector = packet[10];
        let mut frames = Vec::new();
        let frame_kind = if packet_type == 0 {
            if self.access_point.is_some() {
                let access_point = self.access_point.clone().unwrap();
                frames = self.network.receive(&access_point, frame, self.ticks);
            }
            0
        } else {
            frames = self.management(frame);
            3
        };
        let mut body = [0u8; 16];
        body[0] = selector;
        body[1] = 0;
        let event = Self::event(EVENT_TX_DONE, 0, &body);
        Ok((
            Vec::new(),
            Some(Change::Response {
                class,
                frame_kind,
                events: vec![event],
                frames,
            }),
        ))
    }

    fn management(&mut self, frame: &[u8]) -> Vec<Vec<u8>> {
        let Some(access_point) = self.access_point.clone() else {
            return Vec::new();
        };
        if frame.len() < 24 {
            return Vec::new();
        }
        let control = u16::from_le_bytes([frame[0], frame[1]]);
        match control & 0xfc {
            0xb0 => self.authentication(&access_point, frame),
            0x00 => self.association(&access_point, frame),
            0x40 => self.probe_response(&access_point, frame),
            0xc0 => {
                self.station.associated = false;
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn authentication(&mut self, access_point: &AccessPoint, frame: &[u8]) -> Vec<Vec<u8>> {
        if frame.len() < 30 {
            return Vec::new();
        }
        let destination: [u8; 6] = frame[10..16].try_into().unwrap();
        let bssid: [u8; 6] = frame[16..22].try_into().unwrap();
        if bssid != access_point.bssid {
            return Vec::new();
        }
        let algorithm = u16::from_le_bytes([frame[24], frame[25]]);
        let status = if algorithm == 0 { 0u16 } else { 13 };
        let mut reply = vec![0u8; 30];
        reply[..2].copy_from_slice(&0x00b0u16.to_le_bytes());
        reply[4..10].copy_from_slice(&destination);
        reply[10..16].copy_from_slice(&access_point.bssid);
        reply[16..22].copy_from_slice(&access_point.bssid);
        reply[24..26].copy_from_slice(&algorithm.to_le_bytes());
        reply[26..28].copy_from_slice(&2u16.to_le_bytes());
        reply[28..30].copy_from_slice(&status.to_le_bytes());
        self.station.address = Some(destination);
        self.station.bssid = Some(access_point.bssid);
        vec![reply]
    }

    fn association(&mut self, access_point: &AccessPoint, frame: &[u8]) -> Vec<Vec<u8>> {
        if frame.len() < 28 {
            return Vec::new();
        }
        let destination: [u8; 6] = frame[10..16].try_into().unwrap();
        let bssid: [u8; 6] = frame[16..22].try_into().unwrap();
        let status = if bssid == access_point.bssid { 0u16 } else { 1 };
        let mut reply = vec![0u8; 30];
        reply[..2].copy_from_slice(&0x0010u16.to_le_bytes());
        reply[4..10].copy_from_slice(&destination);
        reply[10..16].copy_from_slice(&access_point.bssid);
        reply[16..22].copy_from_slice(&access_point.bssid);
        reply[24..26].copy_from_slice(&0x0001u16.to_le_bytes());
        reply[26..28].copy_from_slice(&status.to_le_bytes());
        reply[28..30].copy_from_slice(&1u16.to_le_bytes());
        reply.extend_from_slice(&[1, 8, 0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24]);
        self.station.address = Some(destination);
        self.station.bssid = Some(access_point.bssid);
        self.station.associated = status == 0;
        vec![reply]
    }

    fn probe_response(&mut self, access_point: &AccessPoint, frame: &[u8]) -> Vec<Vec<u8>> {
        let destination: [u8; 6] = frame[10..16].try_into().unwrap_or([255; 6]);
        vec![beacon(access_point, destination, 0x0050)]
    }

    fn command(
        &mut self,
        packet: &[u8],
        quantity: usize,
    ) -> Result<(Vec<u8>, Option<Change>), String> {
        let cid = packet[4];
        let set = packet[5] == 1;
        let sequence = packet[6];
        let body = &packet[8..];
        if self.rx1.len() >= 16 || self.tx_released[4] == u8::MAX {
            return Err("MT5931 command/RX queue capacity exceeded".into());
        }
        let mut events = Vec::new();
        match cid {
            0x01 if body.len() == 12 => events.push(Self::command_result(cid, sequence)),
            0x04 if body.len() >= 0xdc => {
                let (scan_events, frames) = self.scan(body, sequence, true);
                events = scan_events;
                return Ok((
                    Vec::new(),
                    Some(Change::Response {
                        class: 4,
                        frame_kind: 3,
                        events,
                        frames,
                    }),
                ));
            }
            0x05 if body.len() == 4 => {
                self.wlan_ready = false;
            }
            0x06 if body.len() == 4 => events.push(Self::command_result(cid, sequence)),
            0x08 if body.len() == 64 => events.push(Self::command_result(cid, sequence)),
            0x0a if body.is_empty() => events.push(Self::command_result(cid, sequence)),
            0x0b if body.len() == 4 => events.push(Self::command_result(cid, sequence)),
            0x13 if body.len() == 56 => events.push(Self::command_result(cid, sequence)),
            0x15 if body.len() >= 2 => {
                self.station.channel = (body[1] != 0).then_some(body[0]);
                events.push(Self::command_result(cid, sequence));
            }
            0x16 if body.len() == 80 => {
                self.station.channel = Some(body[0]);
                self.station.bssid = Some(body[36..42].try_into().unwrap());
                events.push(Self::command_result(cid, sequence));
            }
            0x17 if body.len() == 40 => {
                self.station.selector = body[0];
                self.station.address = Some(body[2..8].try_into().unwrap_or([0; 6]));
                events.push(Self::command_result(cid, sequence));
            }
            0x18 if body.len() == 8 => {
                self.station.associated = false;
                events.push(Self::command_result(cid, sequence));
            }
            0x19 | 0x1a | 0x1b | 0x1c | 0x1d | 0x1f | 0x20 | 0x21 | 0x28 | 0x2b | 0x3a | 0xc3
                if cid == 0x1c
                    || matches!(
                        (cid, body.len()),
                        (0x19, 8)
                            | (0x1a, 12)
                            | (0x1b, 4)
                            | (0x1d, 16)
                            | (0x1f, 4)
                            | (0x20, 20)
                            | (0x21, 44)
                            | (0x28, 40)
                            | (0x2b, 4)
                            | (0x3a, 12)
                            | (0xc3, 200)
                    ) =>
            {
                events.push(Self::command_result(cid, sequence));
            }
            0x1e if body.len() >= 0x6e => {
                let (scan_events, frames) = self.scan(body, sequence, false);
                events = scan_events;
                return Ok((
                    Vec::new(),
                    Some(Change::Response {
                        class: 4,
                        frame_kind: 3,
                        events,
                        frames,
                    }),
                ));
            }
            0x30 if body.len() == 4 => {}
            0x31 if body.len() == 144 => {}
            0x36 if body.len() == 4 => {}
            0x80 if !set => {
                let Some(capability) = self.capability_reply.filter(|_| body.len() == 20) else {
                    return Err(format!(
                        "MT5931 NIC capability reply unqualified: CID=0x80 sequence={sequence} query_body_bytes={};",
                        body.len()
                    ));
                };
                events.push(Self::event(EVENT_CAPABILITY, sequence, &capability));
            }
            0x81 => {
                let rcpi = self.access_point.as_ref().map_or(0, |ap| ap.rcpi);
                events.push(Self::event(
                    EVENT_LINK_QUALITY,
                    sequence,
                    &u32::from(rcpi).to_le_bytes(),
                ));
            }
            0xc1 if body.len() == 12 => {
                if set {
                    self.station.address = Some(body[..6].try_into().unwrap());
                } else {
                    let mut config = [0u8; 12];
                    config[..6].copy_from_slice(&self.station.address.unwrap_or(self.mac_address));
                    events.push(Self::event(EVENT_BASIC_CONFIG, sequence, &config));
                }
            }
            0xc6 if body.len() == 264 => {}
            0xf1 if body.len() == 24 => {}
            0xf2 if body.len() == 8 => {}
            _ => {
                return Err(format!(
                    "MT5931 command not modeled CID={cid:#04x} set={set} sequence={sequence} body={:02x?}",
                    body
                ));
            }
        }
        let _ = quantity;
        self.last_command = (cid, sequence);
        Ok((
            Vec::new(),
            Some(Change::Response {
                class: 4,
                frame_kind: 0,
                events,
                frames: Vec::new(),
            }),
        ))
    }

    fn scan(&mut self, body: &[u8], sequence: u8, v2: bool) -> (Vec<Vec<u8>>, Vec<Vec<u8>>) {
        let Some(access_point) = self.access_point.clone() else {
            return (
                vec![Self::event(EVENT_SCAN_DONE, sequence, &[0; 96])],
                Vec::new(),
            );
        };
        let mut matches = true;
        if v2 {
            matches = body
                .windows(access_point.ssid.len())
                .any(|window| window == access_point.ssid.as_bytes());
        } else {
            let length = body.get(4).copied().unwrap_or(0) as usize;
            let ssid = body.get(8..40).unwrap_or(&[]);
            if length != 0 && length <= 32 {
                matches = access_point.ssid.as_bytes().starts_with(&ssid[..length]);
            }
            let channel_count = body.get(43).copied().unwrap_or(0) as usize;
            if channel_count != 0 {
                let channels = body
                    .get(44..44 + channel_count.saturating_mul(2))
                    .unwrap_or(&[]);
                let (pairs, _) = channels.as_chunks::<2>();
                matches &= pairs.iter().any(|entry| entry[0] == access_point.channel);
            }
        }
        let frames = if matches {
            let beacon_frame = beacon(&access_point, [255; 6], 0x0080);
            let probe_frame = beacon(&access_point, [255; 6], 0x0050);
            vec![beacon_frame, probe_frame]
        } else {
            Vec::new()
        };
        let mut info = [0u8; 96];
        info[0] = access_point.channel;
        let mut events = vec![
            Self::event(EVENT_SCAN_DONE, sequence, &info),
            Self::event(
                EVENT_SCAN_CHANGE,
                sequence,
                &[access_point.channel, 1, 0, 0],
            ),
        ];
        if !matches {
            events.truncate(1);
        }
        (events, frames)
    }

    pub fn commit(&mut self, change: Change) {
        match change {
            Change::Control(value) => self.control = value,
            Change::InterruptEnable(value) => {
                self.interrupt_enable = value;
                self.wlan_ready = true;
            }
            Change::LowPower(value) => {
                self.ownership = value & 0x300;
                if value & 1 != 0 {
                    self.interrupt_output = true;
                }
                if value & 2 != 0 {
                    self.interrupt_output = false;
                }
            }
            Change::ClearInterrupt(mask) => self.interrupt_status &= !mask,
            Change::ClearTx(index) => self.tx_released[index..index + 4].fill(0),
            Change::Receive => {
                self.rx1.pop_front();
            }
            Change::Response {
                class,
                frame_kind,
                events,
                frames,
            } => {
                self.commands += 1;
                let credit = self.tx_released.get_mut(class as usize);
                if let Some(credit) = credit {
                    *credit = credit.saturating_add(1);
                }
                self.interrupt_status |= 1;
                for frame in frames {
                    self.push_frame(frame_kind, &frame);
                }
                for event in events {
                    self.push_event(event);
                }
            }
        }
    }
}

fn beacon(access_point: &AccessPoint, destination: [u8; 6], control: u16) -> Vec<u8> {
    let mut frame = vec![0u8; 36];
    frame[..2].copy_from_slice(&control.to_le_bytes());
    frame[4..10].copy_from_slice(&destination);
    frame[10..16].copy_from_slice(&access_point.bssid);
    frame[16..22].copy_from_slice(&access_point.bssid);
    frame[32..34].copy_from_slice(&100u16.to_le_bytes());
    frame[34..36].copy_from_slice(&0x0001u16.to_le_bytes());
    let mut elements = Vec::new();
    elements.push(0);
    elements.push(access_point.ssid.len() as u8);
    elements.extend_from_slice(access_point.ssid.as_bytes());
    elements.extend_from_slice(&[1, 8, 0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24]);
    elements.extend_from_slice(&[3, 1, access_point.channel]);
    elements.extend_from_slice(&[50, 4, 0x30, 0x48, 0x60, 0x6c]);
    frame.extend_from_slice(&elements);
    frame
}

#[cfg(test)]
mod tests {
    use super::*;

    fn access_point() -> AccessPoint {
        AccessPoint {
            ssid: "Camera OTA".into(),
            bssid: [2, 0, 0, 0, 0, 1],
            channel: 6,
            rcpi: 180,
            gateway: [192, 168, 4, 1],
            dns_servers: [[192, 168, 4, 1], [192, 168, 4, 2]],
            hostname: "ota.test".into(),
            host: [203, 0, 113, 10],
            host_port: 443,
            local_port: 8080,
        }
    }

    fn options() -> Options {
        Options {
            mac_address: [2, 3, 4, 5, 6, 7],
            access_point: Some(access_point()),
            capability_reply: None,
        }
    }

    fn endpoint() -> Endpoint {
        let mut endpoint = Endpoint::new(Some(&options()));
        endpoint.commit(Change::InterruptEnable(0xffffff0f));
        endpoint
    }

    fn command(id: u8, set: bool, sequence: u8, body: &[u8]) -> Vec<u8> {
        let mut packet = ((8 + body.len()) as u16).to_le_bytes().to_vec();
        packet.extend_from_slice(&[0, 0x50, id, u8::from(set), sequence, 0xa5]);
        packet.extend_from_slice(body);
        packet.resize(packet.len().div_ceil(4) * 4, 0xa5);
        packet
    }

    fn transfer(
        e: &mut Endpoint,
        address: u32,
        write: bool,
        payload: &[u8],
        quantity: usize,
    ) -> Vec<u8> {
        let (data, change) = e.transfer(address, write, payload, quantity).unwrap();
        if let Some(change) = change {
            e.commit(change);
        }
        data
    }

    fn read(e: &mut Endpoint, address: u32) -> u32 {
        u32::from_le_bytes(transfer(e, address, false, &[], 4).try_into().unwrap())
    }

    #[test]
    fn stock_v2_scan_body_selects_the_configured_beacon() {
        // First 18 bytes are the exact stock channel_submit_scan_start
        // (0x1D9F464) body for one name and SSID "Camera OTA"; the captured
        // request leaves the remaining channel and IE fields empty.
        let mut body = vec![0u8; 0xdc];
        body[..4].copy_from_slice(&[1, 0, 0, 0]);
        body[4..8].copy_from_slice(&10u32.to_le_bytes());
        body[8..18].copy_from_slice(b"Camera OTA");
        let mut e = endpoint();
        let packet = command(0x04, true, 0x41, &body);
        transfer(&mut e, 0x2c, true, &packet, packet.len());
        let beacon = read_rx1(&mut e);
        let frame = &beacon[12..12 + (u16::from_le_bytes([beacon[0], beacon[1]]) as usize - 12)];
        assert_eq!(u16::from_le_bytes([frame[0], frame[1]]) & 0xfc, 0x80);
        assert_eq!(&frame[36..38], &[0, 10]);
        assert_eq!(&frame[38..48], b"Camera OTA");
    }

    fn read_rx1(e: &mut Endpoint) -> Vec<u8> {
        let length = read(e, 0x50) >> 16;
        assert_ne!(length, 0);
        transfer(e, 0x34, false, &[], length as usize)
    }

    fn transmit(e: &mut Endpoint, kind: u8, frame: &[u8]) {
        let mut packet = vec![0u8; 16 + frame.len()];
        let length = (packet.len() as u16) & 0xfff;
        packet[..2].copy_from_slice(&length.to_le_bytes());
        packet[3] = kind << 6;
        packet[4] = if kind == 3 { 0x18 } else { 14 };
        packet[8] = 0;
        packet[10] = 0;
        packet[16..].copy_from_slice(frame);
        packet.resize(packet.len().div_ceil(4) * 4, 0);
        let (_, change) = e.transfer(0x2c, true, &packet, packet.len()).unwrap();
        e.commit(change.unwrap());
    }

    #[test]
    fn stock_frames_scan_authenticate_associate_and_get_a_lease() {
        let mut e = endpoint();
        let mut body = vec![0u8; 0x6e];
        body[4] = 10;
        body[8..18].copy_from_slice(b"Camera OTA");
        body[43] = 1;
        body[44] = 6;
        let packet = command(0x1e, true, 0x31, &body);
        transfer(&mut e, 0x2c, true, &packet, packet.len());
        let beacon = read_rx1(&mut e);
        assert_eq!(beacon[2], 3);
        let beacon_frame =
            &beacon[12..12 + (u16::from_le_bytes([beacon[0], beacon[1]]) as usize - 12)];
        assert_eq!(
            u16::from_le_bytes([beacon_frame[0], beacon_frame[1]]) & 0xfc,
            0x80
        );
        assert_eq!(&beacon_frame[10..16], &access_point().bssid);
        assert_eq!(&beacon_frame[36..38], &[0, 10]);
        assert_eq!(&beacon_frame[38..48], b"Camera OTA");
        let _probe = read_rx1(&mut e);
        let done = read_rx1(&mut e);
        assert_eq!(done[4], EVENT_SCAN_DONE);
        let change = read_rx1(&mut e);
        assert_eq!(change[4], EVENT_SCAN_CHANGE);

        let station = [2, 0, 0, 0, 0, 9];
        let mut auth = vec![0u8; 30];
        auth[..2].copy_from_slice(&0x00b0u16.to_le_bytes());
        auth[4..10].copy_from_slice(&access_point().bssid);
        auth[10..16].copy_from_slice(&station);
        auth[16..22].copy_from_slice(&access_point().bssid);
        auth[26..28].copy_from_slice(&1u16.to_le_bytes());
        transmit(&mut e, 3, &auth);
        let reply = read_rx1(&mut e);
        let reply_frame = &reply[12..12 + (u16::from_le_bytes([reply[0], reply[1]]) as usize - 12)];
        assert_eq!(u16::from_le_bytes([reply_frame[0], reply_frame[1]]), 0x00b0);
        assert_eq!(&reply_frame[4..10], &station);
        assert_eq!(u16::from_le_bytes([reply_frame[26], reply_frame[27]]), 2);
        assert_eq!(u16::from_le_bytes([reply_frame[28], reply_frame[29]]), 0);
        assert_eq!(read_rx1(&mut e)[4], EVENT_TX_DONE);

        let mut assoc = vec![0u8; 24];
        assoc[4..10].copy_from_slice(&access_point().bssid);
        assoc[10..16].copy_from_slice(&station);
        assoc[16..22].copy_from_slice(&access_point().bssid);
        assoc.extend_from_slice(&[1, 0]);
        assoc.extend_from_slice(&[10, 0]);
        assoc.extend_from_slice(&[0, 10]);
        assoc.extend_from_slice(b"Camera OTA");
        assoc.extend_from_slice(&[1, 4, 0x82, 0x84, 0x8b, 0x96]);
        transmit(&mut e, 3, &assoc);
        let reply = read_rx1(&mut e);
        let reply_frame = &reply[12..12 + (u16::from_le_bytes([reply[0], reply[1]]) as usize - 12)];
        assert_eq!(u16::from_le_bytes([reply_frame[0], reply_frame[1]]), 0x0010);
        assert_eq!(u16::from_le_bytes([reply_frame[26], reply_frame[27]]), 0);
        assert!(e.station.associated);
        assert_eq!(read_rx1(&mut e)[4], EVENT_TX_DONE);

        let dhcp = crate::wifi_module::virtual_network::test_dhcp_discover(station);
        transmit(&mut e, 0, &dhcp);
        let reply = read_rx1(&mut e);
        assert_eq!(reply[2], 0);
        let frame = &reply[12..12 + (u16::from_le_bytes([reply[0], reply[1]]) as usize - 12)];
        let ethernet = smoltcp::wire::EthernetFrame::new_checked(frame).unwrap();
        assert_eq!(ethernet.ethertype(), smoltcp::wire::EthernetProtocol::Ipv4);
        let ip = smoltcp::wire::Ipv4Packet::new_checked(ethernet.payload()).unwrap();
        let udp = smoltcp::wire::UdpPacket::new_checked(ip.payload()).unwrap();
        assert_eq!(udp.src_port(), 67);
        let dhcp_packet = smoltcp::wire::DhcpPacket::new_checked(udp.payload()).unwrap();
        let dhcp_reply = smoltcp::wire::DhcpRepr::parse(&dhcp_packet).unwrap();
        assert_eq!(
            dhcp_reply.message_type,
            smoltcp::wire::DhcpMessageType::Offer
        );
        assert_eq!(dhcp_reply.your_ip.octets(), [192, 168, 4, 100]);
        assert_eq!(read_rx1(&mut e)[4], EVENT_TX_DONE);
    }

    #[test]
    fn oscillator_and_basic_config_use_wire_sequence_and_opaque_reserved_bytes() {
        let mut e = endpoint();
        let packet = command(0xf2, true, 0xff, &[0xa5; 8]);
        let (_, effect) = e.transfer(0x2c, true, &packet, packet.len()).unwrap();
        assert_eq!(e.commands, 0);
        assert_eq!(read(&mut e, 0x50), 0);
        e.commit(effect.unwrap());
        assert_eq!(read(&mut e, 0x50), 0);
        let packet = command(0xc1, false, 0x72, &[0; 12]);
        transfer(&mut e, 0x2c, true, &packet, 20);
        assert_eq!(read(&mut e, 0x50), 20 << 16);
        let response = transfer(&mut e, 0x34, false, &[], 20);
        assert_eq!(&response[..8], &[20, 0, 1, 0, 9, 0x72, 0, 0]);
        assert_eq!(&response[8..14], &options().mac_address);
        assert!(
            e.transfer(0x34, false, &[], 20)
                .unwrap_err()
                .contains("underflow")
        );
        assert_eq!(read(&mut e, 0x24), 2);
        assert_eq!(read(&mut e, 0x24), 0);
    }

    #[test]
    fn fifo_order_masks_read_clear_and_write_one_clear() {
        let mut e = endpoint();
        for seq in [9, 2] {
            let packet = command(0xc1, false, seq, &[0; 12]);
            transfer(&mut e, 0x2c, true, &packet, packet.len());
        }
        assert!(!e.interrupt());
        transfer(&mut e, 0x14, true, &5u32.to_le_bytes(), 4);
        transfer(&mut e, 4, true, &1u32.to_le_bytes(), 4);
        assert!(e.interrupt());
        transfer(&mut e, 0xc, true, &2u32.to_le_bytes(), 4);
        assert_eq!(read(&mut e, 0x10), 5);
        assert_eq!(read(&mut e, 0x10), 5);
        transfer(&mut e, 0x10, true, &1u32.to_le_bytes(), 4);
        assert_eq!(read(&mut e, 0x10), 4);
        transfer(&mut e, 0xc, true, &0u32.to_le_bytes(), 4);
        assert_eq!(read(&mut e, 0x10), 4);
        assert!(!e.interrupt());
        assert_eq!(read(&mut e, 0x10), 0);
        assert_eq!(transfer(&mut e, 0x34, false, &[], 20)[5], 9);
        assert_eq!(transfer(&mut e, 0x34, false, &[], 20)[5], 2);
        assert_eq!(read(&mut e, 0x50), 0);
    }

    #[test]
    fn capability_and_unknown_commands_refuse_without_effects() {
        let mut e = endpoint();
        for (id, set, body_length, expected) in [
            (0x80, false, 20, "capability reply unqualified"),
            (0xee, true, 4, "CID=0xee"),
        ] {
            let before = serde_json::to_vec(&e).unwrap();
            let packet = command(id, set, 3, &vec![0; body_length]);
            assert!(
                e.transfer(0x2c, true, &packet, packet.len())
                    .unwrap_err()
                    .contains(expected)
            );
            assert_eq!(serde_json::to_vec(&e).unwrap(), before);
        }
        let packet = command(0xc1, false, 5, &[0; 12]);
        for (index, value) in [(0, 0), (3, 0x80), (5, 2)] {
            let mut bad = packet.clone();
            bad[index] = value;
            assert!(e.transfer(0x2c, true, &bad, bad.len()).is_err());
        }
        transfer(&mut e, 0x2c, true, &packet, 20);
        let before = serde_json::to_vec(&e).unwrap();
        assert!(e.transfer(0x34, false, &[], 16).is_err());
        assert_eq!(serde_json::to_vec(&e).unwrap(), before);
    }

    #[test]
    fn scan_returns_the_configured_beacon_and_done_events() {
        let mut e = endpoint();
        let mut body = vec![0u8; 0x6e];
        body[4] = 10;
        body[8..18].copy_from_slice(b"Camera OTA");
        body[43] = 1;
        body[44] = 6;
        let packet = command(0x1e, true, 0x31, &body);
        let (_, effect) = e.transfer(0x2c, true, &packet, packet.len()).unwrap();
        e.commit(effect.unwrap());
        let mut kinds = Vec::new();
        let mut kinds_events = Vec::new();
        for _ in 0..4 {
            let length = read(&mut e, 0x50) >> 16;
            assert_ne!(length, 0);
            let packet = transfer(&mut e, 0x34, false, &[], length as usize);
            kinds.push(packet[2]);
            if packet[2] == 1 {
                kinds_events.push(packet[4]);
            }
        }
        assert_eq!(kinds, vec![3, 3, 1, 1]);
        assert_eq!(kinds_events, vec![EVENT_SCAN_DONE, EVENT_SCAN_CHANGE]);
    }

    #[test]
    fn binary_checkpoint_preserves_pending_fifo_and_inspection_is_observational() {
        let mut e = endpoint();
        let packet = command(0xc1, false, 11, &[0; 12]);
        transfer(&mut e, 0x2c, true, &packet, 20);
        let before = serde_json::to_vec(&e).unwrap();
        assert_eq!(e.fingerprint(), e.fingerprint());
        assert_eq!(before, serde_json::to_vec(&e).unwrap());
        let mut encoded = Vec::new();
        ciborium::into_writer(&e, &mut encoded).unwrap();
        let mut restored: Endpoint = ciborium::from_reader(encoded.as_slice()).unwrap();
        assert_eq!(e.fingerprint(), restored.fingerprint());
        assert_eq!(
            transfer(&mut e, 0x34, false, &[], 20),
            transfer(&mut restored, 0x34, false, &[], 20)
        );
    }
}
