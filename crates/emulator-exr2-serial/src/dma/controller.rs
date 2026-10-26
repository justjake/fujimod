//! One physical FFF02000 register owner with explicitly selected channel modes.
//!
//! The channel engines retain their own transfer laws. This device owns the
//! shared aperture and dispatches each declared channel to exactly one mode.
use fujimod_emulator_devices::{Cx, Device, checkpoint::Checkpoint, graph::{Mapping, validate_mappings}};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const BASE: u64 = 0xfff02000;
const CHANNELS_END: u64 = BASE + 0x200;
const GLOBAL: u64 = BASE + 0x300;

pub struct ChannelMode {
    pub mappings: Vec<Mapping>,
    pub device: Box<dyn Device>,
}

struct Mode {
    mappings: Vec<Mapping>,
    channels: Vec<u8>,
    device: Box<dyn Device>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Wiring {
    modes: Vec<(String, Vec<Mapping>)>,
}

pub struct Controller {
    modes: Vec<Mode>,
    mappings: Vec<Mapping>,
    wiring: Wiring,
}

impl Controller {
    pub fn new(modes: Vec<ChannelMode>) -> Result<Self, String> {
        if modes.is_empty() {
            return Err("EXR II DMA controller requires at least one channel mode".into());
        }
        let mut occupied_channels = [false; 8];
        let mut mappings = Vec::new();
        let mut built = Vec::with_capacity(modes.len());
        let mut wiring = Vec::with_capacity(modes.len());
        for ChannelMode {
            mappings: mode_mappings,
            device,
        } in modes
        {
            if mode_mappings.is_empty() {
                return Err("EXR II DMA channel mode requires mapped registers".into());
            }
            validate_mappings(&mode_mappings, mappings.iter())?;
            let mut channels = Vec::new();
            let mut has_global = false;
            for mapping in &mode_mappings {
                let end = mapping
                    .base
                    .checked_add(mapping.size)
                    .ok_or("DMA window overflow")?;
                if mapping.base >= BASE && end <= CHANNELS_END {
                    let first = ((mapping.base - BASE) / 0x40) as u8;
                    let last = ((end - 1 - BASE) / 0x40) as u8;
                    if (mapping.base - BASE) % 0x40 != 0 || (end - BASE) % 0x40 != 0 {
                        return Err(
                            "EXR II DMA channel mode must own complete channel windows".into()
                        );
                    }
                    for channel in first..=last {
                        if occupied_channels[channel as usize] {
                            return Err(format!(
                                "EXR II DMA channel {channel} has overlapping modes"
                            ));
                        }
                        occupied_channels[channel as usize] = true;
                        channels.push(channel);
                    }
                } else if mapping.base == GLOBAL && mapping.size == 4 {
                    has_global = true;
                } else {
                    // A firmware-specific channel mode can also own its
                    // directly wired endpoint and selector apertures.
                }
            }
            if channels.is_empty() || has_global && !channels.contains(&0) {
                return Err("EXR II DMA mode owns no channel registers".into());
            }
            mappings.extend_from_slice(&mode_mappings);
            wiring.push((device.name().to_owned(), mode_mappings.clone()));
            built.push(Mode {
                mappings: mode_mappings,
                channels,
                device,
            });
        }
        mappings.sort_by_key(|mapping| mapping.base);
        Ok(Self {
            modes: built,
            mappings,
            wiring: Wiring { modes: wiring },
        })
    }

    pub fn mappings(&self) -> &[Mapping] {
        &self.mappings
    }

    fn mode(&self, address: u64, size: u8) -> Option<usize> {
        self.modes.iter().position(|mode| {
            mode.mappings.iter().any(|mapping| {
                address >= mapping.base
                    && address
                        .checked_add(u64::from(size))
                        .is_some_and(|end| end <= mapping.base + mapping.size)
            })
        })
    }

    fn state(&self) -> Value {
        json!({"modes": self.modes.iter().map(|mode| json!({
            "name": mode.device.name(), "channels": mode.channels,
            "state": mode.device.save()
        })).collect::<Vec<_>>()})
    }
}

impl Device for Controller {
    fn fast_reads(&self, window_base: u64, window_size: u64) -> Vec<fujimod_emulator_devices::bus::FastReadWord> {
        self.modes
            .iter()
            .flat_map(|mode| mode.device.fast_reads(window_base, window_size))
            .collect()
    }
    fn name(&self) -> &'static str {
        "exr2-dma-controller"
    }

    fn checkpoint(&self) -> Result<Checkpoint, String> {
        Ok(
            Checkpoint::encode(self.name(), &self.wiring)?.with_children(
                self.modes
                    .iter()
                    .map(|mode| mode.device.checkpoint())
                    .collect::<Result<Vec<_>, _>>()?,
            ),
        )
    }

    fn validate_checkpoint(&self, saved: &Checkpoint) -> Result<(), String> {
        let wiring: Wiring = saved.decode(self.name())?;
        if wiring.modes != self.wiring.modes
            || saved.children().len() != self.modes.len()
            || !saved.blobs().is_empty()
        {
            return Err("EXR II DMA controller checkpoint wiring differs".into());
        }
        for (mode, child) in self.modes.iter().zip(saved.children()) {
            mode.device.validate_checkpoint(child)?;
        }
        Ok(())
    }

    fn restore_checkpoint(&mut self, saved: &Checkpoint) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        for (mode, child) in self.modes.iter_mut().zip(saved.children()) {
            mode.device.restore_checkpoint(child)?;
        }
        Ok(())
    }

    fn fingerprint(&self) -> Result<Value, String> {
        Ok(
            json!({"wiring": self.wiring.modes, "modes": self.modes.iter().map(|mode| Ok(json!({
            "name": mode.device.name(), "channels": mode.channels,
            "state": mode.device.fingerprint()?
        }))).collect::<Result<Vec<_>, String>>()?}),
        )
    }

    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        self.advance(cx);
        let Some(index) = self.mode(address, size) else {
            cx.unmodeled();
            return 0;
        };
        self.modes[index].device.read(cx, address, size)
    }

    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        self.advance(cx);
        let Some(index) = self.mode(address, size) else {
            cx.unmodeled();
            return;
        };
        self.modes[index].device.write(cx, address, size, value);
    }

    fn access_contract(&self, address: u64, size: u8, value: Option<u64>) -> Option<String> {
        self.mode(address, size).and_then(|index| {
            self.modes[index]
                .device
                .access_contract(address, size, value)
        })
    }

    fn discovery_access(
        &mut self,
        cx: &mut Cx<'_>,
        address: u64,
        size: u8,
        value: Option<u64>,
    ) -> Option<u64> {
        let index = self.mode(address, size)?;
        self.modes[index]
            .device
            .discovery_access(cx, address, size, value)
    }

    fn pollable(&self, address: u64, size: u8) -> bool {
        self.mode(address, size)
            .is_some_and(|index| self.modes[index].device.pollable(address, size))
    }

    fn save(&self) -> Value {
        self.state()
    }

    fn restore(&mut self, state: &Value) -> Result<(), String> {
        if state["modes"]
            .as_array()
            .is_none_or(|modes| modes.len() != self.modes.len())
        {
            return Err("invalid EXR II DMA controller state".into());
        }
        let saved_modes = state["modes"].as_array().unwrap();
        for (mode, saved) in self.modes.iter().zip(saved_modes) {
            if saved["name"] != mode.device.name() || saved["channels"] != json!(mode.channels) {
                return Err("EXR II DMA channel mode state differs".into());
            }
        }
        let before = self
            .modes
            .iter()
            .map(|mode| mode.device.save())
            .collect::<Vec<_>>();
        for index in 0..self.modes.len() {
            if let Err(error) = self.modes[index]
                .device
                .restore(&saved_modes[index]["state"])
            {
                for (mode, state) in self.modes.iter_mut().zip(before) {
                    let _ = mode.device.restore(&state);
                }
                return Err(error);
            }
        }
        Ok(())
    }

    fn finish(&mut self) -> Result<(), String> {
        for mode in &mut self.modes {
            mode.device.finish()?;
        }
        Ok(())
    }

    fn advance(&mut self, cx: &mut Cx<'_>) {
        let now = cx.now();
        loop {
            let next = self
                .modes
                .iter()
                .filter_map(|mode| mode.device.next_deadline(now))
                .filter(|tick| *tick <= now)
                .min();
            let Some(tick) = next else {
                return;
            };
            let mut advanced = false;
            for mode in &mut self.modes {
                if mode
                    .device
                    .next_deadline(tick)
                    .is_some_and(|due| due <= tick)
                {
                    advanced = true;
                    cx.at_tick(tick, |cx| mode.device.advance(cx));
                    if cx.refusal().is_some() {
                        return;
                    }
                }
            }
            if !advanced {
                return;
            }
        }
    }

    fn next_deadline(&self, now: u64) -> Option<u64> {
        self.modes
            .iter()
            .filter_map(|mode| mode.device.next_deadline(now))
            .min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dma::{audio, memory};
    use fujimod_emulator_devices::exr2::irq_routes::IrqRoutes;
    use fujimod_emulator_devices::test_host::{TestHost, advance, read, write};

    fn controller() -> Controller {
        let memory = memory::MemoryDma::new(
            memory::Options {},
            memory::Wiring {
                base: BASE,
                routes: IrqRoutes::default(),
            },
        )
        .unwrap();
        let audio = audio::SerialDma::new(audio::Options {
            ram_bytes: 0x100000,
            max_transfer_bytes: 0x10000,
        })
        .unwrap();
        Controller::new(vec![
            ChannelMode {
                mappings: vec![
                    Mapping {
                        base: BASE,
                        size: 0x40,
                    },
                    Mapping {
                        base: GLOBAL,
                        size: 4,
                    },
                ],
                device: Box::new(memory),
            },
            ChannelMode {
                mappings: vec![Mapping {
                    base: BASE + 0x40,
                    size: 0x40,
                }],
                device: Box::new(audio),
            },
        ])
        .unwrap()
    }

    #[test]
    fn one_owner_dispatches_channel_modes_and_keeps_their_deadlines_and_state() {
        let mut d = controller();
        let mut host = TestHost::new();
        host.ram[0x1000..0x1020].fill(0x6d);
        for (address, value) in [
            (BASE, 0x1000),
            (BASE + 4, 0x2000),
            (BASE + 8, 64),
            (BASE + 0x2c, 0x1555400),
            (BASE + 0x28, 5),
            (BASE + 0x40, 0xfffa0020),
            (BASE + 0x44, 0x3000),
            (BASE + 0x48, 16),
            (BASE + 0x6c, 0x40122011),
        ] {
            write(&mut d, &mut host, address, value);
        }
        assert_eq!(d.mappings().len(), 3);
        assert_eq!(d.next_deadline(0), Some(2));
        let saved = d.checkpoint().unwrap();
        let before = d.fingerprint().unwrap();
        advance(&mut d, &mut host, 1);
        assert_eq!(d.fingerprint().unwrap(), before);
        advance(&mut d, &mut host, 2);
        assert_eq!(&host.ram[0x2000..0x2040], &[0x6d; 64]);
        assert_eq!(read(&mut d, &mut host, BASE + 0x24), 0x40);
        assert_eq!(read(&mut d, &mut host, BASE + 0x44), 0x3000);
        assert_eq!(d.next_deadline(2), None);
        let completed = d.fingerprint().unwrap();
        d.restore_checkpoint(&saved).unwrap();
        assert_eq!(d.fingerprint().unwrap(), before);
        advance(&mut d, &mut host, 2);
        assert_eq!(d.fingerprint().unwrap(), completed);
        assert_eq!(&host.ram[0x2000..0x2040], &[0x6d; 64]);
    }

    #[test]
    fn two_engines_cannot_claim_the_same_physical_channel() {
        let make = || {
            audio::SerialDma::new(audio::Options {
                ram_bytes: 0x100000,
                max_transfer_bytes: 0x10000,
            })
            .unwrap()
        };
        let window = vec![Mapping {
            base: BASE + 0x40,
            size: 0x40,
        }];
        assert!(
            Controller::new(vec![
                ChannelMode {
                    mappings: window.clone(),
                    device: Box::new(make())
                },
                ChannelMode {
                    mappings: window,
                    device: Box::new(make())
                },
            ])
            .is_err()
        );
    }
}
