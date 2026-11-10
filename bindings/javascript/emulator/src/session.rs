//! Transport-owned lifetime and typed resolved inputs for the common runtime.
//! This is not a second processor/body graph resolver. The EXR II resolver will
//! supply construction here when it lands; unsupported kinds refuse meanwhile.
use fujimod_emulator::board::{
    InterruptMap,
    bus::{Bus, Mode},
    factory,
};
use fujimod_emulator_cpu::{Model, Register, arm, timeline::Ratio};
use fujimod_emulator_devices::{graph::{DeviceGraph, DeviceWindow}, graphics::scanout::Frame, parts::{controls::{Controls, Source}, pins::DriveInput}};
use fujimod_emulator_parts_controls::key_input::{ControlsOptions, Key, KeyInput};
use fujimod_framebuffer::raster::PixelOrder;
use fujimod_emulator_xp5_board::board;
use fujimod_emulator_xp5_display::display;
use serde::Deserialize;
use std::{collections::BTreeMap, rc::Rc, sync::Arc, time::Duration};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    schema: u32,
    processor: Processor,
    body: Body,
    scenario: Scenario,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Processor {
    cores: Vec<Core>,
    interrupts: Option<Interrupts>,
    ram: Vec<Ram>,
    mappings: Vec<Mapping>,
    peripherals: Vec<Peripheral>,
    dma_mappings: Vec<fujimod_emulator::board::memory::BusMapping>,
    instructions_per_tick: [u64; 2],
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Interrupts {
    affinities: Vec<u32>,
    lines: usize,
    distributor: u64,
    redistributor: u64,
    stride: u64,
}
#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
enum ModelName {
    CortexR5f,
    CortexA7,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Core {
    model: ModelName,
    released: bool,
    pc: u32,
    registers: BTreeMap<u8, u32>,
    rprofile: Option<fujimod_emulator_cpu_unicorn::rprofile::core::Options>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Ram {
    base: u64,
    size: usize,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Mapping {
    core: usize,
    address: u64,
    physical: u64,
    size: usize,
    permissions: u32,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Peripheral {
    core: usize,
    address: u64,
    physical: u64,
    size: usize,
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
enum Body {
    Bare,
    Xp5 {
        windows: Vec<DeviceWindow>,
        controls: ControlsOptions,
        displays: Vec<Display>,
    },
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Display {
    base: u64,
    period_ns: Option<u64>,
    raw_formats: [Option<String>; 4],
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Scenario {
    quantum: usize,
    firmware: Firmware,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Firmware {
    size: usize,
    pub(super) sha256: String,
    format: Format,
    mappings: Vec<Load>,
}
#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum Format {
    Raw,
    Dat,
}
#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Encoding {
    Raw,
    Method4,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Load {
    name: String,
    offset: u64,
    size: u64,
    base: u64,
    encoding: Encoding,
    executable: bool,
}

pub struct Session {
    pub(super) construction: Option<super::snapshot::Construction>,
    pub(super) capture_order: std::cell::Cell<u64>,
    pub core: fujimod_emulator_session::Session,
    pub(super) reset_image: Option<Rc<fujimod_emulator_session::checkpoint::Snapshot>>,
    pub(super) powered_on: bool,
    pub(super) generation: u64,
    /// Identifies this session in its checkpoint IDs and input logs.
    pub(super) owner: u32,
    pub(crate) camera: Option<super::camera::Metadata>,
    pub(super) quantum: usize,
    pub(super) input: Option<(Arc<dyn KeyInput>, usize)>,
    pub frames: BTreeMap<usize, Arc<Frame>>,
    pub(super) history: Option<super::history::History>,
    pub failed: bool,
}
impl Session {
    pub fn create(
        config: &[u8],
        firmware: &[u8],
        policy: fujimod_emulator_cpu::ExecutionPolicy,
    ) -> Result<Self, String> {
        let construction = super::snapshot::Construction::resolved(config, firmware);
        let definition: serde_json::Value =
            serde_json::from_slice(config).map_err(|e| e.to_string())?;
        let config: Config = serde_json::from_slice(config).map_err(|e| e.to_string())?;
        if config.schema != 1 || config.scenario.quantum == 0 {
            return Err("unsupported session schema or zero quantum".into());
        }
        let images = decode(&config.scenario.firmware, firmware)?;
        let mut devices = DeviceGraph::new();
        let input = match config.body {
            Body::Bare => None,
            Body::Xp5 {
                windows,
                controls,
                displays,
            } => {
                let pins = fujimod_emulator_devices::xp5::pins::Pins::shared();
                let driver: Arc<dyn DriveInput> = pins.clone();
                let input = controls.open(driver)?;
                input.start(Duration::ZERO);
                let mut scanouts = BTreeMap::new();
                for d in displays {
                    let mut formats = [None; 4];
                    for (out, name) in formats.iter_mut().zip(d.raw_formats) {
                        *out = name.as_deref().map(PixelOrder::parse).transpose()?;
                    }
                    if scanouts
                        .insert(
                            d.base,
                            display::Config {
                                period_ns: d.period_ns,
                                raw_formats: formats,
                            },
                        )
                        .is_some()
                    {
                        return Err("duplicate display configuration".into());
                    }
                }
                let mut resources = board::Resources {
                    controls: input.clone() as Arc<dyn Controls>,
                    pins,
                    clocks: fujimod_emulator_devices::xp5::panel_clocks::Clocks::new(),
                    companion: None,
                    storage: None,
                    sd: BTreeMap::new(),
                    displays: scanouts,
                };
                board::register_all(&mut devices, &windows, &mut resources, &mut |w| {
                    Err(format!("unsupported device {}@{:#x}", w.kind, w.base))
                })?;
                if !resources.displays.is_empty() {
                    return Err("unused display configuration".into());
                }
                let source = devices.attach(Box::new(Source::new(input.clone() as Arc<dyn Controls>)));
                for w in windows
                    .iter()
                    .filter(|w| matches!(w.kind.as_str(), "gpio" | "gpio-port3" | "adc"))
                {
                    let id = devices.at(w.base)?;
                    let mut causes = devices.nodes[id].native_dependencies.clone();
                    causes.push(source);
                    devices.depend_on(id, causes)?;
                }
                Some((input, source))
            }
        };
        let p = config.processor;
        let interrupts = p
            .interrupts
            .map(|g| {
                Ok::<_, String>(InterruptMap {
                    cores: g.affinities.len(),
                    gic: Box::new(fujimod_emulator_ip_arm::gicv3::Gic::new(
                        g.affinities,
                        g.lines,
                    )?),
                    registers: fujimod_emulator::board::InterruptRegisters::GicV3 {
                        distributor: g.distributor,
                        redistributor: g.redistributor,
                        stride: g.stride,
                    },
                })
            })
            .transpose()?;
        let graph = factory::Graph {
            processors: p
                .cores
                .into_iter()
                .map(|c| {
                    let mut registers = vec![(Register::Pc, u64::from(c.pc))];
                    for (r, value) in c.registers {
                        if r > 14 {
                            return Err("initial general register must be r0..r14".into());
                        }
                        registers.push((Register::Arm32(arm::Register::R(r)), u64::from(value)));
                    }
                    Ok(factory::Processor {
                        model: match c.model {
                            ModelName::CortexR5f => Model::CortexR5F,
                            ModelName::CortexA7 => Model::CortexA7,
                        },
                        registers,
                        released: c.released,
                        mprofile: None,
                        rprofile: c.rprofile,
                    })
                })
                .collect::<Result<_, String>>()?,
            ram: p
                .ram
                .into_iter()
                .map(|r| factory::Ram {
                    base: r.base,
                    size: r.size,
                })
                .collect(),
            mappings: p
                .mappings
                .into_iter()
                .map(|m| factory::MemoryMap {
                    core: m.core,
                    address: m.address,
                    physical: m.physical,
                    size: m.size,
                    permissions: m.permissions,
                })
                .collect(),
            peripherals: p
                .peripherals
                .into_iter()
                .map(|m| factory::Peripheral {
                    core: m.core,
                    address: m.address,
                    physical: m.physical,
                    size: m.size,
                })
                .collect(),
            dma_mappings: p.dma_mappings,
            decoders: vec![],
            instructions_per_tick: Ratio::new(
                p.instructions_per_tick[0],
                p.instructions_per_tick[1],
            )?,
        };
        let mut board = graph.build(
            factory::Options::new(factory::Engine::Unicorn),
            Bus::from_graph(Mode::Strict, devices)?,
            interrupts,
        )?;
        for image in images {
            board.write_memory(0, image.base, &image.bytes)?;
        }
        board.set_execution_policy(policy)?;
        let mut session = Self {
            construction: Some(construction),
            capture_order: std::cell::Cell::new(1),
            core: fujimod_emulator_session::Session {
                definition,
                controls: None,
                contact_driver: None,
                board,
                resources: Default::default(),
                nodes: Default::default(),
                contacts: Default::default(),
                power_cycle: None,
                recording_frames: Default::default(),
            },
            owner: next_owner(),
            camera: None,
            reset_image: None,
            powered_on: true,
            generation: 0,
            quantum: config.scenario.quantum,
            input,
            frames: BTreeMap::new(),
            history: None,
            failed: false,
        };
        session.reset_image = session.core.checkpoint(None).ok().map(Rc::new);
        Ok(session)
    }
    pub fn run(&mut self, until: u64) -> Result<(), String> {
        if !self.powered_on {
            return Ok(());
        }
        if until < self.core.board.now() {
            return Err("run deadline precedes current guest time".into());
        }
        let count = self
            .core
            .board
            .instruction_counts()
            .into_iter()
            .try_fold(0u64, |sum, n| sum.checked_add(n))
            .ok_or("board instruction count overflow")?;
        let input = fujimod_emulator_session::history::Input::Run {
            instructions: u64::MAX - count,
            quantum: self.quantum,
            pause_after: None,
            until: Some(until),
        };
        self.mark_input(&input)?;
        let result = self.apply_input(input);
        if result.is_err() {
            self.failed = true;
        }
        self.collect_frames();
        result
    }
    pub fn take_host_requests(
        &mut self,
    ) -> Result<Vec<(usize, fujimod_emulator_devices::bus::HostRequest)>, String> {
        self.core.board.take_host_requests()
    }
    pub fn complete_host_request(
        &mut self,
        device: usize,
        id: u64,
        result: &[u8],
    ) -> Result<(), String> {
        self.core.board.complete_host_request(device, id, result)
    }
    pub fn input(&mut self, key: &str, pressed: bool, at: u64) -> Result<(), String> {
        if self.history.is_some() {
            return Err("legacy queued keys do not have a shared history input".into());
        }
        if at < self.core.board.now() {
            return Err("input edge precedes current guest time".into());
        }
        let (input, source) = self
            .input
            .as_ref()
            .ok_or("board has no physical controls")?;
        input.enqueue(Duration::from_nanos(at), Key::parse(key)?, pressed)?;
        self.core.board.inputs_changed(*source)
    }
}

/// Owner IDs are unique within this module instance.
pub(super) fn next_owner() -> u32 {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

pub(super) fn decode(
    spec: &Firmware,
    firmware: &[u8],
) -> Result<Vec<fujimod_firmware::Unit>, String> {
    if firmware.len() != spec.size {
        return Err("firmware size mismatch".into());
    }
    let images = match spec.format {
        Format::Dat => {
            fujimod_firmware::decode(
                firmware,
                &spec
                    .mappings
                    .iter()
                    .map(|m| fujimod_firmware::Mapping {
                        name: m.name.clone(),
                        offset: m.offset,
                        size: m.size,
                        base: m.base,
                        executable: m.executable,
                        encoding: match m.encoding {
                            Encoding::Raw => fujimod_firmware::Encoding::Raw,
                            Encoding::Method4 => fujimod_firmware::Encoding::Method4,
                        },
                    })
                    .collect::<Vec<_>>(),
            )
            .map_err(|e| e.to_string())?
            .units
        }
        Format::Raw => spec
            .mappings
            .iter()
            .map(|m| {
                if !matches!(m.encoding, Encoding::Raw) {
                    return Err("raw image cannot use DAT compression".into());
                }
                Ok(fujimod_firmware::Unit {
                    name: m.name.clone(),
                    base: m.base,
                    executable: m.executable,
                    bytes: fujimod_firmware::extent(firmware, m.offset, m.size)
                        .map_err(|e| e.to_string())?
                        .to_vec(),
                })
            })
            .collect::<Result<Vec<_>, String>>()?,
    };
    Ok(images)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const RAM: u64 = 0x10000;
    const ADC: u64 = 0xff90_6000;
    const RESULT: u64 = 0x11000;
    const RELEASED: u16 = 0x456;

    /// ARM loop: read ADC channel 1 (`+0x14`) and store it at `RESULT`.
    fn program() -> Vec<u8> {
        [0xe591_0014u32, 0xe582_0000, 0xeaff_fffc]
            .iter()
            .flat_map(|word| word.to_le_bytes())
            .collect()
    }

    /// Synthetic ADC ladder: bands and samples are test inputs, not body facts.
    fn config(program: &[u8], controls: serde_json::Value) -> Vec<u8> {
        json!({
            "schema": 1,
            "processor": {
                "cores": [{"model":"cortex-r5f","released":true,"pc":RAM,
                    "registers":{"1":ADC,"2":RESULT},
                    "rprofile":{"cp14_identity":0x4003,"tcm":{"atcm":{"bytes":0,"reset":0},"btcm":{"bytes":0,"reset":0}}}}],
                "interrupts": null,
                "ram": [{"base":RAM,"size":0x4000}],
                "mappings": [{"core":0,"address":RAM,"physical":RAM,"size":0x4000,"permissions":7}],
                "peripherals": [{"core":0,"address":ADC,"physical":ADC,"size":0x1000}],
                "dma_mappings": [{"address":RAM,"physical":RAM,"size":0x4000,"permissions":3}],
                "instructions_per_tick": [1, 1],
            },
            "body": {"kind":"xp5","displays":[],
                "windows":[{"kind":"adc","base":ADC,"size":0x1000,"options":{"samples":vec![RELEASED; 15]}}],
                "controls": controls},
            "scenario": {"quantum": 16, "firmware": {"size": program.len(),
                "sha256": "ab".repeat(32), "format": "raw",
                "mappings": [{"name":"program","offset":0,"size":program.len(),"base":RAM,
                    "encoding":"raw","executable":true}]}},
        })
        .to_string()
        .into_bytes()
    }

    fn ladder_keys() -> serde_json::Value {
        json!({"kind":"ladder-keys","ladders":[{"wire":{"kind":"adc","channel":1},"levels":4096,"rungs":[
            {"key":"Menu","band_low":100,"band_high":200,"sample":150,"basis":"test fixture"},
            {"key":"StickPush","band_low":300,"band_high":400,"sample":350,"basis":"test fixture"}]}]})
    }

    #[test]
    fn decode_preserves_size_mappings_and_format_checks_without_hashing() {
        let program = program();
        let config: Config = serde_json::from_slice(&config(&program, ladder_keys())).unwrap();
        let mut firmware = config.scenario.firmware;
        let mut changed = program.clone();
        changed[0] ^= 0xff;
        let units = decode(&firmware, &changed).unwrap();
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].name, "program");
        assert_eq!(units[0].base, RAM);
        assert!(units[0].executable);
        assert_eq!(units[0].bytes, changed);
        assert_eq!(
            decode(&firmware, &program[..program.len() - 1]).unwrap_err(),
            "firmware size mismatch"
        );
        assert_eq!(
            decode(&firmware, &[program.as_slice(), &[0]].concat()).unwrap_err(),
            "firmware size mismatch"
        );
        firmware.mappings[0].offset = program.len() as u64;
        assert!(decode(&firmware, &program).is_err());
        firmware.mappings[0].offset = 0;
        firmware.mappings[0].encoding = Encoding::Method4;
        assert_eq!(
            decode(&firmware, &program).unwrap_err(),
            "raw image cannot use DAT compression"
        );
        firmware.format = Format::Dat;
        assert!(decode(&firmware, &program).is_err());
    }

    fn sample(session: &mut Session) -> u16 {
        let until = session.core.board.now() + 200;
        session.run(until).unwrap();
        let mut bytes = [0; 2];
        session.core.board.read_memory(0, RESULT, &mut bytes).unwrap();
        u16::from_le_bytes(bytes)
    }

    #[test]
    fn ladder_key_edges_reach_the_adc_the_guest_reads() {
        let program = program();
        let mut session = Session::create(
            &config(&program, ladder_keys()),
            &program,
            fujimod_emulator_cpu::ExecutionPolicy::Exact,
        )
        .unwrap();
        assert_eq!(sample(&mut session), RELEASED);
        let now = session.core.board.now();
        session.input("STICK_PUSH", true, now).unwrap();
        assert_eq!(sample(&mut session), 350);
        let now = session.core.board.now();
        session.input("STICK_PUSH", false, now).unwrap();
        session.input("MENU", true, now).unwrap();
        assert_eq!(sample(&mut session), 150);
        let now = session.core.board.now();
        let error = session.input("DISP/BACK", true, now).unwrap_err();
        assert!(error.contains("no declared ladder rung"), "{error}");
    }

    #[test]
    fn camera_named_key_models_are_refused() {
        let program = program();
        let mut controls = ladder_keys();
        controls["kind"] = json!("x100vi");
        let error = Session::create(
            &config(&program, controls),
            &program,
            fujimod_emulator_cpu::ExecutionPolicy::Exact,
        )
        .err()
        .unwrap();
        assert!(error.contains("unknown variant"), "{error}");
    }
}
