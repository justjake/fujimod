//! Host preparation bindings. Media algorithms and the session stay in Rust.
use super::{CardMedium, NandMedium, error, json};
use fujimod_emulator_devices::parts::sd_card;
use fujimod_emulator_preparation::persistent_inputs::PreparePersistentInputs;
use fujimod_emulator_storage::fat;
use pyo3::{
    prelude::*,
    types::{PyBytes, PyDict},
};
use std::collections::BTreeMap;

#[pymethods]
impl CardMedium {
    /// Persist only the external medium; controller and transfer state reset.
    fn to_bytes<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        Ok(PyBytes::new(
            py,
            &serde_json::to_vec(&self.0).map_err(|e| error(e.to_string()))?,
        ))
    }
    #[staticmethod]
    fn restored(bytes: &[u8]) -> PyResult<Self> {
        let saved: sd_card::Card =
            serde_json::from_slice(bytes).map_err(|e| error(e.to_string()))?;
        saved.validate().map_err(error)?;
        Ok(Self(saved))
    }
    #[staticmethod]
    fn blank(options: &str) -> PyResult<Self> {
        Ok(Self(sd_card::Card::new(json(options)?).map_err(error)?))
    }
    #[staticmethod]
    fn formatted(options: &str, files: Vec<(String, Vec<u8>)>) -> PyResult<Self> {
        Ok(Self(fat::format(&json(options)?, &files).map_err(error)?))
    }
    #[getter]
    fn sectors(&self) -> u32 {
        self.0.sectors()
    }
    #[getter]
    fn revision(&self) -> u64 {
        self.0.revision()
    }
    fn sector<'py>(&self, py: Python<'py>, lba: u32) -> PyResult<Bound<'py, PyBytes>> {
        Ok(PyBytes::new(py, self.0.sector(lba).map_err(error)?))
    }
    fn write_sectors(&mut self, first: u32, bytes: &[u8]) -> PyResult<()> {
        if bytes.len() % 512 != 0 {
            return Err(error("Whole sectors are required".into()));
        }
        self.0.write_bytes(first as u64 * 512, bytes).map_err(error)
    }
    fn sparse_sectors<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let result = PyDict::new(py);
        for (lba, bytes) in self.0.nonzero_sectors() {
            result.set_item(lba, PyBytes::new(py, bytes))?;
        }
        Ok(result)
    }
    fn geometry(&self) -> PyResult<String> {
        serde_json::to_string(&fat::Reader::new(&self.0).map_err(error)?.geometry())
            .map_err(|e| error(e.to_string()))
    }
    fn tree(&self) -> PyResult<String> {
        serde_json::to_string(
            &fat::Reader::new(&self.0)
                .map_err(error)?
                .tree()
                .map_err(error)?,
        )
        .map_err(|e| error(e.to_string()))
    }
    #[pyo3(signature = (path, max_bytes=67108864))]
    fn read<'py>(
        &self,
        py: Python<'py>,
        path: &str,
        max_bytes: usize,
    ) -> PyResult<Bound<'py, PyBytes>> {
        if !(1..=fat::MAX_BYTES).contains(&max_bytes) {
            return Err(error("Card read bound must be 1..64 MiB".into()));
        }
        let bytes = fat::Reader::new(&self.0)
            .map_err(error)?
            .read(path, max_bytes)
            .map_err(error)?;
        Ok(PyBytes::new(py, &bytes))
    }
}
#[pymethods]
impl NandMedium {
    fn replace_input(&self, offset: u64, expected: &[u8], bytes: &[u8]) -> PyResult<Self> {
        Ok(Self(
            self.0
                .replace_input(offset, expected, bytes)
                .map_err(error)?,
        ))
    }

    fn to_bytes<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyBytes>> {
        Ok(PyBytes::new(py, &self.0.committed_image().map_err(error)?))
    }
    fn restored(&self, bytes: &[u8]) -> PyResult<Self> {
        Ok(Self(self.0.restore_committed(bytes).map_err(error)?))
    }

    fn with_inputs(
        &self,
        overlays: Vec<(u64, Vec<u8>)>,
        spare: Vec<(u32, Vec<u8>)>,
    ) -> PyResult<Self> {
        let overlays = overlays
            .into_iter()
            .map(|(offset, bytes)| fujimod_emulator_devices::parts::nand::Overlay {
                offset,
                bytes: bytes.into(),
            })
            .collect();
        let mut tags = BTreeMap::new();
        for (row, bytes) in spare {
            if tags.insert(row, bytes).is_some() {
                return Err(error("Duplicate NAND spare input".into()));
            }
        }
        Ok(Self(self.0.with_inputs(overlays, tags).map_err(error)?))
    }

    fn read<'py>(
        &self,
        py: Python<'py>,
        offset: u64,
        bytes: usize,
    ) -> PyResult<Bound<'py, PyBytes>> {
        if bytes == 0 || bytes > 64 << 20 {
            return Err(error("NAND host read must be 1..64 MiB".into()));
        }
        let mut data = vec![0; bytes];
        self.0.read(offset, &mut data).map_err(error)?;
        Ok(PyBytes::new(py, &data))
    }
    fn spare<'py>(&self, py: Python<'py>, row: u32) -> PyResult<Bound<'py, PyBytes>> {
        let mut data = vec![0; self.0.options().spare_bytes];
        self.0.read_spare(row, &mut data).map_err(error)?;
        Ok(PyBytes::new(py, &data))
    }
}
#[pyfunction]
pub(crate) fn persistent_packet(
    catalog: &str,
    selected: &str,
    assets: &Bound<'_, PyDict>,
) -> PyResult<Option<Vec<u8>>> {
    let spec: fujimod_catalog_settings::persistent_inputs::PersistentInputs = json(catalog)?;
    let loaded = assets
        .iter()
        .map(|(key, value)| Ok((key.extract()?, value.extract()?)))
        .collect::<PyResult<BTreeMap<String, Vec<u8>>>>()?;
    spec.mcu_packet(selected, &loaded)
        .map(|p| p.map(|b| b.to_vec()))
        .map_err(error)
}
#[pyfunction]
pub(crate) fn normalize_card_files<'py>(
    py: Python<'py>,
    files: Vec<(String, Vec<u8>)>,
) -> PyResult<Vec<(String, Bound<'py, PyBytes>)>> {
    let paths = fat::normalize_paths(&files).map_err(error)?;
    Ok(paths
        .into_iter()
        .zip(files)
        .map(|(path, (_, bytes))| (path, PyBytes::new(py, &bytes)))
        .collect())
}
