//! Caller-described XP5 graphs shared by language bindings.
use crate::{board, boot, session::Inputs};
use fujimod_emulator::board::{InterruptMap, InterruptRegisters, aprofile, bus::Mode, factory};
use fujimod_emulator_cpu::{Model, arm64};
use fujimod_emulator_devices::{graph::DeviceWindow, parts::pins::DriveInput};
use fujimod_emulator_ip_arm::gicv3::Gic;
use fujimod_emulator_parts_controls::resistor_keys::{Options, ResistorKeys};
use fujimod_emulator_xp5_display::display;
use fujimod_emulator_xp5_io::mailbox;
use fujimod_emulator_xp5_storage::flash_storage;
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Entry {
    Rom,
    Loader,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, tag = "kind", rename_all = "lowercase")]
pub enum Definition {
    Xp5 {
        boot_entry: Entry,
        windows: Vec<DeviceWindow>,
        controls: Options,
        storage: Option<String>,
        #[serde(default)]
        sd: Vec<board::sd::MediaInputs>,
        companion: Option<Companion>,
        displays: Vec<Display>,
        interrupts: Option<Interrupts>,
        aprofile: Vec<(usize, aprofile::Config)>,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Companion {
    processor: usize,
    request: u32,
    control: u32,
    reply: u32,
    status: u32,
    fault: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Display {
    base: u64,
    period_ns: Option<u64>,
    raw_formats: [Option<String>; 4],
}
impl Display {
    fn config(self) -> Result<(u64, display::Config), String> {
        let mut config = display::Config {
            period_ns: self.period_ns,
            ..Default::default()
        };
        for (format, source) in config.raw_formats.iter_mut().zip(self.raw_formats) {
            *format = source
                .as_deref()
                .map(fujimod_emulator_graphics::raster::PixelOrder::parse)
                .transpose()?;
        }
        Ok((self.base, config))
    }
}

#[derive(Deserialize)]
#[serde(tag = "bank", rename_all = "kebab-case", deny_unknown_fields)]
pub enum InterruptWrite {
    Distributor {
        offset: u64,
        size: u8,
        value: u64,
    },
    Redistributor {
        core: usize,
        offset: u64,
        size: u8,
        value: u64,
    },
    System {
        core: usize,
        register: String,
        value: u64,
    },
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Interrupts {
    affinities: Vec<u32>,
    lines: usize,
    distributor: u64,
    redistributor: u64,
    stride: u64,
    initial: Vec<InterruptWrite>,
}
impl Interrupts {
    fn open(self) -> Result<InterruptMap, String> {
        let cores = self.affinities.len();
        let mut gic = Gic::new(self.affinities, self.lines)?;
        for write in self.initial {
            match write {
                InterruptWrite::Distributor {
                    offset,
                    size,
                    value,
                } => {
                    gic.distributor(offset, size, Some(value))?;
                }
                InterruptWrite::Redistributor {
                    core,
                    offset,
                    size,
                    value,
                } => {
                    gic.redistributor(core, offset, size, Some(value))?;
                }
                InterruptWrite::System {
                    core,
                    register,
                    value,
                } => {
                    gic.system(
                        core,
                        arm64::system_register(&register)?.encoding()?,
                        Some(value),
                    )?;
                }
            }
        }
        Ok(InterruptMap {
            gic: Box::new(gic),
            cores,
            registers: InterruptRegisters::GicV3 {
                distributor: self.distributor,
                redistributor: self.redistributor,
                stride: self.stride,
            },
        })
    }
}

impl Definition {
    /// Open declared devices through the standard XP5 board constructor.
    /// Assets have already passed the caller's size/hash identity checks.
    pub fn open(
        self,
        graph: factory::Graph,
        assets: &BTreeMap<String, Vec<u8>>,
        engine: factory::Engine,
        mode: Mode,
        definition: serde_json::Value,
    ) -> Result<(fujimod_emulator_session::Session, Inputs), String> {
        let Self::Xp5 {
            boot_entry: _,
            windows,
            controls,
            storage,
            sd,
            companion,
            displays,
            interrupts,
            aprofile,
        } = self;
        if companion.as_ref().is_some_and(|c| {
            graph
                .processors
                .get(c.processor)
                .is_none_or(|p| p.model != Model::CortexM4)
        }) {
            return Err("XP5 companion must name a declared M-profile processor".into());
        }
        let pins = fujimod_emulator_devices::xp5::pins::Pins::shared();
        let driver: Arc<dyn DriveInput> = pins.clone();
        let input = Arc::new(ResistorKeys::new(driver, controls.clone())?);
        let storage = storage
            .map(|name| {
                let bytes = assets
                    .get(&name)
                    .ok_or_else(|| format!("Missing XP5 storage asset {name}"))?;
                flash_storage::Storage::new(bytes.clone()).map(|s| Arc::new(Mutex::new(s)))
            })
            .transpose()?;
        let resources = board::Resources {
            sd: board::sd::resolve(sd, assets)?,
            controls: input,
            pins,
            storage,
            clocks: fujimod_emulator_devices::xp5::panel_clocks::Clocks::new(),
            companion: companion.map(|c| board::Companion {
                pins: mailbox::Pins {
                    companion: c.processor,
                    request: c.request,
                    control: c.control,
                    reply: c.reply,
                    status: c.status,
                    fault: c.fault,
                },
                channel: mailbox::Channel::default(),
            }),
            displays: displays
                .into_iter()
                .map(Display::config)
                .collect::<Result<_, _>>()?,
        };
        boot::open(boot::OpenOptions {
            definition,
            mode,
            processors: graph,
            windows,
            controls,
            resources,
            engine,
            interrupts: interrupts.map(Interrupts::open).transpose()?,
            aprofile,
            images: vec![],
            scene: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn specification() -> serde_json::Value {
        json!({"kind":"xp5","boot_entry":"rom",
            "windows":[{"kind":"gpio","base":0xff90b000u64,"size":0x1000,"options":{}}],
            "controls":{
                "layout":{"shutter_half":{"port":0,"pin":0},"shutter_full":{"port":0,"pin":1},
                    "rear_dial":[{"port":1,"pin":0},{"port":1,"pin":1}],
                    "front_dial":[{"port":2,"pin":0},{"port":2,"pin":1}]},
                "initial_levels":[
                {"pin":{"port":0,"pin":0},"high":true}, {"pin":{"port":0,"pin":1},"high":true},
                {"pin":{"port":1,"pin":0},"high":false}, {"pin":{"port":1,"pin":1},"high":false},
                {"pin":{"port":2,"pin":0},"high":false}, {"pin":{"port":2,"pin":1},"high":false}],"samples":{"menu":100,"back":200,"menu_back":150,
                    "up":300,"down":400,"left":500,"right":600},
                "detent_return_ns":100,"shutter_half_delay_ns":100},
            "storage":null,"companion":null,"displays":[],"interrupts":null,"aprofile":[]})
    }
    fn graph() -> factory::Graph {
        serde_json::from_value(json!({"processors":[{"model":"armv8a","released":true,
            "registers":[["Pc",0x10000],["Pstate",0x3cd],
                [{"Arm64System":{"op0":3,"op1":6,"crn":1,"crm":1,"op2":0}},0x401],
                [{"Arm64System":{"op0":3,"op1":4,"crn":1,"crm":1,"op2":0}},0x80000000u64],
                [{"X":1},0x11000]],"mprofile":null,"rprofile":null}],
            "ram":[{"base":0x10000,"size":0x10000}],
            "mappings":[{"core":0,"address":0x10000,"physical":0x10000,"size":0x10000,"permissions":7}],
            "dma_mappings":[{"address":0x10000,"physical":0x10000,"size":0x10000,"permissions":3}],
            "peripherals":[{"core":0,"address":0xff90b000u64,"physical":0xff90b000u64,"size":0x1000}],
            "decoders":[],"instructions_per_tick":[1,1]})).unwrap()
    }

    #[test]
    fn declared_xp5_graph_runs_at_el3_and_drives_physical_inputs() {
        let value = specification();
        let definition: Definition = serde_json::from_value(value.clone()).unwrap();
        let (mut session, inputs) = definition
            .open(
                graph(),
                &BTreeMap::new(),
                factory::Engine::Unicorn,
                Mode::Strict,
                value,
            )
            .unwrap();
        let code: Vec<u8> = [0xd5384240u32, 0xb9000020, 0x14000000]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect();
        session.board.write_physical(0x10000, &code).unwrap();
        inputs.gpio(&mut session, 0, 7, true).unwrap();
        inputs
            .key(&mut session, 0, crate::session::Key::Back, true)
            .unwrap();
        assert_eq!(
            session
                .board
                .read_register(0, fujimod_emulator_cpu::Register::Pstate)
                .unwrap(),
            0x3cd
        );
        session.board.run_for_instructions(4, 100).unwrap();
        let mut actual = [0; 4];
        session.board.read_physical(0x11000, &mut actual).unwrap();
        assert_eq!(u32::from_le_bytes(actual), 12);
        assert!(inputs.gpio_level(0, 7).unwrap());
        inputs.gpio(&mut session, 0, 7, false).unwrap();
        assert!(!inputs.gpio_level(0, 7).unwrap());
    }

    #[test]
    fn missing_storage_and_non_companion_processor_refuse_before_execution() {
        let mut value = specification();
        value["storage"] = json!("flash");
        let definition: Definition = serde_json::from_value(value.clone()).unwrap();
        assert!(
            definition
                .open(
                    graph(),
                    &BTreeMap::new(),
                    factory::Engine::Unicorn,
                    Mode::Strict,
                    value
                )
                .err()
                .unwrap()
                .contains("Missing XP5 storage asset")
        );
        let mut value = specification();
        value["companion"] =
            json!({"processor":0,"request":1,"control":2,"reply":3,"status":4,"fault":5});
        let definition: Definition = serde_json::from_value(value.clone()).unwrap();
        assert!(
            definition
                .open(
                    graph(),
                    &BTreeMap::new(),
                    factory::Engine::Unicorn,
                    Mode::Strict,
                    value
                )
                .err()
                .unwrap()
                .contains("M-profile processor")
        );
    }
}
