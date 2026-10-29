//! Firmware-selected camera workflows share the session's ordinary lease.
use crate::{error::property, session::Operation, session::Session, *};
use fujimod_catalog_firmware_identity::FirmwareIdentity;
use fujimod_catalog_workflows::{DebugPersistence, ServiceSetup, WorkflowDefinition};
use fujimod_workflows::{Limits, bulk, debug_mode, service_mode};

pub(crate) enum Action {
    ArmResetGuard,
    PrepareServiceSession,
    Enter,
    Leave,
}
pub(crate) struct Workflow {
    camera: ServiceSetup,
    persistence: Option<DebugPersistence>,
    limits: Limits,
    action: Action,
}
pub(crate) fn workflow_error(message: impl std::fmt::Display) -> JsValue {
    fail(ErrorKind::Workflow, &message.to_string())
}
impl Session {
    fn workflow(
        &self,
        workflow_facts: &str,
        firmware_id: String,
        firmware_sha256: &str,
        action: Action,
    ) -> Result<js_sys::Promise, JsValue> {
        let identity =
            FirmwareIdentity::new(firmware_id, firmware_sha256).map_err(workflow_error)?;
        let definition =
            WorkflowDefinition::from_json(workflow_facts, &identity).map_err(workflow_error)?;
        let camera = definition.setup().map_err(workflow_error)?.clone();
        let persistence = if matches!(action, Action::Enter | Action::Leave) {
            Some(definition.persistence().map_err(workflow_error)?.clone())
        } else {
            None
        };
        self.call(Operation::Workflow(Workflow {
            camera,
            persistence,
            limits: Limits {
                memory: self.limits.memory,
                configuration: self.limits.configuration,
            },
            action,
        }))
    }
}
#[wasm_bindgen]
impl Session {
    #[wasm_bindgen(js_name=armResetGuard, unchecked_return_type="Promise<ResetGuardResult>")]
    pub fn arm_reset_guard(
        &self,
        workflow_facts: &str,
        firmware_id: String,
        firmware_sha256: &str,
    ) -> Result<js_sys::Promise, JsValue> {
        self.workflow(
            workflow_facts,
            firmware_id,
            firmware_sha256,
            Action::ArmResetGuard,
        )
    }
    #[wasm_bindgen(js_name=prepareServiceSession, unchecked_return_type="Promise<ServicePreparation>")]
    pub fn prepare_service_session(
        &self,
        workflow_facts: &str,
        firmware_id: String,
        firmware_sha256: &str,
    ) -> Result<js_sys::Promise, JsValue> {
        self.workflow(
            workflow_facts,
            firmware_id,
            firmware_sha256,
            Action::PrepareServiceSession,
        )
    }
    #[wasm_bindgen(js_name=enterDebugMode, unchecked_return_type="Promise<DebugModeResult>")]
    pub fn enter_debug_mode(
        &self,
        workflow_facts: &str,
        firmware_id: String,
        firmware_sha256: &str,
    ) -> Result<js_sys::Promise, JsValue> {
        self.workflow(workflow_facts, firmware_id, firmware_sha256, Action::Enter)
    }
    #[wasm_bindgen(js_name=leaveDebugMode, unchecked_return_type="Promise<DebugModeResult>")]
    pub fn leave_debug_mode(
        &self,
        workflow_facts: &str,
        firmware_id: String,
        firmware_sha256: &str,
    ) -> Result<js_sys::Promise, JsValue> {
        self.workflow(workflow_facts, firmware_id, firmware_sha256, Action::Leave)
    }
}
impl Workflow {
    pub(crate) async fn run(self, session: &mut usb::ff80::Session) -> Result<JsValue, JsValue> {
        let mut lines = Vec::new();
        let mut progress = |line: &str| lines.push(line.to_owned());
        let result = js_sys::Object::new();
        match self.action {
            Action::ArmResetGuard | Action::PrepareServiceSession => {
                let armed = match self.action {
                    Action::PrepareServiceSession => {
                        service_mode::prepare_service_session(
                            session,
                            &self.camera,
                            self.limits,
                            &mut progress,
                        )
                        .await
                    }
                    _ => {
                        service_mode::arm_reset_guard(
                            session,
                            &self.camera,
                            self.limits,
                            &mut progress,
                        )
                        .await
                    }
                }
                .map_err(|source| match source {
                    service_mode::ServiceModeError::Usb(source) => error(source),
                    other => workflow_error(format!("{other} (after: {})", lines.join("; "))),
                })?;
                property(&result, "guardBefore", u8::from(armed.guard_was_set));
                property(&result, "debugBefore", armed.debug_flag_before);
                property(&result, "guardWasSet", armed.guard_was_set);
                property(&result, "debugFlagBefore", armed.debug_flag_before);
            }
            Action::Enter | Action::Leave => {
                let outcome = match self.action {
                    Action::Enter => {
                        debug_mode::enter(
                            session,
                            &self.camera,
                            self.persistence.as_ref().expect("qualified persistence"),
                            self.limits,
                            &mut progress,
                        )
                        .await
                    }
                    Action::Leave => {
                        debug_mode::leave(
                            session,
                            &self.camera,
                            self.persistence.as_ref().expect("qualified persistence"),
                            self.limits,
                            &mut progress,
                        )
                        .await
                    }
                    Action::ArmResetGuard | Action::PrepareServiceSession => unreachable!(),
                }
                .map_err(|source| match source {
                    debug_mode::DebugModeError::Usb(source)
                    | debug_mode::DebugModeError::Guard(service_mode::ServiceModeError::Usb(
                        source,
                    )) => error(source),
                    other => workflow_error(format!("{other} (after: {})", lines.join("; "))),
                })?;
                property(&result, "alreadyPersisted", outcome.already_persisted);
            }
        }
        property(
            &result,
            "progress",
            lines
                .into_iter()
                .map(JsValue::from)
                .collect::<js_sys::Array>(),
        );
        Ok(result.into())
    }
}
#[wasm_bindgen(typescript_custom_section)]
const TYPES: &str = r#"
export interface ServicePreparation {
    guardBefore: number;
    debugBefore: number;
    progress: string[];
}
export interface ResetGuardResult {
    guardWasSet: boolean;
    debugFlagBefore: number;
    progress: string[];
}
export interface DebugModeResult {
    alreadyPersisted: boolean;
    progress: string[];
}
"#;

pub(crate) struct StateDump {
    bulk: fujimod_catalog_bulk::FirmwareBulk,
    state: fujimod_catalog::capture::FirmwareState,
    limits: Limits,
    sink: js_sys::Function,
    progress: Option<js_sys::Function>,
}
pub(crate) struct FlashBackup {
    bulk: fujimod_catalog_bulk::FirmwareBulk,
    flash: fujimod_catalog::capture::FirmwareFlash,
    payload: fujimod_workflows::flash::Payload,
    image: Vec<u8>,
    limits: Limits,
    sink: js_sys::Function,
    progress: Option<js_sys::Function>,
}
#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(typescript_type = "ByteProgress")]
    pub type ByteProgressCallback;
}
fn progress_callback(
    value: Option<ByteProgressCallback>,
) -> Result<Option<js_sys::Function>, JsValue> {
    value
        .map(|callback| {
            callback
                .dyn_into()
                .map_err(|_| workflow_error("Progress callback must be a function"))
        })
        .transpose()
}
#[wasm_bindgen]
impl Session {
    #[wasm_bindgen(js_name=dumpState, unchecked_return_type="Promise<void>")]
    pub fn dump_state(
        &self,
        firmware_id: &str,
        #[wasm_bindgen(unchecked_param_type = "(region: CapturedStateRegion) => void")]
        sink: js_sys::Function,
        progress: Option<ByteProgressCallback>,
    ) -> Result<js_sys::Promise, JsValue> {
        let selection = crate::reference::resolve(firmware_id)?;
        let bulk = selection.bulk.ok_or_else(|| {
            workflow_error(fujimod_catalog::generation::missing_table_reason(
                firmware_id,
                "bulk",
            ))
        })?;
        let state = selection.state.ok_or_else(|| {
            workflow_error(fujimod_catalog::generation::missing_table_reason(
                firmware_id,
                "state",
            ))
        })?;
        self.call(Operation::StateDump(StateDump {
            bulk,
            state,
            sink,
            progress: progress_callback(progress)?,
            limits: Limits {
                memory: self.limits.memory,
                configuration: self.limits.configuration,
            },
        }))
    }
    #[wasm_bindgen(js_name=backupFlash, unchecked_return_type="Promise<FlashBackupResult>")]
    pub fn backup_flash(
        &self,
        firmware_id: &str,
        payload_table: &str,
        image: Vec<u8>,
        #[wasm_bindgen(
            unchecked_param_type = "(piece: { offset: number; data: Uint8Array }) => void"
        )]
        sink: js_sys::Function,
        progress: Option<ByteProgressCallback>,
    ) -> Result<js_sys::Promise, JsValue> {
        let selection = crate::reference::resolve(firmware_id)?;
        let bulk = selection.bulk.ok_or_else(|| {
            workflow_error(fujimod_catalog::generation::missing_table_reason(
                firmware_id,
                "bulk",
            ))
        })?;
        let flash = selection.flash.ok_or_else(|| {
            workflow_error(fujimod_catalog::generation::missing_table_reason(
                firmware_id,
                "flash",
            ))
        })?;
        let payload: fujimod_workflows::flash::Payload =
            serde_json::from_str(payload_table).map_err(workflow_error)?;
        if selection
            .firmware
            .update
            .as_ref()
            .is_none_or(|update| payload.firmware_sha256() != update.sha256)
        {
            return Err(workflow_error("Flash payload firmware hash differs"));
        }
        payload
            .validate(&flash, &bulk, &image)
            .map_err(workflow_error)?;
        self.call(Operation::FlashBackup(Box::new(FlashBackup {
            bulk,
            flash,
            payload,
            image,
            sink,
            progress: progress_callback(progress)?,
            limits: Limits {
                memory: self.limits.memory,
                configuration: self.limits.configuration,
            },
        })))
    }
}
fn sink_error(error: JsValue) -> String {
    error
        .as_string()
        .unwrap_or_else(|| format!("Download callback failed: {error:?}"))
}
fn byte_progress(callback: Option<&js_sys::Function>, done: u64, total: u64) -> Result<(), String> {
    if let Some(callback) = callback {
        callback
            .call2(
                &JsValue::UNDEFINED,
                &JsValue::from_f64(done as f64),
                &JsValue::from_f64(total as f64),
            )
            .map_err(sink_error)?;
    }
    Ok(())
}
impl StateDump {
    pub(crate) async fn run(self, session: &mut usb::ff80::Session) -> Result<JsValue, JsValue> {
        fujimod_workflows::state::dump(
            session,
            &self.bulk,
            &self.state,
            self.limits,
            &mut |captured| {
                let result = js_sys::Object::new();
                property(&result, "name", captured.region.name);
                property(&result, "address", captured.region.address as f64);
                property(&result, "size", captured.region.size as f64);
                property(&result, "constant", captured.region.constant);
                property(&result, "sha256", captured.sha256);
                property(&result, "data", crate::error::bytes(captured.data));
                self.sink
                    .call1(&JsValue::UNDEFINED, &result)
                    .map(|_| ())
                    .map_err(sink_error)
            },
            &mut |done, total| byte_progress(self.progress.as_ref(), done, total),
        )
        .await
        .map_err(capture_error)?;
        Ok(JsValue::UNDEFINED)
    }
}
impl FlashBackup {
    pub(crate) async fn run(self, session: &mut usb::ff80::Session) -> Result<JsValue, JsValue> {
        let backup = fujimod_workflows::flash::backup(
            session,
            &self.bulk,
            &self.flash,
            &self.payload,
            &self.image,
            self.limits,
            &mut fujimod_workflows::flash::Callbacks {
                chunk: &mut |offset, data| {
                    let piece = js_sys::Object::new();
                    property(&piece, "offset", offset as f64);
                    property(&piece, "data", crate::error::bytes(data));
                    self.sink
                        .call1(&JsValue::UNDEFINED, &piece)
                        .map(|_| ())
                        .map_err(sink_error)
                },
                progress: &mut |done, total| byte_progress(self.progress.as_ref(), done, total),
            },
        )
        .await
        .map_err(capture_error)?;
        let result = js_sys::Object::new();
        let device = js_sys::Object::new();
        property(&device, "kind", backup.device.kind);
        property(&device, "part", backup.device.part);
        property(&result, "device", device);
        property(&result, "size", backup.size as f64);
        property(&result, "sha256", backup.sha256);
        Ok(result.into())
    }
}
fn capture_error(source: bulk::BulkError) -> JsValue {
    match source {
        bulk::BulkError::Usb(source) => error(source),
        other => workflow_error(other),
    }
}
#[wasm_bindgen(typescript_custom_section)]
const CAPTURE_TYPES: &str = r#"
export type ByteProgress = (done: number, total: number) => void;
export interface CapturedStateRegion {
    name: string;
    address: number;
    size: number;
    constant: boolean;
    sha256: string;
    data: Uint8Array;
}
export interface FlashBackupResult {
    device: { kind: string; part?: string };
    size: number;
    sha256: string;
}
"#;
