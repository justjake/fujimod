//! Shared definition types, without USB ownership or command implementation.
use super::*;
use error::{integer, property, record};
use fujimod_catalog_firmware_identity::IdentityError;
use fujimod_catalog_service as core;

include!(concat!(env!("OUT_DIR"), "/definition_codec_javascript.rs"));
#[wasm_bindgen]
#[derive(Clone, Copy)]
pub enum DefinitionFailureKind {
    Json,
    Schema,
    EmptyIdentity,
    InvalidDigest,
    FirmwareMismatch,
    EmptyModes,
    InvalidMailbox,
    InvalidLayout,
    SourceCount,
    EmptyLabel,
}

pub(crate) fn invalid(source: core::DefinitionError) -> JsValue {
    use DefinitionFailureKind as K;
    use core::DefinitionError as E;
    let error = fail(ErrorKind::Definition, &source.to_string());
    let reason = match source {
        E::Json(e) => record(
            K::Json as u32,
            &[
                ("line", (e.line() as u32).into()),
                ("column", (e.column() as u32).into()),
            ],
        ),
        E::Schema(schema) => record(K::Schema as u32, &[("schema", schema.into())]),
        E::Identity(IdentityError::EmptyIdentity) => record(K::EmptyIdentity as u32, &[]),
        E::Identity(IdentityError::InvalidDigest) => record(K::InvalidDigest as u32, &[]),
        E::Identity(IdentityError::FirmwareMismatch) => record(K::FirmwareMismatch as u32, &[]),
        E::EmptyModes => record(K::EmptyModes as u32, &[]),
        E::InvalidMailbox => record(K::InvalidMailbox as u32, &[]),
        E::InvalidLayout { code, codec } => record(
            K::InvalidLayout as u32,
            &[
                ("code", code.into()),
                ("codec", (ServiceCodec::from(codec) as u32).into()),
            ],
        ),
        E::SourceCount => record(K::SourceCount as u32, &[]),
        E::EmptyLabel => record(K::EmptyLabel as u32, &[]),
    };
    property(&error, "reason", reason);
    error
}
#[wasm_bindgen]
#[derive(Clone)]
pub struct FirmwareIdentity {
    pub(crate) inner: fujimod_catalog_firmware_identity::FirmwareIdentity,
}
#[wasm_bindgen]
impl FirmwareIdentity {
    #[wasm_bindgen(constructor)]
    pub fn new(id: String, sha256: &str) -> Result<Self, JsValue> {
        fujimod_catalog_firmware_identity::FirmwareIdentity::new(id, sha256)
            .map(|inner| Self { inner })
            .map_err(|error| invalid(error.into()))
    }
    #[wasm_bindgen(getter)]
    pub fn id(&self) -> String {
        self.inner.id().to_owned()
    }
    #[wasm_bindgen(getter)]
    pub fn sha256(&self) -> String {
        self.inner.digest().hex()
    }
}
#[wasm_bindgen]
#[derive(Clone, Copy)]
pub struct ServiceModes {
    pub(crate) inner: core::ServiceModes,
}
#[wasm_bindgen]
impl ServiceModes {
    #[wasm_bindgen(constructor)]
    pub fn new(
        #[wasm_bindgen(unchecked_param_type = "number")] mask: JsValue,
    ) -> Result<Self, JsValue> {
        core::ServiceModes::new(integer(mask, u32::MAX)?)
            .map(|inner| Self { inner })
            .map_err(invalid)
    }
    #[wasm_bindgen(getter)]
    pub fn mask(&self) -> u32 {
        self.inner.mask()
    }
}
#[wasm_bindgen]
#[derive(Clone, Copy)]
pub enum ModeAvailabilityKind {
    Modes,
    Disabled,
    Unreviewed,
}
#[wasm_bindgen]
#[derive(Clone, Copy)]
pub enum ModeTransitionKind {
    Keep,
    Set,
}
#[wasm_bindgen]
pub struct ServiceEntry {
    inner: core::Entry,
}
#[wasm_bindgen]
impl ServiceEntry {
    #[wasm_bindgen(getter)]
    pub fn code(&self) -> u8 {
        self.inner.code()
    }
    #[wasm_bindgen(getter)]
    pub fn label(&self) -> String {
        self.inner.label().to_owned()
    }
    #[wasm_bindgen(getter)]
    pub fn codec(&self) -> ServiceCodec {
        self.inner.codec().into()
    }
    #[wasm_bindgen(getter, unchecked_return_type = "Uint8Array")]
    pub fn slots(&self) -> JsValue {
        error::bytes(self.inner.slots())
    }
    #[wasm_bindgen(getter,js_name=enabledModes,unchecked_return_type="ModeAvailability")]
    pub fn enabled_modes(&self) -> JsValue {
        use ModeAvailabilityKind as K;
        use core::ModeAvailability as M;
        match self.inner.enabled_modes() {
            M::Modes(inner) => record(K::Modes as u32, &[("modes", ServiceModes { inner }.into())]),
            M::Disabled => record(K::Disabled as u32, &[]),
            M::Unreviewed => record(K::Unreviewed as u32, &[]),
        }
    }
    #[wasm_bindgen(getter, unchecked_return_type = "ModeTransition")]
    pub fn transition(&self) -> JsValue {
        match self.inner.transition() {
            core::ModeTransition::Keep => record(ModeTransitionKind::Keep as u32, &[]),
            core::ModeTransition::Set(inner) => record(
                ModeTransitionKind::Set as u32,
                &[("modes", ServiceModes { inner }.into())],
            ),
        }
    }
    #[wasm_bindgen(getter,js_name=firmwareKind)]
    pub fn firmware_kind(&self) -> u32 {
        self.inner.firmware_kind()
    }
    #[wasm_bindgen(getter)]
    pub fn handler(&self) -> u64 {
        self.inner.handler()
    }
    #[wasm_bindgen(getter,js_name=parameterTable)]
    pub fn parameter_table(&self) -> u64 {
        self.inner.parameter_table()
    }
}
#[wasm_bindgen]
pub struct ServiceMailbox {
    inner: core::Mailbox,
}
#[wasm_bindgen]
impl ServiceMailbox {
    #[wasm_bindgen(getter)]
    pub fn page(&self) -> u32 {
        self.inner.page()
    }
    #[wasm_bindgen(getter)]
    pub fn offset(&self) -> u32 {
        self.inner.offset()
    }
    #[wasm_bindgen(getter,js_name=configurationLimit)]
    pub fn configuration_limit(&self) -> u32 {
        self.inner.configuration_limit().get()
    }
}
#[wasm_bindgen]
#[derive(Clone)]
pub struct ServiceDefinition {
    pub(crate) inner: core::Definition,
}
#[wasm_bindgen]
impl ServiceDefinition {
    #[wasm_bindgen(js_name=fromJson)]
    pub fn from_json(json: &str, expected: &FirmwareIdentity) -> Result<Self, JsValue> {
        core::Definition::from_json(json, &expected.inner)
            .map(|inner| Self { inner })
            .map_err(invalid)
    }
    #[wasm_bindgen(getter)]
    pub fn identity(&self) -> FirmwareIdentity {
        FirmwareIdentity {
            inner: self.inner.identity().clone(),
        }
    }
    #[wasm_bindgen(getter)]
    pub fn model(&self) -> String {
        self.inner.model().to_owned()
    }
    #[wasm_bindgen(getter)]
    pub fn version(&self) -> String {
        self.inner.version().to_owned()
    }
    #[wasm_bindgen(getter)]
    pub fn mailbox(&self) -> ServiceMailbox {
        ServiceMailbox {
            inner: self.inner.mailbox(),
        }
    }
    #[wasm_bindgen(getter, unchecked_return_type = "ServiceEntry[]")]
    pub fn entries(&self) -> js_sys::Array {
        self.inner
            .entries()
            .iter()
            .cloned()
            .map(|inner| JsValue::from(ServiceEntry { inner }))
            .collect()
    }
    pub fn select(
        &self,
        #[wasm_bindgen(unchecked_param_type = "number")] code: JsValue,
        modes: &ServiceModes,
    ) -> Result<Option<ServiceEntry>, JsValue> {
        let code = integer(code, u8::MAX.into())? as u8;
        Ok(self
            .inner
            .select(code, modes.inner)
            .cloned()
            .map(|inner| ServiceEntry { inner }))
    }
}
#[wasm_bindgen(typescript_custom_section)]
const TYPES: &str = r#"
export type ModeAvailability =
  | Readonly<{ kind: ModeAvailabilityKind.Modes; modes: ServiceModes }>
  | Readonly<{ kind: ModeAvailabilityKind.Disabled | ModeAvailabilityKind.Unreviewed }>;
export type ModeTransition =
  | Readonly<{ kind: ModeTransitionKind.Keep }>
  | Readonly<{ kind: ModeTransitionKind.Set; modes: ServiceModes }>;
export type DefinitionFailure =
  | Readonly<{ kind: DefinitionFailureKind.Json; line: number; column: number }>
  | Readonly<{ kind: DefinitionFailureKind.Schema; schema: number }>
  | Readonly<{ kind: DefinitionFailureKind.InvalidLayout; code: number; codec: ServiceCodec }>
  | Readonly<{ kind: DefinitionFailureKind.EmptyIdentity | DefinitionFailureKind.InvalidDigest |
      DefinitionFailureKind.FirmwareMismatch | DefinitionFailureKind.EmptyModes |
      DefinitionFailureKind.InvalidMailbox | DefinitionFailureKind.SourceCount | DefinitionFailureKind.EmptyLabel }>;
"#;
