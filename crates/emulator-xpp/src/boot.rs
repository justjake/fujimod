//! X-Processor Pro cold-boot reset, DDR startup, and low-memory remapping.
//! External boot-selection contacts and register writes are the only inputs.
use fujimod_emulator_devices::{Cx, Device, signals::Level};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub selector_base: u64,
    pub clock_base: u64,
    pub ddr_base: u64,
    pub remap_base: u64,
    pub bus_bases: Vec<u64>,
    pub memory_port_bases: Vec<u64>,
    pub decoder: usize,
    pub normal_selector: u32,
    pub card_selector: u32,
    pub settle_ticks: u64,
    pub refresh_level: u8,
}
#[derive(Serialize, Deserialize)]
struct State {
    remap: u32,
    io_words: std::collections::BTreeMap<u64, u32>,
}
pub struct BootController {
    options: Options,
    contacts: Vec<Level>,
    state: State,
    gates: crate::resets::Gates,
    power: crate::power::Control,
    dram: crate::dram::Controller,
}
impl BootController {
    pub fn new(options: Options, contacts: Vec<Level>) -> Result<Self, String> {
        if options.settle_ticks == 0
            || contacts.is_empty()
            || options
                .bus_bases
                .iter()
                .chain(&options.memory_port_bases)
                .any(|a| a & 0xfff != 0)
            || [
                options.selector_base,
                options.clock_base,
                options.ddr_base,
                options.remap_base,
            ]
            .iter()
            .any(|a| a & 0xfff != 0)
        {
            return Err("Invalid X-Processor Pro boot wiring".into());
        }
        let gates = crate::resets::Gates::new(options.selector_base)?.with_selector(
            contacts.clone(),
            options.normal_selector,
            options.card_selector,
        );
        let power =
            crate::power::Control::new(options.clock_base)?.with_startup(options.settle_ticks)?;
        let dram = crate::dram::Controller::new(options.ddr_base, options.refresh_level)?
            .with_startup(options.settle_ticks)?;
        Ok(Self {
            options,
            contacts,
            gates,
            power,
            dram,
            state: State {
                remap: 0,
                io_words: Default::default(),
            },
        })
    }
    /// Reset signals of the same gate bank used during cold boot.
    pub fn sd_reset(&self, channel: usize) -> Result<crate::resets::ResetPulses, String> {
        self.gates.sd_reset(channel)
    }
    pub fn external_lines(&self) -> crate::resets::ExternalLines {
        self.gates.external_lines()
    }
    fn transfer(
        &mut self,
        cx: &mut Cx<'_>,
        address: u64,
        size: u8,
        value: Option<u64>,
    ) -> Result<u64, String> {
        if size != 4 || address & 3 != 0 || value.is_some_and(|v| v > u32::MAX.into()) {
            return Err("Boot register access width/alignment".into());
        }
        self.advance(cx);
        let o = &self.options;
        let device: Option<&mut dyn Device> =
            if (o.selector_base..o.selector_base + 0x1000).contains(&address) {
                Some(&mut self.gates)
            } else if (o.clock_base..o.clock_base + 0x1000).contains(&address) {
                Some(&mut self.power)
            } else if (o.ddr_base..o.ddr_base + 0x1000).contains(&address) {
                Some(&mut self.dram)
            } else {
                None
            };
        if let Some(device) = device {
            return Ok(match value {
                Some(value) => {
                    device.write(cx, address, size, value);
                    0
                }
                None => device.read(cx, address, size),
            });
        }
        if o.bus_bases.iter().any(|base| address == base + 0x100) {
            let word = self.state.io_words.entry(address).or_default();
            if let Some(v) = value {
                if v > 3 {
                    return Err("Unsupported bus port selection".into());
                }
                *word = v as u32;
            }
            return Ok((*word).into());
        }
        if o.memory_port_bases
            .iter()
            .any(|base| address == base + 0x104)
        {
            let word = self.state.io_words.entry(address).or_default();
            if let Some(v) = value {
                if v > 15 {
                    return Err("Unsupported memory port access mask".into());
                }
                *word = v as u32;
            }
            return Ok((*word).into());
        }
        if address == o.remap_base {
            if let Some(v) = value {
                if v != 1 {
                    return Err("Unsupported low-memory remap selection".into());
                }
                cx.select_memory(o.decoder, 1)?;
                self.state.remap = 1;
            }
            return Ok(self.state.remap.into());
        }
        Err(format!("Unmapped boot register {address:#x}"))
    }
}
impl Device for BootController {
    fn name(&self) -> &'static str {
        "xpp-boot"
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
        self.power.advance(cx);
        self.dram.advance(cx);
    }
    fn next_deadline(&self, now: u64) -> Option<u64> {
        [self.power.next_deadline(now), self.dram.next_deadline(now)]
            .into_iter()
            .flatten()
            .min()
    }
    fn save(&self) -> Value {
        json!({"state":self.state,"contacts":self.contacts.iter().map(Level::high).collect::<Vec<_>>(), "gates": self.gates.save(), "power": self.power.save(), "dram": self.dram.save()})
    }
    fn checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        fujimod_emulator_devices::checkpoint::Checkpoint::encode(
            self.name(),
            &(&self.options, self.save()),
        )
    }
    fn validate_checkpoint(
        &self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<(), String> {
        let (options, state): (Options, Value) = saved.decode(self.name())?;
        let contacts: Vec<bool> =
            serde_json::from_value(state["contacts"].clone()).map_err(|e| e.to_string())?;
        if options != self.options || contacts.len() != self.contacts.len() {
            return Err("Boot checkpoint wiring differs".into());
        }
        let mut probe = Self::new(options, contacts.iter().copied().map(Level::new).collect())?;
        probe.restore(&state)?;
        Ok(())
    }
    fn restore_checkpoint(
        &mut self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        let (_, state): (Options, Value) = saved.decode(self.name())?;
        self.restore(&state)
    }
    fn fingerprint(&self) -> Result<Value, String> {
        Ok(self.checkpoint()?.fingerprint())
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let state: State =
            serde_json::from_value(value["state"].clone()).map_err(|e| e.to_string())?;
        let contacts: Vec<bool> =
            serde_json::from_value(value["contacts"].clone()).map_err(|e| e.to_string())?;
        if contacts.len() != self.contacts.len() {
            return Err("Boot checkpoint contacts differ".into());
        }
        self.gates.restore(&value["gates"])?;
        self.power.restore(&value["power"])?;
        self.dram.restore(&value["dram"])?;
        self.state = state;
        for (contact, high) in self.contacts.iter().zip(contacts) {
            contact.drive(high);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::test_host::{TestHost, advance, read, write};
    #[test]
    fn physical_contacts_and_controller_deadlines_restore_without_guest_inspection() {
        let levels = vec![Level::new(false), Level::new(false)];
        let options = Options {
            selector_base: 0x10000,
            clock_base: 0x20000,
            ddr_base: 0x30000,
            remap_base: 0x40000,
            bus_bases: vec![0x60000],
            memory_port_bases: vec![0x70000],
            decoder: 0,
            normal_selector: 0x18,
            card_selector: 0,
            settle_ticks: 5,
            refresh_level: 0,
        };
        let mut d = BootController::new(options, levels.clone()).unwrap();
        let reset = d.sd_reset(0).unwrap();
        let mut h = TestHost::new();
        assert_eq!(read(&mut d, &mut h, 0x10000), 0x18);
        levels[0].drive(true);
        assert_eq!(read(&mut d, &mut h, 0x10000), 0x18);
        levels[1].drive(true);
        assert_eq!(read(&mut d, &mut h, 0x10000), 0);
        write(&mut d, &mut h, 0x30000 + 0x14c, 0xffffffff);
        assert_eq!(read(&mut d, &mut h, 0x3014c) & 8, 0);
        write(&mut d, &mut h, 0x2000c, 0x1234);
        write(&mut d, &mut h, 0x30000, 1);
        write(&mut d, &mut h, 0x1001c, 1 << 18);
        write(&mut d, &mut h, 0x1001c, 0);
        assert_eq!(reset.count(), 1);
        let saved = d.checkpoint().unwrap();
        advance(&mut d, &mut h, 4);
        assert_eq!(read(&mut d, &mut h, 0x2002c) & 0xc0000000, 0);
        assert_eq!(read(&mut d, &mut h, 0x3014c) & 8, 0);
        advance(&mut d, &mut h, 5);
        assert_eq!(read(&mut d, &mut h, 0x2002c), 0xc0001234);
        assert_eq!(read(&mut d, &mut h, 0x3014c) & 8, 8);
        levels[0].drive(false);
        write(&mut d, &mut h, 0x1001c, 1 << 18);
        write(&mut d, &mut h, 0x1001c, 0);
        assert_eq!(reset.count(), 2);
        d.restore_checkpoint(&saved).unwrap();
        assert_eq!(reset.count(), 1);
        h.now = 0;
        assert!(levels[0].high());
        assert_eq!(d.next_deadline(0), Some(5));
        assert_eq!(read(&mut d, &mut h, 0x3014c) & 8, 0);
        advance(&mut d, &mut h, 5);
        assert_eq!(read(&mut d, &mut h, 0x3014c) & 8, 8);
        write(&mut d, &mut h, 0x70104, 15);
        assert_eq!(read(&mut d, &mut h, 0x70104), 15);
        let mut cx = h.cx();
        d.write(&mut cx, 0x60104, 4, 1);
        assert!(cx.refusal().is_some());
    }
}
