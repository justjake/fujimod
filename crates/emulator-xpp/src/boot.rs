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
}
#[derive(Serialize, Deserialize)]
struct State {
    clocks: [u32; 12],
    #[serde(with = "fujimod_emulator_devices::checkpoint::array")]
    ddr: [u32; 512],
    routing: u32,
    result: u32,
    remap: u32,
    clock_due: Option<u64>,
    ddr_due: Option<u64>,
    io_words: std::collections::BTreeMap<u64, u32>,
}
pub struct BootController {
    options: Options,
    contacts: Vec<Level>,
    state: State,
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
        Ok(Self {
            options,
            contacts,
            state: State {
                clocks: [0; 12],
                ddr: [0; 512],
                routing: 0,
                result: 0,
                remap: 0,
                clock_due: None,
                ddr_due: None,
                io_words: Default::default(),
            },
        })
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
        if address == o.selector_base {
            if value.is_some() {
                return Err("Boot selector is an external input".into());
            }
            return Ok((if self.contacts.iter().all(Level::high) {
                o.card_selector
            } else {
                o.normal_selector
            })
            .into());
        }
        if address == o.selector_base + 0xd0 || address == o.selector_base + 0xd4 {
            let word = if address == o.selector_base + 0xd0 {
                &mut self.state.routing
            } else {
                &mut self.state.result
            };
            if let Some(v) = value {
                *word = v as u32;
            }
            return Ok((*word).into());
        }
        if [o.selector_base + 8, o.selector_base + 0x70].contains(&address) {
            let word = self.state.io_words.entry(address).or_default();
            if let Some(v) = value {
                *word = v as u32;
            }
            return Ok((*word).into());
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
        if (o.clock_base..o.clock_base + 0x30).contains(&address) {
            let offset = (address - o.clock_base) as usize;
            if !matches!(offset, 0 | 4 | 8 | 12 | 16 | 20 | 44) {
                return Err("Unsupported boot clock register".into());
            }
            if let Some(v) = value {
                if offset == 44 {
                    return Err("Boot clock status is read-only".into());
                }
                self.state.clocks[offset / 4] = v as u32;
                if offset == 12 {
                    self.state.clock_due = Some(
                        cx.now()
                            .checked_add(o.settle_ticks)
                            .ok_or("Clock deadline overflow")?,
                    );
                    cx.schedule(self.state.clock_due.unwrap(), 0);
                }
            }
            return Ok(self.state.clocks[offset / 4].into());
        }
        if (o.ddr_base..o.ddr_base + 0x800).contains(&address) {
            let offset = (address - o.ddr_base) as usize;
            if !(offset < 0x2e8 || (0x400..0x718).contains(&offset)) {
                return Err("Unsupported DDR register".into());
            }
            if let Some(v) = value {
                self.state.ddr[offset / 4] = if offset == 0x14c {
                    v as u32 & !8 | self.state.ddr[offset / 4] & 8
                } else {
                    v as u32
                };
                if offset == 0 && v & 1 != 0 {
                    self.state.ddr_due = Some(
                        cx.now()
                            .checked_add(o.settle_ticks)
                            .ok_or("DDR deadline overflow")?,
                    );
                    cx.schedule(self.state.ddr_due.unwrap(), 0);
                }
            }
            return Ok(self.state.ddr[offset / 4].into());
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
        if self.state.clock_due.is_some_and(|t| t <= cx.now()) {
            self.state.clock_due = None;
            self.state.clocks[11] = 0xc0000000;
        }
        if self.state.ddr_due.is_some_and(|t| t <= cx.now()) {
            self.state.ddr_due = None;
            self.state.ddr[0x14c / 4] |= 8;
        }
    }
    fn event(&mut self, cx: &mut Cx<'_>, _: u64) {
        self.advance(cx);
    }
    fn next_deadline(&self, _: u64) -> Option<u64> {
        self.state
            .clock_due
            .into_iter()
            .chain(self.state.ddr_due)
            .min()
    }
    fn save(&self) -> Value {
        json!({"state":self.state,"contacts":self.contacts.iter().map(Level::high).collect::<Vec<_>>()})
    }
    fn checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        fujimod_emulator_devices::checkpoint::Checkpoint::encode(
            self.name(),
            &(
                &self.options,
                &self.state,
                self.contacts.iter().map(Level::high).collect::<Vec<_>>(),
            ),
        )
    }
    fn validate_checkpoint(
        &self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<(), String> {
        let (options, _, contacts): (Options, State, Vec<bool>) = saved.decode(self.name())?;
        if options != self.options || contacts.len() != self.contacts.len() {
            return Err("Boot checkpoint wiring differs".into());
        }
        Ok(())
    }
    fn restore_checkpoint(
        &mut self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        let (_, state, contacts): (Options, State, Vec<bool>) = saved.decode(self.name())?;
        self.state = state;
        for (contact, high) in self.contacts.iter().zip(contacts) {
            contact.drive(high);
        }
        Ok(())
    }
    fn restore(&mut self, _: &Value) -> Result<(), String> {
        Err("Boot restoration requires its exact checkpoint".into())
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
        };
        let mut d = BootController::new(options, levels.clone()).unwrap();
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
        let saved = d.checkpoint().unwrap();
        advance(&mut d, &mut h, 4);
        assert_eq!(read(&mut d, &mut h, 0x2002c), 0);
        assert_eq!(read(&mut d, &mut h, 0x3014c) & 8, 0);
        advance(&mut d, &mut h, 5);
        assert_eq!(read(&mut d, &mut h, 0x2002c), 0xc0000000);
        assert_eq!(read(&mut d, &mut h, 0x3014c) & 8, 8);
        levels[0].drive(false);
        d.restore_checkpoint(&saved).unwrap();
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
