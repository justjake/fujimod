//! Catalog run preparation and boot for the Python emulator bindings, installed through
//! `fujimod_python_base::boot` when the module initializes.
use fujimod_emulator_boot as boot;
use fujimod_emulator::board::bus::Mode;
use fujimod_emulator_session::Session;
use fujimod_python_base::boot::{Boot, Inputs, Opened};
use fujimod_catalog_configuration::{RunInputs, RunInstructionRate};

struct CatalogBoot;

impl Boot for CatalogBoot {
    fn prepare(&self, configuration_id: &str, overlays: &[String]) -> Result<String, String> {
        let prepared = boot::prepare(configuration_id, overlays)?;
        serde_json::to_string(&prepared.to_json()).map_err(|failure| failure.to_string())
    }
    fn open(&self, inputs: Inputs) -> Result<Opened, String> {
        let mut resolved = fujimod_catalog_resolution::ResolvedConfiguration::with_overlays(
            &inputs.configuration_id, &inputs.overlays,
        )?;
        if let Some(engine) = inputs.engine {
            resolved.override_engine(engine.parse()
                .map_err(|error| format!("invalid CPU engine: {error}"))?)?;
        }
        if let Some((numerator, denominator)) = inputs.instruction_rate {
            let rate = RunInstructionRate { numerator, denominator };
            rate.validate()?;
            resolved.configuration.execution.as_mut().ok_or("configuration has no execution policy")?
                .instructions_per_tick = rate;
        }
        #[cfg(not(target_arch = "wasm32"))]
        if matches!(resolved.reference.soc.id.as_str(), "xp4" | "xpp") {
            if inputs.boot_rom.is_some() || inputs.manifest.is_some() || inputs.nand.is_some()
                || inputs.settings.is_some() || inputs.camera.is_some() || inputs.access_point.is_some()
                || inputs.capability_reply.is_some() || inputs.clock_seconds_2000.is_some()
                || !inputs.preferred_languages.is_empty() || !inputs.camera_settings.is_empty()
            {
                return Err("application-profile inputs belong in the startup plan".into());
            }
            if inputs.second_card_image.is_some() || !inputs.second_card_files.is_empty() {
                return Err("a card in SD slot 2 requires an XP5 run configuration".into());
            }
            let plan = inputs.application_plan.ok_or("application startup needs an application plan")?;
            let startup = boot::application::Startup {
                update: inputs.update, plan, discovery: inputs.discovery, quantum: inputs.quantum,
                card_image: inputs.card_image,
                card_files: inputs.card_files,
            };
            if resolved.reference.soc.id == "xp4" {
                let mut started = boot::application::open(&resolved, startup)?;
                if let Some(path) = inputs.scene {
                    started.board.set_scene(&boot::host::read_scene(&path)?)?;
                }
                return match started.board {
                    boot::application::Board::Xp4(session) => fujimod_python_xp4::adopt(*session, started.quantum)
                        .map(Opened::Xp4).map_err(|error| error.to_string()),
                    boot::application::Board::Xpp(_) => Err("XP4 startup returned another processor generation".into()),
                };
            }
            let mut camera = boot::open(resolved, boot::StartupInputs::Application(startup))?;
            if let Some(path) = inputs.scene {
                camera.session.board.set_scene(&boot::host::read_scene(&path)?)?;
            }
            return Ok(Opened::Graph(camera.session));
        }
        if inputs.application_plan.is_some() {
            return Err("application startup plan does not match this configuration".into());
        }
        let mode = if inputs.discovery { Mode::Discovery } else { Mode::Strict };
        #[cfg(not(target_arch = "wasm32"))]
        if matches!(resolved.configuration.inputs, Some(RunInputs::Xp5(_))) {
            if inputs.nand.is_some() || inputs.settings.is_some() || inputs.clock_seconds_2000.is_some()
                || !inputs.preferred_languages.is_empty() || inputs.camera.is_some()
                || inputs.access_point.is_some() || inputs.capability_reply.is_some()
            {
                return Err("EXR II startup settings do not apply to XP5".into());
            }
            let mut camera = boot::open(resolved, boot::StartupInputs::Xp5(boot::xp5::Startup {
                update: inputs.update, manifest: inputs.manifest, boot_rom: inputs.boot_rom,
                card_image: inputs.card_image, card_files: inputs.card_files,
                second_card_image: inputs.second_card_image, second_card_files: inputs.second_card_files,
                scene: inputs.scene,
                output: None,
                mode, quantum: inputs.quantum, camera_settings: inputs.camera_settings,
            }))?;
            let inputs = camera.take_xp5_inputs()?;
            let read_limit = camera.read_limit;
            let session = camera.session;
            return Ok(Opened::Xp5 {session, inputs, read_limit});
        }
        if inputs.manifest.is_some() {
            return Err("a boot manifest requires an XP5 run configuration".into());
        }
        if inputs.second_card_image.is_some() || !inputs.second_card_files.is_empty() {
            return Err("a card in SD slot 2 requires an XP5 run configuration".into());
        }
        if !inputs.camera_settings.is_empty() {
            return Err("camera settings require an XP5 run configuration".into());
        }
        if let Some(quantum) = inputs.quantum {
            if quantum == 0 {
                return Err("processor quantum must be positive".into());
            }
            resolved.configuration.execution.as_mut().ok_or("configuration has no execution policy")?
                .quantum = quantum;
        }
        if let Some(path) = inputs.scene {
            boot::host::set_exr2_scene(&mut resolved, &path)?;
        }
        let access_point = inputs
            .access_point
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .map_err(|error| format!("virtual access point JSON: {error}"))?;
        let capability_reply = inputs
            .capability_reply
            .as_deref()
            .map(|bytes| <[u8; 20]>::try_from(bytes).map_err(|_| "MT5931 capability fixture must be 20 bytes"))
            .transpose()?;
        let options = boot::Options {
            update: inputs.update,
            boot_rom: inputs.boot_rom.ok_or("EXR II requires a boot ROM")?,
            nand: inputs.nand,
            settings: inputs.settings,
            clock_seconds_2000: inputs.clock_seconds_2000,
            preferred_languages: inputs.preferred_languages,
            camera: inputs.camera,
            access_point,
            capability_reply,
        };
        let mut session = boot::open(resolved, boot::StartupInputs::Exr2 { options, mode })?.session;
        #[cfg(not(target_arch = "wasm32"))]
        boot::media::attach(&mut session, inputs.card_image.as_deref(), &inputs.card_files)?;
        Ok(Opened::Graph(session))
    }
    fn to_rec(&self, session: &mut Session, limit: u64) -> Result<String, String> {
        let report = boot::to_rec(session, limit)?;
        serde_json::to_string(&report).map_err(|failure| failure.to_string())
    }
    fn screenshot(&self, session: &Session) -> Result<Vec<u8>, String> {
        boot::screenshot(session)
    }
}

/// Install catalog boots for `Board.from_inputs`, `Board.boot_to_rec`, `Board.screenshot` and
/// `prepare_configuration`.
pub(crate) fn register() {
    fujimod_python_base::boot::install(&CatalogBoot);
}

/// The device windows a default XP5 boot of `configuration_id` builds from the catalog, as JSON.
#[pyo3::pyfunction]
pub(crate) fn xp5_device_windows(configuration_id: &str) -> pyo3::PyResult<String> {
    boot::xp5::device_windows_json(configuration_id).map_err(pyo3::exceptions::PyValueError::new_err)
}

/// X-Processor Pro execution and device graphs of a bundled run configuration, as JSON.
/// `registers` is a JSON list of each application core's reset registers.
#[pyo3::pyfunction]
#[pyo3(signature = (configuration_id, registers, overlays=Vec::new(), sub_cpu_registers=Vec::new(), assets=None, sensor_image=None, sensor_light=None))]
pub(crate) fn xpp_definition(
    configuration_id: &str,
    registers: &str,
    overlays: Vec<String>,
    sub_cpu_registers: Vec<(u8, u8)>,
    assets: Option<&pyo3::Bound<'_, pyo3::types::PyDict>>,
    sensor_image: Option<(u32, u32, Vec<u8>)>,
    sensor_light: Option<&str>,
) -> pyo3::PyResult<String> {
    use pyo3::types::{PyAnyMethods, PyBytes, PyBytesMethods, PyDictMethods};
    if sensor_light.is_some() && sensor_image.is_none() {
        return Err(pyo3::exceptions::PyValueError::new_err(
            "a sensor picture light needs a sensor picture",
        ));
    }
    let light = match sensor_light {
        None => boot::xpp::SensorImageLight::Rendered,
        Some(name) => boot::xpp::SensorImageLight::parse(name)
            .map_err(pyo3::exceptions::PyValueError::new_err)?,
    };
    let mut loaded = std::collections::BTreeMap::new();
    for (name, bytes) in assets.into_iter().flat_map(|assets| assets.iter()) {
        loaded.insert(
            name.extract::<String>()?,
            bytes.cast::<PyBytes>()?.as_bytes().to_vec(),
        );
    }
    let registers = serde_json::from_str(registers)
        .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;
    let definition = boot::xpp::definition(
        configuration_id,
        &overlays,
        registers,
        &sub_cpu_registers,
        &loaded,
        sensor_image
            .map(|(width, height, rgb)| boot::xpp::SensorImage {
                width,
                height,
                rgb,
                light,
            })
            .as_ref(),
    )
    .map_err(fujimod_python_base::error)?;
    serde_json::to_string(&definition)
        .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))
}
