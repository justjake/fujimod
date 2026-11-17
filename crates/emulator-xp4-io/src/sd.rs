//! The X-Processor 4 SD host: a standard SDHCI register bank at the window
//! base. Word accesses are decomposed into the standard registers they cover,
//! as the generic host exposes byte and halfword registers.
use fujimod_emulator_devices::{Cx, Device};
use fujimod_emulator_ip::sdhci::host::{self, SdHost};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// Size of the standard register bank this host decodes.
const BANK: u64 = 0x108;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub base: u64,
    pub transfer_ticks: u64,
    pub clock_ticks: u64,
    pub rca: u16,
    pub ram_bytes: u64,
    /// The card offers high speed (CMD6 group 1 function 1).
    #[serde(default)]
    pub high_speed: bool,
    /// Further DRAM banks SDMA reaches: (physical base, bytes).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ram_banks: Vec<(u64, u64)>,
    /// Capabilities register (+0x40).
    pub capabilities: u32,
    /// Capabilities 2 register (+0x44).
    pub capabilities_2: u32,
    /// Slot status halfword (+0xfc) and host controller version (+0xfe).
    pub slot_and_version: u32,
    /// Offsets of vendor words that read back their last write; a declared
    /// offset takes precedence over the standard bank.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub vendor_offsets: Vec<u64>,
}

pub struct Controller {
    options: Options,
    host: SdHost,
    vendor: BTreeMap<u64, u32>,
}

impl Controller {
    pub fn new(
        options: Options,
        card: fujimod_emulator_devices::parts::sd_card::Card,
        contacts: fujimod_emulator_devices::parts::card_contacts::Contacts,
        irq: Option<fujimod_emulator_devices::bus::InterruptLine>,
    ) -> Result<Self, String> {
        let host_options = host::Options {
            base: options.base,
            protocol: host::Protocol::Sdhc,
            transfer_ticks: options.transfer_ticks,
            clock_ticks: options.clock_ticks,
            rca: options.rca,
            ram_bytes: options.ram_bytes,
            initialization_fault: host::InitializationFault::None,
            high_speed: options.high_speed,
            ram_banks: options.ram_banks.clone(),
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
            vendor: BTreeMap::new(),
        })
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
        self.host.advance(cx);
        let offset = address
            .checked_sub(self.options.base)
            .ok_or("SD access below the register bank")?;
        if size == 4 && self.options.vendor_offsets.contains(&offset) {
            return Ok(match value {
                Some(v) => {
                    self.vendor.insert(offset, v as u32);
                    0
                }
                None => u64::from(self.vendor.get(&offset).copied().unwrap_or(0)),
            });
        }
        if offset + u64::from(size) > BANK || address & u64::from(size - 1) != 0 {
            return Err(format!(
                "SD access outside the register bank: +{offset:#x}/{size}"
            ));
        }
        // Read-only identity words the generic host does not carry.
        let identity = match offset {
            0x40 | 0x44 | 0xfc if size == 4 => Some(match offset {
                0x40 => self.options.capabilities,
                0x44 => self.options.capabilities_2,
                _ => self.options.slot_and_version,
            }),
            0x40 | 0x44 | 0xfc | 0xfe if size == 2 => {
                let word = match offset & !3 {
                    0x40 => self.options.capabilities,
                    0x44 => self.options.capabilities_2,
                    _ => self.options.slot_and_version,
                };
                Some(word >> ((offset & 2) * 8) & 0xffff)
            }
            _ => None,
        };
        if let Some(word) = identity {
            if value.is_some() {
                return Err("SD capabilities and version registers are read-only".into());
            }
            return Ok(u64::from(word));
        }
        if size == 4 && offset == 0x28 {
            // Host control and power control share one word; block-gap and
            // wakeup control (the upper halfword) are not modeled.
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
        "xp4-sd"
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
        self.host.advance(cx);
    }
    fn event(&mut self, cx: &mut Cx<'_>, _: u64) {
        self.host.advance(cx);
    }
    fn next_deadline(&self, now: u64) -> Option<u64> {
        self.host.next_deadline(now)
    }
    fn save(&self) -> Value {
        json!({"host":self.host.save(),"vendor":self.vendor})
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
            &(&self.options, &self.vendor),
        )?
        .with_children(vec![self.host.checkpoint()?]))
    }
    fn validate_checkpoint(
        &self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<(), String> {
        let (options, vendor): (Options, BTreeMap<u64, u32>) = saved.decode(self.name())?;
        if options != self.options || vendor.keys().any(|o| !self.options.vendor_offsets.contains(o)) {
            return Err("SD host checkpoint wiring differs".into());
        }
        if saved.children().len() != 1 {
            return Err("SD host checkpoint child count differs".into());
        }
        self.host.validate_checkpoint(&saved.children()[0])
    }
    fn restore_checkpoint(
        &mut self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        self.host.restore_checkpoint(&saved.children()[0])?;
        let (_, vendor): (Options, BTreeMap<u64, u32>) = saved.decode(self.name())?;
        self.vendor = vendor;
        Ok(())
    }
    fn restore(&mut self, _: &Value) -> Result<(), String> {
        Err("SD host restoration requires its full checkpoint".into())
    }
    fn fingerprint(&self) -> Result<Value, String> {
        let wrapper = fujimod_emulator_devices::checkpoint::Checkpoint::encode(
            self.name(),
            &(&self.options, &self.vendor),
        )?;
        Ok(json!({"wrapper":wrapper.state(),"host":self.host.fingerprint()?}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::parts::{
        card_contacts::Contacts,
        sd_card::{Card, Options as CardOptions},
    };
    use fujimod_emulator_devices::test_host::{TestHost, read, write};

    fn controller(base: u64) -> Controller {
        let card = Card::new(CardOptions {
            sectors: 0x800000,
            max_dirty_sectors: 4,
        })
        .unwrap();
        let options = Options {
            base,
            transfer_ticks: 10,
            clock_ticks: 2,
            rca: 1,
            ram_bytes: 0x10000,
            high_speed: false,
            ram_banks: vec![],
            capabilities: 0x25e8_30b0,
            capabilities_2: 0x2077,
            slot_and_version: 0x0102_0000,
            vendor_offsets: vec![0x100, 0x1e0],
        };
        Controller::new(options, card, Contacts::new(true, false), None).unwrap()
    }

    #[test]
    fn present_state_word_reports_the_inserted_card() {
        let base = 0x80000;
        let mut d = controller(base);
        let mut h = TestHost::new();
        let state = read(&mut d, &mut h, base + 0x24);
        assert_ne!(state & 0x0001_0000, 0, "card inserted");
        assert_ne!(state & 0x0002_0000, 0, "card state stable");
    }

    #[test]
    fn declared_vendor_words_read_back_their_last_write_and_others_are_refused() {
        let base = 0x80000;
        let mut d = controller(base);
        let mut h = TestHost::new();
        assert_eq!(read(&mut d, &mut h, base + 0x100), 0);
        assert_eq!(read(&mut d, &mut h, base + 0x1e0), 0);
        write(&mut d, &mut h, base + 0x1e0, 0x400);
        assert_eq!(read(&mut d, &mut h, base + 0x1e0), 0x400);
        let mut cx = h.cx();
        d.read(&mut cx, base + 0x1e4, 4);
        assert!(cx.refusal().is_some(), "undeclared vendor word");
    }

    #[test]
    fn word_reads_cover_the_capabilities_and_version_registers() {
        let base = 0x80000;
        let mut d = controller(base);
        let mut h = TestHost::new();
        assert_eq!(read(&mut d, &mut h, base + 0x40), 0x25e8_30b0);
        assert_eq!(read(&mut d, &mut h, base + 0x44), 0x2077);
        let version = read(&mut d, &mut h, base + 0xfc) >> 16;
        assert_eq!(version & 0xff, 2, "SDHCI 3.00");
    }
}
