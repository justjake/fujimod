//! Camera restart through stock code, from one firmware's catalog restart facts.
use super::*;
use fujimod_catalog_restart::FirmwareRestart;
use fujimod_workflows::{
    bulk::BulkError,
    restart::{self, Requested},
};

#[pymethods]
impl AsyncSession {
    /// Serialize the validated timer/wake facts into the portable ARM32
    /// queue-auto-restart guard contract. This performs no USB I/O.
    fn deferred_restart_contract(&self, profile: String, milliseconds: u32) -> PyResult<Vec<u8>> {
        let restart: FirmwareRestart =
            serde_json::from_str(&profile).map_err(|e| PyValueError::new_err(e.to_string()))?;
        restart::deferred_guard_contract(&restart, milliseconds).map_err(|failure| match failure {
            BulkError::Usb(source) => error(source),
            other => PyValueError::new_err(other.to_string()),
        })
    }

    /// Request a restart with the firmware's `restart.generated.json` profile
    /// (`fujimod.catalog.restart_profiles()`). Returns "accepted" when the
    /// query returned, or "session-ended" when the camera cut power before
    /// answering. A failed check raises RuntimeError with nothing written;
    /// USB failures raise UsbError.
    #[pyo3(signature = (profile, recover=false))]
    async fn request_restart(&self, profile: String, recover: bool) -> PyResult<String> {
        let restart: FirmwareRestart =
            serde_json::from_str(&profile).map_err(|e| PyValueError::new_err(e.to_string()))?;
        restart.validate().map_err(PyValueError::new_err)?;
        let limits = fujimod_workflows::Limits {
            memory: self.limits.memory,
            configuration: self.limits.configuration,
        };
        let outcome = self
            .with(move |session| {
                Box::pin(async move {
                    match restart::request(session, &restart, limits, recover).await {
                        Ok(requested) => Ok(Ok(requested)),
                        Err(BulkError::Usb(error)) => Err(error),
                        Err(other) => Ok(Err(other.to_string())),
                    }
                })
            })
            .await?;
        Ok(match outcome.map_err(PyRuntimeError::new_err)? {
            Requested::Accepted => "accepted",
            Requested::SessionEnded => "session-ended",
        }
        .to_string())
    }
}
