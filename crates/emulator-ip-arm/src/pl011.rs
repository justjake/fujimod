//! Arm PrimeCell PL011 UART (r1p5), optionally connected to a virtual peer.
//!
//! The X-T3 5.11 has three ports at `0xfff39000 + 0x1000·n` (n = 0..2). Its
//! serial driver opens a port by writing `+0x30` CR = 0, the integer and
//! fractional baud rate divisors `+0x24` / `+0x28`, then read-modify-writing
//! `+0x2c` LCR_H; it clears bits of `+0x38` IMSC and reads `+0x40` MIS. The
//! firmware update program opens port 0.
//!
//! Model: the transmitter drains at once (FR: transmit FIFO empty, never busy)
//! and each byte written to DR raises the transmit interrupt (RIS bit 5) until
//! ICR clears it. Transmitted bytes are kept (bounded) for inspection. The
//! receiver stays empty when no peer is attached. An optional virtual RL78
//! peer supplies ordered response bytes, a FIFO-threshold receive interrupt,
//! and a virtual immediate receive timeout for a nonempty partial FIFO. Its
//! outbound bytes are buffered without baud timing. MIS = RIS & IMSC drives
//! the optional level interrupt line.
use fujimod_emulator_devices::{Cx, Device};
pub use fujimod_emulator_parts_io::rl78_programmer::Config as Rl78Config;
use fujimod_emulator_parts_io::rl78_programmer::MAX_RESPONSE_BYTES;
use fujimod_emulator_parts_io::rl78_programmer::Peer as Rl78;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::VecDeque;

/// Register window size in bytes.
pub const WINDOW: u64 = 0x1000;
const DR: u64 = 0x000;
const RSR: u64 = 0x004;
const FR: u64 = 0x018;
const ILPR: u64 = 0x020;
const IBRD: u64 = 0x024;
const FBRD: u64 = 0x028;
const LCR_H: u64 = 0x02c;
const CR: u64 = 0x030;
const IFLS: u64 = 0x034;
const IMSC: u64 = 0x038;
const RIS: u64 = 0x03c;
const MIS: u64 = 0x040;
const ICR: u64 = 0x044;
const DMACR: u64 = 0x048;
const IDS: std::ops::RangeInclusive<u64> = 0xfe0..=0xffc;
/// FR: transmit FIFO empty, receive FIFO empty.
const TX_EMPTY: u32 = 1 << 7;
const RX_EMPTY: u32 = 1 << 4;
const RX_FULL: u32 = 1 << 6;
/// RIS/IMSC/MIS/ICR: transmit interrupt.
pub const TX_INTERRUPT: u32 = 1 << 5;
pub const RX_INTERRUPT: u32 = 1 << 4;
pub const RX_TIMEOUT_INTERRUPT: u32 = 1 << 6;
/// Interrupt bits 10..0 exist.
const INTERRUPTS: u32 = 0x7ff;
/// Bytes kept from what the port transmitted.
pub const KEPT_BYTES: usize = 4096;
const RX_CAPACITY: usize = 4096;
/// PeriphID0..3 and PCellID0..3 of an r1p5 PL011.
const ID_WORDS: [u32; 8] = [0x11, 0x10, 0x34, 0x00, 0x0d, 0xf0, 0x05, 0xb1];

mod queue_bytes {
    use serde::{Deserializer, Serializer};
    use std::collections::VecDeque;

    pub fn serialize<S: Serializer>(
        queue: &VecDeque<u8>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        let bytes: Vec<u8> = queue.iter().copied().collect();
        serializer.serialize_bytes(&bytes)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<VecDeque<u8>, D::Error> {
        fujimod_emulator_devices::checkpoint::bytes::deserialize(deserializer)
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Debug)]
#[serde(deny_unknown_fields)]
struct State {
    ilpr: u32,
    ibrd: u32,
    fbrd: u32,
    lcr_h: u32,
    cr: u32,
    ifls: u32,
    imsc: u32,
    ris: u32,
    dmacr: u32,
    /// The first bytes transmitted, at most `KEPT_BYTES`.
    sent: Vec<u8>,
    /// Every byte transmitted.
    sent_count: u64,
    #[serde(
        default,
        skip_serializing_if = "VecDeque::is_empty",
        with = "queue_bytes"
    )]
    rx: VecDeque<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rl78: Option<Rl78>,
}

impl Default for State {
    /// The reset values: CR has TXE and RXE set, IFLS selects half-full.
    fn default() -> Self {
        Self {
            ilpr: 0,
            ibrd: 0,
            fbrd: 0,
            lcr_h: 0,
            cr: 0x300,
            ifls: 0x12,
            imsc: 0,
            ris: 0,
            dmacr: 0,
            sent: Vec::new(),
            sent_count: 0,
            rx: VecDeque::new(),
            rl78: None,
        }
    }
}

pub struct Pl011 {
    base: u64,
    irq: Option<u32>,
    state: State,
}

impl Pl011 {
    /// A port at the page-aligned `base`, raising `irq` (a shared peripheral
    /// line, 32..511) when declared.
    pub fn new(base: u64, irq: Option<u32>) -> Result<Self, String> {
        Self::with_rl78(base, irq, None)
    }
    /// Attach a virtual RL78 programming peer only when declared.
    pub fn with_rl78(
        base: u64,
        irq: Option<u32>,
        config: Option<Rl78Config>,
    ) -> Result<Self, String> {
        if base & 0xfff != 0
            || base.checked_add(WINDOW).is_none()
            || irq.is_some_and(|line| !(32..512).contains(&line))
        {
            return Err("Invalid PL011 UART".into());
        }
        let mut state = State::default();
        state.rl78 = config.map(Rl78::new).transpose()?;
        Ok(Self { base, irq, state })
    }
    pub fn base(&self) -> u64 {
        self.base
    }
    /// The first bytes the port transmitted (at most `KEPT_BYTES`).
    pub fn sent(&self) -> &[u8] {
        &self.state.sent
    }
    /// How many bytes the port transmitted.
    pub fn sent_count(&self) -> u64 {
        self.state.sent_count
    }
    fn offset(&self, address: u64, size: u8) -> Option<u64> {
        let offset = address.checked_sub(self.base)?;
        let data = offset == DR && matches!(size, 1 | 2 | 4);
        (data || size == 4 && offset & 3 == 0 && offset < WINDOW).then_some(offset)
    }
    fn update(&self, cx: &mut Cx<'_>) {
        if let Some(line) = self.irq {
            cx.set_irq(line, self.state.ris & self.state.imsc != 0);
        }
    }
    fn rx_bits(s: &State) -> u32 {
        if s.rx.is_empty() {
            return 0;
        }
        let threshold = if s.lcr_h & 0x10 == 0 {
            1
        } else {
            [2, 4, 8, 12, 14]
                .get(((s.ifls >> 3) & 7) as usize)
                .copied()
                .unwrap_or(8)
        };
        if s.rx.len() >= threshold {
            RX_INTERRUPT
        } else {
            RX_TIMEOUT_INTERRUPT
        }
    }
    fn update_rx(&mut self) {
        self.state.ris =
            (self.state.ris & !(RX_INTERRUPT | RX_TIMEOUT_INTERRUPT)) | Self::rx_bits(&self.state);
    }
    fn check(&self, state: &State) -> Result<(), String> {
        if state.sent.len() > KEPT_BYTES
            || (state.sent.len() as u64) > state.sent_count
            || state.sent.len() < KEPT_BYTES && state.sent.len() as u64 != state.sent_count
            || state.ilpr > 0xff
            || state.ibrd > 0xffff
            || state.fbrd > 0x3f
            || state.lcr_h > 0xff
            || state.cr > 0xffff
            || state.ifls > 0x3f
            || state.imsc & !INTERRUPTS != 0
            || state.ris & !(TX_INTERRUPT | RX_INTERRUPT | RX_TIMEOUT_INTERRUPT) != 0
            || state.rx.len() > RX_CAPACITY
            || state.ris & (RX_INTERRUPT | RX_TIMEOUT_INTERRUPT) != Self::rx_bits(state)
            || !state.rx.is_empty() && state.rl78.is_none()
            || state.rl78.as_ref().is_some_and(|peer| !peer.valid())
            || state.rl78.as_ref().map(Rl78::config) != self.state.rl78.as_ref().map(Rl78::config)
            || state.dmacr > 7
        {
            return Err("Invalid PL011 UART checkpoint".into());
        }
        Ok(())
    }
}

impl Device for Pl011 {
    fn name(&self) -> &'static str {
        "pl011"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let Some(offset) = self.offset(address, size) else {
            cx.unmodeled();
            return 0;
        };
        if offset == DR {
            let byte = self.state.rx.pop_front().unwrap_or(0);
            self.update_rx();
            self.update(cx);
            return u64::from(byte);
        }
        let s = &self.state;
        u64::from(match offset {
            RSR => 0,
            FR => {
                TX_EMPTY
                    | if s.rx.is_empty() { RX_EMPTY } else { 0 }
                    | if s.rx.len() >= 16 { RX_FULL } else { 0 }
            }
            ILPR => s.ilpr,
            IBRD => s.ibrd,
            FBRD => s.fbrd,
            LCR_H => s.lcr_h,
            CR => s.cr,
            IFLS => s.ifls,
            IMSC => s.imsc,
            RIS => s.ris,
            MIS => s.ris & s.imsc,
            DMACR => s.dmacr,
            o if IDS.contains(&o) => ID_WORDS[((o - IDS.start()) / 4) as usize],
            _ => {
                cx.unmodeled();
                0
            }
        })
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let Some(offset) = self.offset(address, size) else {
            return cx.unmodeled();
        };
        let value = value as u32;
        let s = &mut self.state;
        match offset {
            DR => {
                // Near capacity, stage the peer transition so an overflow is
                // refused without consuming the byte or losing its response.
                // The queue models virtual backpressure, not UART wire timing.
                let response = if let Some(peer) = &mut s.rl78 {
                    if s.rx.len() == RX_CAPACITY {
                        return cx.unmodeled();
                    }
                    if s.rx.len() > RX_CAPACITY - MAX_RESPONSE_BYTES {
                        let mut next = peer.clone();
                        let response = next.receive(value as u8);
                        if s.rx.len() + response.len() > RX_CAPACITY {
                            return cx.unmodeled();
                        }
                        *peer = next;
                        response
                    } else {
                        peer.receive(value as u8)
                    }
                } else {
                    Vec::new()
                };
                if s.sent.len() < KEPT_BYTES {
                    s.sent.push(value as u8);
                }
                s.sent_count += 1;
                s.ris |= TX_INTERRUPT;
                s.rx.extend(response);
            }
            // Writing RSR/ECR clears the receive errors, of which there are none.
            RSR => {}
            ILPR => s.ilpr = value & 0xff,
            IBRD => s.ibrd = value & 0xffff,
            FBRD => s.fbrd = value & 0x3f,
            LCR_H => s.lcr_h = value & 0xff,
            CR => s.cr = value & 0xffff,
            IFLS => s.ifls = value & 0x3f,
            IMSC => s.imsc = value & INTERRUPTS,
            ICR => s.ris &= !value,
            DMACR => s.dmacr = value & 7,
            _ => return cx.unmodeled(),
        }
        self.update_rx();
        self.update(cx);
    }
    fn pollable(&self, address: u64, size: u8) -> bool {
        self.offset(address, size)
            .is_some_and(|o| matches!(o, FR | RIS | MIS))
    }
    fn save(&self) -> Value {
        let s = &self.state;
        let mut result = serde_json::json!({
            "ilpr": s.ilpr, "ibrd": s.ibrd, "fbrd": s.fbrd,
            "lcr_h": s.lcr_h, "cr": s.cr, "ifls": s.ifls,
            "imsc": s.imsc, "ris": s.ris, "dmacr": s.dmacr,
            "sent": s.sent, "sent_count": s.sent_count,
        });
        if !s.rx.is_empty() {
            result["rx"] = serde_json::json!({"bytes": s.rx.len()});
        }
        if let Some(peer) = &s.rl78 {
            result["rl78"] = peer.diagnostic_state();
        }
        result
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        if value.get("rl78").is_some() || value.get("rx").is_some() {
            return Err("PL011 binary buffers require a typed checkpoint".into());
        }
        let state: State = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        self.check(&state)?;
        self.state = state;
        Ok(())
    }
    fujimod_emulator_devices::checkpoint_state!(state: State, Self::check);
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::test_host::recording::RecordingHost;

    const PORT: u64 = 0xfff3_9000;

    fn write(u: &mut Pl011, h: &mut RecordingHost, offset: u64, size: u8, value: u64) {
        u.write(&mut Cx::new(h, 0, 0, 0), PORT + offset, size, value);
    }
    fn read(u: &mut Pl011, h: &mut RecordingHost, offset: u64) -> u64 {
        u.read(&mut Cx::new(h, 0, 0, 0), PORT + offset, 4)
    }

    #[test]
    fn an_idle_port_takes_the_driver_setup_and_drains_what_it_sends() {
        let mut u = Pl011::new(PORT, Some(253)).unwrap();
        let mut h = RecordingHost::default();
        assert_eq!(read(&mut u, &mut h, CR), 0x300);
        assert_eq!(read(&mut u, &mut h, IFLS), 0x12);
        // Open as the X-T3 update program does in emulation: disable, the
        // divisors, then 8 data bits with the FIFOs off.
        write(&mut u, &mut h, CR, 4, 0);
        write(&mut u, &mut h, IBRD, 4, 11);
        write(&mut u, &mut h, FBRD, 4, 46);
        let lcr_h = read(&mut u, &mut h, LCR_H);
        write(&mut u, &mut h, LCR_H, 4, lcr_h | 0x60);
        assert_eq!(read(&mut u, &mut h, IBRD), 11);
        assert_eq!(read(&mut u, &mut h, FBRD), 46);
        assert_eq!(read(&mut u, &mut h, LCR_H), 0x60);
        assert_eq!(read(&mut u, &mut h, FR), 0x90);
        assert_eq!(read(&mut u, &mut h, DR), 0);
        // A transmitted byte raises TX; masked until IMSC enables it.
        write(&mut u, &mut h, CR, 4, 0x301);
        write(&mut u, &mut h, DR, 1, 0x41);
        assert_eq!(read(&mut u, &mut h, RIS), u64::from(TX_INTERRUPT));
        assert_eq!(read(&mut u, &mut h, MIS), 0);
        assert_eq!(h.irqs.last(), Some(&(253, false)));
        write(&mut u, &mut h, IMSC, 4, u64::from(TX_INTERRUPT));
        assert_eq!(read(&mut u, &mut h, MIS), u64::from(TX_INTERRUPT));
        assert_eq!(h.irqs.last(), Some(&(253, true)));
        write(&mut u, &mut h, ICR, 4, u64::from(TX_INTERRUPT));
        assert_eq!(read(&mut u, &mut h, MIS), 0);
        assert_eq!(h.irqs.last(), Some(&(253, false)));
        assert_eq!(u.sent(), &[0x41]);
        assert_eq!(u.sent_count(), 1);
        assert_eq!(read(&mut u, &mut h, 0xfe0), 0x11);
        assert_eq!(read(&mut u, &mut h, 0xfe8), 0x34);
        assert_eq!(read(&mut u, &mut h, 0xffc), 0xb1);
        assert!(u.pollable(PORT + FR, 4));
        assert!(!u.pollable(PORT + DR, 4));
    }

    #[test]
    fn unmodeled_accesses_and_bad_wiring_are_refused() {
        assert!(Pl011::new(PORT + 4, None).is_err());
        assert!(Pl011::new(PORT, Some(31)).is_err());
        assert!(Pl011::new(PORT, Some(512)).is_err());
        assert!(Pl011::new(!0xfff, None).is_err());
        let mut u = Pl011::new(PORT, None).unwrap();
        let mut h = RecordingHost::default();
        for (offset, size) in [(FR, 1), (IBRD + 2, 4), (0x04c, 4), (WINDOW, 4)] {
            let mut cx = Cx::new(&mut h, 0, 0, 0);
            u.read(&mut cx, PORT + offset, size);
            assert!(cx.is_unmodeled(), "{offset:#x}/{size}");
        }
        let mut cx = Cx::new(&mut h, 0, 0, 0);
        u.write(&mut cx, PORT + 0x004c, 4, 1);
        assert!(cx.is_unmodeled());
        // Without a declared line the port drives nothing.
        write(&mut u, &mut h, IMSC, 4, u64::from(TX_INTERRUPT));
        write(&mut u, &mut h, DR, 4, 0x55);
        assert!(h.irqs.is_empty());
    }

    #[test]
    fn declared_peer_receives_short_fifo_reply_and_restores_state() {
        let config = Rl78Config {
            flash_bytes: 48 * 1024,
        };
        let mut uart = Pl011::with_rl78(PORT, Some(253), Some(config.clone())).unwrap();
        let mut h = RecordingHost::default();
        // FIFO enabled, half-full threshold, RX and RX-timeout interrupts.
        write(&mut uart, &mut h, LCR_H, 4, 0x70);
        write(
            &mut uart,
            &mut h,
            IMSC,
            4,
            u64::from(RX_INTERRUPT | RX_TIMEOUT_INTERRUPT),
        );
        for byte in [1, 3, 0x9a, 0, 0x21, 0x42, 3] {
            write(&mut uart, &mut h, DR, 1, byte);
        }
        assert_eq!(read(&mut uart, &mut h, FR) & u64::from(RX_EMPTY), 0);
        assert_eq!(
            read(&mut uart, &mut h, MIS),
            u64::from(RX_TIMEOUT_INTERRUPT)
        );
        assert_eq!(h.irqs.last(), Some(&(253, true)));
        let checkpoint = uart.checkpoint().unwrap();
        let mut restored = Pl011::with_rl78(PORT, Some(253), Some(config)).unwrap();
        restored.restore_checkpoint(&checkpoint).unwrap();
        let reply: Vec<_> = (0..7)
            .map(|_| read(&mut restored, &mut h, DR) as u8)
            .collect();
        assert_eq!(reply, [2, 3, 6, 32, 0, 0xd7, 3]);
        assert_eq!(
            read(&mut restored, &mut h, FR) & u64::from(RX_EMPTY),
            u64::from(RX_EMPTY)
        );
        assert_eq!(read(&mut restored, &mut h, MIS), 0);
        assert_eq!(h.irqs.last(), Some(&(253, false)));
        assert!(
            Pl011::new(PORT, Some(253))
                .unwrap()
                .restore_checkpoint(&checkpoint)
                .is_err()
        );

        // A signature reply spans more than the receive FIFO and is drained in order.
        for byte in [1, 1, 0, 0xff, 3] {
            write(&mut restored, &mut h, DR, 1, byte);
        }
        for _ in 0..5 {
            read(&mut restored, &mut h, DR);
        }
        for byte in [1, 1, 0xc0, 0x3f, 3] {
            write(&mut restored, &mut h, DR, 1, byte);
        }
        assert_eq!(
            read(&mut restored, &mut h, FR) & u64::from(RX_FULL),
            u64::from(RX_FULL)
        );
        assert_eq!(read(&mut restored, &mut h, MIS), u64::from(RX_INTERRUPT));
        let reply: Vec<_> = (0..31)
            .map(|_| read(&mut restored, &mut h, DR) as u8)
            .collect();
        assert_eq!(&reply[..5], &[2, 1, 6, 0xf9, 3]);
        assert_eq!(&reply[5..7], &[2, 22]);
        assert_eq!(
            read(&mut restored, &mut h, FR) & u64::from(RX_EMPTY),
            u64::from(RX_EMPTY)
        );
    }

    #[test]
    fn checkpoint_preserves_programmed_peer_and_binary_buffers() {
        fn frame(start: u8, payload: &[u8], end: u8) -> Vec<u8> {
            let mut frame = vec![start, payload.len() as u8];
            frame.extend_from_slice(payload);
            frame.push(frame[1..].iter().fold(0u8, |sum, b| sum.wrapping_sub(*b)));
            frame.push(end);
            frame
        }
        fn exchange(u: &mut Pl011, h: &mut RecordingHost, bytes: &[u8]) -> Vec<u8> {
            for &byte in bytes {
                write(u, h, DR, 1, u64::from(byte));
            }
            let mut reply = Vec::new();
            while !u.state.rx.is_empty() {
                reply.push(read(u, h, DR) as u8);
            }
            reply
        }
        let command = |bytes: &[u8]| frame(1, bytes, 3);
        let mut u = Pl011::with_rl78(
            PORT,
            None,
            Some(Rl78Config {
                flash_bytes: 16 * 1024,
            }),
        )
        .unwrap();
        let mut h = RecordingHost::default();
        exchange(&mut u, &mut h, &command(&[0x9a, 0, 0x21]));
        exchange(&mut u, &mut h, &command(&[0]));
        exchange(&mut u, &mut h, &command(&[0x40, 0, 0, 0, 0xff, 3, 0]));
        for i in 0..4 {
            let reply = exchange(
                &mut u,
                &mut h,
                &frame(2, &[0x5a; 256], if i == 3 { 3 } else { 0x17 }),
            );
            assert_eq!(&reply[..6], &[2, 2, 6, 6, 0xf2, 3]);
        }
        // Retain a large receive queue and a partial command as well as flash.
        for _ in 0..30 {
            for byte in command(&[0]) {
                write(&mut u, &mut h, DR, 1, u64::from(byte));
            }
        }
        for byte in [1, 1] {
            write(&mut u, &mut h, DR, 1, byte);
        }
        let diagnostic = u.save();
        assert!(u.sent().len() > 128);
        assert_eq!(diagnostic["sent"], serde_json::json!(u.sent()));
        assert_eq!(diagnostic["rx"], serde_json::json!({"bytes": 150}));
        assert_eq!(diagnostic["rl78"]["frame"], serde_json::json!({"bytes": 2}));
        assert_eq!(
            diagnostic["rl78"]["flash"],
            serde_json::json!({"bytes": 16384})
        );
        let saved = u.checkpoint().unwrap();
        assert!(
            saved.bytes() < 20_000,
            "RL78 flash must use a CBOR byte string"
        );

        let archive = saved.to_bytes().unwrap();
        let decoded =
            fujimod_emulator_devices::checkpoint::Checkpoint::from_bytes(&archive).unwrap();
        assert_eq!(decoded, saved);
        exchange(&mut u, &mut h, &[0, 0xff, 3]);
        exchange(&mut u, &mut h, &command(&[0x22, 0, 0, 0]));
        let mut restored = Pl011::with_rl78(
            PORT,
            None,
            Some(Rl78Config {
                flash_bytes: 16 * 1024,
            }),
        )
        .unwrap();
        restored.restore_checkpoint(&decoded).unwrap();
        assert_eq!(restored.checkpoint().unwrap(), saved);
        exchange(&mut restored, &mut h, &[0, 0xff, 3]);
        let blank = exchange(
            &mut restored,
            &mut h,
            &command(&[0x32, 0, 0, 0, 0xff, 3, 0, 0]),
        );
        assert_eq!(blank, [2, 1, 0x1b, 0xe4, 3]);

        let mut unpeered = Pl011::new(PORT, None).unwrap();
        for byte in 0..=255 {
            write(&mut unpeered, &mut h, DR, 1, byte);
        }
        let diagnostic = unpeered.save();
        assert_eq!(diagnostic["sent"], serde_json::json!(unpeered.sent()));
        let mut copy = Pl011::new(PORT, None).unwrap();
        copy.restore(&diagnostic).unwrap();
        assert_eq!(copy.sent(), unpeered.sent());
        assert_eq!(copy.sent_count(), unpeered.sent_count());
    }

    #[test]
    fn full_receive_queue_refuses_final_byte_without_consuming_peer_state() {
        let mut uart = Pl011::with_rl78(
            PORT,
            None,
            Some(Rl78Config {
                flash_bytes: 48 * 1024,
            }),
        )
        .unwrap();
        let mut h = RecordingHost::default();
        for byte in [1, 3, 0x9a, 0, 0x21, 0x42, 3, 1, 1, 0, 0xff, 3] {
            write(&mut uart, &mut h, DR, 1, byte);
        }
        uart.state.rx.clear();
        uart.update_rx();
        let cases: [([u8; 5], usize, usize); 2] =
            [([1, 1, 0, 0xff, 3], 5, 5), ([1, 1, 0xc0, 0x3f, 3], 31, 1)];
        for (command, reply_len, to_drain) in cases {
            for byte in &command[..4] {
                write(&mut uart, &mut h, DR, 1, u64::from(*byte));
            }
            uart.state.rx = vec![0xa5; RX_CAPACITY - reply_len + to_drain].into();
            uart.update_rx();
            let saved = uart.state.clone();
            let mut cx = Cx::new(&mut h, 0, 0, 0);
            uart.write(&mut cx, PORT + DR, 1, u64::from(command[4]));
            assert!(cx.is_unmodeled());
            assert_eq!(uart.state, saved);
            for _ in 0..to_drain {
                assert_eq!(read(&mut uart, &mut h, DR), 0xa5);
            }
            write(&mut uart, &mut h, DR, 1, u64::from(command[4]));
            for _ in 0..RX_CAPACITY - reply_len {
                assert_eq!(read(&mut uart, &mut h, DR), 0xa5);
            }
            let reply: Vec<_> = (0..reply_len)
                .map(|_| read(&mut uart, &mut h, DR) as u8)
                .collect();
            assert_eq!(&reply[..5], &[2, 1, 6, 0xf9, 3]);
            if reply_len == 31 {
                assert_eq!(&reply[5..7], &[2, 22]);
            }
            assert!(uart.state.rx.is_empty());
        }
    }

    #[test]
    fn a_checkpoint_keeps_the_registers_and_refuses_impossible_states() {
        let mut u = Pl011::new(PORT, Some(253)).unwrap();
        let mut h = RecordingHost::default();
        write(&mut u, &mut h, IBRD, 4, 11);
        for byte in [0x01, 0x03, 0x9a] {
            write(&mut u, &mut h, DR, 1, byte);
        }
        let saved = u.checkpoint().unwrap();
        let mut resumed = Pl011::new(PORT, Some(253)).unwrap();
        resumed.restore_checkpoint(&saved).unwrap();
        assert_eq!(resumed.state, u.state);
        assert_eq!(resumed.fingerprint().unwrap(), u.fingerprint().unwrap());
        let mut other = Pl011::new(PORT, None).unwrap();
        other.restore(&u.save()).unwrap();
        assert_eq!(other.sent(), &[0x01, 0x03, 0x9a]);
        for bad in [
            serde_json::json!({"ibrd": 0x10000}),
            serde_json::json!({"ris": 1}),
            serde_json::json!({"sent_count": 2}),
        ] {
            let mut value = u.save();
            for (key, field) in bad.as_object().unwrap() {
                value[key] = field.clone();
            }
            assert!(other.restore(&value).is_err(), "{bad}");
        }
        let mut value = u.save();
        value["extra"] = 1.into();
        assert!(other.restore(&value).is_err());
    }
}
