use super::*;
use error::{bytes, nz};
use futures_channel::oneshot;
use requests::{App80Request, Mailbox, ResearchReply, ResearchRequest, UploadReply};
use std::{
    cell::RefCell,
    num::NonZeroU32,
    rc::{Rc, Weak},
};
use usb::ff80::{
    self,
    commands::{
        configuration::{ConfigurationLocation, ReadConfiguration, WriteConfiguration},
        debug, diagnostic,
        memory::{Address, MemoryRange, ReadMemory, WriteMemory},
    },
};

pub(crate) enum Cancellation {
    Interrupted,
    Abandoned,
}
impl Cancellation {
    pub(crate) fn error(self) -> JsValue {
        match self {
            Self::Interrupted => fail(
                ErrorKind::Canceled,
                "host operation was canceled; inspect session state before further I/O",
            ),
            Self::Abandoned => fail(
                ErrorKind::Abandoned,
                "operation was abandoned; camera execution may continue",
            ),
        }
    }
}
pub(crate) enum Slot {
    Open(ff80::Session),
    InFlight(oneshot::Sender<Cancellation>),
    Canceling,
    Range(Weak<RefCell<range::ReaderState>>),
    Closed,
    CompletionUnknown,
    Abandoned,
}
impl Slot {
    fn unavailable(&self) -> JsValue {
        match self {
            Self::InFlight(_) | Self::Canceling | Self::Range(_) => {
                fail(ErrorKind::Busy, "session is owned by an active operation")
            }
            Self::Open(_) | Self::CompletionUnknown => {
                fail(ErrorKind::CompletionUnknown, "camera completion is unknown")
            }
            Self::Closed => fail(ErrorKind::Closed, "session is closed"),
            Self::Abandoned => fail(ErrorKind::Abandoned, "session was abandoned"),
        }
    }
}
pub(crate) fn abandon(owner: &Rc<RefCell<Slot>>) {
    let mut slot = owner.borrow_mut();
    if matches!(*slot, Slot::Closed | Slot::Abandoned) {
        return;
    }
    let old = std::mem::replace(&mut *slot, Slot::Abandoned);
    drop(slot);
    match old {
        Slot::InFlight(cancel) => {
            let _ = cancel.send(Cancellation::Abandoned);
        }
        Slot::Range(reader) => {
            if let Some(reader) = reader.upgrade() {
                reader.borrow_mut().abandon();
            }
        }
        _ => (),
    }
}
pub(crate) fn take_native(
    owner: &Rc<RefCell<Slot>>,
    replacement: Slot,
    allow_unknown: bool,
) -> Result<ff80::Session, JsValue> {
    let mut slot = owner.borrow_mut();
    match &*slot {
        Slot::Open(native) if allow_unknown || native.state() == ff80::SessionState::Ready => (),
        _ => return Err(slot.unavailable()),
    }
    let Slot::Open(native) = std::mem::replace(&mut *slot, replacement) else {
        unreachable!("checked under a single borrow")
    };
    Ok(native)
}
enum LeasedSession {
    Open(ff80::Session),
    /// The close future owns the native session. If dropped, its completion is unknown.
    Closing,
    Closed,
}
struct Lease {
    resource: LeasedSession,
    owner: Rc<RefCell<Slot>>,
}
impl Drop for Lease {
    fn drop(&mut self) {
        let mut slot = self.owner.borrow_mut();
        let resource = std::mem::replace(&mut self.resource, LeasedSession::Closing);
        if matches!(*slot, Slot::InFlight(_) | Slot::Canceling) {
            *slot = match resource {
                LeasedSession::Open(native) => Slot::Open(native),
                LeasedSession::Closed => Slot::Closed,
                LeasedSession::Closing => Slot::CompletionUnknown,
            };
        }
    }
}
#[wasm_bindgen]
pub struct Session {
    pub(crate) owner: Rc<RefCell<Slot>>,
    pub(crate) limits: FirmwareLimits,
    info: ff80::SessionInfo,
    max_reply_bytes: u32,
}
impl Session {
    pub fn wrap(native: ff80::Session, limits: FirmwareLimits) -> Self {
        let info = native.info();
        let max_reply_bytes = native.max_reply_bytes().get();
        Self {
            owner: Rc::new(RefCell::new(Slot::Open(native))),
            limits,
            info,
            max_reply_bytes,
        }
    }
    pub(crate) fn call(&self, operation: Operation) -> Result<js_sys::Promise, JsValue> {
        let (cancel, mut canceled) = oneshot::channel();
        let native = take_native(
            &self.owner,
            Slot::InFlight(cancel),
            matches!(operation, Operation::Close),
        )?;
        let mut lease = Lease {
            resource: LeasedSession::Open(native),
            owner: self.owner.clone(),
        };
        Ok(wasm_bindgen_futures::future_to_promise(async move {
            // Dropping the command future invokes the core's conservative
            // cancellation state. Abandoning sends no firmware cleanup.
            futures_lite::future::or(
                async {
                    Err((&mut canceled)
                        .await
                        .unwrap_or(Cancellation::Abandoned)
                        .error())
                },
                operation.run(&mut lease),
            )
            .await
        }))
    }
}
impl Session {
    pub(crate) fn open_named(
        device: &connection::Device,
        interface: &InterfaceSelection,
        limits: &FirmwareLimits,
        options: &TransferOptions,
        tool: Option<String>,
    ) -> js_sys::Promise {
        let info = device.inner.clone();
        let proxy = device.proxy.clone();
        let interface = interface.inner;
        let trace = options.trace.clone();
        let options = options.inner;
        let limits = *limits;

        wasm_bindgen_futures::future_to_promise(async move {
            let mut claim = proxy
                .as_ref()
                .map(|proxy| proxy.acquire(tool.clone()))
                .transpose()
                .map_err(|e| error(e.into()))?;
            let connection = usb::connection::Connection::open(&info, interface, options).await;
            let mut connection = match connection {
                Ok(connection) => connection,
                Err(failure) => {
                    if let Some(claim) = &mut claim {
                        claim.release();
                    }
                    return Err(error(failure));
                }
            };
            if let Some(tool) = tool {
                connection.set_tool(tool);
            }
            if let Some(claim) = claim {
                connection.observe_remote(claim);
            }
            Ok(Self::wrap(
                connection.begin_session(trace).await.map_err(error)?,
                limits,
            )
            .into())
        })
    }
}
#[wasm_bindgen]
impl Session {
    #[wasm_bindgen(unchecked_return_type = "Promise<Session>")]
    pub fn open(
        device: &connection::Device,
        interface: &InterfaceSelection,
        limits: &FirmwareLimits,
        options: &TransferOptions,
    ) -> js_sys::Promise {
        Self::open_named(device, interface, limits, options, None)
    }
    #[wasm_bindgen(getter)]
    pub fn state(&self) -> SessionState {
        match &*self.owner.borrow() {
            Slot::Open(native) => match native.state() {
                ff80::SessionState::Ready => SessionState::Ready,
                ff80::SessionState::CompletionUnknown => SessionState::CompletionUnknown,
            },
            Slot::InFlight(_) | Slot::Canceling => SessionState::InFlight,
            Slot::Range(_) => SessionState::ReadingRange,
            Slot::Closed => SessionState::Closed,
            Slot::CompletionUnknown => SessionState::CompletionUnknown,
            Slot::Abandoned => SessionState::Abandoned,
        }
    }
    #[wasm_bindgen(getter,js_name=packetBytes)]
    pub fn packet_bytes(&self) -> u32 {
        self.info.advertised_packet_bytes.get()
    }
    #[wasm_bindgen(getter,js_name=maxReplyBytes)]
    pub fn max_reply_bytes(&self) -> u32 {
        self.max_reply_bytes
    }
    #[wasm_bindgen(getter,js_name=sessionId)]
    pub fn session_id(&self) -> u16 {
        self.info.id
    }
    #[wasm_bindgen(getter,js_name=rawOpenReply, unchecked_return_type="Uint8Array")]
    pub fn raw_open_reply(&self) -> JsValue {
        bytes(self.info.raw_open_reply)
    }
    pub fn abandon(&self) {
        abandon(&self.owner)
    }
    /// Drop pending host work. Await its Promise before further session I/O.
    #[wasm_bindgen(js_name=cancelPending)]
    pub fn cancel_pending(&self) {
        let mut slot = self.owner.borrow_mut();
        if matches!(*slot, Slot::InFlight(_)) {
            let Slot::InFlight(cancel) = std::mem::replace(&mut *slot, Slot::Canceling) else {
                unreachable!()
            };
            let _ = cancel.send(Cancellation::Interrupted);
        }
    }
    #[wasm_bindgen(unchecked_return_type = "Promise<Uint8Array>")]
    pub fn ping(&self) -> Result<js_sys::Promise, JsValue> {
        self.call(Operation::Ping)
    }
    #[wasm_bindgen(unchecked_return_type = "Promise<Uint8Array>")]
    pub fn close(&self) -> Result<js_sys::Promise, JsValue> {
        self.call(Operation::Close)
    }
    #[wasm_bindgen(js_name=deviceInfo,unchecked_return_type="Promise<InfoReply>")]
    pub fn device_info(
        &self,
        #[wasm_bindgen(unchecked_param_type = "InfoSelector")] selector: JsValue,
    ) -> Result<js_sys::Promise, JsValue> {
        let selector = InfoSelector::parse(selector)?;

        self.call(Operation::Info(selector))
    }
    #[wasm_bindgen(js_name=readMemory,unchecked_return_type="Promise<Uint8Array>")]
    pub fn read_memory(
        &self,
        #[wasm_bindgen(unchecked_param_type = "number")] address: JsValue,
        #[wasm_bindgen(unchecked_param_type = "number")] length: JsValue,
    ) -> Result<js_sys::Promise, JsValue> {
        let address = error::integer(address, u32::MAX)?;
        let length = error::integer(length, u32::MAX)?;

        self.call(Operation::ReadMemory(
            ReadMemory::new(
                MemoryRange::new(Address(address), nz(length)?).map_err(error)?,
                self.limits.memory,
            )
            .map_err(error)?,
        ))
    }
    #[wasm_bindgen(js_name=writeMemory,unchecked_return_type="Promise<void>")]
    pub fn write_memory(
        &self,
        #[wasm_bindgen(unchecked_param_type = "number")] address: JsValue,
        #[wasm_bindgen(unchecked_param_type = "Uint8Array")] data: JsValue,
    ) -> Result<js_sys::Promise, JsValue> {
        let data = error::buffer(data)?;

        let address = error::integer(address, u32::MAX)?;

        self.call(Operation::WriteMemory(
            WriteMemory::new(Address(address), data, self.limits.memory).map_err(error)?,
        ))
    }
    #[wasm_bindgen(js_name=configurationGeometry,unchecked_return_type="Promise<ConfigurationGeometry>")]
    pub fn configuration_geometry(
        &self,
        #[wasm_bindgen(unchecked_param_type = "ConfigurationView")] view: JsValue,
    ) -> Result<js_sys::Promise, JsValue> {
        let view = ConfigurationView::parse(view)?;

        self.call(Operation::Geometry(view))
    }
    #[wasm_bindgen(js_name=readConfiguration,unchecked_return_type="Promise<Uint8Array>")]
    pub fn read_configuration(
        &self,
        #[wasm_bindgen(unchecked_param_type = "ConfigurationView")] view: JsValue,
        #[wasm_bindgen(unchecked_param_type = "number")] page: JsValue,
        #[wasm_bindgen(unchecked_param_type = "number")] offset: JsValue,
        #[wasm_bindgen(unchecked_param_type = "number")] length: JsValue,
    ) -> Result<js_sys::Promise, JsValue> {
        let view = ConfigurationView::parse(view)?;

        let page = error::integer(page, u32::MAX)?;
        let offset = error::integer(offset, u32::MAX)?;
        let length = error::integer(length, u32::MAX)?;

        self.call(Operation::ReadConfiguration(
            ReadConfiguration::new(
                ConfigurationLocation {
                    view: view.into(),
                    page,
                    offset,
                },
                nz(length)?,
                self.limits.configuration,
            )
            .map_err(error)?,
        ))
    }
    #[wasm_bindgen(js_name=writeConfiguration,unchecked_return_type="Promise<void>")]
    pub fn write_configuration(
        &self,
        #[wasm_bindgen(unchecked_param_type = "ConfigurationView")] view: JsValue,
        #[wasm_bindgen(unchecked_param_type = "number")] page: JsValue,
        #[wasm_bindgen(unchecked_param_type = "number")] offset: JsValue,
        #[wasm_bindgen(unchecked_param_type = "Uint8Array")] data: JsValue,
    ) -> Result<js_sys::Promise, JsValue> {
        let data = error::buffer(data)?;

        let view = ConfigurationView::parse(view)?;

        let page = error::integer(page, u32::MAX)?;
        let offset = error::integer(offset, u32::MAX)?;

        self.call(Operation::WriteConfiguration(
            WriteConfiguration::new(
                ConfigurationLocation {
                    view: view.into(),
                    page,
                    offset,
                },
                data,
                self.limits.configuration,
            )
            .map_err(error)?,
        ))
    }
    #[wasm_bindgen(js_name=diagnosticQuery,unchecked_return_type="Promise<DiagnosticReply>")]
    pub fn diagnostic_query(&self, tag: &DiagnosticTag) -> Result<js_sys::Promise, JsValue> {
        self.call(Operation::Diagnostic(tag.inner))
    }
    #[wasm_bindgen(js_name=prepareDataRead,unchecked_return_type="Promise<DiagnosticReply>")]
    pub fn prepare_data_read(
        &self,
        #[wasm_bindgen(unchecked_param_type = "DataReadMode")] mode: JsValue,
    ) -> Result<js_sys::Promise, JsValue> {
        let mode = DataReadMode::parse(mode)?;

        self.call(Operation::PrepareRead(mode))
    }
    #[wasm_bindgen(js_name=prepareDataWrite,unchecked_return_type="Promise<DiagnosticReply>")]
    pub fn prepare_data_write(
        &self,
        #[wasm_bindgen(unchecked_param_type = "number")] length: JsValue,
        #[wasm_bindgen(unchecked_param_type = "DataWriteMode")] mode: JsValue,
    ) -> Result<js_sys::Promise, JsValue> {
        let mode = DataWriteMode::parse(mode)?;

        let length = error::integer(length, u32::MAX)?;

        self.call(Operation::PrepareWrite(nz(length)?, mode))
    }
    #[wasm_bindgen(js_name=readData,unchecked_return_type="Promise<Uint8Array>")]
    pub fn read_data(
        &self,
        #[wasm_bindgen(unchecked_param_type = "number")] offset: JsValue,
        #[wasm_bindgen(unchecked_param_type = "number")] length: JsValue,
    ) -> Result<js_sys::Promise, JsValue> {
        let offset = error::integer(offset, u32::MAX)?;
        let length = error::integer(length, u32::MAX)?;

        self.call(Operation::ReadData(offset, nz(length)?))
    }
    #[wasm_bindgen(js_name=writeData,unchecked_return_type="Promise<void>")]
    pub fn write_data(
        &self,
        #[wasm_bindgen(unchecked_param_type = "Uint8Array")] data: JsValue,
    ) -> Result<js_sys::Promise, JsValue> {
        let data = error::buffer(data)?;

        if data.is_empty() {
            return Err(error(usb::Error::InvalidLength));
        }
        self.call(Operation::WriteData(data))
    }
    #[wasm_bindgen(js_name=emptyResponse,unchecked_return_type="Promise<Uint8Array>")]
    pub fn empty_response(&self) -> Result<js_sys::Promise, JsValue> {
        self.call(Operation::EmptyResponse)
    }
    #[wasm_bindgen(js_name=prepareCapabilities,unchecked_return_type="Promise<CapabilitiesReply>")]
    pub fn prepare_capabilities(&self) -> Result<js_sys::Promise, JsValue> {
        self.call(Operation::Capabilities)
    }
    #[wasm_bindgen(js_name=firmwareDownloadStatus,unchecked_return_type="Promise<FirmwareDownloadReply>")]
    pub fn firmware_download_status(&self) -> Result<js_sys::Promise, JsValue> {
        self.call(Operation::DownloadStatus)
    }
    #[wasm_bindgen(js_name=clearFirmwareDownloadStatus,unchecked_return_type="Promise<FirmwareDownloadReply>")]
    pub fn clear_firmware_download_status(&self) -> Result<js_sys::Promise, JsValue> {
        self.call(Operation::ClearDownloadStatus)
    }
    #[wasm_bindgen(js_name=sendKey,unchecked_return_type="Promise<Uint8Array>")]
    pub fn send_key(
        &self,
        name: String,
        #[wasm_bindgen(unchecked_param_type = "KeyState")] state: JsValue,
    ) -> Result<js_sys::Promise, JsValue> {
        let state = KeyState::parse(state)?;

        self.call(Operation::Event(
            debug::DebugEvent::Key {
                name: debug::EventName::new(name).map_err(error)?,
                state: state.into(),
            },
            self.limits.console,
        ))
    }
    #[wasm_bindgen(js_name=requestMode,unchecked_return_type="Promise<Uint8Array>")]
    pub fn request_mode(
        &self,
        #[wasm_bindgen(unchecked_param_type = "CameraMode")] mode: JsValue,
    ) -> Result<js_sys::Promise, JsValue> {
        let mode = CameraMode::parse(mode)?;

        self.call(Operation::Event(
            debug::DebugEvent::Mode(mode.into()),
            self.limits.console,
        ))
    }
    #[wasm_bindgen(js_name=commandEvent,unchecked_return_type="Promise<Uint8Array>")]
    pub fn command_event(&self, argument: String) -> Result<js_sys::Promise, JsValue> {
        self.call(Operation::Event(
            debug::DebugEvent::Command(argument),
            self.limits.console,
        ))
    }
    #[wasm_bindgen(js_name=submitConsole,unchecked_return_type="Promise<Uint8Array>")]
    pub fn submit_console(
        &self,
        #[wasm_bindgen(unchecked_param_type = "Uint8Array")] data: JsValue,
    ) -> Result<js_sys::Promise, JsValue> {
        let data = error::buffer(data)?;

        self.call(Operation::Console(
            debug::ConsoleCommand::new(data, self.limits.console).map_err(error)?,
        ))
    }
    #[wasm_bindgen(js_name=pollConsole,unchecked_return_type="Promise<ConsoleReply>")]
    pub fn poll_console(
        &self,
        #[wasm_bindgen(unchecked_param_type = "number")] capacity: JsValue,
    ) -> Result<js_sys::Promise, JsValue> {
        let capacity = error::integer(capacity, u32::MAX)?;

        self.call(Operation::PollConsole(nz(capacity)?))
    }
    #[wasm_bindgen(js_name=readLog,unchecked_return_type="Promise<LogReply>")]
    pub fn read_log(
        &self,
        #[wasm_bindgen(unchecked_param_type = "number")] capacity: JsValue,
    ) -> Result<js_sys::Promise, JsValue> {
        let capacity = error::integer(capacity, u32::MAX)?;

        self.call(Operation::Log(nz(capacity)?))
    }
    #[wasm_bindgen(js_name=researchCommand,unchecked_return_type="Promise<ResearchReply>")]
    pub fn research_command(&self, request: &ResearchRequest) -> Result<js_sys::Promise, JsValue> {
        self.call(Operation::Research(request.inner.clone()))
    }
}
pub(crate) enum Operation {
    Workflow(crate::workflows::Workflow),
    Bulk(crate::workflows::BulkRead),
    StateDump(crate::workflows::StateDump),
    FlashBackup(Box<crate::workflows::FlashBackup>),
    UploadOwned(
        fujimod_workflows::install::OwnedUpload,
        fujimod_workflows::install::UploadDestination,
        usb::ff80::commands::memory::DebugMemoryLimits,
    ),
    VerifyOwned(
        fujimod_workflows::install::OwnedUpload,
        usb::operations::data::DataWindow,
    ),
    Sequence(
        Vec<usb::ff80::sequence::SequenceCommand>,
        usb::ff80::commands::memory::DebugMemoryLimits,
    ),
    Ping,
    Close,
    Info(InfoSelector),
    ReadMemory(ReadMemory),
    WriteMemory(WriteMemory),
    Geometry(ConfigurationView),
    ReadConfiguration(ReadConfiguration),
    WriteConfiguration(WriteConfiguration),
    Diagnostic(diagnostic::DiagnosticTag),
    PrepareRead(DataReadMode),
    PrepareWrite(NonZeroU32, DataWriteMode),
    ReadData(u32, NonZeroU32),
    WriteData(Vec<u8>),
    EmptyResponse,
    Capabilities,
    DownloadStatus,
    ClearDownloadStatus,
    Event(debug::DebugEvent, NonZeroU32),
    Console(debug::ConsoleCommand),
    PollConsole(NonZeroU32),
    Log(NonZeroU32),
    Research(usb::ff80::commands::research::Request),
    Upload(Vec<u8>, NonZeroU32, DataWriteMode),
    App80(
        fujimod_usb_app80::operations::Mailbox,
        fujimod_usb_app80_types::Request,
    ),
    Service(fujimod_usb_app80::operations::Invocation),
    ReadRange(MemoryRange, usb::ff80::commands::memory::DebugMemoryLimits),
    WriteRange(
        Address,
        Vec<u8>,
        usb::ff80::commands::memory::DebugMemoryLimits,
    ),
}
impl Operation {
    async fn run(self, lease: &mut Lease) -> Result<JsValue, JsValue> {
        if matches!(self, Self::Close) {
            let LeasedSession::Open(native) =
                std::mem::replace(&mut lease.resource, LeasedSession::Closing)
            else {
                unreachable!("new operation owns its native session")
            };
            let result = native.close().await;
            if result.is_ok() {
                lease.resource = LeasedSession::Closed;
            }
            return result.map(bytes).map_err(error);
        }
        let LeasedSession::Open(s) = &mut lease.resource else {
            unreachable!("ordinary operation owns its native session")
        };
        match self {
            Self::Workflow(workflow) => return workflow.run(s).await,
            Self::Bulk(read) => return read.run(s).await,
            Self::StateDump(dump) => return dump.run(s).await,
            Self::FlashBackup(backup) => return backup.run(s).await,
            Self::UploadOwned(image, destination, limits) => image
                .upload(s, destination, limits)
                .await
                .map(|inner| UploadReply { inner }.into()),
            Self::VerifyOwned(image, window) => {
                image.verify(s, window).await.map(|_| JsValue::UNDEFINED)
            }
            Self::Sequence(commands, limits) => s.sequence(commands, limits).await.map(|replies| {
                replies
                    .into_iter()
                    .map(bytes)
                    .collect::<js_sys::Array>()
                    .into()
            }),
            Self::Ping => s.ping().await.map(bytes),
            Self::Close => unreachable!(),
            Self::Info(selector) => s
                .device_info(selector.into())
                .await
                .map(|inner| InfoReply { inner }.into()),
            Self::ReadMemory(cmd) => s.read_memory(cmd).await.map(bytes),
            Self::WriteMemory(cmd) => s.write_memory(cmd).await.map(|_| JsValue::UNDEFINED),
            Self::Geometry(view) => s
                .configuration_geometry(view.into())
                .await
                .map(|inner| ConfigurationGeometry { inner }.into()),
            Self::ReadConfiguration(cmd) => s.read_configuration(cmd).await.map(bytes),
            Self::WriteConfiguration(cmd) => {
                s.write_configuration(cmd).await.map(|_| JsValue::UNDEFINED)
            }
            Self::Diagnostic(tag) => s
                .diagnostic_query(tag)
                .await
                .map(|inner| DiagnosticReply { inner }.into()),
            Self::PrepareRead(mode) => s
                .prepare_data_read(mode.into())
                .await
                .map(|inner| DiagnosticReply { inner }.into()),
            Self::PrepareWrite(length, mode) => s
                .prepare_data_write(length, mode.into())
                .await
                .map(|inner| DiagnosticReply { inner }.into()),
            Self::ReadData(offset, length) => s.read_data(offset, length).await.map(bytes),
            Self::WriteData(data) => s.write_data(&data).await.map(|_| JsValue::UNDEFINED),
            Self::EmptyResponse => s.empty_response().await.map(bytes),
            Self::Capabilities => s
                .prepare_capabilities()
                .await
                .map(|inner| CapabilitiesReply { inner }.into()),
            Self::DownloadStatus => s
                .firmware_download_status()
                .await
                .map(|inner| FirmwareDownloadReply { inner }.into()),
            Self::ClearDownloadStatus => s
                .clear_firmware_download_status()
                .await
                .map(|inner| FirmwareDownloadReply { inner }.into()),
            Self::Event(event, limit) => s.send_event(event, limit).await.map(bytes),
            Self::Console(cmd) => s.submit_console(cmd).await.map(bytes),
            Self::PollConsole(capacity) => s
                .poll_console(capacity)
                .await
                .map(|inner| ConsoleReply { inner }.into()),
            Self::Log(capacity) => s
                .read_log(capacity)
                .await
                .map(|inner| LogReply { inner }.into()),
            Self::Research(request) => s
                .research_command(&request)
                .await
                .map(|inner| ResearchReply { inner }.into()),
            Self::Upload(data, piece, mode) => {
                usb::operations::data::upload(s, &data, piece, mode.into())
                    .await
                    .map(|inner| UploadReply { inner }.into())
            }
            Self::App80(mailbox, request) => {
                mailbox.exchange(s, &request).await.map(|r| bytes(r.raw()))
            }
            Self::Service(invocation) => invocation
                .execute(s)
                .await
                .map(|inner| service::App80CommandReply { inner }.into()),
            Self::ReadRange(range, limits) => usb::operations::memory::read_range(s, range, limits)
                .await
                .map(bytes),
            Self::WriteRange(address, data, limits) => {
                usb::operations::memory::write_range(s, address, &data, limits)
                    .await
                    .map(|_| JsValue::UNDEFINED)
            }
        }
        .map_err(error)
    }
}
#[wasm_bindgen(js_name=uploadData,unchecked_return_type="Promise<UploadReply>")]
pub fn upload_data(
    session: &Session,
    #[wasm_bindgen(unchecked_param_type = "Uint8Array")] data: JsValue,
    #[wasm_bindgen(unchecked_param_type = "number")] piece_bytes: JsValue,
    #[wasm_bindgen(unchecked_param_type = "DataWriteMode")] mode: JsValue,
) -> Result<js_sys::Promise, JsValue> {
    let data = error::buffer(data)?;

    let mode = DataWriteMode::parse(mode)?;

    let piece_bytes = error::integer(piece_bytes, u32::MAX)?;

    if data.is_empty() {
        return Err(error(usb::Error::InvalidLength));
    }
    session.call(Operation::Upload(data, nz(piece_bytes)?, mode))
}
#[wasm_bindgen(js_name=exchangeApp80,unchecked_return_type="Promise<Uint8Array>")]
pub fn exchange_app80(
    session: &Session,
    mailbox: &Mailbox,
    request: &App80Request,
) -> Result<js_sys::Promise, JsValue> {
    if session.limits.configuration.get() < 33 {
        return Err(fail(
            ErrorKind::InvalidArgument,
            "session configuration limit cannot carry an App80 mailbox",
        ));
    }
    session.call(Operation::App80(mailbox.inner, request.inner.clone()))
}
#[wasm_bindgen(js_name=executeApp80,unchecked_return_type="Promise<App80CommandReply>")]
pub fn execute_app80(
    session: &Session,
    invocation: &service::App80Invocation,
) -> Result<js_sys::Promise, JsValue> {
    if (session.limits.configuration.get() as usize)
        < invocation.inner.command().request().encoded_len()
    {
        return Err(fail(
            ErrorKind::InvalidArgument,
            "session configuration limit cannot carry an App80 mailbox",
        ));
    }
    session.call(Operation::Service(invocation.inner.clone()))
}
#[wasm_bindgen]
impl Session {
    #[wasm_bindgen(js_name=executeSequence,unchecked_return_type="Promise<Uint8Array[]>")]
    pub fn execute_sequence(
        &self,
        sequence: &crate::sequence::CommandSequence,
    ) -> Result<js_sys::Promise, JsValue> {
        self.call(Operation::Sequence(
            sequence.commands.clone(),
            self.limits.memory,
        ))
    }
}
#[wasm_bindgen(js_name=readMemoryRange,unchecked_return_type="Promise<Uint8Array>")]
pub fn read_memory_range(
    session: &Session,
    #[wasm_bindgen(unchecked_param_type = "number")] address: JsValue,
    #[wasm_bindgen(unchecked_param_type = "number")] length: JsValue,
) -> Result<js_sys::Promise, JsValue> {
    let address = error::integer(address, u32::MAX)?;
    let length = error::integer(length, u32::MAX)?;

    session.call(Operation::ReadRange(
        MemoryRange::new(Address(address), nz(length)?).map_err(error)?,
        session.limits.memory,
    ))
}
#[wasm_bindgen(js_name=writeMemoryRange,unchecked_return_type="Promise<void>")]
pub fn write_memory_range(
    session: &Session,
    #[wasm_bindgen(unchecked_param_type = "number")] address: JsValue,
    #[wasm_bindgen(unchecked_param_type = "Uint8Array")] data: JsValue,
) -> Result<js_sys::Promise, JsValue> {
    let data = error::buffer(data)?;

    let address = error::integer(address, u32::MAX)?;

    MemoryRange::new(
        Address(address),
        nz(u32::try_from(data.len()).map_err(|_| error(usb::Error::InvalidLength))?)?,
    )
    .map_err(error)?;
    session.call(Operation::WriteRange(
        Address(address),
        data,
        session.limits.memory,
    ))
}
