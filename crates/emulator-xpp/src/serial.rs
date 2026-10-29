//! X-Processor Pro serial configuration and a provisional discovery endpoint.
//!
//! On the discovery endpoint, +0x28 holds flags; +0x2c clears written bits.
//! A send word written to +0x60 completes at once and sets done bit 2. Bit 3
//! reads set because the send queue never fills. The receive word +0x48
//! reads zero. Other words retain writes. No timing or IRQ is modeled; this
//! is a provisional law for discovery.
//!
//! A port may carry a declared register-file peer: each 16-bit send word names
//! register bits 14..8, bit 15 writes the low byte into it, and the word read
//! back from +0x60 is that register's value. Without a peer, +0x60 reads the
//! last send word.
//!
//! A register-file peer may keep a clock: four registers holding a
//! little-endian seconds count that advances with device time. Writing 0 to
//! its hold register stops the count and writing 1 resumes it; a byte written
//! into the count replaces that byte of the current value. The X-T20 sub-CPU
//! keeps its calendar this way (registers 0x41..0x44, hold 0x45): 0xc70058
//! reads the four bytes until the low byte reads the same twice, and 0xc700c0
//! writes the hold register 0, the four bytes, then 1.
//!
//! A port may instead carry a byte-frame peer with 16-bit registers. Writing 1
//! to +0x08 (the enable of each transfer) starts a frame. Its first byte names
//! the register in bits 7..1 and writes it when bit 0 is set; the next two
//! bytes clock the value out, high byte first, or in when writing, and the
//! byte read back from +0x60 after each send is the value byte shifted out.
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// Register window size in bytes.
pub const REGISTERS: u64 = 0x100;
const STATUS: usize = 0x28 / 4;
const CLEAR: u64 = 0x2c;
const RECEIVE: u64 = 0x48;
const SEND: u64 = 0x60;
const DONE: u32 = 1 << 2;
const ROOM: u32 = 1 << 3;
/// Registers addressed by a peer send word (bits 14..8).
pub const PEER_REGISTERS: usize = 128;
const PEER_WRITE: u32 = 1 << 15;
const ENABLE: u64 = 0x08;
const FRAME_BYTES: usize = 3;

/// Controller configuration only. Status and data transfers require a modeled
/// controller/endpoint protocol and refuse on this port.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigurationPort {
    base: u64,
    registers: BTreeMap<u64, u32>,
}
impl ConfigurationPort {
    pub fn new(base: u64) -> Result<Self, String> {
        if base & 0xfff != 0 {
            return Err("Invalid XPP serial configuration base".into());
        }
        Ok(Self {
            base,
            registers: BTreeMap::new(),
        })
    }
    fn configuration(offset: u64) -> bool {
        matches!(
            offset,
            0 | 4
                | 8
                | 0xc
                | 0x10
                | 0x14
                | 0x18
                | 0x1c
                | 0x20
                | 0x24
                | 0x30
                | 0x34
                | 0x38
                | 0x3c
                | 0x4c
                | 0x50
                | 0x54
                | 0xf0
        )
    }
}
impl Device for ConfigurationPort {
    fn name(&self) -> &'static str {
        "xpp-serial-configuration"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let offset = address - self.base;
        if size != 4 || !Self::configuration(offset) {
            cx.fail(format!(
                "XPP serial operation at +{offset:#x} needs a modeled controller/endpoint protocol"
            ));
            return 0;
        }
        self.registers.get(&offset).copied().unwrap_or(0).into()
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let offset = address - self.base;
        if size != 4 || value > u32::MAX.into() {
            cx.fail("XPP serial configuration width/value");
        } else if offset == CLEAR && value & !0x7f == 0 {
            // Acknowledge flags while no transfer engine is attached.
        } else if offset == 0xf0 && value > 4 {
            cx.fail("Unsupported XPP serial configuration selector");
        } else if Self::configuration(offset) {
            self.registers.insert(offset, value as u32);
        } else {
            cx.fail(format!(
                "XPP serial operation at +{offset:#x} needs a modeled controller/endpoint protocol"
            ));
        }
    }
    fn save(&self) -> Value {
        json!({"base":self.base,"configuration":self.registers})
    }
    fn restore(&mut self, _: &Value) -> Result<(), String> {
        Err("Serial configuration restoration requires its full checkpoint".into())
    }
    fn checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        fujimod_emulator_devices::checkpoint::Checkpoint::encode(self.name(), self)
    }
    fn validate_checkpoint(
        &self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<(), String> {
        if !saved.children().is_empty() {
            return Err("Serial configuration checkpoint has unexpected children".into());
        }
        let state: Self = saved.decode(self.name())?;
        if state.base != self.base || state.registers.keys().any(|&o| !Self::configuration(o)) {
            return Err("Serial configuration checkpoint wiring/registers differ".into());
        }
        Ok(())
    }
    fn restore_checkpoint(
        &mut self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        *self = saved.decode(self.name())?;
        Ok(())
    }
}

/// Byte-frame peer: 16-bit registers addressed by the even command byte and
/// the bytes of the current frame.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FramePeer {
    registers: BTreeMap<u8, u16>,
    frame: Vec<u8>,
}
impl FramePeer {
    /// The byte shifted out while `byte` is sent, or None past the frame.
    fn send(&mut self, byte: u8) -> Option<u8> {
        if self.frame.len() == FRAME_BYTES {
            return None;
        }
        self.frame.push(byte);
        let register = self.frame[0] & !1;
        let value = self.registers.get(&register).copied().unwrap_or(0);
        Some(match self.frame.len() {
            1 => 0,
            2 => (value >> 8) as u8,
            _ => {
                if self.frame[0] & 1 != 0 {
                    let value = u16::from_be_bytes([self.frame[1], self.frame[2]]);
                    self.registers.insert(register, value);
                }
                value as u8
            }
        })
    }
}

/// Seconds count kept by a register-file peer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Clock {
    /// First of the four count registers, least significant byte first.
    pub first: u8,
    /// Register whose bit 0 runs the count.
    pub hold: u8,
    /// Count at device time 0.
    pub seconds: u32,
    /// Device ticks per second.
    pub ticks_per_second: u64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClockState {
    clock: Clock,
    /// Count when it last started or stopped.
    seconds: u32,
    /// Device time at which a running count last started.
    since: Option<u64>,
}
impl ClockState {
    fn count(&self, now: u64) -> u32 {
        match self.since {
            Some(since) => self
                .seconds
                .wrapping_add((now.saturating_sub(since) / self.clock.ticks_per_second) as u32),
            None => self.seconds,
        }
    }
    /// The register's value, after applying a written byte.
    fn transfer(&mut self, now: u64, register: u8, write: Option<u8>) -> Option<u8> {
        let byte = register.wrapping_sub(self.clock.first);
        if byte < 4 {
            let mut bytes = self.count(now).to_le_bytes();
            if let Some(value) = write {
                bytes[byte as usize] = value;
                self.seconds = u32::from_le_bytes(bytes);
                self.since = self.since.map(|_| now);
            }
            return Some(bytes[byte as usize]);
        }
        if register == self.clock.hold
            && let Some(value) = write
        {
            self.seconds = self.count(now);
            self.since = (value & 1 != 0).then_some(now);
        }
        None
    }
}

/// The peer declared behind a discovery port.
pub enum Peer {
    None,
    /// Register file addressed by 16-bit send words (`PEER_REGISTERS` bytes).
    Registers(Vec<u8>),
    /// Byte-frame peer with even-numbered 16-bit registers.
    Frames(BTreeMap<u8, u16>),
}

/// Discovery endpoint with immediate completion and fixed receive words.
pub struct DiscoveryPort {
    base: u64,
    registers: Vec<u32>,
    peer: Option<Vec<u8>>,
    clock: Option<ClockState>,
    frames: Option<FramePeer>,
}
impl DiscoveryPort {
    pub fn new(base: u64) -> Result<Self, String> {
        if base & 0xfff != 0 {
            return Err("Invalid X-Processor Pro serial port base".into());
        }
        Ok(Self {
            base,
            registers: vec![0; (REGISTERS / 4) as usize],
            peer: None,
            clock: None,
            frames: None,
        })
    }
    /// A port with its declared peer.
    pub fn attached(base: u64, peer: Peer) -> Result<Self, String> {
        match peer {
            Peer::None => Self::new(base),
            Peer::Registers(registers) => Self::with_peer(base, registers),
            Peer::Frames(registers) => Self::with_frame_peer(base, registers),
        }
    }
    /// A port whose peer is a register file with these values after reset.
    pub fn with_peer(base: u64, registers: Vec<u8>) -> Result<Self, String> {
        if registers.len() != PEER_REGISTERS {
            return Err("X-Processor Pro serial peer needs 128 registers".into());
        }
        let mut port = Self::new(base)?;
        port.peer = Some(registers);
        Ok(port)
    }
    /// Keep `clock` in the register-file peer; the count runs from device
    /// time 0 and its hold register reads 1.
    pub fn with_clock(mut self, clock: Clock) -> Result<Self, String> {
        let Some(peer) = &mut self.peer else {
            return Err("X-Processor Pro serial clock needs a register-file peer".into());
        };
        if usize::from(clock.first) + 4 > PEER_REGISTERS
            || usize::from(clock.hold) >= PEER_REGISTERS
            || clock.hold.wrapping_sub(clock.first) < 4
            || clock.ticks_per_second == 0
        {
            return Err("Invalid X-Processor Pro serial clock registers".into());
        }
        peer[usize::from(clock.hold)] = 1;
        self.clock = Some(ClockState {
            seconds: clock.seconds,
            since: Some(0),
            clock,
        });
        Ok(self)
    }
    /// A port whose peer exchanges byte frames with these even-numbered
    /// 16-bit registers after reset; unlisted registers read zero.
    pub fn with_frame_peer(base: u64, registers: BTreeMap<u8, u16>) -> Result<Self, String> {
        if registers.keys().any(|r| r & 1 != 0) {
            return Err("X-Processor Pro serial frame peer registers are even".into());
        }
        let mut port = Self::new(base)?;
        port.frames = Some(FramePeer {
            registers,
            frame: Vec::new(),
        });
        Ok(port)
    }
}
impl Device for DiscoveryPort {
    fn name(&self) -> &'static str {
        "xpp-serial"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let o = address - self.base;
        if size != 4 || o & 3 != 0 {
            cx.unmodeled();
            return 0;
        }
        u64::from(match o {
            RECEIVE => 0,
            0x28 => self.registers[STATUS] | ROOM,
            _ => self.registers[(o / 4) as usize],
        })
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let o = address - self.base;
        if size != 4 || o & 3 != 0 || o == 0x28 {
            cx.unmodeled();
            return;
        }
        match o {
            CLEAR => self.registers[STATUS] &= !(value as u32),
            ENABLE => {
                if value & 1 != 0
                    && let Some(frames) = &mut self.frames
                {
                    frames.frame.clear();
                }
                self.registers[(o / 4) as usize] = value as u32;
            }
            SEND if self.frames.is_some() => {
                let Some(byte) = self.frames.as_mut().and_then(|f| f.send(value as u8)) else {
                    return cx.unmodeled();
                };
                self.registers[(SEND / 4) as usize] = byte.into();
                self.registers[STATUS] |= DONE;
            }
            SEND => {
                let word = value as u32;
                self.registers[(SEND / 4) as usize] = match &mut self.peer {
                    Some(peer) => {
                        let register = ((word >> 8) & 0x7f) as usize;
                        let write = (word & PEER_WRITE != 0).then_some(word as u8);
                        if let Some(value) = write {
                            peer[register] = value;
                        }
                        let counted = self
                            .clock
                            .as_mut()
                            .and_then(|c| c.transfer(cx.now(), register as u8, write));
                        counted.unwrap_or(peer[register]).into()
                    }
                    None => word,
                };
                self.registers[STATUS] |= DONE;
            }
            _ => self.registers[(o / 4) as usize] = value as u32,
        }
    }
    fn pollable(&self, address: u64, _size: u8) -> bool {
        address - self.base == 0x28
    }
    fn save(&self) -> Value {
        let mut state = json!({ "registers": self.registers, "peer": self.peer });
        if let Some(frames) = &self.frames {
            state["frames"] = json!(frames);
        }
        if let Some(clock) = &self.clock {
            state["clock"] = json!(clock);
        }
        state
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let registers: Vec<u32> =
            serde_json::from_value(value["registers"].clone()).map_err(|e| e.to_string())?;
        let peer: Option<Vec<u8>> =
            serde_json::from_value(value["peer"].clone()).map_err(|e| e.to_string())?;
        let frames: Option<FramePeer> = match value.get("frames") {
            Some(frames) => {
                Some(serde_json::from_value(frames.clone()).map_err(|e| e.to_string())?)
            }
            None => None,
        };
        let clock: Option<ClockState> = match value.get("clock") {
            Some(clock) => Some(serde_json::from_value(clock.clone()).map_err(|e| e.to_string())?),
            None => None,
        };
        if clock.as_ref().map(|c| &c.clock) != self.clock.as_ref().map(|c| &c.clock) {
            return Err("X-Processor Pro serial clock checkpoint differs".into());
        }
        if registers.len() != self.registers.len()
            || peer.as_ref().map(Vec::len) != self.peer.as_ref().map(Vec::len)
            || frames.is_some() != self.frames.is_some()
            || frames.as_ref().is_some_and(|f| {
                f.frame.len() > FRAME_BYTES || f.registers.keys().any(|r| r & 1 != 0)
            })
        {
            return Err("X-Processor Pro serial checkpoint size differs".into());
        }
        self.registers = registers;
        self.peer = peer;
        self.clock = clock;
        self.frames = frames;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::test_host::{TestHost, read, write};

    #[test]
    fn configuration_restores_but_status_and_data_require_an_endpoint() {
        let base = 0x80000;
        let mut d = ConfigurationPort::new(base).unwrap();
        let mut h = TestHost::new();
        write(&mut d, &mut h, base + 8, 0);
        write(&mut d, &mut h, base + 0x14, 10);
        write(&mut d, &mut h, base + 0x2c, 0x7f);
        let saved = d.checkpoint().unwrap();
        write(&mut d, &mut h, base + 0x14, 11);
        d.restore_checkpoint(&saved).unwrap();
        assert_eq!(read(&mut d, &mut h, base + 0x14), 10);
        assert!(
            ConfigurationPort::new(base + 0x1000)
                .unwrap()
                .validate_checkpoint(&saved)
                .is_err()
        );
        for offset in [0x28, 0x48] {
            let mut cx = h.cx();
            d.read(&mut cx, base + offset, 4);
            assert!(
                cx.refusal()
                    .unwrap()
                    .contains("needs a modeled controller/endpoint protocol")
            );
        }
        let mut cx = h.cx();
        d.write(&mut cx, base + 0x60, 4, 0x1234);
        assert!(
            cx.refusal()
                .unwrap()
                .contains("needs a modeled controller/endpoint protocol")
        );
    }

    #[test]
    fn send_sets_done_and_clear_drops_written_status_bits() {
        let base = 0xfff2a000;
        let mut port = DiscoveryPort::new(base).unwrap();
        let mut h = TestHost::new();
        assert_eq!(read(&mut port, &mut h, base + 0x28), u64::from(ROOM));
        write(&mut port, &mut h, base + 0x2c, 0x7f);
        assert_eq!(read(&mut port, &mut h, base + 0x48), 0);
        write(&mut port, &mut h, base + 0x60, 0xe800);
        assert_eq!(read(&mut port, &mut h, base + 0x28), u64::from(ROOM | DONE));
        write(&mut port, &mut h, base + 0x2c, DONE);
        assert_eq!(read(&mut port, &mut h, base + 0x28), u64::from(ROOM));
        write(&mut port, &mut h, base + 0x14, 0xa);
        assert_eq!(read(&mut port, &mut h, base + 0x14), 0xa);
        let mut cx = h.cx();
        port.write(&mut cx, base + 0x28, 4, 0);
        assert!(cx.is_unmodeled());
    }

    #[test]
    fn register_peer_answers_reads_and_keeps_writes() {
        let base = 0xfff2a000;
        let mut registers = vec![0; PEER_REGISTERS];
        registers[0x60] = 0x10;
        assert!(DiscoveryPort::with_peer(base, vec![0; 127]).is_err());
        let mut port = DiscoveryPort::with_peer(base, registers).unwrap();
        let mut h = TestHost::new();
        write(&mut port, &mut h, base + 0x60, 0x6000);
        assert_eq!(read(&mut port, &mut h, base + 0x60), 0x10);
        assert_eq!(read(&mut port, &mut h, base + 0x28), u64::from(ROOM | DONE));
        write(&mut port, &mut h, base + 0x60, 0x5c00);
        assert_eq!(read(&mut port, &mut h, base + 0x60), 0);
        write(&mut port, &mut h, base + 0x60, 0xdc12);
        assert_eq!(read(&mut port, &mut h, base + 0x60), 0x12);
        let saved = port.save();
        write(&mut port, &mut h, base + 0x60, 0xdc1c);
        port.restore(&saved).unwrap();
        write(&mut port, &mut h, base + 0x60, 0x5c00);
        assert_eq!(read(&mut port, &mut h, base + 0x60), 0x12);
        assert!(DiscoveryPort::new(base).unwrap().restore(&saved).is_err());
    }

    #[test]
    fn peer_clock_counts_seconds_and_holds_while_it_is_set() {
        use fujimod_emulator_devices::test_host::advance;
        let base = 0xfff2a000;
        let clock = Clock {
            first: 0x41,
            hold: 0x45,
            seconds: 0x0102_03ff,
            ticks_per_second: 10,
        };
        assert!(
            DiscoveryPort::new(base)
                .unwrap()
                .with_clock(clock.clone())
                .is_err()
        );
        let mut port = DiscoveryPort::with_peer(base, vec![0; PEER_REGISTERS])
            .unwrap()
            .with_clock(clock)
            .unwrap();
        let mut h = TestHost::new();
        fn get(port: &mut DiscoveryPort, h: &mut TestHost, register: u32) -> u8 {
            write(port, h, 0xfff2a060, register << 8);
            read(port, h, 0xfff2a060) as u8
        }
        // 0xc70058: least significant byte first.
        fn count(port: &mut DiscoveryPort, h: &mut TestHost) -> u32 {
            u32::from_le_bytes([0x41, 0x42, 0x43, 0x44].map(|r| get(port, h, r)))
        }
        assert_eq!(get(&mut port, &mut h, 0x45), 1);
        assert_eq!(count(&mut port, &mut h), 0x0102_03ff);
        advance(&mut port, &mut h, 25);
        assert_eq!(count(&mut port, &mut h), 0x0102_0401);
        // 0xc700c0: hold, write the bytes, resume.
        write(&mut port, &mut h, base + 0x60, 0xc500);
        for (register, byte) in [(0x41, 0x10), (0x42, 0), (0x43, 0), (0x44, 0)] {
            write(
                &mut port,
                &mut h,
                base + 0x60,
                0x8000 | register << 8 | byte,
            );
        }
        advance(&mut port, &mut h, 100);
        assert_eq!(count(&mut port, &mut h), 0x10);
        write(&mut port, &mut h, base + 0x60, 0xc501);
        let saved = port.save();
        advance(&mut port, &mut h, 130);
        assert_eq!(count(&mut port, &mut h), 0x13);
        port.restore(&saved).unwrap();
        assert_eq!(count(&mut port, &mut h), 0x13);
    }

    #[test]
    fn frame_peer_shifts_register_values_out_and_in() {
        let base = 0xfff2c000;
        assert!(DiscoveryPort::with_frame_peer(base, BTreeMap::from([(1, 0)])).is_err());
        let mut port =
            DiscoveryPort::with_frame_peer(base, BTreeMap::from([(0, 0xfd50), (0x10, 0x2b6)]))
                .unwrap();
        let mut h = TestHost::new();
        // 0xc70748 sends (register << 16 | value) as three bytes inside one
        // enabled transfer (0xc4b87c, 0xc4b8c0).
        let frame = |port: &mut DiscoveryPort, h: &mut TestHost, bytes: [u32; 3]| {
            write(port, h, base + ENABLE, 1);
            let reply: Vec<u64> = bytes
                .iter()
                .map(|&b| {
                    write(port, h, base + SEND, b);
                    read(port, h, base + SEND)
                })
                .collect();
            write(port, h, base + ENABLE, 0);
            reply
        };
        assert_eq!(frame(&mut port, &mut h, [0, 0, 0]), [0, 0xfd, 0x50]);
        assert_eq!(read(&mut port, &mut h, base + 0x28), u64::from(ROOM | DONE));
        assert_eq!(frame(&mut port, &mut h, [0x11, 0, 0]), [0, 0x02, 0xb6]);
        let saved = port.save();
        // 0xc71014 writes 0x405 to register 0x60.
        frame(&mut port, &mut h, [0x61, 0x04, 0x05]);
        assert_eq!(frame(&mut port, &mut h, [0x60, 0, 0]), [0, 0x04, 0x05]);
        assert_eq!(frame(&mut port, &mut h, [0x0c, 0, 0]), [0, 0, 0]);
        port.restore(&saved).unwrap();
        assert_eq!(frame(&mut port, &mut h, [0x60, 0, 0]), [0, 0, 0]);
        write(&mut port, &mut h, base + ENABLE, 1);
        for _ in 0..3 {
            write(&mut port, &mut h, base + SEND, 0);
        }
        let mut cx = h.cx();
        port.write(&mut cx, base + SEND, 4, 0);
        assert!(cx.is_unmodeled());
        assert!(DiscoveryPort::new(base).unwrap().restore(&saved).is_err());
    }
}
