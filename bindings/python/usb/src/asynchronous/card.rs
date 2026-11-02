//! Ownership adapters for optional card file capabilities. Operations run in Rust.
use super::*;
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
}
