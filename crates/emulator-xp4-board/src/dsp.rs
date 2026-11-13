//! The XP4 still DSP's host interface (X-T3 5.11 driver).
//!
//! The still step "DSP" runs a program on a DSP core whose code the
//! firmware keeps in DRAM. What the DSP executes is not modeled: this is
//! the host side of the driver only, and a command finishes after declared
//! virtual time without writing anything. In an emulated X-T3 5.11 still
//! capture the still chain issues one command per border strip of the RAW
//! frame before the RECC step, so the strips keep their read-out contents.
//!
//! - Power-up: the driver sets the upper half of 0xfff4aa20 and 0xfff4aa10
//!   bits 0, 2 and 1; it drives the reset bits 5, 4, 3, 6 and 1..0 of
//!   0xff70f01c, a gate word of the control request block 0xff70f000 that is
//!   modeled there, and clears and sets bit 5 of the clock words
//!   0xff482000/0xff482004 (bits 15..0 are accepted; the boot setup script
//!   writes 0xffff and 0xff). It writes the program address to 0xfff4aa1c and
//!   0xfa000010, the memory type 0xfff4a760, 0xfa000000 = 0, the data
//!   addresses 0xfa000014/0xfa000018 and 0xfa000008, waits for 0xfa400518
//!   bit 1 to read clear, then clears 0xfff4aa10 bit 1.
//! - A command registers the driver's handler on interrupt line 145 (level),
//!   copies the command words to 0xfa000040, writes its length to
//!   0xfa00001c, waits for 0xfa000000 bit 16 and sets bit 0.
//! - The handler (and the driver's end) halts the DSP: it sets 0xfff4aa10
//!   bit 3, waits for 0xfa000000 bit 0 to read clear and clears bit 3.
//!
//! Bit 16 of 0xfa000000 reads set (ready) and bit 0 reads set while a
//! command runs. The finished-command interrupt holds until the halt request
//! (0xfff4aa10 bit 3); that the halt is what drops it is a declared
//! assumption, as the handler writes nothing else before returning.
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const CONTROL: u64 = 0xfa00_0000;
const CONTROL_WINDOW: u64 = 0x1000;
const STATUS: u64 = 0xfa40_0518;
const CLOCK: [u64; 2] = [0xff48_2000, 0xff48_2004];
const SYSTEM: u64 = 0xfff4_aa10;
const PROGRAM: u64 = 0xfff4_aa1c;
const GATES: u64 = 0xfff4_aa20;

const READY: u32 = 1 << 16;
const RUN: u32 = 1;
const HALT: u32 = 1 << 3;
/// Command words 0xfa000040.., copied with any access size.
const MAILBOX: std::ops::Range<u64> = 0x40..0x58;

/// Words outside the control page and the bits the driver may set.
fn outside(address: u64) -> Option<u32> {
    match address {
        // Bit 5 gates the DSP; the boot setup script writes 0xffff and 0xff to the words, so the other
        // gate bits are accepted and stored (their targets are not modeled).
        a if CLOCK.contains(&a) => Some(0xffff),
        SYSTEM => Some(0xf),
        PROGRAM => Some(u32::MAX),
        GATES => Some(u32::MAX),
        _ => None,
    }
}

/// Control-page words and the bits the driver may set.
fn control(offset: u64) -> Option<u32> {
    match offset {
        0x00 => Some(RUN | READY),
        0x08 => Some(0x0f0f_000f),
        0x10 | 0x14 | 0x18 | 0x1c => Some(u32::MAX),
        o if MAILBOX.contains(&o) => Some(u32::MAX),
        _ => None,
    }
}

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    words: BTreeMap<u64, u32>,
    mailbox: Vec<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    due: Option<u64>,
    #[serde(default)]
    finished: bool,
    #[serde(default)]
    commands: u64,
}

pub struct Dsp {
    irq: u32,
    ticks: u64,
    state: State,
}

impl Dsp {
    pub fn new(irq: u32, ticks: u64) -> Result<Self, String> {
        if !(32..512).contains(&irq) || ticks == 0 {
            return Err("Invalid XP4 DSP wiring".into());
        }
        Ok(Self {
            irq,
            ticks,
            state: State {
                mailbox: vec![0; (MAILBOX.end - MAILBOX.start) as usize],
                ..State::default()
            },
        })
    }
    /// The physical windows the DSP host interface owns.
    pub fn windows(&self) -> Vec<(u64, u64)> {
        let mut windows = vec![(CONTROL, CONTROL + CONTROL_WINDOW), (STATUS, STATUS + 4)];
        windows.extend([CLOCK[0], CLOCK[1], SYSTEM, PROGRAM, GATES].map(|a| (a, a + 4)));
        windows
    }
    fn word(&self, address: u64) -> u32 {
        self.state.words.get(&address).copied().unwrap_or(0)
    }
    fn store(&mut self, address: u64, value: u32) {
        if value == 0 {
            self.state.words.remove(&address);
        } else {
            self.state.words.insert(address, value);
        }
    }
    fn check(&self, state: &State) -> Result<(), String> {
        let declared = |a: &u64, v: &u32| {
            outside(*a)
                .or_else(|| control(a.wrapping_sub(CONTROL)).filter(|_| *a < STATUS))
                .is_some_and(|m| v & !m == 0)
        };
        if state.words.iter().any(|(a, v)| *v == 0 || !declared(a, v))
            || state.mailbox.len() != (MAILBOX.end - MAILBOX.start) as usize
            || (state.due.is_some() && state.finished)
        {
            return Err("Invalid XP4 DSP checkpoint".into());
        }
        Ok(())
    }
}

impl Device for Dsp {
    fn name(&self) -> &'static str {
        "xp4-dsp"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let offset = address.wrapping_sub(CONTROL);
        if offset < CONTROL_WINDOW && MAILBOX.contains(&offset) {
            let start = (offset - MAILBOX.start) as usize;
            let Some(bytes) = self.state.mailbox.get(start..start + usize::from(size)) else {
                cx.unmodeled();
                return 0;
            };
            return bytes
                .iter()
                .rev()
                .fold(0, |v, &b| v << 8 | u64::from(b));
        }
        if size != 4 || address & 3 != 0 {
            cx.unmodeled();
            return 0;
        }
        let value = if address == STATUS {
            // Bit 1 (busy) reads clear.
            0
        } else if outside(address).is_some() {
            self.word(address)
        } else if offset == 0 {
            let running = if self.state.due.is_some() { RUN } else { 0 };
            READY | running
        } else if offset < CONTROL_WINDOW && control(offset).is_some() {
            self.word(address)
        } else {
            cx.unmodeled();
            return 0;
        };
        u64::from(value)
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let offset = address.wrapping_sub(CONTROL);
        if offset < CONTROL_WINDOW && MAILBOX.contains(&offset) {
            let start = (offset - MAILBOX.start) as usize;
            let Some(bytes) = self
                .state
                .mailbox
                .get_mut(start..start + usize::from(size))
            else {
                return cx.unmodeled();
            };
            for (i, b) in bytes.iter_mut().enumerate() {
                *b = (value >> (8 * i)) as u8;
            }
            return;
        }
        let Ok(value) = u32::try_from(value) else {
            return cx.unmodeled();
        };
        if size != 4 || address & 3 != 0 {
            return cx.unmodeled();
        }
        let mask = if let Some(mask) = outside(address) {
            mask
        } else if offset < CONTROL_WINDOW
            && let Some(mask) = control(offset)
        {
            mask
        } else {
            return cx.unmodeled();
        };
        if value & !mask != 0 {
            return cx.fail(format!(
                "XP4 DSP {address:#x} write {value:#x} sets bits outside {mask:#x}"
            ));
        }
        if address == CONTROL {
            if value & RUN != 0 {
                if self.state.due.is_some() || self.state.finished {
                    return cx.fail("XP4 DSP command started before the last one was halted");
                }
                let Some(due) = cx.now().checked_add(self.ticks) else {
                    return cx.fail("XP4 DSP deadline overflow");
                };
                self.state.due = Some(due);
            }
            return;
        }
        if address == SYSTEM && value & HALT != 0 && self.state.finished {
            self.state.finished = false;
            cx.set_irq(self.irq, false);
        }
        self.store(address, value);
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        if !self.state.due.is_some_and(|at| at <= cx.now()) {
            return;
        }
        self.state.due = None;
        self.state.commands += 1;
        self.state.finished = true;
        cx.set_irq(self.irq, true);
    }
    fn next_deadline(&self, _now: u64) -> Option<u64> {
        self.state.due
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

    const IRQ: u32 = 145;

    fn access(d: &mut Dsp, h: &mut RecordingHost, a: u64, size: u8, v: Option<u32>) -> u32 {
        let mut cx = Cx::new(h, 0, 0, 0);
        let r = match v {
            Some(v) => {
                d.write(&mut cx, a, size, v.into());
                0
            }
            None => d.read(&mut cx, a, size) as u32,
        };
        assert!(cx.refusal().is_none(), "{a:#x}");
        r
    }

    #[test]
    fn a_command_finishes_after_its_ticks_and_holds_its_interrupt_until_halted() {
        let mut d = Dsp::new(IRQ, 30).unwrap();
        let mut h = RecordingHost::default();
        // Boot leaves 0xfff4aa10 = 7 and 0xfff4aa20 = 0xffff0000.
        access(&mut d, &mut h, SYSTEM, 4, Some(7));
        access(&mut d, &mut h, GATES, 4, Some(0xffff_0000));
        // Clock bit 5 set; program and data.
        for a in CLOCK {
            let v = access(&mut d, &mut h, a, 4, None);
            access(&mut d, &mut h, a, 4, Some(v | 0x20));
        }
        access(&mut d, &mut h, PROGRAM, 4, Some(0x014b_1400));
        access(&mut d, &mut h, CONTROL + 0x10, 4, Some(0x014b_1400));
        access(&mut d, &mut h, CONTROL, 4, Some(0));
        assert_eq!(access(&mut d, &mut h, STATUS, 4, None) & 2, 0);
        // Command words, then the length.
        access(&mut d, &mut h, CONTROL + 0x40, 4, Some(0x1234_5678));
        access(&mut d, &mut h, CONTROL + 0x44, 2, Some(0x9abc));
        assert_eq!(access(&mut d, &mut h, CONTROL + 0x44, 2, None), 0x9abc);
        access(&mut d, &mut h, CONTROL + 0x1c, 4, Some(0x18));
        let control = access(&mut d, &mut h, CONTROL, 4, None);
        assert_eq!(control & READY, READY);
        access(&mut d, &mut h, CONTROL, 4, Some(control | RUN));
        assert_eq!(access(&mut d, &mut h, CONTROL, 4, None) & RUN, RUN);
        h.now = 30;
        let mut cx = Cx::new(&mut h, 0, 0, 0);
        d.advance(&mut cx);
        assert_eq!(h.irqs.last(), Some(&(IRQ, true)));
        // The handler's halt.
        let system = access(&mut d, &mut h, SYSTEM, 4, None);
        access(&mut d, &mut h, SYSTEM, 4, Some(system | HALT));
        assert_eq!(h.irqs.last(), Some(&(IRQ, false)));
        assert_eq!(access(&mut d, &mut h, CONTROL, 4, None) & RUN, 0);
        access(&mut d, &mut h, SYSTEM, 4, Some(system & !HALT));
        let saved = d.save();
        let mut other = Dsp::new(IRQ, 30).unwrap();
        other.restore(&saved).unwrap();
        assert_eq!(access(&mut other, &mut h, PROGRAM, 4, None), 0x014b_1400);
    }

    #[test]
    fn undeclared_words_and_bits_refuse() {
        let mut d = Dsp::new(IRQ, 30).unwrap();
        let mut h = RecordingHost::default();
        for (a, v) in [
            (CONTROL + 0x04, 0),
            (CONTROL + 0x60, 0),
            (CONTROL, 2),
            (CLOCK[0], 0x10000),
            (SYSTEM, 0x10),
        ] {
            let mut cx = Cx::new(&mut h, 0, 0, 0);
            d.write(&mut cx, a, 4, v);
            assert!(cx.refusal().is_some(), "{a:#x} {v:#x}");
        }
    }
}
