//! Direct commands: each delegates to its typed Rust primitive.
use super::*;

#[pymethods]
impl AsyncSession {
    async fn execute_sequence(
        &self,
        sequence: super::sequence::CommandSequence,
    ) -> PyResult<Vec<Py<PyBytes>>> {
        let limits = self.limits.memory;
        self.with(move |s| Box::pin(s.sequence(sequence.commands, limits)))
            .await
            .map(|replies| replies.into_iter().map(bytes).collect())
    }
    #[staticmethod]
    #[pyo3(signature=(device,interface,limits,*,timeout_ms=5000,max_reply_bytes=8388608,trace=None))]
    async fn open(
        device: Device,
        interface: InterfaceSelection,
        limits: FirmwareLimits,
        timeout_ms: u64,
        max_reply_bytes: u32,
        trace: Option<tracing::Trace>,
    ) -> PyResult<Self> {
        let budget = nz(max_reply_bytes)?;
        let native = detached(async {
            Connection::open(
                &device.inner,
                interface.inner,
                TransferOptions {
                    timeout: Duration::from_millis(timeout_ms),
                    max_reply_bytes: budget,
                },
            )
            .await?
            .begin_session(trace.map(|t| t.inner))
            .await
        })
        .await
        .map_err(error)?;
        Ok(Self::new(native, limits))
    }
    #[getter]
    fn state(&self) -> PyResult<SessionState> {
        Ok(match &*lock(&self.inner)? {
            Slot::Open(native) => match native.state() {
                ff80::SessionState::Ready => SessionState::Ready,
                ff80::SessionState::CompletionUnknown => SessionState::CompletionUnknown,
            },
            Slot::InFlight(_) => SessionState::InFlight,
            Slot::ReadingRange(_) => SessionState::ReadingRange,
            Slot::Closed => SessionState::Closed,
            Slot::CompletionUnknown => SessionState::CompletionUnknown,
            Slot::Abandoned => SessionState::Abandoned,
        })
    }
    #[getter]
    fn packet_bytes(&self) -> u32 {
        self.packet_bytes
    }
    #[getter]
    fn token(&self) -> u16 {
        self.token
    }
    async fn close(&self) -> PyResult<Py<PyBytes>> {
        let (mut lease, cancel) = Lease::new(self)?;
        let Resource::Open(native) = std::mem::replace(&mut lease.resource, Resource::Closing)
        else {
            unreachable!()
        };
        let reply = detached(futures_lite::future::or(
            async {
                let _ = cancel.await;
                Err(binding_error(BindingError::Abandoned))
            },
            async { native.close().await.map_err(error) },
        ))
        .await?;
        lease.resource = Resource::Closed;
        Ok(bytes(reply))
    }
    /// Host cancellation only. Does not close, reset, or stop the camera.
    fn abandon(&self) -> PyResult<()> {
        let previous = {
            let mut slot = lock(&self.inner)?;
            match &*slot {
                Slot::Closed => return Err(binding_error(BindingError::Closed)),
                Slot::Abandoned => return Err(binding_error(BindingError::Abandoned)),
                _ => (),
            }
            std::mem::replace(&mut *slot, Slot::Abandoned)
        };
        match previous {
            Slot::InFlight(cancel) => {
                let _ = cancel.send(());
            }
            Slot::ReadingRange(reader) => {
                if let Some(reader) = reader.upgrade() {
                    reader::abandon(&reader)?;
                }
            }
            _ => (),
        }
        Ok(())
    }
    async fn ping(&self) -> PyResult<Py<PyBytes>> {
        self.with(|s| Box::pin(s.ping())).await.map(bytes)
    }
    /// Neutral tool progress in the owner's Dev Tools log.
    async fn owner_status(&self, message: String) -> PyResult<()> {
        self.with(move |s| Box::pin(async move { s.owner_status(&message).await }))
            .await
    }
    /// Returns only after the owner's ten-second countdown finishes.
    async fn owner_attention(&self, message: String) -> PyResult<()> {
        self.with(move |s| Box::pin(async move { s.owner_attention(&message).await }))
            .await
    }
    /// What the tool is doing now, above the owner's log footer ('' clears).
    async fn owner_activity(&self, title: String) -> PyResult<()> {
        self.with(move |s| Box::pin(async move { s.owner_activity(&title).await }))
            .await
    }
    /// Start a progress bar of `total` units under the owner's log.
    async fn owner_progress_start(&self, label: String, total: u64) -> PyResult<()> {
        self.with(move |s| Box::pin(async move { s.owner_progress_start(&label, total).await }))
            .await
    }
    async fn owner_progress_update(&self, done: u64) -> PyResult<()> {
        self.with(move |s| Box::pin(async move { s.owner_progress_update(done).await }))
            .await
    }
    async fn owner_progress_done(&self) -> PyResult<()> {
        self.with(|s| Box::pin(s.owner_progress_done())).await
    }
    /// Tell a share owner the camera's battery level; a no-op unless shared.
    async fn report_battery(&self, level: String) -> PyResult<()> {
        self.with(move |s| Box::pin(async move { s.report_battery(&level).await }))
            .await
    }
    /// Ask a share owner's user to fix a problem; a no-op unless shared.
    async fn request_attention(&self, message: String) -> PyResult<()> {
        self.with(move |s| Box::pin(async move { s.request_attention(&message).await }))
            .await
    }
    async fn device_info(&self, selector: InfoSelector) -> PyResult<String> {
        self.with(move |s| Box::pin(s.device_info(selector.into())))
            .await
            .map(|r| r.text)
    }
    async fn read_memory(&self, address: u32, length: u32) -> PyResult<Py<PyBytes>> {
        let command = ReadMemory::new(
            MemoryRange::new(Address(address), nz(length)?).map_err(error)?,
            self.limits.memory,
        )
        .map_err(error)?;
        self.with(move |s| Box::pin(s.read_memory(command)))
            .await
            .map(bytes)
    }
    async fn write_memory(&self, address: u32, data: Vec<u8>) -> PyResult<()> {
        let command =
            WriteMemory::new(Address(address), data, self.limits.memory).map_err(error)?;
        self.with(move |s| Box::pin(s.write_memory(command))).await
    }
    async fn configuration_geometry(
        &self,
        view: ConfigurationView,
    ) -> PyResult<ConfigurationGeometry> {
        self.with(move |s| Box::pin(s.configuration_geometry(view.into())))
            .await
            .map(|inner| ConfigurationGeometry { inner })
    }
    #[pyo3(signature=(view,offset,length,*,page=0))]
    async fn read_configuration(
        &self,
        view: ConfigurationView,
        offset: u32,
        length: u32,
        page: u32,
    ) -> PyResult<Py<PyBytes>> {
        let command = ReadConfiguration::new(
            ConfigurationLocation {
                view: view.into(),
                page,
                offset,
            },
            nz(length)?,
            self.limits.configuration,
        )
        .map_err(error)?;
        self.with(move |s| Box::pin(s.read_configuration(command)))
            .await
            .map(bytes)
    }
    #[pyo3(signature=(view,offset,data,*,page=0))]
    async fn write_configuration(
        &self,
        view: ConfigurationView,
        offset: u32,
        data: Vec<u8>,
        page: u32,
    ) -> PyResult<()> {
        let command = WriteConfiguration::new(
            ConfigurationLocation {
                view: view.into(),
                page,
                offset,
            },
            data,
            self.limits.configuration,
        )
        .map_err(error)?;
        self.with(move |s| Box::pin(s.write_configuration(command)))
            .await
    }
    async fn diagnostic_query(&self, tag: DiagnosticTag) -> PyResult<DiagnosticReply> {
        self.with(move |s| Box::pin(s.diagnostic_query(tag.inner)))
            .await
            .map(|inner| DiagnosticReply { inner })
    }
    async fn firmware_download_status(&self) -> PyResult<FirmwareDownloadReply> {
        self.with(|s| Box::pin(s.firmware_download_status()))
            .await
            .map(|inner| FirmwareDownloadReply { inner })
    }
    async fn clear_firmware_download_status(&self) -> PyResult<FirmwareDownloadReply> {
        self.with(|s| Box::pin(s.clear_firmware_download_status()))
            .await
            .map(|inner| FirmwareDownloadReply { inner })
    }
    async fn research_command(&self, request: research::Request) -> PyResult<research::Reply> {
        self.with(move |s| Box::pin(async move { s.research_command(&request.inner).await }))
            .await
            .map(|inner| research::Reply { inner })
    }
    async fn prepare_capabilities(&self) -> PyResult<CapabilitiesReply> {
        self.with(|s| Box::pin(s.prepare_capabilities()))
            .await
            .map(|inner| CapabilitiesReply { inner })
    }
    async fn prepare_data_read(&self, mode: DataReadMode) -> PyResult<DiagnosticReply> {
        self.with(move |s| Box::pin(s.prepare_data_read(mode.into())))
            .await
            .map(|inner| DiagnosticReply { inner })
    }
    async fn prepare_data_write(
        &self,
        length: u32,
        mode: DataWriteMode,
    ) -> PyResult<DiagnosticReply> {
        let length = nz(length)?;
        self.with(move |s| Box::pin(s.prepare_data_write(length, mode.into())))
            .await
            .map(|inner| DiagnosticReply { inner })
    }
    async fn read_data(&self, offset: u32, length: u32) -> PyResult<Py<PyBytes>> {
        let length = nz(length)?;
        self.with(move |s| Box::pin(s.read_data(offset, length)))
            .await
            .map(bytes)
    }
    async fn write_data(&self, data: Vec<u8>) -> PyResult<()> {
        self.with(move |s| Box::pin(async move { s.write_data(&data).await }))
            .await
    }
    async fn empty_response(&self) -> PyResult<Py<PyBytes>> {
        self.with(|s| Box::pin(s.empty_response())).await.map(bytes)
    }
    async fn send_key(&self, name: String, state: KeyState) -> PyResult<Py<PyBytes>> {
        let name = debug::EventName::new(name).map_err(error)?;
        let limit = self.limits.console;
        self.with(move |s| {
            Box::pin(s.send_event(
                debug::DebugEvent::Key {
                    name,
                    state: state.into(),
                },
                limit,
            ))
        })
        .await
        .map(bytes)
    }
    async fn request_mode(&self, mode: CameraMode) -> PyResult<Py<PyBytes>> {
        let limit = self.limits.console;
        self.with(move |s| Box::pin(s.send_event(debug::DebugEvent::Mode(mode.into()), limit)))
            .await
            .map(bytes)
    }
    async fn command_event(&self, argument: String) -> PyResult<Py<PyBytes>> {
        let limit = self.limits.console;
        self.with(move |s| Box::pin(s.send_event(debug::DebugEvent::Command(argument), limit)))
            .await
            .map(bytes)
    }
    async fn submit_console(&self, data: Vec<u8>) -> PyResult<()> {
        let command = debug::ConsoleCommand::new(data, self.limits.console).map_err(error)?;
        self.with(move |s| Box::pin(s.submit_console(command)))
            .await
            .map(|_| ())
    }
    async fn read_log(&self, capacity: u32) -> PyResult<LogReply> {
        let capacity = nz(capacity)?;
        self.with(move |s| Box::pin(s.read_log(capacity)))
            .await
            .map(|inner| LogReply { inner })
    }
    async fn poll_console(&self, capacity: u32) -> PyResult<ConsoleReply> {
        let capacity = nz(capacity)?;
        self.with(move |s| Box::pin(s.poll_console(capacity)))
            .await
            .map(|inner| ConsoleReply { inner })
    }
}
