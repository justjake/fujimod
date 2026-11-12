//! Ownership adapters for optional card file capabilities. Operations run in Rust.
use super::*;
use fujimod_camera_mount::harness::MountHarness;
use fujimod_catalog::ReferenceCatalog;
use usb::operations::card::{self, FirmwareCard, Request, Response};

fn profile(raw: &str) -> PyResult<FirmwareCard> {
    serde_json::from_str(raw).map_err(|e| PyValueError::new_err(e.to_string()))
}
impl AsyncSession {
    async fn card(&self, capability: FirmwareCard, request: Request) -> PyResult<Response> {
        let limits = self.limits.memory;
        self.with(move |session| {
            Box::pin(async move { card::execute(session, limits, &capability, request).await })
        })
        .await
    }

    async fn mount(&self, request: Request) -> PyResult<Response> {
        let slot = self.mount.clone();
        let limits = self.limits.memory;
        self.with(move |session| {
            Box::pin(async move {
                let mut helper = slot
                    .lock()
                    .map_err(|_| usb::Error::Card("Mount helper state is poisoned".into()))?
                    .take()
                    .ok_or_else(|| usb::Error::Card("Mount helper is not installed".into()))?;
                let result = helper.execute(session, limits, request).await;
                *slot
                    .lock()
                    .map_err(|_| usb::Error::Card("Mount helper state is poisoned".into()))? =
                    Some(helper);
                result
            })
        })
        .await
    }
}
#[pymethods]
impl AsyncSession {
    /// List matching files on SD0 with the selected firmware capability.
    async fn card_list(&self, capability: String, pattern: String) -> PyResult<String> {
        match self
            .card(profile(&capability)?, Request::List { pattern })
            .await?
        {
            Response::Entries(entries) => {
                serde_json::to_string(&entries).map_err(|e| PyRuntimeError::new_err(e.to_string()))
            }
            _ => unreachable!("list response"),
        }
    }
    /// Read through EOF, refusing files above the explicit limit.
    async fn card_read(
        &self,
        capability: String,
        path: String,
        max_bytes: u32,
    ) -> PyResult<Py<PyBytes>> {
        match self
            .card(profile(&capability)?, Request::Read { path, max_bytes })
            .await?
        {
            Response::Bytes(data) => Ok(bytes(data)),
            _ => unreachable!("read response"),
        }
    }
    /// Write a file through the selected backend, then close it.
    async fn card_write(&self, capability: String, path: String, data: Vec<u8>) -> PyResult<()> {
        self.card(profile(&capability)?, Request::Write { path, data })
            .await?;
        Ok(())
    }
    async fn card_delete(&self, capability: String, path: String) -> PyResult<()> {
        self.card(profile(&capability)?, Request::Delete { path })
            .await?;
        Ok(())
    }

    /// Allocate and load the mounted firmware's owned file helper.
    #[pyo3(signature = (capability, bulk=None))]
    async fn mount_install(&self, capability: String, bulk: Option<String>) -> PyResult<()> {
        let capability = profile(&capability)?;
        let bulk = match bulk {
            Some(raw) => serde_json::from_str(&raw)
                .map_err(|error| PyValueError::new_err(error.to_string()))?,
            None => ReferenceCatalog::bundled()
                .map_err(PyRuntimeError::new_err)?
                .resolve_firmware(&capability.firmware_id)
                .map_err(PyRuntimeError::new_err)?
                .bulk
                .ok_or_else(|| PyRuntimeError::new_err("Selected firmware has no bulk profile"))?,
        };
        let slot = self.mount.clone();
        let limits = self.limits.memory;
        self.with(move |session| {
            Box::pin(async move {
                let installed = slot
                    .lock()
                    .map_err(|_| usb::Error::Card("Mount helper state is poisoned".into()))?
                    .is_some();
                if installed {
                    return Err(usb::Error::Card("Mount helper is already installed".into()));
                }
                let helper = MountHarness::install(session, limits, &bulk, &capability).await?;
                *slot
                    .lock()
                    .map_err(|_| usb::Error::Card("Mount helper state is poisoned".into()))? =
                    Some(helper);
                Ok(())
            })
        })
        .await
    }

    async fn mount_list(&self, path: String) -> PyResult<String> {
        match self.mount(Request::ReadDirectory { path }).await? {
            Response::Directory(entries) => {
                serde_json::to_string(&entries).map_err(|e| PyRuntimeError::new_err(e.to_string()))
            }
            _ => unreachable!("directory response"),
        }
    }

    async fn mount_read(&self, path: String, offset: u32, size: u32) -> PyResult<Py<PyBytes>> {
        match self.mount(Request::ReadAt { path, offset, size }).await? {
            Response::Bytes(data) => Ok(bytes(data)),
            _ => unreachable!("read response"),
        }
    }

    async fn mount_write_at(&self, path: String, offset: u32, data: Vec<u8>) -> PyResult<()> {
        self.mount(Request::WriteAt { path, offset, data }).await?;
        Ok(())
    }

    async fn mount_create(&self, path: String) -> PyResult<()> {
        self.mount(Request::Create { path }).await?;
        Ok(())
    }

    async fn mount_shrink(&self, path: String, size: u32) -> PyResult<()> {
        self.mount(Request::Shrink { path, size }).await?;
        Ok(())
    }

    async fn mount_delete(&self, path: String) -> PyResult<()> {
        self.mount(Request::Delete { path }).await?;
        Ok(())
    }

    async fn mount_unload(&self) -> PyResult<()> {
        let slot = self.mount.clone();
        let limits = self.limits.memory;
        self.with(move |session| {
            Box::pin(async move {
                let helper = slot
                    .lock()
                    .map_err(|_| usb::Error::Card("Mount helper state is poisoned".into()))?
                    .take();
                match helper {
                    Some(helper) => helper.unload(session, limits).await,
                    None => Ok(()),
                }
            })
        })
        .await
    }
}
