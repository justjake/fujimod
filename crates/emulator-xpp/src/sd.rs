//! The X-Processor Pro SD wrapper: reset at +0 and a relocated SDHCI bank.
//! Word accesses are decomposed into the standard registers they cover.
//! Vendor words +0x20, +0x24 and +0xec read back their last write (X-T20
//! 2.11 0xc6592c..0xc65a48 programs bit fields there before the UHS-II
//! attempt); what they configure is not modeled. +0x10 is an update
//! handshake (0xc64ac8): bits 0-5 select and 8-12 value, bit 24 requests the
//! update and bit 26 acknowledges it while bit 24 is set (polled up to 1000
//! times at 0xc64bd8); updates complete at once. An optional reset input
//! line resets the SDHCI bank once per pulse, before the next access.
use fujimod_emulator_devices::{Cx, Device};
use fujimod_emulator_ip::sdhci::host::{self, SdHost};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

const VENDOR: [u64; 4] = [0x10, 0x20, 0x24, 0xec];
const UPDATE: u64 = 0x10;
const UPDATE_REQUEST: u32 = 1 << 24;
const UPDATE_DONE: u32 = 1 << 26;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub base: u64,
    pub register_offset: u64,
    pub transfer_ticks: u64,
    pub clock_ticks: u64,
    pub reset_ticks: u64,
    pub rca: u16,
    pub ram_bytes: u64,
    /// The card offers high speed (CMD6 group 1 function 1).
    #[serde(default)]
    pub high_speed: bool,
}
type Saved = (Options, Option<u64>, BTreeMap<u64, u32>, Option<u64>);
pub struct Controller {
    options: Options,
    host: SdHost,
    reset_due: Option<u64>,
    vendor: BTreeMap<u64, u32>,
    reset: Option<(crate::resets::ResetPulses, u64)>,
}
impl Controller {
    pub fn new(
        options: Options,
        card: fujimod_emulator_devices::parts::sd_card::Card,
        contacts: fujimod_emulator_devices::parts::card_contacts::Contacts,
        irq: Option<fujimod_emulator_devices::bus::InterruptLine>,
    ) -> Result<Self, String> {
        if !matches!(options.register_offset, 0x100 | 0x200) || options.reset_ticks == 0 {
            return Err("Invalid X-Processor Pro SD register wiring".into());
        }
        let host_options = host::Options {
            base: options.base,
            protocol: host::Protocol::Sdhc,
            transfer_ticks: options.transfer_ticks,
            clock_ticks: options.clock_ticks,
            rca: options.rca,
            ram_bytes: options.ram_bytes,
            initialization_fault: host::InitializationFault::None,
            high_speed: options.high_speed,
        };
        let host = if let Some(irq) = irq {
            SdHost::new(
                host_options,
                host::Wiring {
                    card,
                    contacts,
                    irq,
                },
            )?
        } else {
            SdHost::new_polled(host_options, card, contacts)?
        };
        Ok(Self {
            options,
            host,
            reset_due: None,
            vendor: BTreeMap::new(),
            reset: None,
        })
    }
    /// Connect a hardware reset line; pulses before this call are ignored.
    pub fn with_reset(mut self, line: crate::resets::ResetPulses) -> Self {
        let seen = line.count();
        self.reset = Some((line, seen));
        self
    }
    fn transfer(
        &mut self,
        cx: &mut Cx<'_>,
        address: u64,
        size: u8,
        value: Option<u64>,
    ) -> Result<u64, String> {
        if !matches!(size, 1 | 2 | 4) || value.is_some_and(|v| v >> (size * 8) != 0) {
            return Err("SD register access width/value".into());
        }
        self.advance(cx);
        if let Some((line, seen)) = &mut self.reset
            && line.count() != *seen
        {
            *seen = line.count();
            self.reset_due = None;
            self.host.hardware_reset(cx)?;
        }
        let vendor = address.wrapping_sub(self.options.base);
        if size == 4 && VENDOR.contains(&vendor) {
            if let Some(v) = value {
                if vendor == UPDATE && v as u32 & UPDATE_DONE != 0 {
                    return Err("SD update acknowledge is read-only".into());
                }
                self.vendor.insert(vendor, v as u32);
                return Ok(0);
            }
            let word = self.vendor.get(&vendor).copied().unwrap_or(0);
            return Ok(if vendor == UPDATE && word & UPDATE_REQUEST != 0 {
                word | UPDATE_DONE
            } else {
                word
            }
            .into());
        }
        if address == self.options.base && size == 4 {
            if let Some(v) = value {
                if v != 1 {
                    return Err("Unsupported SD wrapper reset command".into());
                }
                self.reset_due = Some(
                    cx.now()
                        .checked_add(self.options.reset_ticks)
                        .ok_or("SD reset deadline overflow")?,
                );
                self.host.write(cx, self.options.base + 0x2f, 1, 1);
                cx.schedule(self.reset_due.unwrap(), 0);
            }
            return Ok(u64::from(self.reset_due.is_some()));
        }
        let Some(offset) = address.checked_sub(self.options.base + self.options.register_offset)
        else {
            return Err(format!(
                "SD access outside register bank: +{:#x}/{size}",
                address - self.options.base
            ));
        };
        if offset + u64::from(size) > 0x108 || address & u64::from(size - 1) != 0 {
            return Err("SD register alignment/extent".into());
        }
        if self.reset_due.is_some() {
            return Err("SD register access during wrapper reset".into());
        }
        let address = self.options.base + offset;
        if size == 4 && offset == 0x28 {
            if value.is_some_and(|v| v >> 16 != 0) {
                return Err("SD block-gap/wakeup control is unsupported".into());
            }
            if let Some(v) = value {
                self.host.write(cx, address, 1, v & 255);
                self.host.write(cx, address + 1, 1, (v >> 8) & 255);
                return Ok(0);
            }
            return Ok(self.host.read(cx, address, 1) | self.host.read(cx, address + 1, 1) << 8);
        }
        let atomic_word = matches!(offset, 0 | 8 | 12 | 0x20 | 0x24);
        if size == 4 && !atomic_word {
            if let Some(v) = value {
                self.host.write(cx, address, 2, v & 0xffff);
                self.host.write(cx, address + 2, 2, v >> 16);
                Ok(0)
            } else {
                Ok(self.host.read(cx, address, 2) | self.host.read(cx, address + 2, 2) << 16)
            }
        } else if let Some(v) = value {
            self.host.write(cx, address, size, v);
            Ok(0)
        } else {
            Ok(self.host.read(cx, address, size))
        }
    }
}
impl Device for Controller {
    fn name(&self) -> &'static str {
        "xpp-sd"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        match self.transfer(cx, address, size, None) {
            Ok(v) => v,
            Err(e) => {
                cx.fail(e);
                0
            }
        }
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        if let Err(e) = self.transfer(cx, address, size, Some(value)) {
            cx.fail(e);
        }
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        if self.reset_due.is_some_and(|t| t <= cx.now()) {
            self.reset_due = None;
        }
        self.host.advance(cx);
    }
    fn event(&mut self, cx: &mut Cx<'_>, _: u64) {
        self.advance(cx);
    }
    fn next_deadline(&self, now: u64) -> Option<u64> {
        self.reset_due
            .into_iter()
            .chain(self.host.next_deadline(now))
            .min()
    }
    fn save(&self) -> Value {
        json!({"reset_due":self.reset_due,"vendor":self.vendor,"reset_seen":self.reset.as_ref().map(|r| r.1),"host":self.host.save()})
    }
    fn card_slot(&self) -> Option<&(dyn fujimod_emulator_devices::capability::CardSlot + 'static)> {
        self.host.card_slot()
    }
    fn card_slot_mut(
        &mut self,
    ) -> Option<&mut (dyn fujimod_emulator_devices::capability::CardSlot + 'static)> {
        self.host.card_slot_mut()
    }
    fn checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        // The SD host owns the exact card, FIFO, pending command and clocks.
        Ok(fujimod_emulator_devices::checkpoint::Checkpoint::encode(
            self.name(),
            &(
                &self.options,
                self.reset_due,
                &self.vendor,
                self.reset.as_ref().map(|r| r.1),
            ),
        )?
        .with_children(vec![self.host.checkpoint()?]))
    }
    fn validate_checkpoint(
        &self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<(), String> {
        let (options, _, vendor, seen): Saved = saved.decode(self.name())?;
        if options != self.options {
            return Err("SD wrapper checkpoint wiring differs".into());
        }
        if vendor.keys().any(|offset| !VENDOR.contains(offset))
            || seen.is_some() != self.reset.is_some()
        {
            return Err("SD wrapper checkpoint vendor/reset shape differs".into());
        }
        if saved.children().len() != 1 {
            return Err("SD wrapper checkpoint child count differs".into());
        }
        self.host.validate_checkpoint(&saved.children()[0])
    }
    fn restore_checkpoint(
        &mut self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        self.host.restore_checkpoint(&saved.children()[0])?;
        let (_, due, vendor, seen): Saved = saved.decode(self.name())?;
        self.reset_due = due;
        self.vendor = vendor;
        if let (Some((_, current)), Some(seen)) = (&mut self.reset, seen) {
            *current = seen;
        }
        Ok(())
    }
    fn restore(&mut self, _: &Value) -> Result<(), String> {
        Err("SD wrapper restoration requires its full checkpoint".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::parts::{
        card_contacts::Contacts,
        sd_card::{Card, Options as CardOptions},
    };
    use fujimod_emulator_devices::test_host::{TestHost, advance, read, write};
    #[test]
    fn relocated_word_bank_initializes_card_and_reads_a_complete_pio_sector() {
        for bank in [0x100, 0x200] {
            let base = 0x80000;
            let registers = base + bank;
            let mut card = Card::new(CardOptions {
                sectors: 0x800000,
                max_dirty_sectors: 4,
            })
            .unwrap();
            let expected: Vec<u8> = (0..512).map(|i| (i % 251) as u8).collect();
            card.write_bytes(0, &expected).unwrap();
            let options = Options {
                base,
                register_offset: bank,
                transfer_ticks: 10,
                clock_ticks: 2,
                reset_ticks: 3,
                rca: 1,
                ram_bytes: 0x10000,
                high_speed: false,
            };
            let mut d = Controller::new(options, card, Contacts::new(true, false), None).unwrap();
            let mut h = TestHost::new();
            write(&mut d, &mut h, base, 1);
            let reset = d.checkpoint().unwrap();
            assert_eq!(read(&mut d, &mut h, base), 1);
            advance(&mut d, &mut h, 3);
            assert_eq!(read(&mut d, &mut h, base), 0);
            d.restore_checkpoint(&reset).unwrap();
            h.now = 0;
            assert_eq!(read(&mut d, &mut h, base), 1);
            advance(&mut d, &mut h, 3);
            write(&mut d, &mut h, registers + 0x2c, 0x3f03);
            advance(&mut d, &mut h, 5);
            write(&mut d, &mut h, registers + 0x2c, 0x3f07);
            write(&mut d, &mut h, registers + 0x28, 0xf00);
            write(&mut d, &mut h, registers + 0x34, 0xffffffff);
            let command = |d: &mut Controller, h: &mut TestHost, word, arg| {
                write(d, h, registers + 0x30, 0xffffffff);
                write(d, h, registers + 8, arg);
                write(d, h, registers + 12, word);
                let now = h.now + 10;
                advance(d, h, now);
                assert_ne!(read(d, h, registers + 0x30) & 1, 0);
            };
            for (word, arg) in [
                (0, 0),
                (0x081a0000, 0x1aa),
                (0x371a0000, 0),
                (0x29020000, 0x40100000),
                (0x02090000, 0),
                (0x031a0000, 0),
                (0x071b0000, 0x10600),
                (0x101a0000, 512),
                (0x371a0000, 0x10600),
                (0x061a0000, 2),
            ] {
                command(&mut d, &mut h, word, arg);
            }
            write(&mut d, &mut h, registers + 4, 0x10200);
            command(&mut d, &mut h, 0x113a0010, 0);
            write(&mut d, &mut h, registers + 0x30, 0x21);
            let saved = d.checkpoint().unwrap();
            for pass in 0..2 {
                let bytes: Vec<u8> = (0..128)
                    .flat_map(|_| (read(&mut d, &mut h, registers + 0x20) as u32).to_le_bytes())
                    .collect();
                assert_eq!(bytes, expected);
                if pass == 0 {
                    d.restore_checkpoint(&saved).unwrap();
                }
            }
            let now = h.now + 10;
            advance(&mut d, &mut h, now);
            assert_eq!(read(&mut d, &mut h, registers + 0x30) & 2, 2);
        }
    }
    #[test]
    fn vendor_words_read_back_and_a_reset_pulse_clears_the_sdhci_bank() {
        let base = 0xfff92000;
        let card = Card::new(CardOptions {
            sectors: 0x800000,
            max_dirty_sectors: 4,
        })
        .unwrap();
        let options = Options {
            base,
            register_offset: 0x200,
            transfer_ticks: 10,
            clock_ticks: 2,
            reset_ticks: 3,
            rca: 1,
            ram_bytes: 0x10000,
            high_speed: false,
        };
        let mut gates = crate::resets::Gates::new(0xfff0a000).unwrap();
        let mut d = Controller::new(options, card, Contacts::new(true, false), None)
            .unwrap()
            .with_reset(gates.sd_reset(0).unwrap());
        let mut h = TestHost::new();
        write(&mut d, &mut h, base + 0x20, 0x8770);
        write(&mut d, &mut h, base + 0x24, 0x790080c);
        write(&mut d, &mut h, base + 0xec, 1);
        assert_eq!(read(&mut d, &mut h, base + 0x24), 0x790080c);
        write(&mut d, &mut h, base + 0x10, 0x1000501);
        assert_eq!(read(&mut d, &mut h, base + 0x10), 0x5000501);
        write(&mut d, &mut h, base + 0x10, 0x501);
        assert_eq!(read(&mut d, &mut h, base + 0x10), 0x501);
        let half = |d: &mut Controller, h: &mut TestHost, offset: u64, value: Option<u64>| {
            let mut cx = h.cx();
            let result = match value {
                Some(v) => {
                    d.write(&mut cx, base + 0x200 + offset, 2, v);
                    0
                }
                None => d.read(&mut cx, base + 0x200 + offset, 2),
            };
            assert!(cx.refusal().is_none(), "{:?}", cx.refusal());
            result
        };
        half(&mut d, &mut h, 0x28, Some(0xbfc0));
        half(&mut d, &mut h, 0x3e, Some(0x107));
        let saved = d.checkpoint().unwrap();
        let gates_saved = gates.save();
        write(&mut gates, &mut h, 0xfff0a01c, 0x40000);
        write(&mut gates, &mut h, 0xfff0a01c, 0);
        assert_eq!(half(&mut d, &mut h, 0x3e, None), 0);
        assert_eq!(half(&mut d, &mut h, 0x28, None), 0);
        assert_eq!(read(&mut d, &mut h, base + 0x20), 0x8770);
        gates.restore(&gates_saved).unwrap();
        d.restore_checkpoint(&saved).unwrap();
        assert_eq!(half(&mut d, &mut h, 0x3e, None), 0x107);
    }
}
