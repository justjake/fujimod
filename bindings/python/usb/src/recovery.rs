//! Python ownership only; all card byte operations run in fujimod-recovery.
use fujimod_recovery::{self as core, image};
use pyo3::{
    exceptions::PyValueError,
    prelude::*,
    types::{PyBytes, PyDict, PyModule},
};
fn error(e: impl std::fmt::Display) -> PyErr {
    PyValueError::new_err(e.to_string())
}
fn value<'py>(py: Python<'py>, v: &serde_json::Value) -> PyResult<Bound<'py, PyAny>> {
    py.import("json")?.call_method1("loads", (v.to_string(),))
}
#[pyclass(module = "fujimod.recovery")]
struct RamImage {
    data: Py<PyBytes>,
    manifest: Py<PyDict>,
}
#[pymethods]
impl RamImage {
    #[new]
    fn new(data: Py<PyBytes>, manifest: Py<PyDict>) -> Self {
        Self { data, manifest }
    }
    #[getter]
    fn data(&self, py: Python<'_>) -> Py<PyBytes> {
        self.data.clone_ref(py)
    }
    #[getter]
    fn manifest(&self, py: Python<'_>) -> Py<PyDict> {
        self.manifest.clone_ref(py)
    }
}
#[pyfunction]
fn transform(py: Python<'_>, inverted: &[u8], plan: &Bound<'_, PyDict>) -> PyResult<RamImage> {
    let plan_json: String = py
        .import("json")?
        .call_method1("dumps", (plan,))?
        .extract()?;
    let plan: image::Plan = serde_json::from_str(&plan_json).map_err(error)?;
    let bytes = image::transform(inverted, &plan).map_err(error)?;
    let mut layout = Vec::new();
    for region in &plan.regions {
        let size = region
            .source
            .initialized_prefix
            .as_ref()
            .map_or(region.source.decoded_size, |p| p.size);
        let hash = region
            .source
            .initialized_prefix
            .as_ref()
            .map_or(&region.source.sha256, |p| &p.sha256);
        layout.push(serde_json::json!({"name":region.name,"ram":format!("{:#x}",region.address),"end":format!("{:#x}",region.address+size),"source":format!("DAT {:#x}; {}",region.source.dat_offset,match region.source.compression{image::Compression::Raw=>"raw",image::Compression::Method4=>"method4"}),"sha256":hash}));
    }
    for bss in &plan.bss {
        layout.push(serde_json::json!({"name":format!("{} BSS (zero)",bss.name),"ram":format!("{:#x}",bss.address),"end":format!("{:#x}",bss.address+bss.size)}));
    }
    let edits:Vec<_>=plan.edits.iter().map(|e|serde_json::json!({"address":format!("{:#x}",e.address),"old":format!("{:#x}",e.old),"new":format!("{:#x}",e.new),"what":e.what})).collect();
    let manifest = serde_json::json!({"layout":layout,"edits":edits,"entry":plan.entry,"bytes":bytes.len(),"sha256":fujimod_firmware::sha256(&bytes)});
    Ok(RamImage {
        data: PyBytes::new(py, &bytes).unbind(),
        manifest: value(py, &manifest)?.cast_into::<PyDict>()?.unbind(),
    })
}
#[pyfunction]
fn boot_header<'py>(
    py: Python<'py>,
    packaging: &Bound<'_, PyDict>,
    inverted: &[u8],
) -> PyResult<Bound<'py, PyBytes>> {
    let packaging_json: String = py
        .import("json")?
        .call_method1("dumps", (packaging,))?
        .extract()?;
    let mut json: serde_json::Value = serde_json::from_str(&packaging_json).map_err(error)?;
    // Descriptive source annotations are outside the executable recipe.
    for key in ["status", "basis", "limits"] {
        json.as_object_mut()
            .ok_or_else(|| error("Packaging must be an object"))?
            .remove(key);
    }
    let packaging: image::Packaging = serde_json::from_value(json).map_err(error)?;
    Ok(PyBytes::new(
        py,
        &packaging.header(inverted).map_err(error)?,
    ))
}
#[pyfunction]
fn package<'py>(
    py: Python<'py>,
    result: PyRef<'_, RamImage>,
    packaging: Option<&Bound<'py, PyDict>>,
    inverted: &[u8],
) -> PyResult<(Bound<'py, PyDict>, Option<String>)> {
    let output = PyDict::new(py);
    let Some(packaging) = packaging else {
        return Ok((
            output,
            Some("SD-path packaging has not been established".into()),
        ));
    };
    let text: String = py
        .import("json")?
        .call_method1("dumps", (packaging,))?
        .extract()?;
    let mut json: serde_json::Value = serde_json::from_str(&text).map_err(error)?;
    for key in ["status", "basis", "limits"] {
        json.as_object_mut()
            .ok_or_else(|| error("Packaging must be an object"))?
            .remove(key);
    }
    let packaging: image::Packaging = serde_json::from_value(json).map_err(error)?;
    let entry: usize = result
        .manifest
        .bind(py)
        .get_item("entry")?
        .ok_or_else(|| error("Missing image entry"))?
        .extract()?;
    for file in packaging
        .package_bytes(inverted, entry, result.data.bind(py).as_bytes().to_vec())
        .map_err(error)?
    {
        output.set_item(file.name, PyBytes::new(py, &file.bytes))?;
    }
    Ok((output, None))
}
#[pyfunction]
fn availability(py: Python<'_>, firmware_id: &str) -> PyResult<Py<PyAny>> {
    let status = core::availability(firmware_id).map_err(error)?;
    Ok(value(py, &serde_json::to_value(status).map_err(error)?)?.unbind())
}
#[pyfunction(signature=(firmware_id,source,*,startup=false))]
fn build<'py>(
    py: Python<'py>,
    firmware_id: &str,
    source: &[u8],
    startup: bool,
) -> PyResult<Bound<'py, PyDict>> {
    let recipe = core::Recipe::bundled(firmware_id)
        .map_err(error)?
        .ok_or_else(|| error("Missing recovery recipe"))?;
    let files = if startup {
        recipe.build_startup(source)
    } else {
        recipe.build(source)
    }
    .map_err(error)?;
    let output = PyDict::new(py);
    for file in files {
        output.set_item(file.name, PyBytes::new(py, &file.bytes))?;
    }
    Ok(output)
}
#[pyfunction]
fn cold_worker(py: Python<'_>, firmware_id: &str, source: &[u8]) -> PyResult<Py<PyAny>> {
    let recipe = core::Recipe::bundled(firmware_id)
        .map_err(error)?
        .ok_or_else(|| error("Missing recovery recipe"))?;
    Ok(value(py, &recipe.cold_worker(source).map_err(error)?)?.unbind())
}
#[pyfunction]
fn cold_plan(py: Python<'_>, firmware_id: &str, source: &[u8]) -> PyResult<Py<PyAny>> {
    let recipe = core::Recipe::bundled(firmware_id)
        .map_err(error)?
        .ok_or_else(|| error("Missing recovery recipe"))?;
    Ok(value(
        py,
        &serde_json::to_value(recipe.cold_plan(source).map_err(error)?).map_err(error)?,
    )?
    .unbind())
}
#[pyfunction]
fn recipe(py: Python<'_>, firmware_id: &str) -> PyResult<Py<PyAny>> {
    let recipe = core::Recipe::bundled(firmware_id)
        .map_err(error)?
        .ok_or_else(|| error("Missing recovery recipe"))?;
    Ok(value(py, &serde_json::to_value(recipe).map_err(error)?)?.unbind())
}
#[pyfunction]
fn compile_ffinit<'py>(
    py: Python<'py>,
    firmware_id: &str,
    source: &[u8],
    payload: &[u8],
    base: u32,
    api: &Bound<'py, PyDict>,
    workspace: u32,
) -> PyResult<(Bound<'py, PyBytes>, Py<PyAny>)> {
    let recipe = core::Recipe::bundled(firmware_id)
        .map_err(error)?
        .ok_or_else(|| error("Missing recovery recipe"))?;
    let text: String = py
        .import("json")?
        .call_method1("dumps", (api,))?
        .extract()?;
    let api: core::FileApi = serde_json::from_str(&text).map_err(error)?;
    let (bytes, layout) = recipe
        .compile_ffinit(source, payload, base, &api, workspace)
        .map_err(error)?;
    Ok((PyBytes::new(py, &bytes), value(py, &layout)?.unbind()))
}
pub(crate) fn register(py: Python<'_>, parent: &Bound<'_, PyModule>) -> PyResult<()> {
    let module = PyModule::new(py, "fujimod.recovery")?;
    module.add_class::<RamImage>()?;
    module.add_function(wrap_pyfunction!(cold_worker, &module)?)?;
    module.add_function(wrap_pyfunction!(cold_plan, &module)?)?;
    module.add_function(wrap_pyfunction!(recipe, &module)?)?;
    module.add_function(wrap_pyfunction!(compile_ffinit, &module)?)?;
    module.add_function(wrap_pyfunction!(transform, &module)?)?;
    module.add_function(wrap_pyfunction!(boot_header, &module)?)?;
    module.add_function(wrap_pyfunction!(availability, &module)?)?;
    module.add_function(wrap_pyfunction!(package, &module)?)?;
    module.add_function(wrap_pyfunction!(build, &module)?)?;
    parent.add_submodule(&module)?;
    py.import("sys")?
        .getattr("modules")?
        .set_item("fujimod.recovery", module)?;
    Ok(())
}
