//! Python conversion only; all format parsing lives in fujimod-firmware.
use fujimod_firmware as core;
use pyo3::{
    exceptions::PyValueError,
    prelude::*,
    types::{PyBytes, PyDict, PyList},
};
fn error(e: core::Error) -> PyErr {
    PyValueError::new_err(e.to_string())
}
#[pyfunction]
fn invert<'py>(py: Python<'py>, data: &[u8]) -> Bound<'py, PyBytes> {
    PyBytes::new(py, &core::invert(data))
}
#[pyfunction]
fn recompress<'py>(
    py: Python<'py>,
    source: &[u8],
    replacement: &[u8],
) -> PyResult<Bound<'py, PyBytes>> {
    let bytes = core::recompress(source, replacement).map_err(error)?;
    Ok(PyBytes::new(py, &bytes))
}
#[pyfunction]
fn inner_header(data: &[u8]) -> Option<(u8, u64, u64)> {
    core::inner_header(data).map(|h| (h.word_bits, h.base, h.initialized_size))
}
#[pyfunction]
fn decompress<'py>(
    py: Python<'py>,
    data: &[u8],
) -> PyResult<(Bound<'py, PyBytes>, Bound<'py, PyDict>)> {
    let (bytes, c) = core::decompress(data).map_err(error)?;
    let d = PyDict::new(py);
    d.set_item("packed_size", c.packed_size)?;
    d.set_item("unpacked_size", c.unpacked_size)?;
    d.set_item("page_size", c.page_size)?;
    d.set_item("page_count", c.page_count)?;
    d.set_item("method", c.method)?;
    let counts = PyDict::new(py);
    counts.set_item("literals", c.literals)?;
    counts.set_item("matches", c.matches)?;
    d.set_item("token_counts", counts)?;
    Ok((PyBytes::new(py, &bytes), d))
}
#[pyfunction]
fn scan<'py>(py: Python<'py>, source: &[u8]) -> PyResult<Bound<'py, PyList>> {
    let list = PyList::empty(py);
    for c in core::scan(source) {
        let d = PyDict::new(py);
        d.set_item("name", format!("stream-{:08x}", c.offset))?;
        d.set_item("dat_offset", format!("{:#x}", c.offset))?;
        d.set_item("packed_size", c.compression.packed_size)?;
        d.set_item("decoded_size", c.decoded.len())?;
        d.set_item("sha256", core::sha256(&c.decoded))?;
        if let Some(h) = c.header {
            d.set_item("header_word_bits", h.word_bits)?;
            d.set_item("base", format!("{:#x}", h.base))?;
            d.set_item("initialized_size", format!("{:#x}", h.initialized_size))?;
            d.set_item("executable", true)?;
        }
        list.append(d)?;
    }
    Ok(list)
}
#[pyfunction]
#[pyo3(signature=(data, offset, size, base, digest, architecture_bits, header_bits=None, entry=None))]
#[allow(clippy::too_many_arguments)]
fn raw<'py>(
    py: Python<'py>,
    data: &[u8],
    offset: u64,
    size: u64,
    base: u64,
    digest: &str,
    architecture_bits: u8,
    header_bits: Option<u8>,
    entry: Option<u64>,
) -> PyResult<Bound<'py, PyBytes>> {
    let bytes = core::raw(
        data,
        &core::RawSpec {
            offset,
            size,
            base,
            sha256: digest,
            header_bits,
            entry,
            architecture_bits,
        },
    )
    .map_err(error)?;
    Ok(PyBytes::new(py, bytes))
}
#[pyfunction]
#[allow(clippy::too_many_arguments)]
fn overlays<'py>(
    py: Python<'py>,
    data: &[u8],
    offset: u64,
    base: u64,
    size: u64,
    digest: &str,
    alias: u64,
    bias: u64,
    section_id: u32,
    streams: Vec<(u64, u64)>,
) -> PyResult<Bound<'py, PyList>> {
    let records = core::overlays(
        data,
        &core::OverlayTable {
            offset,
            base,
            size,
            sha256: digest,
            alias,
            flash_dat_bias: bias,
            section_id,
            streams: &streams,
        },
    )
    .map_err(error)?;
    let list = PyList::empty(py);
    for r in records {
        let d = PyDict::new(py);
        d.set_item("name", format!("overlay{:02x}", r.semaphore))?;
        d.set_item("logical_id", r.logical_id)?;
        d.set_item("semaphore", r.semaphore)?;
        d.set_item("address", r.address)?;
        d.set_item("size", r.size)?;
        d.set_item("dat_offset", r.dat_offset)?;
        d.set_item("sha256", r.sha256)?;
        list.append(d)?;
    }
    Ok(list)
}
#[pyfunction]
fn outer_header<'py>(py: Python<'py>, source: &[u8]) -> PyResult<Bound<'py, PyDict>> {
    let h = core::outer_header(source).map_err(error)?;
    let d = PyDict::new(py);
    d.set_item("kind", h.kind)?;
    d.set_item("version_major", h.version_major)?;
    d.set_item("version_minor", h.version_minor)?;
    d.set_item("payload_offset", h.payload_offset)?;
    d.set_item("header", PyBytes::new(py, h.header))?;
    d.set_item("signed_size", h.signed_bytes.map(|v| v.len()))?;
    d.set_item(
        "signature_der",
        h.signature_der.map(|v| PyBytes::new(py, v)),
    )?;
    Ok(d)
}
#[pyclass(frozen, module = "fujimod.firmware")]
struct Unit {
    inner: core::Unit,
}
#[pymethods]
impl Unit {
    #[getter]
    fn name(&self) -> &str {
        &self.inner.name
    }
    #[getter]
    fn base(&self) -> u64 {
        self.inner.base
    }
    #[getter]
    fn executable(&self) -> bool {
        self.inner.executable
    }
    #[getter]
    fn bytes<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, &self.inner.bytes)
    }
}
#[pyclass(frozen, module = "fujimod.firmware")]
struct Image {
    inner: core::Image,
}
#[pymethods]
impl Image {
    #[getter]
    fn source_sha256(&self) -> &str {
        &self.inner.source_sha256
    }
    #[getter]
    fn units(&self) -> Vec<Unit> {
        self.inner
            .units
            .iter()
            .cloned()
            .map(|inner| Unit { inner })
            .collect()
    }
}
/// Mappings: (name, DAT offset, initialized size, base, executable, method4).
#[pyfunction]
#[allow(clippy::type_complexity)]
fn decode(source: &[u8], mappings: Vec<(String, u64, u64, u64, bool, bool)>) -> PyResult<Image> {
    let mappings: Vec<_> = mappings
        .into_iter()
        .map(|(name, offset, size, base, executable, lz)| core::Mapping {
            name,
            offset,
            size,
            base,
            executable,
            encoding: if lz {
                core::Encoding::Method4
            } else {
                core::Encoding::Raw
            },
        })
        .collect();
    core::decode(source, &mappings)
        .map(|inner| Image { inner })
        .map_err(error)
}
pub(crate) fn register(py: Python<'_>, parent: &Bound<'_, PyModule>, name: &str) -> PyResult<()> {
    let module = PyModule::new(py, name)?;
    module.add_function(wrap_pyfunction!(invert, &module)?)?;
    module.add_function(wrap_pyfunction!(inner_header, &module)?)?;
    module.add_function(wrap_pyfunction!(decompress, &module)?)?;
    module.add_function(wrap_pyfunction!(recompress, &module)?)?;
    module.add_function(wrap_pyfunction!(scan, &module)?)?;
    module.add_function(wrap_pyfunction!(raw, &module)?)?;
    module.add_function(wrap_pyfunction!(overlays, &module)?)?;
    module.add_function(wrap_pyfunction!(outer_header, &module)?)?;
    module.add_function(wrap_pyfunction!(decode, &module)?)?;
    module.add_class::<Image>()?;
    module.add_class::<Unit>()?;
    parent.add_submodule(&module)?;
    py.import("sys")?
        .getattr("modules")?
        .set_item(name, module)?;
    Ok(())
}
