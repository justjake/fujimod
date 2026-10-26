//! Python bindings for the graphics data path (`fujimod_emulator_devices::graphics`).
//!
//! Refusals raise `DeviceRefusal` with the reference message. Register banks
//! cross as dicts in insertion order; draws stay native in a `UiBatch`
//! (`draw_dicts()` gives the reference structure for tests).

use fujimod_emulator_devices::Unsupported;
use fujimod_emulator_graphics::{batch, clear, init, regs::Registers, render};
use fujimod_python_base::values::{DeviceRefusal, value_to_py};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict, PyList, PyTuple};
use std::collections::HashMap;

fn refusal(error: Unsupported) -> PyErr {
    DeviceRefusal::new_err(error.0)
}

fn registers(dict: Option<&Bound<'_, PyDict>>) -> PyResult<Option<Registers>> {
    let Some(dict) = dict else { return Ok(None) };
    let mut out = Registers::new();
    for (key, value) in dict.iter() {
        out.set(key.extract()?, value.extract()?);
    }
    Ok(Some(out))
}

fn registers_dict<'py>(py: Python<'py>, registers: &Registers) -> PyResult<Bound<'py, PyDict>> {
    let dict = PyDict::new(py);
    for (key, value) in registers.iter() {
        dict.set_item(key, value)?;
    }
    Ok(dict)
}

fn descriptor_dict<'py>(
    py: Python<'py>,
    descriptor: &clear::Descriptor,
) -> PyResult<Bound<'py, PyDict>> {
    let dict = PyDict::new(py);
    for (key, value) in descriptor {
        dict.set_item(*key, value_to_py(py, value)?)?;
    }
    Ok(dict)
}

fn affine<'py>(py: Python<'py>, m: &batch::Affine) -> PyResult<Bound<'py, PyTuple>> {
    PyTuple::new(py, m.iter())
}

fn draw_dict<'py>(py: Python<'py>, draw: &batch::Draw) -> PyResult<Bound<'py, PyDict>> {
    let d = PyDict::new(py);
    let (address, width, height, order) = draw.target;
    d.set_item("target", (address, width, height, order))?;
    d.set_item("sequence", draw.sequence)?;
    d.set_item("matrix", affine(py, &draw.matrix)?)?;
    let geometry = PyList::empty(py);
    for (code, flags, values) in &draw.geometry {
        geometry.append((*code, *flags, PyTuple::new(py, values.iter())?))?;
    }
    d.set_item("geometry", geometry)?;
    d.set_item("samples", draw.samples)?;
    let clips = PyList::empty(py);
    for clip in &draw.clips {
        clips.append(*clip)?;
    }
    d.set_item("clips", clips)?;
    d.set_item("nonzero", draw.nonzero)?;
    d.set_item("blend", draw.blend)?;
    d.set_item("color", PyTuple::new(py, draw.color.iter())?)?;
    d.set_item("stroke", draw.stroke)?;
    d.set_item("texture", py.None())?;
    match &draw.gradient {
        None => d.set_item("gradient", py.None())?,
        Some(g) => {
            let gradient = PyDict::new(py);
            gradient.set_item("coefficients", g.coefficients)?;
            let colors = PyList::empty(py);
            for c in &g.colors {
                colors.append(PyTuple::new(py, c.iter())?)?;
            }
            gradient.set_item("colors", PyTuple::new(py, colors.iter())?)?;
            gradient.set_item("spread", g.spread)?;
            gradient.set_item("premultiplied", g.premultiplied)?;
            if let Some(radial) = &g.radial {
                let r = PyDict::new(py);
                r.set_item("quadratics", radial.quadratics)?;
                r.set_item("zero_x", radial.zero_x)?;
                r.set_item("zero_y", radial.zero_y)?;
                r.set_item("negative_denominator", radial.negative_denominator)?;
                gradient.set_item("radial", r)?;
            }
            d.set_item("gradient", gradient)?;
        }
    }
    if draw.stroke {
        d.set_item("radius", draw.radius)?;
        d.set_item("cap", draw.cap)?;
        d.set_item("join", draw.join)?;
        d.set_item("device_radius", draw.device_radius)?;
    }
    if let Some(t) = &draw.texture {
        let texture = PyDict::new(py);
        texture.set_item("source", t.source)?;
        texture.set_item("width", t.width)?;
        texture.set_item("height", t.height)?;
        texture.set_item("valid_width", t.valid_width)?;
        texture.set_item("valid_height", t.valid_height)?;
        texture.set_item("matrix", affine(py, &t.matrix)?)?;
        d.set_item("texture", texture)?;
    }
    Ok(d)
}

/// A validated UI batch (`_ui_batch`'s draws and register banks).
#[pyclass(module = "fujimod._native.emulator", unsendable)]
pub struct UiBatch {
    draws: Vec<batch::Draw>,
    banks: batch::Banks,
}

#[pymethods]
impl UiBatch {
    /// (path bank, fill bank) as dicts.
    fn banks<'py>(&self, py: Python<'py>) -> PyResult<(Bound<'py, PyDict>, Bound<'py, PyDict>)> {
        Ok((
            registers_dict(py, &self.banks.path)?,
            registers_dict(py, &self.banks.fill)?,
        ))
    }
    /// Decoded draws as structured Python dictionaries.
    fn draw_dicts<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        let list = PyList::empty(py);
        for draw in &self.draws {
            list.append(draw_dict(py, draw)?)?;
        }
        Ok(list)
    }
    fn __len__(&self) -> usize {
        self.draws.len()
    }
    /// The draws as reference dicts (stand-ins that read draw fields).
    fn __iter__<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        self.draw_dicts(py)?.try_iter().map(|i| i.into_any())
    }
    /// (filled paths, stroked paths, sampled images, linear gradients, radial gradients).
    fn counts(&self) -> (usize, usize, usize, usize, usize) {
        let d = &self.draws;
        (
            d.iter()
                .filter(|x| !x.stroke && x.texture.is_none())
                .count(),
            d.iter().filter(|x| x.stroke).count(),
            d.iter().filter(|x| x.texture.is_some()).count(),
            d.iter()
                .filter(|x| x.gradient.as_ref().is_some_and(|g| g.radial.is_none()))
                .count(),
            d.iter()
                .filter(|x| x.gradient.as_ref().is_some_and(|g| g.radial.is_some()))
                .count(),
        )
    }
    /// Snapshot reads the renderer needs, in the reference order:
    /// ("target", address, bytes) and ("texture", (source, width, height), bytes).
    fn reads<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        let list = PyList::empty(py);
        let (mut targets, mut textures) = (Vec::new(), Vec::new());
        for draw in &self.draws {
            let (address, width, height, _) = draw.target;
            if !targets.contains(&address) {
                targets.push(address);
                list.append(("target", address, width as u64 * height as u64 * 4))?;
            }
            if let Some(t) = &draw.texture {
                let key = (t.source, t.width, t.height);
                if !textures.contains(&key) {
                    textures.push(key);
                    list.append(("texture", key, t.width as u64 * t.height as u64 * 4))?;
                }
            }
        }
        Ok(list)
    }
    /// Render with the snapshots `reads()` asked for: `targets` maps address
    /// to bytes, `textures` (source, width, height) to bytes. Returns
    /// ([(address, bytes)] in first-use order, covered byte count).
    fn render<'py>(
        &self,
        py: Python<'py>,
        targets: Bound<'py, PyDict>,
        textures: Bound<'py, PyDict>,
    ) -> PyResult<(Bound<'py, PyList>, u64)> {
        let mut t = HashMap::new();
        for (key, value) in targets.iter() {
            t.insert(key.extract::<u64>()?, value.extract::<Vec<u8>>()?);
        }
        let mut x = HashMap::new();
        for (key, value) in textures.iter() {
            x.insert(
                key.extract::<(u64, u32, u32)>()?,
                value.extract::<Vec<u8>>()?,
            );
        }
        let (outputs, touched) = render::render(&self.draws, &t, &x, &mut |draw| {
            render::coverage(draw).map(std::rc::Rc::new)
        })
        .map_err(refusal)?;
        let list = PyList::empty(py);
        for (address, bytes) in outputs {
            list.append((address, PyBytes::new(py, &bytes)))?;
        }
        Ok((list, touched))
    }
}

/// `_ui_batch(path_words, fill_words, initial_banks=..., ram_bytes=, max_bytes=)`.
#[pyfunction]
#[pyo3(signature=(path_words, fill_words, initial_path, initial_fill, ram_bytes, max_bytes))]
pub fn ui_batch(
    path_words: Vec<u32>,
    fill_words: Vec<u32>,
    initial_path: Option<Bound<'_, PyDict>>,
    initial_fill: Option<Bound<'_, PyDict>>,
    ram_bytes: u64,
    max_bytes: u64,
) -> PyResult<UiBatch> {
    let (p, f) = (
        registers(initial_path.as_ref())?,
        registers(initial_fill.as_ref())?,
    );
    let (draws, banks) = batch::batch(
        &path_words,
        &fill_words,
        p.as_ref(),
        f.as_ref(),
        ram_bytes,
        max_bytes,
    )
    .map_err(refusal)?;
    Ok(UiBatch { draws, banks })
}

/// `_ui_initialize(...)`: (descriptor, path bank, fill bank).
#[pyfunction]
pub fn ui_initialize<'py>(
    py: Python<'py>,
    path_words: Vec<u32>,
    fill_words: Vec<u32>,
    initial_path: Option<Bound<'py, PyDict>>,
    initial_fill: Option<Bound<'py, PyDict>>,
    max_bytes: u64,
) -> PyResult<(Bound<'py, PyDict>, Bound<'py, PyDict>, Bound<'py, PyDict>)> {
    let (p, f) = (
        registers(initial_path.as_ref())?,
        registers(initial_fill.as_ref())?,
    );
    let (descriptor, (path, fill)) =
        init::initialize(&path_words, &fill_words, p.as_ref(), f.as_ref(), max_bytes)
            .map_err(refusal)?;
    Ok((
        descriptor_dict(py, &descriptor)?,
        registers_dict(py, &path)?,
        registers_dict(py, &fill)?,
    ))
}

/// `path_descriptor(words, ram_bytes=, max_clear_bytes=)`.
#[pyfunction]
pub fn clear_path_descriptor<'py>(
    py: Python<'py>,
    words: Vec<u32>,
    ram_bytes: u64,
    max_clear_bytes: u64,
) -> PyResult<Bound<'py, PyDict>> {
    descriptor_dict(
        py,
        &clear::path_descriptor(&words, ram_bytes, max_clear_bytes).map_err(refusal)?,
    )
}

/// `parse_clear(...)`: (descriptor, path bank, fill bank).
#[pyfunction]
pub fn clear_parse<'py>(
    py: Python<'py>,
    path_words: Vec<u32>,
    fill_words: Vec<u32>,
    ram_bytes: u64,
    max_clear_bytes: u64,
    initial_path: Option<Bound<'py, PyDict>>,
    initial_fill: Option<Bound<'py, PyDict>>,
) -> PyResult<(Bound<'py, PyDict>, Bound<'py, PyDict>, Bound<'py, PyDict>)> {
    let (p, f) = (
        registers(initial_path.as_ref())?,
        registers(initial_fill.as_ref())?,
    );
    let (descriptor, (path, fill)) = clear::parse_clear(
        &path_words,
        &fill_words,
        ram_bytes,
        max_clear_bytes,
        p.as_ref(),
        f.as_ref(),
    )
    .map_err(refusal)?;
    Ok((
        descriptor_dict(py, &descriptor)?,
        registers_dict(py, &path)?,
        registers_dict(py, &fill)?,
    ))
}

/// `render_clear(source, descriptor)` from its fields.
#[pyfunction]
pub fn clear_render<'py>(
    py: Python<'py>,
    source: &[u8],
    width: u64,
    rectangle: [i64; 4],
    pixel: &[u8],
) -> Bound<'py, PyBytes> {
    PyBytes::new(py, &clear::render_clear(source, width, rectangle, pixel))
}

/// Validate one list (`_ui_stream`): its draw count, or DeviceRefusal.
#[pyfunction]
pub fn ui_stream_draws(
    words: Vec<u32>,
    path: bool,
    initial: Option<Bound<'_, PyDict>>,
) -> PyResult<usize> {
    let initial = registers(initial.as_ref())?;
    fujimod_emulator_graphics::stream::parse(&words, path, initial.as_ref())
        .map(|(draws, _)| draws.len())
        .map_err(refusal)
}

/// Decode a command stream through the shared interpreter and expose its
/// sampled draw state without implementing a second parser in the binding.
#[pyfunction]
pub fn ui_stream_parse<'py>(
    py: Python<'py>,
    words: Vec<u32>,
    path: bool,
    initial: Option<&Bound<'py, PyDict>>,
) -> PyResult<(Bound<'py, PyList>, Bound<'py, PyDict>)> {
    let initial = registers(initial)?;
    let (draws, bank) = fujimod_emulator_graphics::stream::parse(&words, path, initial.as_ref())
        .map_err(|error| pyo3::exceptions::PyValueError::new_err(error.0))?;
    let result = PyList::empty(py);
    for draw in draws {
        let value = PyDict::new(py);
        value.set_item("sequence", draw.sequence)?;
        value.set_item("registers", registers_dict(py, &draw.registers)?)?;
        if let Some(geometry) = draw.geometry {
            let records = PyList::empty(py);
            for (code, flags, values) in geometry {
                records.append((code, flags, PyTuple::new(py, values)?))?;
            }
            value.set_item("geometry", records)?;
        }
        result.append(value)?;
    }
    Ok((result, registers_dict(py, &bank)?))
}

/// The hardware copy's untiled, bottom-up output (tile bytes unchanged).
#[pyfunction]
pub fn copy_untile<'py>(
    py: Python<'py>,
    input: &[u8],
    width: usize,
    height: usize,
) -> PyResult<Bound<'py, PyBytes>> {
    if input.len() != width * height * 4 || !width.is_multiple_of(8) || !height.is_multiple_of(8) {
        return Err(pyo3::exceptions::PyValueError::new_err(
            "copy_untile needs 8-aligned dimensions and matching bytes",
        ));
    }
    Ok(PyBytes::new(py, &render::copy_untile(input, width, height)))
}

/// CPython float semantics, exposed for tests: (hypot(x, y), dist, sum, pow).
#[pyfunction]
pub fn math_check(x: f64, y: f64, values: Vec<f64>) -> (f64, f64, f64, f64) {
    use fujimod_emulator_devices::math;
    (
        math::hypot2(x, y),
        math::dist2((x, y), (y, x)),
        math::sum(values),
        math::pow(x, 2.0),
    )
}

/// Host-only raster input for the pure renderer, without a device adapter.
struct RasterInput<'py> {
    reader: Bound<'py, PyAny>,
    error: Option<(String, PyErr)>,
}
impl fujimod_emulator_exr2_gpu::Inputs for RasterInput<'_> {
    fn has_reader(&self) -> bool {
        true
    }
    fn has_irq(&self) -> bool {
        false
    }
    fn routed(&self) -> bool {
        false
    }
    fn read(&mut self, cpu: i64, address: u64, length: u64) -> Result<Vec<u8>, String> {
        let kwargs = pyo3::types::PyDict::new(self.reader.py());
        let result = kwargs
            .set_item("cpu_id", cpu)
            .and_then(|_| self.reader.call((address, length), Some(&kwargs)))
            .and_then(|v| v.extract());
        result.map_err(|error| {
            let message = error.to_string();
            self.error = Some((message.clone(), error));
            message
        })
    }
    fn targets(&mut self) -> fujimod_emulator_devices::record::Result<(usize, i64)> {
        Err("pure raster input has no interrupt routing".into())
    }
}
/// Snapshot orchestration is part of the portable Rust model.
#[pyfunction]
#[pyo3(signature = (batch, reader: "MemoryReader", cpu) -> "tuple[list[tuple[int, bytes]], int]")]
pub fn gpu_ui_render<'py>(
    py: Python<'py>,
    batch: &UiBatch,
    reader: Bound<'py, PyAny>,
    cpu: i64,
) -> PyResult<(Bound<'py, PyList>, u64)> {
    let mut host = RasterInput {
        reader,
        error: None,
    };
    let result = fujimod_emulator_exr2_gpu::ui_render(&batch.draws, cpu, &mut host);
    let (outputs, touched) = match result {
        Ok(out) => out,
        Err(e) => {
            if let Some((message, error)) = host.error
                && message == e.0
            {
                return Err(error);
            }
            return Err(refusal(e));
        }
    };
    let out = PyList::empty(py);
    for (a, p) in outputs {
        out.append((a, PyBytes::new(py, &p)))?;
    }
    Ok((out, touched))
}
