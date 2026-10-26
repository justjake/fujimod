//! Source adapters enter the same transport-independent Rust recorder. The
//! native scanout protocol is observational; no Python rendering is called.
use fujimod_recording::{Frame as RgbFrame, Recorder as NativeRecorder};
use pyo3::{
    exceptions::PyValueError,
    prelude::*,
    types::{PyBytes, PyDict, PyModule},
};
use std::path::PathBuf;

fn error(message: String) -> PyErr {
    PyValueError::new_err(message)
}

#[pyclass(module = "fujimod.recording", frozen)]
#[derive(Clone)]
pub struct Frame {
    inner: RgbFrame,
    state: String,
    instance: Option<usize>,
}
#[pymethods]
impl Frame {
    #[staticmethod]
    fn rgb(width: u32, height: u32, data: &[u8]) -> PyResult<Self> {
        Ok(Self {
            inner: RgbFrame::rgb(width, height, data.to_vec()).map_err(error)?,
            state: "{}".into(),
            instance: None,
        })
    }
    #[staticmethod]
    fn bgra(width: u32, height: u32, stride: usize, data: &[u8]) -> PyResult<Self> {
        Ok(Self {
            inner: RgbFrame::bgra(width, height, stride, data).map_err(error)?,
            state: "{}".into(),
            instance: None,
        })
    }
    #[staticmethod]
    fn from_png(data: &[u8]) -> PyResult<Self> {
        Ok(Self {
            inner: RgbFrame::from_png(data).map_err(error)?,
            state: "{}".into(),
            instance: None,
        })
    }
    fn png<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        Ok(PyBytes::new(py, self.inner.png().map_err(error)?))
    }
    fn pixels<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, self.inner.pixels())
    }
    fn save(&self, path: PathBuf) -> PyResult<String> {
        self.inner.save(&path).map_err(error)
    }
    fn annotated(&self, labels: Vec<(String, u8)>) -> PyResult<Self> {
        Ok(Self {
            inner: fujimod_recording::annotate(&self.inner, &labels).map_err(error)?,
            state: "{}".into(),
            instance: None,
        })
    }
    #[getter]
    fn sha256(&self) -> PyResult<String> {
        self.inner.sha256().map_err(error)
    }
    #[getter]
    fn width(&self) -> u32 {
        self.inner.width()
    }
    #[getter]
    fn height(&self) -> u32 {
        self.inner.height()
    }
    #[getter]
    fn state(&self) -> &str {
        &self.state
    }
    #[getter]
    fn instance(&self) -> Option<usize> {
        self.instance
    }
}

/// Native boards expose the same capture protocol regardless of binding module.
/// RGB and metadata travel together across that boundary. None means no plane.
fn scanout(
    source: &Bound<'_, PyAny>,
    instance: usize,
    diagnostics: bool,
) -> PyResult<Option<Frame>> {
    let value = source.call_method1("recording_frame", ("scanout", instance, diagnostics))?;
    if value.is_none() {
        return Ok(None);
    }
    let (rgb, state, width, height): (Bound<'_, PyBytes>, String, u32, u32) = value.extract()?;
    Ok(Some(Frame {
        inner: RgbFrame::rgb(width, height, rgb.as_bytes().to_vec()).map_err(error)?,
        state,
        instance: Some(instance),
    }))
}
#[pyfunction]
#[pyo3(signature=(source,instance=None,diagnostics=true))]
fn capture_scanout(
    source: &Bound<'_, PyAny>,
    instance: Option<usize>,
    diagnostics: bool,
) -> PyResult<Option<Frame>> {
    if let Some(instance) = instance {
        return scanout(source, instance, diagnostics);
    }
    let mut refused = None;
    for instance in 0..2 {
        match scanout(source, instance, diagnostics) {
            Ok(Some(frame)) => return Ok(Some(frame)),
            Ok(None) => (),
            Err(error) => refused = Some(error),
        }
    }
    if let Some(error) = refused {
        return Err(error);
    }
    Ok(None)
}

#[pyclass(module = "fujimod.recording")]
struct Recorder {
    inner: NativeRecorder,
}
#[pymethods]
impl Recorder {
    #[new]
    #[pyo3(signature=(output,*,fps=30,video=false))]
    fn new(output: PathBuf, fps: u32, video: bool) -> PyResult<Self> {
        Ok(Self {
            inner: NativeRecorder::new(output, fps, video).map_err(error)?,
        })
    }
    fn frame(&mut self, frame: &Frame, seconds: f64) -> PyResult<()> {
        self.inner
            .frame(frame.inner.clone(), seconds)
            .map_err(error)
    }
    #[pyo3(signature=(snapshot=None))]
    fn card_panel(&mut self, snapshot: Option<&str>) -> PyResult<()> {
        let panel = snapshot
            .map(fujimod_recording::CardPanel::parse)
            .transpose()
            .map_err(error)?;
        self.inner.card_panel(panel).map_err(error)
    }
    fn card_formatting(&mut self) -> PyResult<()> {
        self.inner.card_formatting().map_err(error)
    }
    #[pyo3(signature=(frame=None,*,label="",width=0.6))]
    fn display_panel(&mut self, frame: Option<&Frame>, label: &str, width: f32) -> PyResult<()> {
        self.inner
            .display_panel(frame.map(|frame| frame.inner.clone()), label.into(), width)
            .map_err(error)
    }
    fn animate(&mut self, seconds: f64) -> PyResult<()> {
        self.inner.animate(seconds).map_err(error)
    }
    fn input(&mut self, name: &str, pressed: bool, seconds: f64) -> PyResult<()> {
        self.inner.input(name, pressed, seconds).map_err(error)
    }
    fn touch(&mut self, phase: &str, x: f64, y: f64, seconds: f64) -> PyResult<()> {
        let phase = phase.parse().map_err(error)?;
        self.inner.touch(phase, x, y, seconds).map_err(error)
    }
    fn still(&mut self, frame: &Frame, seconds: f64) -> PyResult<()> {
        self.inner
            .still(frame.inner.clone(), seconds)
            .map_err(error)
    }
    fn screenshot(&mut self, frame: &Frame, name: &str) -> PyResult<String> {
        self.inner.screenshot(&frame.inner, name).map_err(error)
    }
    #[pyo3(signature=(source,seconds,*,instance=None,force=false))]
    fn sample_scanout(
        &mut self,
        source: &Bound<'_, PyAny>,
        seconds: f64,
        instance: Option<usize>,
        force: bool,
    ) -> PyResult<bool> {
        if !self.inner.due(seconds, force).map_err(error)? {
            return Ok(false);
        }
        if let Some(frame) = capture_scanout(source, instance, false)? {
            self.inner.frame(frame.inner, seconds).map_err(error)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
    fn finish(&mut self) -> PyResult<()> {
        self.inner.finish().map_err(error)
    }
    #[getter]
    fn seconds(&self) -> f64 {
        self.inner.seconds()
    }
    #[getter]
    fn fps(&self) -> u32 {
        self.inner.fps
    }
    /// How often LCD frames are sampled: at most 15 a second, whatever the
    /// video frame rate.
    #[getter]
    fn sample_fps(&self) -> u32 {
        self.inner.sample_fps()
    }
    #[getter]
    fn video(&self) -> bool {
        self.inner.video
    }
    #[getter]
    fn has_poster(&self) -> bool {
        self.inner.has_poster()
    }
    #[getter]
    fn idle_limit(&self) -> f64 {
        self.inner.idle_limit
    }
    #[setter]
    fn set_idle_limit(&mut self, value: f64) -> PyResult<()> {
        self.inner.idle_limit = fujimod_recording::seconds(value).map_err(error)?;
        Ok(())
    }
    #[getter]
    fn skipped_seconds(&self) -> f64 {
        self.inner.skipped_seconds
    }
    #[getter]
    fn frames(&self) -> u64 {
        self.inner.frames
    }
    fn metrics(&self) -> PyResult<String> {
        serde_json::to_string(&self.inner.metrics).map_err(|e| error(e.to_string()))
    }
}

#[pyfunction]
fn seconds_value(value: f64) -> PyResult<f64> {
    fujimod_recording::seconds(value).map_err(error)
}
#[pyfunction]
fn controls() -> &'static str {
    fujimod_recording::CONTROLS
}

pub fn register(py: Python<'_>, parent: &Bound<'_, PyModule>) -> PyResult<()> {
    let module = PyModule::new(py, "fujimod.recording")?;
    module.add_class::<Frame>()?;
    module.add_class::<Recorder>()?;
    module.add_function(wrap_pyfunction!(capture_scanout, &module)?)?;
    module.add_function(wrap_pyfunction!(seconds_value, &module)?)?;
    module.add_function(wrap_pyfunction!(controls, &module)?)?;
    parent.add_submodule(&module)?;
    py.import("sys")?
        .getattr("modules")?
        .cast::<PyDict>()?
        .set_item("fujimod.recording", module)?;
    Ok(())
}
