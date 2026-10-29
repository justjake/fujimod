//! X-Processor Pro imaging control page: block stop and the register bus.
//!
//! The X-T20 2.11 section-5 driver (0xc345b4..0xc34864) runs one command
//! channel at +0x200: +0x208 configuration (bits 8..12 select the target),
//! +0x20c/+0x210 address and length of a transmit frame in RAM, +0x204 bit 0
//! start/busy (polled by 0xc34824) and eight receive words at +0x21c whose
//! bits 15:8 hold the reply byte (0xc347d8). Frames are
//! `0x81 hi lo value` (write) and `0x82 hi lo` (read). The imaging-side chip
//! is a register file per channel and select: a read returns the last value
//! written there; the start-up writes a register and reads it back until it
//! matches. What the chip does with the values is not modeled. Commands
//! complete at once, so busy reads clear.
//!
//! Six transfer channels follow at `+0x240 + 0x40*n` (0xc34910..0xc34e94):
//! +0x00 reset (written 1, then 0), +0x08 mode, +0x0c..+0x14 28-bit address
//! and length words, +0x18 an optional word, and +0x04 bit 0 a run bit that
//! section 6 sets to start a channel and clears to stop it (0xc34864 and
//! 0xc34e24 refuse a channel already in the requested state). A reset also
//! stops the channel: section 6 0x16b7fbc configures the channels in its mask
//! (0x156a654), resets each (0x156a5f8 via 0xc348d8) and then waits at
//! 0x16b8118 for channel 0's run bit to read clear without clearing it
//! itself. Leaving SETUP for REC takes that path with channel 0 running. The
//! other words read back their last write; no data moves.
//!
//! +0x000 bit 0 is a run bit that reads set: 0xc33ec8 clears it and waits
//! for it to read set again (stopped). Other words read back their last
//! write. With a frame sync ([`crate::sync`]) on the board, the frame
//! interrupt words +0x140/+0x144 belong to it instead.
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Register window size in bytes.
pub const WINDOW: u64 = 0x1000;
const CONFIGURATION: u64 = 0x208;
const FRAME: u64 = 0x20c;
const LENGTH: u64 = 0x210;
const START: u64 = 0x204;
const RECEIVE: std::ops::Range<u64> = 0x21c..0x23c;
const BUSY: u32 = 1;
const CHANNELS: std::ops::Range<u64> = 0x240..0x240 + 6 * CHANNEL;
const CHANNEL: u64 = 0x40;
const RUN: u64 = 0x04;
const MAX_FRAME: u32 = 16;
const WRITE: u8 = 0x81;
const READ: u8 = 0x82;

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    words: BTreeMap<u64, u32>,
    /// (select, register) -> value.
    chip: Vec<((u8, u16), u8)>,
    received: u8,
}
pub struct Control {
    base: u64,
    words: BTreeMap<u64, u32>,
    chip: BTreeMap<(u8, u16), u8>,
    received: u8,
}
impl Control {
    pub fn new(base: u64) -> Result<Self, String> {
        if base & 0xfff != 0 {
            return Err("Invalid X-Processor Pro imaging control base".into());
        }
        Ok(Self {
            base,
            words: BTreeMap::new(),
            chip: BTreeMap::new(),
            received: 0,
        })
    }
    fn word(&self, offset: u64) -> u32 {
        self.words.get(&offset).copied().unwrap_or(0)
    }
    fn start(&mut self, cx: &mut Cx<'_>) {
        let select = (self.word(CONFIGURATION) >> 8 & 0x1f) as u8;
        let len = self.word(LENGTH).clamp(1, MAX_FRAME) as usize;
        let frame = match cx.ram_bytes(self.word(FRAME).into(), len) {
            Ok(frame) => frame,
            Err(e) => return cx.fail(format!("Imaging bus frame read failed: {e}")),
        };
        let key = |frame: &[u8]| (select, u16::from_be_bytes([frame[1], frame[2]]));
        match frame[0] {
            WRITE if frame.len() >= 4 => {
                self.chip.insert(key(&frame), frame[3]);
            }
            READ if frame.len() >= 3 => {
                self.received = self.chip.get(&key(&frame)).copied().unwrap_or(0);
            }
            _ => cx.unsupported("Unrecognized imaging bus frame"),
        }
    }
}
impl Device for Control {
    fn name(&self) -> &'static str {
        "xpp-imaging-control"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let offset = address.wrapping_sub(self.base);
        if size != 4 || offset & 3 != 0 || offset >= WINDOW {
            cx.unmodeled();
            return 0;
        }
        u64::from(match offset {
            0 => self.word(0) | 1,
            o if RECEIVE.contains(&o) => u32::from(self.received) * 0x101,
            o => self.word(o),
        })
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let offset = address.wrapping_sub(self.base);
        let Ok(mut value) = u32::try_from(value) else {
            return cx.unmodeled();
        };
        if size != 4 || offset & 3 != 0 || offset >= WINDOW {
            return cx.unmodeled();
        }
        match offset {
            o if RECEIVE.contains(&o) => return cx.unmodeled(),
            START if value & BUSY != 0 => {
                value &= !BUSY;
                self.start(cx);
            }
            o if CHANNELS.contains(&o) && (o - CHANNELS.start) % CHANNEL == 0 && value & 1 != 0 => {
                let run = o + RUN;
                match self.word(run) & !1 {
                    0 => self.words.remove(&run),
                    stopped => self.words.insert(run, stopped),
                };
            }
            _ => {}
        }
        if value == 0 {
            self.words.remove(&offset);
        } else {
            self.words.insert(offset, value);
        }
    }
    fn save(&self) -> Value {
        serde_json::to_value(State {
            words: self.words.clone(),
            chip: self.chip.iter().map(|(k, v)| (*k, *v)).collect(),
            received: self.received,
        })
        .unwrap_or(Value::Null)
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let state: State = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        if state
            .words
            .iter()
            .any(|(o, v)| o & 3 != 0 || *o >= WINDOW || *v == 0)
            || state.chip.iter().any(|((select, _), _)| *select > 0x1f)
        {
            return Err("Invalid imaging control checkpoint".into());
        }
        self.words = state.words;
        self.chip = state.chip.into_iter().collect();
        self.received = state.received;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::Host;
    use fujimod_emulator_devices::test_host::{TestHost, read, write};

    const BASE: u64 = 0xfff39000;

    #[test]
    fn register_bus_reads_back_chip_writes_and_completes_at_once() {
        let mut control = Control::new(BASE).unwrap();
        let mut h = TestHost::new();
        let channel = BASE + 0x200;
        h.ram_write(0x1000, &[WRITE, 0x01, 0x23, 0x5a]).unwrap();
        h.ram_write(0x1010, &[READ, 0x01, 0x23]).unwrap();
        write(&mut control, &mut h, channel + 8, 0x1_0300);
        write(&mut control, &mut h, channel + 0xc, 0x1000);
        write(&mut control, &mut h, channel + 0x10, 4);
        write(&mut control, &mut h, channel + 4, 1);
        assert_eq!(read(&mut control, &mut h, channel + 4), 0);
        write(&mut control, &mut h, channel + 0xc, 0x1010);
        write(&mut control, &mut h, channel + 0x10, 3);
        write(&mut control, &mut h, channel + 4, 1);
        assert_eq!(read(&mut control, &mut h, channel + 0x1c), 0x5a5a);
        // Another select has its own registers.
        write(&mut control, &mut h, channel + 8, 0x2_0400);
        write(&mut control, &mut h, channel + 4, 1);
        assert_eq!(read(&mut control, &mut h, channel + 0x38), 0);
        let saved = control.save();
        let mut other = Control::new(BASE).unwrap();
        other.restore(&saved).unwrap();
        write(&mut other, &mut h, channel + 8, 0x1_0300);
        write(&mut other, &mut h, channel + 4, 1);
        assert_eq!(read(&mut other, &mut h, channel + 0x1c), 0x5a5a);
    }

    #[test]
    fn run_bit_reads_set_and_unknown_frames_are_unsupported() {
        let mut control = Control::new(BASE).unwrap();
        let mut h = TestHost::new();
        write(&mut control, &mut h, BASE, 0x10);
        assert_eq!(read(&mut control, &mut h, BASE), 0x11);
        assert_eq!(read(&mut control, &mut h, BASE + 0x50), 0);
        h.ram_write(0x2000, &[0x7f]).unwrap();
        write(&mut control, &mut h, BASE + 0x20c, 0x2000);
        let mut cx = h.cx();
        control.write(&mut cx, BASE + 0x204, 4, 1);
        assert!(cx.is_unmodeled());
    }

    #[test]
    fn transfer_channels_keep_their_run_bit_and_leave_the_bus_alone() {
        let mut control = Control::new(BASE).unwrap();
        let mut h = TestHost::new();
        let channel = BASE + 0x240 + 0x40 * 5;
        // 0xc3493c configures, 0xc34864 starts, 0xc34e24 stops.
        write(&mut control, &mut h, channel + 8, 0x0100_0201);
        write(&mut control, &mut h, channel + 0xc, 0xa7d_6000);
        write(&mut control, &mut h, channel + 4, 1);
        assert_eq!(read(&mut control, &mut h, channel + 4), 1);
        assert_eq!(read(&mut control, &mut h, channel + 0xc), 0xa7d_6000);
        write(&mut control, &mut h, channel + 4, 0);
        assert_eq!(read(&mut control, &mut h, channel + 4), 0);
        assert_eq!(read(&mut control, &mut h, BASE + 0x21c), 0);
    }

    #[test]
    fn a_channel_reset_stops_a_running_channel() {
        let mut control = Control::new(BASE).unwrap();
        let mut h = TestHost::new();
        let channel = BASE + 0x240;
        write(&mut control, &mut h, channel + 8, 0x100);
        write(&mut control, &mut h, channel + 4, 1);
        // 0xc348d8 pulses the reset; 0x16b8118 then waits for the run bit to clear.
        write(&mut control, &mut h, channel, 1);
        assert_eq!(read(&mut control, &mut h, channel + 4), 0);
        write(&mut control, &mut h, channel, 0);
        assert_eq!(read(&mut control, &mut h, channel + 8), 0x100);
        assert_eq!(read(&mut control, &mut h, channel), 0);
        // Other channels keep running.
        write(&mut control, &mut h, channel + 0x44, 1);
        write(&mut control, &mut h, channel, 1);
        assert_eq!(read(&mut control, &mut h, channel + 0x44), 1);
    }
}
