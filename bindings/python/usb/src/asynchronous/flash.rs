//! Flash backups and range reads with a staged reader artifact and the bundled catalog.
use super::*;
use fujimod_catalog::{ReferenceCatalog, capture::FirmwareFlash};
use fujimod_catalog_bulk::FirmwareBulk;
use fujimod_workflows::{bulk::BulkError, flash};
use std::{
    fs::OpenOptions,
    io::{Seek, SeekFrom, Write},
};

/// The artifact, checked against the bundled catalog's flash and bulk facts.
fn staged(
    artifact: &str,
    image: &[u8],
) -> PyResult<(flash::Artifact, FirmwareFlash, FirmwareBulk)> {
    let artifact: flash::Artifact =
        serde_json::from_str(artifact).map_err(|e| PyValueError::new_err(e.to_string()))?;
    let selected = ReferenceCatalog::bundled()
        .and_then(|catalog| catalog.resolve_firmware(artifact.payload.firmware_id()))
        .map_err(PyValueError::new_err)?;
    let (Some(flash), Some(bulk)) = (selected.flash, selected.bulk) else {
        return Err(PyValueError::new_err(
            "The catalog has no flash or bulk facts for this firmware",
        ));
    };
    if selected
        .firmware
        .update
        .as_ref()
        .is_none_or(|update| update.sha256 != artifact.payload.firmware_sha256())
    {
        return Err(PyValueError::new_err(
            "The artifact was staged for a different update",
        ));
    }
    artifact
        .payload
        .validate(&flash, &bulk, image)
        .map_err(|e| PyValueError::new_err(e.to_string()))?;
    Ok((artifact, flash, bulk))
}

#[pymethods]
impl AsyncSession {
    /// Back up the whole flash with a staged `flash-reader.json` artifact and
    /// the image staged beside it, if any. The flash and bulk facts come from
    /// the bundled catalog. The image is written to a new file at `output`;
    /// returns its size, its SHA-256, the page offsets an uncorrectable NAND
    /// read covered and their granularity in bytes. A nonempty offset list
    /// marks those pages of the image as unreliable. A failed check raises
    /// RuntimeError and USB failures raise UsbError.
    #[pyo3(signature = (artifact, output, image=Vec::new()))]
    async fn backup_flash(
        &self,
        artifact: String,
        output: String,
        image: Vec<u8>,
    ) -> PyResult<(u64, String, Vec<u64>, u32)> {
        let (artifact, flash, bulk) = staged(&artifact, &image)?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&output)
            .map_err(|e| PyValueError::new_err(e.to_string()))?;
        let limits = fujimod_workflows::Limits {
            memory: self.limits.memory,
            configuration: self.limits.configuration,
        };
        let outcome = self
            .with(move |session| {
                Box::pin(async move {
                    let mut chunk = |offset: u64, bytes: Vec<u8>| {
                        file.seek(SeekFrom::Start(offset))
                            .and_then(|_| file.write_all(&bytes))
                            .map_err(|e| e.to_string())
                    };
                    let mut progress = |_: u64, _: u64| Ok(());
                    let mut callbacks = flash::Callbacks {
                        chunk: &mut chunk,
                        progress: &mut progress,
                    };
                    match flash::backup(
                        session,
                        &bulk,
                        &flash,
                        &artifact.payload,
                        &image,
                        limits,
                        &mut callbacks,
                    )
                    .await
                    {
                        Ok(backup) => Ok(Ok((
                            backup.size,
                            backup.sha256,
                            backup.uncorrectable_page_offsets,
                            backup.uncorrectable_granularity_bytes,
                        ))),
                        Err(BulkError::Usb(error)) => Err(error),
                        Err(other) => Ok(Err(other.to_string())),
                    }
                })
            })
            .await?;
        outcome.map_err(PyRuntimeError::new_err)
    }

    /// Read `size` flash bytes at window offset `offset` with a staged
    /// artifact, as `backup_flash` does for the whole flash. The reader
    /// checks its control bytes and restores what it borrowed first.
    #[pyo3(signature = (artifact, offset, size, image=Vec::new()))]
    async fn read_flash_range(
        &self,
        artifact: String,
        offset: u64,
        size: u32,
        image: Vec<u8>,
    ) -> PyResult<Vec<u8>> {
        let (artifact, flash, bulk) = staged(&artifact, &image)?;
        let limits = fujimod_workflows::Limits {
            memory: self.limits.memory,
            configuration: self.limits.configuration,
        };
        let outcome = self
            .with(move |session| {
                Box::pin(async move {
                    match flash::read_range(
                        session,
                        &bulk,
                        &flash,
                        &artifact.payload,
                        &image,
                        limits,
                        offset,
                        size,
                    )
                    .await
                    {
                        Ok(bytes) => Ok(Ok(bytes)),
                        Err(BulkError::Usb(error)) => Err(error),
                        Err(other) => Ok(Err(other.to_string())),
                    }
                })
            })
            .await?;
        outcome.map_err(PyRuntimeError::new_err)
    }
}
