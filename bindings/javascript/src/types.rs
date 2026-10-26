use super::*;
use error::{bytes, nz};
use usb::ff80::commands::{configuration, debug, diagnostic, info, memory};

macro_rules! enum_binding {
    ($name:ident,$native:ty,{$($variant:ident),+ $(,)?}) => {
        #[wasm_bindgen]
        #[derive(Clone,Copy,Debug,Eq,PartialEq)]
        pub enum $name { $($variant),+ }
        impl $name { pub(crate) fn parse(value:JsValue)->Result<Self,JsValue>{
            let n=error::integer(value,u32::MAX)?;
            $(if n==Self::$variant as u32{return Ok(Self::$variant);})+
            Err(fail(ErrorKind::InvalidArgument,"unknown enum choice"))
        } }
        impl From<$name> for $native { fn from(value:$name)->Self { match value { $($name::$variant => <$native>::$variant),+ } } }
    };
}
enum_binding!(InfoSelector,info::InfoSelector,{Vendor,Manufacturer,Product,InternalVersion,DeviceType,Serial,Framework});
enum_binding!(ConfigurationView,configuration::ConfigurationView,{Storage,Ram});
enum_binding!(KeyState,debug::KeyState,{On,Off});
enum_binding!(CameraMode,debug::CameraMode,{Record,Playback});
enum_binding!(DataReadMode,diagnostic::DataReadMode,{Pattern,Echo});
enum_binding!(DataWriteMode,diagnostic::DataWriteMode,{Preserve,FillOnes,Echo});
enum_binding!(DeviceFilter,usb::connection::DeviceFilter,{Fujifilm,Diagnostic});
enum_binding!(OffsetMode,usb::operations::data::OffsetMode,{Checked,Wrapping});

#[wasm_bindgen]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionState {
    Ready,
    InFlight,
    ReadingRange,
    CompletionUnknown,
    Closed,
    Abandoned,
}
#[wasm_bindgen]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandStatus {
    Success,
    DeviceError,
}
#[wasm_bindgen]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConsoleProgress {
    Complete,
    Pending,
    Unknown,
}
#[wasm_bindgen]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FirmwareDownloadState {
    Normal,
    Writing,
    Success,
    Failed,
    Unknown,
}

#[wasm_bindgen]
#[derive(Clone, Copy)]
pub struct FirmwareLimits {
    pub(crate) memory: memory::DebugMemoryLimits,
    pub(crate) configuration: std::num::NonZeroU32,
    pub(crate) console: std::num::NonZeroU32,
}
#[wasm_bindgen]
impl FirmwareLimits {
    #[wasm_bindgen(constructor)]
    pub fn new(
        #[wasm_bindgen(unchecked_param_type = "number")] memory_read: JsValue,
        #[wasm_bindgen(unchecked_param_type = "number")] memory_write: JsValue,
        #[wasm_bindgen(unchecked_param_type = "number")] configuration: JsValue,
        #[wasm_bindgen(unchecked_param_type = "number")] console: JsValue,
    ) -> Result<Self, JsValue> {
        let memory_read = error::integer(memory_read, u32::MAX)?;
        let memory_write = error::integer(memory_write, u32::MAX)?;
        let configuration = error::integer(configuration, u32::MAX)?;
        let console = error::integer(console, u32::MAX)?;

        Ok(Self {
            memory: memory::DebugMemoryLimits {
                read: nz(memory_read)?,
                write: nz(memory_write)?,
            },
            configuration: nz(configuration)?,
            console: nz(console)?,
        })
    }
    #[wasm_bindgen(getter,js_name=memoryRead)]
    pub fn memory_read(&self) -> u32 {
        self.memory.read.get()
    }
    #[wasm_bindgen(getter,js_name=memoryWrite)]
    pub fn memory_write(&self) -> u32 {
        self.memory.write.get()
    }
    #[wasm_bindgen(getter)]
    pub fn configuration(&self) -> u32 {
        self.configuration.get()
    }
    #[wasm_bindgen(getter)]
    pub fn console(&self) -> u32 {
        self.console.get()
    }
}
#[wasm_bindgen]
#[derive(Clone)]
pub struct TransferOptions {
    pub(crate) inner: usb::connection::TransferOptions,
    pub(crate) trace: Option<usb::trace::Trace>,
}
#[wasm_bindgen]
impl TransferOptions {
    #[wasm_bindgen(constructor)]
    pub fn new(
        #[wasm_bindgen(unchecked_param_type = "number")] timeout_ms: JsValue,
        #[wasm_bindgen(unchecked_param_type = "number")] max_reply_bytes: JsValue,
    ) -> Result<Self, JsValue> {
        let timeout_ms = error::integer(timeout_ms, u32::MAX)?;
        let max_reply_bytes = error::integer(max_reply_bytes, u32::MAX)?;

        nz(timeout_ms)?;
        if max_reply_bytes < 8 {
            return Err(fail(
                ErrorKind::InvalidArgument,
                "session open requires at least eight reply bytes",
            ));
        }
        Ok(Self {
            inner: usb::connection::TransferOptions {
                timeout: std::time::Duration::from_millis(timeout_ms.into()),
                max_reply_bytes: nz(max_reply_bytes)?,
            },
            trace: None,
        })
    }
    #[wasm_bindgen(js_name=setTrace)]
    pub fn set_trace(&mut self, trace: &tracing::Trace) {
        self.trace = Some(trace.inner.clone());
    }
    #[wasm_bindgen(js_name=clearTrace)]
    pub fn clear_trace(&mut self) {
        self.trace = None;
    }
    #[wasm_bindgen(getter,js_name=timeoutMs)]
    pub fn timeout_ms(&self) -> u32 {
        self.inner.timeout.as_millis() as u32
    }
    #[wasm_bindgen(getter,js_name=maxReplyBytes)]
    pub fn max_reply_bytes(&self) -> u32 {
        self.inner.max_reply_bytes.get()
    }
}
impl TransferOptions {
    /// The trace these options attach to the sessions they open.
    pub fn trace(&self) -> Option<usb::trace::Trace> {
        self.trace.clone()
    }
}
#[wasm_bindgen]
#[derive(Clone, Copy)]
pub struct InterfaceSelection {
    pub(crate) inner: usb::connection::InterfaceSelection,
}
#[wasm_bindgen]
impl InterfaceSelection {
    #[wasm_bindgen(constructor)]
    pub fn new(
        #[wasm_bindgen(unchecked_param_type = "number")] configuration: JsValue,
        #[wasm_bindgen(unchecked_param_type = "number")] interface: JsValue,
        #[wasm_bindgen(unchecked_param_type = "number")] alternate: JsValue,
    ) -> Result<Self, JsValue> {
        let configuration = error::integer(configuration, u8::MAX.into())? as u8;
        let interface = error::integer(interface, u8::MAX.into())? as u8;
        let alternate = error::integer(alternate, u8::MAX.into())? as u8;
        Ok({
            Self {
                inner: usb::connection::InterfaceSelection {
                    configuration,
                    interface,
                    alternate,
                },
            }
        })
    }
    #[wasm_bindgen(getter)]
    pub fn configuration(&self) -> u8 {
        self.inner.configuration
    }
    #[wasm_bindgen(getter)]
    pub fn interface(&self) -> u8 {
        self.inner.interface
    }
    #[wasm_bindgen(getter)]
    pub fn alternate(&self) -> u8 {
        self.inner.alternate
    }
}
#[wasm_bindgen]
pub struct InfoReply {
    pub(crate) inner: info::DeviceInfo,
}
#[wasm_bindgen]
impl InfoReply {
    #[wasm_bindgen(getter)]
    pub fn text(&self) -> String {
        self.inner.text.clone()
    }
    #[wasm_bindgen(getter, unchecked_return_type = "Uint8Array")]
    pub fn raw(&self) -> JsValue {
        bytes(&self.inner.raw)
    }
}
#[wasm_bindgen]
pub struct ConfigurationGeometry {
    pub(crate) inner: configuration::ConfigurationGeometry,
}
#[wasm_bindgen]
impl ConfigurationGeometry {
    #[wasm_bindgen(getter)]
    pub fn capacity(&self) -> u32 {
        self.inner.capacity
    }
    #[wasm_bindgen(getter,js_name=pageBytes)]
    pub fn page_bytes(&self) -> u32 {
        self.inner.page_bytes.get()
    }
    #[wasm_bindgen(getter, unchecked_return_type = "Uint8Array")]
    pub fn raw(&self) -> JsValue {
        bytes(&self.inner.raw)
    }
}
#[wasm_bindgen]
pub struct DiagnosticTag {
    pub(crate) inner: diagnostic::DiagnosticTag,
}
#[wasm_bindgen]
impl DiagnosticTag {
    #[wasm_bindgen(js_name=bulkSource)]
    pub fn bulk_source() -> Self {
        Self {
            inner: diagnostic::DiagnosticTag::BulkSource,
        }
    }
    pub fn research(
        #[wasm_bindgen(unchecked_param_type = "number")] value: JsValue,
    ) -> Result<Self, JsValue> {
        let value = error::integer(value, u32::MAX)?;
        Ok({
            Self {
                inner: diagnostic::DiagnosticTag::Research(value),
            }
        })
    }
    #[wasm_bindgen(getter)]
    pub fn value(&self) -> u32 {
        self.inner.value()
    }
}
#[wasm_bindgen]
pub struct DiagnosticReply {
    pub(crate) inner: diagnostic::DiagnosticReply,
}
#[wasm_bindgen]
impl DiagnosticReply {
    #[wasm_bindgen(getter)]
    pub fn status(&self) -> CommandStatus {
        command_status(self.inner.status)
    }
    #[wasm_bindgen(getter)]
    pub fn primary(&self) -> u32 {
        status_values(self.inner.status).0
    }
    #[wasm_bindgen(getter)]
    pub fn secondary(&self) -> u32 {
        status_values(self.inner.status).1
    }
    #[wasm_bindgen(getter)]
    pub fn fields(&self) -> js_sys::Uint32Array {
        js_sys::Uint32Array::from(self.inner.fields.as_slice())
    }
    #[wasm_bindgen(getter, unchecked_return_type = "Uint8Array")]
    pub fn raw(&self) -> JsValue {
        bytes(self.inner.raw)
    }
}
pub(crate) fn command_status(status: diagnostic::CommandStatus) -> CommandStatus {
    match status {
        diagnostic::CommandStatus::Success => CommandStatus::Success,
        _ => CommandStatus::DeviceError,
    }
}
pub(crate) fn status_values(status: diagnostic::CommandStatus) -> (u32, u32) {
    match status {
        diagnostic::CommandStatus::Success => (0, 0),
        diagnostic::CommandStatus::DeviceError { primary, secondary } => (primary, secondary),
    }
}
#[wasm_bindgen]
pub struct ConsoleReply {
    pub(crate) inner: debug::ConsoleReply,
}
#[wasm_bindgen]
impl ConsoleReply {
    #[wasm_bindgen(getter)]
    pub fn progress(&self) -> ConsoleProgress {
        match self.inner.progress {
            debug::ConsoleProgress::Complete => ConsoleProgress::Complete,
            debug::ConsoleProgress::Pending => ConsoleProgress::Pending,
            _ => ConsoleProgress::Unknown,
        }
    }
    #[wasm_bindgen(getter,js_name=unknownProgress)]
    pub fn unknown_progress(&self) -> Option<u32> {
        match self.inner.progress {
            debug::ConsoleProgress::Unknown(n) => Some(n),
            _ => None,
        }
    }
    #[wasm_bindgen(getter)]
    pub fn status(&self) -> u32 {
        self.inner.status
    }
    #[wasm_bindgen(getter, unchecked_return_type = "Uint8Array")]
    pub fn output(&self) -> JsValue {
        bytes(&self.inner.output)
    }
    #[wasm_bindgen(getter, unchecked_return_type = "Uint8Array")]
    pub fn raw(&self) -> JsValue {
        bytes(&self.inner.raw)
    }
}
#[wasm_bindgen]
pub struct LogReply {
    pub(crate) inner: debug::LogReply,
}
#[wasm_bindgen]
impl LogReply {
    #[wasm_bindgen(getter)]
    pub fn status(&self) -> u32 {
        self.inner.status
    }
    #[wasm_bindgen(getter, unchecked_return_type = "Uint8Array")]
    pub fn data(&self) -> JsValue {
        bytes(&self.inner.data)
    }
    #[wasm_bindgen(getter, unchecked_return_type = "Uint8Array")]
    pub fn raw(&self) -> JsValue {
        bytes(&self.inner.raw)
    }
}
#[wasm_bindgen]
pub struct CapabilitiesReply {
    pub(crate) inner: usb::ff80::commands::capabilities::CapabilitiesReply,
}
#[wasm_bindgen]
impl CapabilitiesReply {
    #[wasm_bindgen(getter,js_name=byteLength)]
    pub fn byte_length(&self) -> u32 {
        self.inner.byte_length
    }
    #[wasm_bindgen(getter, unchecked_return_type = "Uint8Array")]
    pub fn raw(&self) -> JsValue {
        bytes(self.inner.raw)
    }
}
#[wasm_bindgen]
pub struct FirmwareDownloadReply {
    pub(crate) inner: info::FirmwareDownloadReply,
}
#[wasm_bindgen]
impl FirmwareDownloadReply {
    #[wasm_bindgen(getter)]
    pub fn state(&self) -> FirmwareDownloadState {
        match self.inner.state {
            info::FirmwareDownloadState::Normal => FirmwareDownloadState::Normal,
            info::FirmwareDownloadState::Writing => FirmwareDownloadState::Writing,
            info::FirmwareDownloadState::Success => FirmwareDownloadState::Success,
            info::FirmwareDownloadState::Failed { .. } => FirmwareDownloadState::Failed,
            info::FirmwareDownloadState::Unknown(_) => FirmwareDownloadState::Unknown,
        }
    }
    #[wasm_bindgen(getter,js_name=failureCode)]
    pub fn failure_code(&self) -> Option<u8> {
        match self.inner.state {
            info::FirmwareDownloadState::Failed { code } => Some(code),
            _ => None,
        }
    }
    #[wasm_bindgen(getter)]
    pub fn text(&self) -> String {
        self.inner.text.clone()
    }
    #[wasm_bindgen(getter, unchecked_return_type = "Uint8Array")]
    pub fn raw(&self) -> JsValue {
        bytes(&self.inner.raw)
    }
}
