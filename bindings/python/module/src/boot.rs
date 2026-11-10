//! Catalog run preparation and boot for the Python emulator bindings, installed through
//! `fujimod_python_base::boot` when the module initializes.
use fujimod_emulator_boot as boot;
use fujimod_emulator_session::Session;
use fujimod_python_base::boot::{Boot, Inputs};

struct CatalogBoot;

impl Boot for CatalogBoot {
    fn prepare(&self, configuration_id: &str, overlays: &[String]) -> Result<String, String> {
        let prepared = boot::prepare(configuration_id, overlays)?;
        serde_json::to_string(&prepared.to_json()).map_err(|failure| failure.to_string())
    }
    fn open(&self, inputs: Inputs) -> Result<Session, String> {
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
        boot::open(boot::Options {
            configuration_id: inputs.configuration_id,
            overlays: inputs.overlays,
            update: inputs.update,
            boot_rom: inputs.boot_rom,
            nand: inputs.nand,
            settings: inputs.settings,
            clock_seconds_2000: inputs.clock_seconds_2000,
            preferred_languages: inputs.preferred_languages,
            camera: inputs.camera,
            access_point,
            capability_reply,
        })
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
