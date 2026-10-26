//! Typed App80 values and conversions. Wire codecs live in fujimod-usb.
use super::*;
use catalog::{ServiceCodec, ServiceDefinition, ServiceModes};
use error::{integer, property, record};
use fujimod_usb_app80_types::catalog as core;
mod values {
    include!(concat!(env!("OUT_DIR"), "/service_values_javascript.rs"));
}
use values::*;

#[wasm_bindgen]
#[derive(Clone, Copy)]
pub enum CommandFailureKind {
    Unsupported,
    Ambiguous,
    Unavailable,
    Argument,
    OversizeArguments,
}

fn invalid(source: core::CommandError) -> JsValue {
    use CommandFailureKind as K;
    use core::CommandError as E;
    let error = fail(ErrorKind::ServiceCommand, &source.to_string());
    let reason = match source {
        E::Unsupported(codec) => record(
            K::Unsupported as u32,
            &[("codec", (ServiceCodec::from(codec) as u32).into())],
        ),
        E::Ambiguous(codec) => record(
            K::Ambiguous as u32,
            &[("codec", (ServiceCodec::from(codec) as u32).into())],
        ),
        E::Unavailable(code) => record(K::Unavailable as u32, &[("code", code.0.into())]),
        E::Argument(kind) => record(
            K::Argument as u32,
            &[("argument", (ArgumentKind::from(kind) as u32).into())],
        ),
        E::OversizeArguments => record(K::OversizeArguments as u32, &[]),
    };
    property(&error, "reason", reason);
    error
}

#[wasm_bindgen]
pub struct App80CommandReply {
    pub(crate) inner: core::CommandReply,
}
#[wasm_bindgen]
impl App80CommandReply {
    #[wasm_bindgen(getter, unchecked_return_type = "Uint8Array")]
    pub fn raw(&self) -> JsValue {
        error::bytes(self.inner.raw().raw())
    }
    #[wasm_bindgen(getter, unchecked_return_type = "App80ReplyValue")]
    pub fn value(&self) -> JsValue {
        App80ReplyKind::value(self.inner.value())
    }
}
#[wasm_bindgen]
#[derive(Clone)]
pub struct App80Invocation {
    pub(crate) inner: fujimod_usb_app80::operations::Invocation,
}
#[wasm_bindgen]
impl App80Invocation {
    #[wasm_bindgen(getter, unchecked_return_type = "Uint8Array")]
    pub fn raw(&self) -> JsValue {
        error::bytes(self.inner.command().request().encode())
    }
    #[wasm_bindgen(getter,js_name=configurationLimit)]
    pub fn configuration_limit(&self) -> u32 {
        self.inner.configuration_limit().get()
    }
    pub fn decode(
        &self,
        #[wasm_bindgen(unchecked_param_type = "Uint8Array")] raw: JsValue,
    ) -> Result<App80CommandReply, JsValue> {
        self.inner
            .command()
            .decode_bytes(&error::buffer(raw)?)
            .map(|inner| App80CommandReply { inner })
            .map_err(|source| error::fail(error::ErrorKind::Protocol, &source.to_string()))
    }
}
#[wasm_bindgen]
pub struct App80Service {
    inner: fujimod_usb_app80::operations::Service,
}
#[wasm_bindgen]
impl App80Service {
    #[wasm_bindgen(constructor)]
    pub fn new(definition: &ServiceDefinition) -> Result<Self, JsValue> {
        fujimod_usb_app80::operations::Service::new(definition.inner.clone())
            .map(|inner| Self { inner })
            .map_err(error)
    }
    pub fn prepare(
        &self,
        modes: &ServiceModes,
        command: &App80Command,
    ) -> Result<App80Invocation, JsValue> {
        self.inner
            .prepare(modes.inner, command.inner)
            .map(|inner| App80Invocation { inner })
            .map_err(invalid)
    }
    #[wasm_bindgen(js_name=prepareCode)]
    pub fn prepare_code(
        &self,
        code: &requests::App80CommandCode,
        modes: &ServiceModes,
        command: &App80Command,
    ) -> Result<App80Invocation, JsValue> {
        self.inner
            .prepare_code(
                fujimod_usb_app80_types::CommandCode(code.value()),
                modes.inner,
                command.inner,
            )
            .map(|inner| App80Invocation { inner })
            .map_err(invalid)
    }
    #[wasm_bindgen(js_name=prepareResearch)]
    pub fn prepare_research(
        &self,
        code: &requests::App80CommandCode,
        modes: &ServiceModes,
        #[wasm_bindgen(unchecked_param_type = "Uint8Array")] arguments: JsValue,
    ) -> Result<App80Invocation, JsValue> {
        self.inner
            .prepare_research(
                fujimod_usb_app80_types::CommandCode(code.value()),
                modes.inner,
                &error::buffer(arguments)?,
            )
            .map(|inner| App80Invocation { inner })
            .map_err(invalid)
    }
}
#[wasm_bindgen(typescript_custom_section)]
const TYPES: &str = r#"
export type CommandFailure =
  | Readonly<{ kind: CommandFailureKind.Unsupported | CommandFailureKind.Ambiguous; codec: ServiceCodec }>
  | Readonly<{ kind: CommandFailureKind.Unavailable; code: number }>
  | Readonly<{ kind: CommandFailureKind.Argument; argument: ArgumentKind }>
  | Readonly<{ kind: CommandFailureKind.OversizeArguments }>;
"#;
