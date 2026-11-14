//! Arm PrimeCell PL011 UART (r1p5) with nothing attached to its far end.
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
//! receiver stays empty: FR reports an empty receive FIFO and DR reads 0. No
//! baud rate timing is modeled. MIS = RIS & IMSC drives the optional level
//! interrupt line.
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::Value;

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
/// RIS/IMSC/MIS/ICR: transmit interrupt.
pub const TX_INTERRUPT: u32 = 1 << 5;
/// Interrupt bits 10..0 exist; the receive side never raises one here.
const INTERRUPTS: u32 = 0x7ff;
/// Bytes kept from what the port transmitted.
pub const KEPT_BYTES: usize = 4096;
/// PeriphID0..3 and PCellID0..3 of an r1p5 PL011.
const ID_WORDS: [u32; 8] = [0x11, 0x10, 0x34, 0x00, 0x0d, 0xf0, 0x05, 0xb1];

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
        if base & 0xfff != 0
            || base.checked_add(WINDOW).is_none()
            || irq.is_some_and(|line| !(32..512).contains(&line))
        {
            return Err("Invalid PL011 UART".into());
        }
        Ok(Self {
            base,
            irq,
            state: State::default(),
        })
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
            || state.ris & !TX_INTERRUPT != 0
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
        let s = &self.state;
        u64::from(match offset {
            DR | RSR => 0,
            FR => TX_EMPTY | RX_EMPTY,
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
                if s.sent.len() < KEPT_BYTES {
                    s.sent.push(value as u8);
                }
                s.sent_count += 1;
                s.ris |= TX_INTERRUPT;
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
        self.update(cx);
    }
    fn pollable(&self, address: u64, size: u8) -> bool {
        self.offset(address, size)
            .is_some_and(|o| matches!(o, FR | RIS | MIS))
    }
    fn save(&self) -> Value {
        serde_json::to_value(&self.state).unwrap_or(Value::Null)
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
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
