//! XP5 HDMI transmitter control path with a synthetic EDID and SCDC sink.
//! DDC completes in the request access. Signal integrity, PHY lock, packet
//! transmission and video pixels are outside this register model.
use fujimod_emulator_devices::parts::hdmi_sink::{HdmiSink, Options};
use fujimod_emulator_devices::{Cx, Device, checkpoint::Checkpoint};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

const STATUS: u64 = 0x7090;
const ENABLE: u64 = 0x7098;
const SINK_STATUS: u64 = 0x7008;
const DDC_REQUEST: u64 = 0x7a80;
const DDC_STATUS: u64 = 0x7a84;
const FIFO: u64 = 0x7c00;
const FIFO_WORDS: u64 = 64;
pub const HPD_WORD: u64 = 0xff90e200;
const PLUG_EVENT: u32 = 1 << 18;
const UNPLUG_EVENT: u32 = 1 << 19;
const DDC_BITS: u32 = 0xe0;
const SCDC_READ: u8 = 0x40;
const SCDC_WRITE: u8 = 0x44;
const SCDC_ADDRESS: u32 = 0xa8;
const VIDEO_STATUS: u64 = 0x7010;
const PACKET_STROBES: u64 = 0x7604;
const PACKET_SELECT: u64 = 0x7610;

/// SCDC registers of the synthetic sink (HDMI 2.0 section 10.4): sink version 1, plain read/write
/// bytes elsewhere, and the scrambling status (0x21 bit 0) mirrors the TMDS configuration (0x20 bit 0).
fn scdc_register(scdc: &[u8], offset: u32) -> u8 {
    let at = |index: usize| scdc.get(index).copied().unwrap_or(0);
    match offset {
        1 => 1,
        0x21 => at(0x20) & 1,
        _ => at(offset as usize),
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct State {
    registers: BTreeMap<u64, u32>,
    irq: bool,
    ddc_reads: u32,
    /// SCDC register file of the sink (written through DDC command 0x44).
    scdc: Vec<u8>,
    /// First 64 DDC requests: [tick, command, slave/segment, offset, length].
    ddc_trace: Vec<[u64; 5]>,
    /// DDC completion bits (+0x7094 bits 5-7) that were latched while their enable was on.
    ddc_irq: u32,
    pending_connection: Option<(u64, bool)>,
    connected: bool,
}

pub struct HdmiTransmitter {
    base: u64,
    size: u64,
    irq_line: u32,
    sink: HdmiSink,
    state: State,
}

/// A synthetic two-block EDID: base block with one detailed timing and a CTA-861 extension.
fn synthetic_edid() -> Vec<u8> {
    let mut base = [0u8; 128];
    base[..8].copy_from_slice(&[0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0]);
    base[8..10].copy_from_slice(&0x19a4u16.to_be_bytes()); // FMD
    base[18] = 1; // EDID 1.4
    base[19] = 4;
    base[20] = 0xa5; // digital, 10 bit
    base[21] = 0x4c; // 16:9 landscape-ish dimensions
    base[22] = 0x23;
    base[23] = 0x78;
    base[24] = 0x0a;
    // 1080p60 detailed timing (148.5 MHz), written from the CTA-861 timing definition.
    base[54..72].copy_from_slice(&[
        0x02, 0x3a, 0x80, 0x18, 0x71, 0x38, 0x2d, 0x40, 0x58, 0x2c, 0x45, 0x00, 0x20, 0xc2, 0x31,
        0x00, 0x00, 0x1e,
    ]);
    base[72..90].copy_from_slice(&[
        0, 0, 0, 0xfc, 0, b'F', b'M', b' ', b'T', b'E', b'S', b'T', 0x0a, 0x20, 0x20, 0x20, 0x20,
        0x20,
    ]);
    base[90..108].copy_from_slice(&[
        0, 0, 0, 0xfd, 0, 0x18, 0x3c, 0x1e, 0x87, 0x1e, 0x00, 0x0a, 0x20, 0x20, 0x20, 0x20, 0x20,
        0x20,
    ]);
    base[126] = 1; // one extension
    let sum: u8 = base[..127].iter().fold(0u8, |a, b| a.wrapping_add(*b));
    base[127] = 0u8.wrapping_sub(sum);

    let mut ext = [0u8; 128];
    ext[0] = 0x02;
    ext[1] = 0x03;
    let mut blocks: Vec<u8> = Vec::new();
    // HDMI video identifiers for 1080p60/24 and 2160p24/30/60.
    blocks.extend_from_slice(&[0x40 | 5, 16, 32, 93, 95, 97]);
    // HDMI vendor block (OUI 00-0C-03, physical address 1.0.0.0).
    blocks.extend_from_slice(&[0x60 | 5, 0x03, 0x0c, 0x00, 0x10, 0x00]);
    // HDMI Forum vendor block (OUI C4-5D-D8): max TMDS 600 MHz, SCDC present and scrambling above
    // 340 MHz supported (flags 0x88), no deep-colour 4:2:0 bits.
    blocks.extend_from_slice(&[0x60 | 7, 0xd8, 0x5d, 0xc4, 0x01, 0x78, 0x88, 0x00]);
    // Video capability data block (extended tag 0).
    blocks.extend_from_slice(&[0xe0 | 2, 0x00, 0x00]);
    // An empty YCbCr 4:2:0 capability bitmap applies to all video modes.
    blocks.extend_from_slice(&[0xe0 | 1, 0x0f]);
    ext[2] = (4 + blocks.len()) as u8;
    ext[3] = 0x00;
    ext[4..4 + blocks.len()].copy_from_slice(&blocks);
    let sum: u8 = ext[..127].iter().fold(0u8, |a, b| a.wrapping_add(*b));
    ext[127] = 0u8.wrapping_sub(sum);
    let mut out = base.to_vec();
    out.extend_from_slice(&ext);
    out
}

impl HdmiTransmitter {
    pub fn new(base: u64, size: u64, irq_line: u32) -> Result<Self, String> {
        if base & 3 != 0 || size < FIFO + FIFO_WORDS * 4 || size & 3 != 0 {
            return Err("invalid hdmi-transmitter window".into());
        }
        let sink = HdmiSink::new(Options {
            edid: synthetic_edid(),
            connected: false,
        })?;
        Ok(Self {
            base,
            size,
            irq_line,
            sink,
            state: State::default(),
        })
    }

    /// Apply an external cable edge at the current device tick.
    pub fn set_connected(&mut self, now: u64, connected: bool) {
        self.state.pending_connection = Some((now, connected));
    }

    fn offset(&self, address: u64, size: u8) -> Option<u64> {
        address
            .checked_sub(self.base)
            .filter(|offset| size == 4 && *offset < self.size && offset & 3 == 0)
    }

    fn word(&self, offset: u64) -> u32 {
        self.state.registers.get(&offset).copied().unwrap_or(0)
    }

    fn update_irq(&mut self, cx: &mut Cx<'_>) {
        let status1 = self.word(STATUS + 4);
        self.state.ddc_irq &= status1;
        let pending = (self.word(STATUS) & self.word(ENABLE))
            | (status1 & self.word(ENABLE + 4) & !DDC_BITS)
            | (self.state.ddc_irq & self.word(ENABLE + 4));
        let level = pending != 0;
        if level != self.state.irq {
            self.state.irq = level;
            cx.set_irq(self.irq_line, level);
        }
    }

    fn ddc(&mut self, cx: &mut Cx<'_>, request: u32) {
        let command = (request >> 24) as u8;
        let length = (((request >> 16) & 0xff) + 1) as usize;
        let offset = (request >> 8) & 0xff;
        let segment = request & 0xff;
        self.state.ddc_reads = self.state.ddc_reads.saturating_add(1);
        // Kept in the checkpoint (board-state.json) as a short diagnostic trace of the DDC engine use.
        if self.state.ddc_trace.len() < 64 {
            self.state.ddc_trace.push([
                cx.now(),
                u64::from(command),
                u64::from(segment),
                u64::from(offset),
                length as u64,
            ]);
        }
        let mut data = [0u8; 256];
        let mut ok = self.state.connected;
        if ok && command == 0x48 && length == 128 {
            match self.sink.read_block(segment, offset) {
                Ok(block) => data[..128].copy_from_slice(&block),
                Err(_) => ok = false,
            }
        } else if ok
            && command == SCDC_READ
            && segment == SCDC_ADDRESS
            && offset as usize + length <= 256
        {
            // Polled SCDC read.
            for (index, byte) in data.iter_mut().take(length).enumerate() {
                *byte = scdc_register(&self.state.scdc, offset + index as u32);
            }
        } else if ok
            && command == SCDC_WRITE
            && segment == SCDC_ADDRESS
            && offset as usize + length <= 256
        {
            // Write: the firmware put the bytes in the FIFO first then strobed the command.
            self.state.scdc.resize(256, 0);
            for index in 0..length {
                let word = self.word(FIFO + 4 * (index as u64 / 4));
                self.state.scdc[offset as usize + index] = (word >> (8 * (index % 4))) as u8;
            }
            self.complete(true);
            return;
        }
        for (index, chunk) in data[..length].chunks(4).enumerate() {
            let mut word = [0u8; 4];
            word[..chunk.len()].copy_from_slice(chunk);
            self.state
                .registers
                .insert(FIFO + 4 * index as u64, u32::from_le_bytes(word));
        }
        self.complete(ok);
    }

    /// Finish a DDC transaction: status bits 0x27 + 0x25 on success, 0x27 + 0x26 with code 0x20 on failure.
    fn complete(&mut self, ok: bool) {
        let done = if ok { 0xa0 } else { 0xc0 };
        let status = self.word(STATUS + 4) | done;
        self.state.registers.insert(STATUS + 4, status);
        // DDC status is latched for polling. Only completions enabled at
        // the time of the request are eligible to drive the interrupt line.
        self.state.ddc_irq |= done & self.word(ENABLE + 4);
        self.state
            .registers
            .insert(DDC_STATUS, if ok { 0 } else { 0x2000 });
    }

    fn replace_state(&mut self, state: State) -> Result<(), String> {
        self.validate_state(&state)?;
        self.sink.set_connected(state.connected);
        self.state = state;
        Ok(())
    }

    fn validate_state(&self, state: &State) -> Result<(), String> {
        if state.scdc.len() > 256
            || state.ddc_trace.len() > 64
            || state
                .registers
                .keys()
                .any(|offset| *offset >= self.size || offset & 3 != 0)
        {
            return Err("invalid hdmi-transmitter state".into());
        }
        Ok(())
    }
}

impl Device for HdmiTransmitter {
    fn name(&self) -> &'static str {
        "hdmi-transmitter"
    }

    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        if address == HPD_WORD && size == 4 {
            return u64::from(self.state.connected);
        }
        let Some(offset) = self.offset(address, size) else {
            cx.unmodeled();
            return 0;
        };
        if offset == SINK_STATUS {
            let sink = if self.state.connected { 0x10 } else { 0x08 };
            return u64::from(self.word(offset) & !0x18 | sink);
        }
        match offset {
            PACKET_STROBES => u64::from(self.word(offset) & !0xff),
            PACKET_SELECT => u64::from(self.word(offset) & !0xf00),
            _ => u64::from(self.word(offset)),
        }
    }

    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let Some(offset) = self.offset(address, size) else {
            cx.unmodeled();
            return;
        };
        if value > u64::from(u32::MAX) {
            cx.unmodeled();
            return;
        }
        let value = value as u32;
        match offset {
            STATUS | 0x7094 => {
                let cleared = self.word(offset) & !value;
                self.state.registers.insert(offset, cleared);
            }
            VIDEO_STATUS => {
                // Bits 15:8 clear when written as one (the firmware rewrites the register read-modify-write).
                let flags = self.word(offset) >> 8 & 0xff & !(value >> 8 & 0xff);
                self.state
                    .registers
                    .insert(offset, value & 0xffff_00ff | flags << 8);
            }
            DDC_STATUS => {}
            DDC_REQUEST => {
                self.state.registers.insert(offset, value);
                self.ddc(cx, value);
            }
            _ => {
                self.state.registers.insert(offset, value);
            }
        }
        self.update_irq(cx);
    }

    fn next_deadline(&self, _: u64) -> Option<u64> {
        self.state.pending_connection.map(|(at, _)| at)
    }

    fn advance(&mut self, cx: &mut Cx<'_>) {
        let Some((at, connected)) = self.state.pending_connection else {
            return;
        };
        if at > cx.now() {
            return;
        }
        self.state.pending_connection = None;
        if connected == self.state.connected {
            return;
        }
        self.state.connected = connected;
        self.sink.set_connected(connected);
        self.state.scdc.clear();
        let event = if connected { PLUG_EVENT } else { UNPLUG_EVENT };
        self.state
            .registers
            .insert(STATUS, self.word(STATUS) | event);
        self.update_irq(cx);
    }

    fn checkpoint(&self) -> Result<Checkpoint, String> {
        Checkpoint::encode(self.name(), &self.state)
    }

    fn validate_checkpoint(&self, checkpoint: &Checkpoint) -> Result<(), String> {
        self.validate_state(&checkpoint.decode(self.name())?)
    }

    fn restore_checkpoint(&mut self, checkpoint: &Checkpoint) -> Result<(), String> {
        self.replace_state(checkpoint.decode(self.name())?)
    }

    fn save(&self) -> Value {
        serde_json::to_value(&self.state).unwrap()
    }

    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let state: State = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        self.replace_state(state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::test_host::recording::RecordingHost;

    const BASE: u64 = 0xff9c0000;

    fn plugged(host: &mut RecordingHost) -> HdmiTransmitter {
        let mut tx = HdmiTransmitter::new(BASE, 0x8000, 287).unwrap();
        tx.set_connected(10, true);
        host.now = 10;
        tx.advance(&mut Cx::new(host, 0, 0, 0));
        tx
    }

    #[test]
    fn edid_blocks_have_valid_checksums() {
        let edid = synthetic_edid();
        assert_eq!(edid.len(), 256);
        for block in edid.chunks(128) {
            assert_eq!(block.iter().fold(0u8, |a, b| a.wrapping_add(*b)), 0);
        }
    }

    #[test]
    fn explicit_cable_edge_changes_hpd_and_raises_the_interrupt() {
        let mut host = RecordingHost::default();
        let mut tx = HdmiTransmitter::new(BASE, 0x8000, 287).unwrap();
        assert_eq!(tx.read(&mut Cx::new(&mut host, 0, 0, 0), 0xff90e200, 4), 0);
        host.now = 10;
        tx.write(&mut Cx::new(&mut host, 0, 0, 0), BASE + 0x709c, 4, 0xe0);
        tx.write(&mut Cx::new(&mut host, 0, 0, 0), BASE + 0x7098, 4, 0xc0000);
        assert_eq!(tx.next_deadline(10), None);
        tx.set_connected(1010, true);
        assert_eq!(tx.next_deadline(10), Some(1010));
        tx.advance(&mut Cx::new(&mut host, 0, 0, 0));
        assert_eq!(tx.read(&mut Cx::new(&mut host, 0, 0, 0), HPD_WORD, 4), 0);
        host.now = 1010;
        tx.advance(&mut Cx::new(&mut host, 0, 0, 0));
        assert_eq!(tx.read(&mut Cx::new(&mut host, 0, 0, 0), 0xff90e200, 4), 1);
        assert_eq!(
            tx.read(&mut Cx::new(&mut host, 0, 0, 0), BASE + 0x7008, 4) >> 3 & 3,
            2,
            "connected: absent flag clear, sink sense set"
        );
        assert_eq!(host.irqs.last().map(|i| (i.0, i.1)), Some((287, true)));
        tx.write(&mut Cx::new(&mut host, 0, 0, 0), BASE + 0x7090, 4, 0x40000);
        assert_eq!(host.irqs.last().map(|i| (i.0, i.1)), Some((287, false)));

        let checkpoint = tx.checkpoint().unwrap();
        tx.set_connected(host.now, false);
        tx.advance(&mut Cx::new(&mut host, 0, 0, 0));
        assert_eq!(tx.read(&mut Cx::new(&mut host, 0, 0, 0), HPD_WORD, 4), 0);
        assert_eq!(tx.word(STATUS) & UNPLUG_EVENT, UNPLUG_EVENT);
        assert_eq!(host.irqs.last().map(|i| (i.0, i.1)), Some((287, true)));
        assert!(tx.sink.read_block(0, 0).is_err());

        tx.restore_checkpoint(&checkpoint).unwrap();
        assert_eq!(tx.read(&mut Cx::new(&mut host, 0, 0, 0), HPD_WORD, 4), 1);
        assert!(tx.sink.read_block(0, 0).is_ok());
    }

    #[test]
    fn ddc_block_read_completes_with_interrupt_and_data() {
        let mut host = RecordingHost::default();
        let mut tx = plugged(&mut host);
        tx.write(&mut Cx::new(&mut host, 0, 0, 0), BASE + 0x709c, 4, 0xe0);
        tx.write(
            &mut Cx::new(&mut host, 0, 0, 0),
            BASE + 0x7a80,
            4,
            0x487f_0000,
        );
        assert_eq!(
            tx.read(&mut Cx::new(&mut host, 0, 0, 0), BASE + 0x7094, 4) & 0xa0,
            0xa0
        );
        assert_eq!(
            tx.read(&mut Cx::new(&mut host, 0, 0, 0), BASE + 0x7a84, 4) >> 8 & 0xff,
            0
        );
        assert_eq!(
            tx.read(&mut Cx::new(&mut host, 0, 0, 0), BASE + 0x7c00, 4),
            0xffff_ff00
        );
        assert_eq!(host.irqs.last().map(|i| (i.0, i.1)), Some((287, true)));
        tx.write(&mut Cx::new(&mut host, 0, 0, 0), BASE + 0x7094, 4, 0xa0);
        assert_eq!(host.irqs.last().map(|i| (i.0, i.1)), Some((287, false)));
    }

    #[test]
    fn scdc_polled_read_returns_the_sink_version_and_the_edid_advertises_scdc() {
        let mut host = RecordingHost::default();
        let mut tx = plugged(&mut host);
        tx.write(&mut Cx::new(&mut host, 0, 0, 0), BASE + 0x709c, 4, 0xe0);
        tx.write(
            &mut Cx::new(&mut host, 0, 0, 0),
            BASE + 0x7a80,
            4,
            0x4000_01a8,
        );
        assert_eq!(
            tx.read(&mut Cx::new(&mut host, 0, 0, 0), BASE + 0x7094, 4) & 0xa0,
            0xa0
        );
        assert_eq!(
            tx.read(&mut Cx::new(&mut host, 0, 0, 0), BASE + 0x7a84, 4) >> 8 & 0xff,
            0
        );
        assert_eq!(
            tx.read(&mut Cx::new(&mut host, 0, 0, 0), BASE + 0x7c00, 4) & 0xff,
            1
        );
        let edid = synthetic_edid();
        let forum = edid[128..]
            .windows(4)
            .position(|w| w == [0x67, 0xd8, 0x5d, 0xc4])
            .unwrap()
            + 128;
        assert_eq!(edid[forum + 6] & 0x80, 0x80, "SCDC_Present");
    }

    #[test]
    fn polled_ddc_completion_does_not_raise_the_interrupt_when_the_enable_is_restored() {
        let mut host = RecordingHost::default();
        let mut tx = plugged(&mut host);
        // Poll with interrupts masked, then restore the mask.
        tx.write(&mut Cx::new(&mut host, 0, 0, 0), BASE + 0x709c, 4, 0);
        tx.write(&mut Cx::new(&mut host, 0, 0, 0), BASE + 0x7094, 4, 0xe0);
        tx.write(
            &mut Cx::new(&mut host, 0, 0, 0),
            BASE + 0x7a80,
            4,
            0x4000_01a8,
        );
        assert_eq!(
            tx.read(&mut Cx::new(&mut host, 0, 0, 0), BASE + 0x7094, 4) & 0xa0,
            0xa0,
            "polled status latched"
        );
        tx.write(&mut Cx::new(&mut host, 0, 0, 0), BASE + 0x709c, 4, 0xe0);
        assert_ne!(
            host.irqs.last().map(|i| (i.0, i.1)),
            Some((287, true)),
            "no interrupt on restore"
        );
    }

    #[test]
    fn scdc_scrambling_status_follows_the_tmds_configuration_write() {
        let mut host = RecordingHost::default();
        let mut tx = plugged(&mut host);
        let read = |tx: &mut HdmiTransmitter, host: &mut RecordingHost, offset: u32| {
            tx.write(&mut Cx::new(host, 0, 0, 0), BASE + 0x7094, 4, 0xe0);
            tx.write(
                &mut Cx::new(host, 0, 0, 0),
                BASE + 0x7a80,
                4,
                0x4000_00a8 | u64::from(offset) << 8,
            );
            tx.read(&mut Cx::new(host, 0, 0, 0), BASE + 0x7c00, 4) & 0xff
        };
        assert_eq!(
            read(&mut tx, &mut host, 0x21),
            0,
            "scrambling off before the configuration write"
        );
        // FIFO word, then command 0x44, slave 0xa8, offset 0x20, one byte.
        tx.write(&mut Cx::new(&mut host, 0, 0, 0), BASE + 0x7c00, 4, 0x03);
        tx.write(
            &mut Cx::new(&mut host, 0, 0, 0),
            BASE + 0x7a80,
            4,
            0x4400_20a8,
        );
        assert_eq!(
            tx.read(&mut Cx::new(&mut host, 0, 0, 0), BASE + 0x7094, 4) & 0xa0,
            0xa0
        );
        assert_eq!(read(&mut tx, &mut host, 0x20), 3);
        assert_eq!(read(&mut tx, &mut host, 0x21), 1, "scrambling status set");
    }

    #[test]
    fn video_status_flags_clear_on_write_and_packet_strobes_read_zero() {
        let mut host = RecordingHost::default();
        let mut tx = plugged(&mut host);
        tx.write(&mut Cx::new(&mut host, 0, 0, 0), BASE + 0x7010, 4, 0xff55);
        assert_eq!(
            tx.read(&mut Cx::new(&mut host, 0, 0, 0), BASE + 0x7010, 4),
            0x0055,
            "0xff clears bits 15:8"
        );
        tx.write(
            &mut Cx::new(&mut host, 0, 0, 0),
            BASE + 0x7604,
            4,
            0x1000_0040,
        );
        tx.write(&mut Cx::new(&mut host, 0, 0, 0), BASE + 0x7610, 4, 0x0f00);
        assert_eq!(
            tx.read(&mut Cx::new(&mut host, 0, 0, 0), BASE + 0x7604, 4),
            0x1000_0000
        );
        assert_eq!(
            tx.read(&mut Cx::new(&mut host, 0, 0, 0), BASE + 0x7610, 4),
            0
        );
    }

    #[test]
    fn ddc_before_the_plug_reports_an_error() {
        let mut host = RecordingHost::default();
        let mut tx = HdmiTransmitter::new(BASE, 0x8000, 287).unwrap();
        tx.write(
            &mut Cx::new(&mut host, 0, 0, 0),
            BASE + 0x7a80,
            4,
            0x487f_0000,
        );
        assert_eq!(
            tx.read(&mut Cx::new(&mut host, 0, 0, 0), BASE + 0x7094, 4) & 0xc0,
            0xc0
        );
        assert_eq!(
            tx.read(&mut Cx::new(&mut host, 0, 0, 0), BASE + 0x7a84, 4) >> 8 & 0xff,
            0x20
        );
        assert_eq!(
            tx.read(&mut Cx::new(&mut host, 0, 0, 0), BASE + 0x7008, 4) >> 3 & 3,
            1,
            "unplugged: absent flag set"
        );
    }
}
