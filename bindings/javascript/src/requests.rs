use super::*;
use error::{bytes, nz};
use usb::ff80::commands::research;

#[wasm_bindgen]
pub struct ResearchCommandCode {
    inner: research::CommandCode,
}
#[wasm_bindgen]
impl ResearchCommandCode {
    #[wasm_bindgen(constructor)]
    pub fn new(
        #[wasm_bindgen(unchecked_param_type = "number")] value: JsValue,
    ) -> Result<Self, JsValue> {
        let value = error::integer(value, u32::MAX)?;
        Ok({
            Self {
                inner: research::CommandCode(value),
            }
        })
    }
    #[wasm_bindgen(getter)]
    pub fn value(&self) -> u32 {
        self.inner.0
    }
}
#[wasm_bindgen]
#[derive(Clone)]
pub struct ResearchRequest {
    pub(crate) inner: research::Request,
}
#[wasm_bindgen]
impl ResearchRequest {
    #[wasm_bindgen(constructor)]
    pub fn new(
        code: &ResearchCommandCode,
        #[wasm_bindgen(unchecked_param_type = "Uint8Array")] body: JsValue,
        #[wasm_bindgen(unchecked_param_type = "number")] reply_capacity: JsValue,
    ) -> Result<Self, JsValue> {
        let body = error::buffer(body)?;

        let reply_capacity = error::integer(reply_capacity, u32::MAX)?;

        research::Request::new(code.inner, body, nz(reply_capacity)?)
            .map(|inner| Self { inner })
            .map_err(error)
    }
    #[wasm_bindgen(getter)]
    pub fn code(&self) -> ResearchCommandCode {
        ResearchCommandCode {
            inner: self.inner.code(),
        }
    }
    #[wasm_bindgen(getter, unchecked_return_type = "Uint8Array")]
    pub fn body(&self) -> JsValue {
        bytes(self.inner.body())
    }
    #[wasm_bindgen(getter,js_name=replyCapacity)]
    pub fn reply_capacity(&self) -> u32 {
        self.inner.reply_capacity().get()
    }
}
#[wasm_bindgen]
pub struct ResearchReply {
    pub(crate) inner: research::Reply,
}
#[wasm_bindgen]
impl ResearchReply {
    #[wasm_bindgen(getter)]
    pub fn code(&self) -> ResearchCommandCode {
        ResearchCommandCode {
            inner: self.inner.code,
        }
    }
    #[wasm_bindgen(getter)]
    pub fn status(&self) -> CommandStatus {
        types::command_status(self.inner.status)
    }
    #[wasm_bindgen(getter)]
    pub fn primary(&self) -> u32 {
        types::status_values(self.inner.status).0
    }
    #[wasm_bindgen(getter)]
    pub fn secondary(&self) -> u32 {
        types::status_values(self.inner.status).1
    }
    #[wasm_bindgen(getter, unchecked_return_type = "Uint8Array")]
    pub fn raw(&self) -> JsValue {
        bytes(self.inner.raw())
    }
    #[wasm_bindgen(getter, unchecked_return_type = "Uint8Array")]
    pub fn body(&self) -> JsValue {
        bytes(self.inner.body())
    }
}
#[wasm_bindgen]
pub struct App80CommandCode {
    inner: fujimod_usb_app80_types::CommandCode,
}
#[wasm_bindgen]
impl App80CommandCode {
    #[wasm_bindgen(constructor)]
    pub fn new(
        #[wasm_bindgen(unchecked_param_type = "number")] value: JsValue,
    ) -> Result<Self, JsValue> {
        let value = error::integer(value, u8::MAX.into())? as u8;
        Ok({
            Self {
                inner: fujimod_usb_app80_types::CommandCode(value),
            }
        })
    }
    #[wasm_bindgen(getter)]
    pub fn value(&self) -> u8 {
        self.inner.0
    }
}
#[wasm_bindgen]
#[derive(Clone)]
pub struct App80Request {
    pub(crate) inner: fujimod_usb_app80_types::Request,
}
#[wasm_bindgen]
impl App80Request {
    #[wasm_bindgen(constructor)]
    pub fn new(
        code: &App80CommandCode,
        #[wasm_bindgen(unchecked_param_type = "Uint8Array")] arguments: JsValue,
    ) -> Result<Self, JsValue> {
        let arguments = error::buffer(arguments)?;

        fujimod_usb_app80_types::Request::new(code.inner, &arguments)
            .map(|inner| Self { inner })
            .map_err(|source| error::fail(error::ErrorKind::InvalidArgument, &source.to_string()))
    }
    #[wasm_bindgen(getter, unchecked_return_type = "Uint8Array")]
    pub fn raw(&self) -> JsValue {
        bytes(self.inner.encode())
    }
}
#[wasm_bindgen]
#[derive(Clone, Copy)]
pub struct Mailbox {
    pub(crate) inner: fujimod_usb_app80::operations::Mailbox,
}
#[wasm_bindgen]
impl Mailbox {
    #[wasm_bindgen(constructor)]
    pub fn new(
        #[wasm_bindgen(unchecked_param_type = "number")] page: JsValue,
        #[wasm_bindgen(unchecked_param_type = "number")] offset: JsValue,
        #[wasm_bindgen(unchecked_param_type = "number")] configuration_limit: JsValue,
    ) -> Result<Self, JsValue> {
        let page = error::integer(page, u32::MAX)?;
        let offset = error::integer(offset, u32::MAX)?;
        let configuration_limit = error::integer(configuration_limit, u32::MAX)?;

        fujimod_usb_app80::operations::Mailbox::new(page, offset, nz(configuration_limit)?)
            .map(|inner| Self { inner })
            .map_err(error)
    }
}
#[wasm_bindgen]
#[derive(Clone, Copy)]
pub struct DataWindow {
    pub(crate) inner: usb::operations::data::DataWindow,
}
#[wasm_bindgen]
impl DataWindow {
    #[wasm_bindgen(constructor)]
    pub fn new(
        #[wasm_bindgen(unchecked_param_type = "number")] base: JsValue,
        #[wasm_bindgen(unchecked_param_type = "number")] aperture_address: JsValue,
        #[wasm_bindgen(unchecked_param_type = "number")] aperture_length: JsValue,
        #[wasm_bindgen(unchecked_param_type = "number")] capacity: JsValue,
        #[wasm_bindgen(unchecked_param_type = "OffsetMode")] mode: JsValue,
    ) -> Result<Self, JsValue> {
        let mode = OffsetMode::parse(mode)?;

        let base = error::integer(base, u32::MAX)?;
        let aperture_address = error::integer(aperture_address, u32::MAX)?;
        let aperture_length = error::integer(aperture_length, u32::MAX)?;
        let capacity = error::integer(capacity, u32::MAX)?;

        use usb::ff80::commands::memory::{Address, MemoryRange};
        usb::operations::data::DataWindow::new(
            Address(base),
            MemoryRange::new(Address(aperture_address), nz(aperture_length)?).map_err(error)?,
            nz(capacity)?,
            mode.into(),
        )
        .map(|inner| Self { inner })
        .map_err(error)
    }
    #[wasm_bindgen(getter)]
    pub fn capacity(&self) -> u32 {
        self.inner.capacity().get()
    }
}
#[wasm_bindgen]
pub struct UploadReply {
    pub(crate) inner: usb::operations::data::UploadReply,
}
#[wasm_bindgen]
impl UploadReply {
    #[wasm_bindgen(getter)]
    pub fn setup(&self) -> DiagnosticReply {
        DiagnosticReply {
            inner: self.inner.setup.clone(),
        }
    }
    #[wasm_bindgen(getter, unchecked_return_type = "Uint8Array")]
    pub fn acknowledgment(&self) -> JsValue {
        bytes(&self.inner.acknowledgment)
    }
    #[wasm_bindgen(getter,js_name=bytesWritten)]
    pub fn bytes_written(&self) -> u32 {
        self.inner.bytes_written
    }
}
