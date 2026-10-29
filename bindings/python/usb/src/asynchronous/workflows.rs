//! Camera workflows on an open session. Firmware facts arrive as the selected
//! firmware's generated capability contracts; the workflow, not Python, decides each step.
use super::*;
use fujimod_catalog_firmware_identity::FirmwareIdentity;
use fujimod_catalog_workflows::WorkflowDefinition;
use fujimod_workflows::{debug_mode, service_mode};

fn definition(json: &str, id: String, sha256: &str) -> PyResult<WorkflowDefinition> {
    let identity =
        FirmwareIdentity::new(id, sha256).map_err(|e| PyValueError::new_err(e.to_string()))?;
    WorkflowDefinition::from_json(json, &identity).map_err(|e| PyValueError::new_err(e.to_string()))
}

#[pymethods]
impl AsyncSession {
    /// Confirm stock service mode for the selected firmware and arm its reset
    /// guard. Returns (guard_was_set, debug_flag_before, progress_lines).
    /// A failed check raises RuntimeError; USB failures raise UsbError.
    async fn arm_reset_guard(
        &self,
        workflow_facts: String,
        firmware_id: String,
        firmware_sha256: String,
    ) -> PyResult<(bool, u8, Vec<String>)> {
        let definition = definition(&workflow_facts, firmware_id, &firmware_sha256)?;
        let camera = definition
            .setup()
            .map_err(|e| PyValueError::new_err(e.to_string()))?
            .clone();
        let limits = fujimod_workflows::Limits {
            memory: self.limits.memory,
            configuration: self.limits.configuration,
        };
        let outcome = self
            .with(move |session| {
                Box::pin(async move {
                    let mut lines = Vec::new();
                    let result =
                        service_mode::arm_reset_guard(session, &camera, limits, &mut |line| {
                            lines.push(line.to_string())
                        })
                        .await;
                    match result {
                        Ok(armed) => Ok(Ok((armed, lines))),
                        Err(service_mode::ServiceModeError::Usb(error)) => Err(error),
                        Err(other) => Ok(Err(format!("{other} (after: {})", lines.join("; ")))),
                    }
                })
            })
            .await?;
        let (armed, lines) = outcome.map_err(PyRuntimeError::new_err)?;
        Ok((armed.guard_was_set, armed.debug_flag_before, lines))
    }
}

impl AsyncSession {
    async fn persist_debug_mode(
        &self,
        workflow_facts: String,
        firmware_id: String,
        firmware_sha256: String,
        enabled: bool,
    ) -> PyResult<(bool, Vec<String>)> {
        let definition = definition(&workflow_facts, firmware_id, &firmware_sha256)?;
        let camera = definition
            .setup()
            .map_err(|e| PyValueError::new_err(e.to_string()))?
            .clone();
        let persistence = definition
            .persistence()
            .map_err(|e| PyValueError::new_err(e.to_string()))?
            .clone();
        let limits = fujimod_workflows::Limits {
            memory: self.limits.memory,
            configuration: self.limits.configuration,
        };
        let outcome = self
            .with(move |session| {
                Box::pin(async move {
                    let mut lines = Vec::new();
                    let mut progress = |line: &str| lines.push(line.to_string());
                    let result = if enabled {
                        debug_mode::enter(session, &camera, &persistence, limits, &mut progress)
                            .await
                    } else {
                        debug_mode::leave(session, &camera, &persistence, limits, &mut progress)
                            .await
                    };
                    match result {
                        Ok(persisted) => Ok(Ok((persisted.already_persisted, lines))),
                        Err(
                            debug_mode::DebugModeError::Usb(error)
                            | debug_mode::DebugModeError::Guard(service_mode::ServiceModeError::Usb(
                                error,
                            )),
                        ) => Err(error),
                        Err(other) => Ok(Err(format!("{other} (after: {})", lines.join("; ")))),
                    }
                })
            })
            .await?;
        outcome.map_err(PyRuntimeError::new_err)
    }
}

#[pymethods]
impl AsyncSession {
    /// Persist debug access enabled, retaining the RAM reset guard and keeping
    /// its stored value clear. Returns (already_persisted, progress_lines).
    async fn enter_debug_mode(
        &self,
        workflow_facts: String,
        firmware_id: String,
        firmware_sha256: String,
    ) -> PyResult<(bool, Vec<String>)> {
        self.persist_debug_mode(workflow_facts, firmware_id, firmware_sha256, true)
            .await
    }

    /// Persist debug access disabled. Returns (already_persisted, progress_lines).
    async fn leave_debug_mode(
        &self,
        workflow_facts: String,
        firmware_id: String,
        firmware_sha256: String,
    ) -> PyResult<(bool, Vec<String>)> {
        self.persist_debug_mode(workflow_facts, firmware_id, firmware_sha256, false)
            .await
    }
}
