use super::super::commands::{
    capabilities::{self, CapabilitiesReply},
    configuration::{
        ConfigurationGeometry, ConfigurationView, ReadConfiguration, WriteConfiguration,
    },
    debug::{self, ConsoleCommand, ConsoleReply, DebugEvent, LogReply},
    diagnostic::{self, DataReadMode, DataWriteMode, DiagnosticReply, DiagnosticTag},
    info::{DownloadStatusAction, FirmwareDownloadReply},
    research,
};
use super::*;
use crate::ff80::framing::data_envelope;

impl Session {
    /// One unclassified command, with only common-header validation. This never
    /// retries, waits on application fields, downloads data or sends cleanup.
    pub async fn research_command(
        &mut self,
        request: &research::Request,
    ) -> Result<research::Reply> {
        self.core.research_command(request).await
    }

    /// Read application download state once. Does not wait for `Writing` to end.
    pub async fn firmware_download_status(&mut self) -> Result<FirmwareDownloadReply> {
        self.core.download_status(DownloadStatusAction::Read).await
    }
    /// Clear the firmware's download-status field. This is a mutating command,
    /// separate from identification and from reading download state.
    pub async fn clear_firmware_download_status(&mut self) -> Result<FirmwareDownloadReply> {
        self.core.download_status(DownloadStatusAction::Clear).await
    }

    /// Select capability data and report its size. No E1 or E8 is sent.
    /// Decode the downloaded table using the selected firmware's definition.
    pub async fn prepare_capabilities(&mut self) -> Result<CapabilitiesReply> {
        self.core.prepare_capabilities().await
    }

    pub async fn configuration_geometry(
        &mut self,
        view: ConfigurationView,
    ) -> Result<ConfigurationGeometry> {
        self.core.configuration_geometry(view).await
    }
    pub async fn read_configuration(&mut self, command: ReadConfiguration) -> Result<Vec<u8>> {
        self.core.read_configuration(command).await
    }
    pub async fn write_configuration(&mut self, command: WriteConfiguration) -> Result<()> {
        self.core.write_configuration(command).await
    }
    pub async fn diagnostic_query(&mut self, tag: DiagnosticTag) -> Result<DiagnosticReply> {
        self.core.diagnostic_query(tag).await
    }
    pub async fn prepare_data_read(&mut self, mode: DataReadMode) -> Result<DiagnosticReply> {
        self.core.prepare_data_read(mode).await
    }
    pub async fn prepare_data_write(
        &mut self,
        length: NonZeroU32,
        mode: DataWriteMode,
    ) -> Result<DiagnosticReply> {
        self.core.prepare_data_write(length, mode).await
    }
    /// E1 reads from the current diagnostic buffer. Offset is a protocol value,
    /// not a CPU address; firmware-specific mapping belongs in operations.
    pub async fn read_data(&mut self, offset: u32, length: NonZeroU32) -> Result<Vec<u8>> {
        self.core.read_data(offset, length).await
    }
    /// One E0 request. Does not prepare the destination or send E8 implicitly.
    pub async fn write_data(&mut self, bytes: &[u8]) -> Result<()> {
        self.core.write_data(bytes).await
    }
    /// One E8 acknowledgment: four raw bytes or a 32-byte command reply.
    /// The caller interprets the reply; it does not establish execution completion.
    pub async fn empty_response(&mut self) -> Result<Vec<u8>> {
        self.core.empty_response().await
    }
    pub async fn send_event(&mut self, event: DebugEvent, limit: NonZeroU32) -> Result<Vec<u8>> {
        self.core.send_event(event, limit).await
    }
    pub async fn submit_console(&mut self, command: ConsoleCommand) -> Result<Vec<u8>> {
        self.core.submit_console(command).await
    }
    pub async fn poll_console(&mut self, capacity: NonZeroU32) -> Result<ConsoleReply> {
        self.core.poll_console(capacity).await
    }
    /// Retrieve up to `capacity` log bytes. Treat as consuming: firmware may
    /// advance its cursor and clear overrun state. This is not an observational
    /// RAM read or a safely repeatable retry.
    pub async fn read_log(&mut self, capacity: NonZeroU32) -> Result<LogReply> {
        self.core.read_log(capacity).await
    }
}
impl<I: Io> Core<I> {
    pub(super) async fn research_command(
        &mut self,
        request: &research::Request,
    ) -> Result<research::Reply> {
        self.command(request.encode(), request.reply_capacity().get(), |r| {
            request.decode(r)
        })
        .await
    }

    pub(super) async fn download_status(
        &mut self,
        action: DownloadStatusAction,
    ) -> Result<FirmwareDownloadReply> {
        self.command(action.encode(), 128, |r| action.decode(r))
            .await
    }

    pub(super) async fn prepare_capabilities(&mut self) -> Result<CapabilitiesReply> {
        self.command(capabilities::request(), 32, capabilities::decode)
            .await
    }

    pub(super) async fn configuration_geometry(
        &mut self,
        view: ConfigurationView,
    ) -> Result<ConfigurationGeometry> {
        self.command(view.encode_geometry(), 48, |r| view.decode_geometry(r))
            .await
    }
    pub(super) async fn read_configuration(
        &mut self,
        command: ReadConfiguration,
    ) -> Result<Vec<u8>> {
        let maximum = command
            .length
            .get()
            .checked_add(36)
            .ok_or(Error::InvalidLength)?;
        self.command(command.encode(), maximum, |r| command.decode(r))
            .await
    }
    pub(super) async fn write_configuration(&mut self, command: WriteConfiguration) -> Result<()> {
        self.command(command.encode(), 36, |r| command.decode(r))
            .await
    }
    pub(super) async fn diagnostic_query(&mut self, tag: DiagnosticTag) -> Result<DiagnosticReply> {
        self.command(diagnostic::query(tag), 32, |r| {
            DiagnosticReply::decode(r, 0x450006)
        })
        .await
    }
    pub(super) async fn prepare_data_read(
        &mut self,
        mode: DataReadMode,
    ) -> Result<DiagnosticReply> {
        self.command(diagnostic::read_test(mode), 32, |r| {
            DiagnosticReply::decode(r, 0x550001)
        })
        .await
    }
    pub(super) async fn prepare_data_write(
        &mut self,
        length: NonZeroU32,
        mode: DataWriteMode,
    ) -> Result<DiagnosticReply> {
        self.command(diagnostic::write_test(length, mode), 32, |r| {
            DiagnosticReply::decode(r, 0x760001)
        })
        .await
    }
    pub(super) async fn send_event(
        &mut self,
        event: DebugEvent,
        limit: NonZeroU32,
    ) -> Result<Vec<u8>> {
        let (bytes, params) = event.encode(limit)?;
        self.command(bytes, 36, |r| DebugEvent::decode(r, &params))
            .await
    }
    pub(super) async fn submit_console(&mut self, command: ConsoleCommand) -> Result<Vec<u8>> {
        self.command(command.encode(), 50, |r| {
            debug::console_header(&r, 6)?;
            Ok(r)
        })
        .await
    }
    pub(super) async fn poll_console(&mut self, capacity: NonZeroU32) -> Result<ConsoleReply> {
        let maximum = capacity.get().checked_add(32).ok_or(Error::InvalidLength)?;
        self.command(debug::console_poll(capacity), maximum, |r| {
            debug::decode_console(r, capacity)
        })
        .await
    }
    pub(super) async fn read_log(&mut self, capacity: NonZeroU32) -> Result<LogReply> {
        let maximum = capacity.get().checked_add(36).ok_or(Error::InvalidLength)?;
        self.command(debug::log_request(capacity), maximum, |r| {
            debug::decode_log(r, capacity)
        })
        .await
    }
    pub(super) async fn read_data(&mut self, offset: u32, length: NonZeroU32) -> Result<Vec<u8>> {
        check_limit(length.get(), self.max_reply_bytes)?;
        self.start()?;
        self.io
            .control(
                RequestKind::ReadData,
                data_envelope(RequestKind::ReadData, offset, length.get(), self.info.id),
            )
            .await?;
        let mut data = Vec::new();
        while data.len() < length.get() as usize {
            let remaining = length.get() as usize - data.len();
            match self.io.read(remaining).await {
                Ok(chunk) if !chunk.is_empty() && chunk.len() <= remaining => {
                    data.extend_from_slice(&chunk)
                }
                result => {
                    let error = match result {
                        Ok(chunk) => Error::Protocol(ProtocolError::DataLength {
                            available: remaining,
                            declared: chunk.len(),
                        }),
                        Err(error) => {
                            // Preserve bytes delivered with the failed USB completion too.
                            if let Error::Transfer { partial, .. } = &error
                                && partial.len() <= remaining
                            {
                                data.extend_from_slice(partial);
                            }
                            error
                        }
                    };
                    return Err(Error::PartialRead {
                        data_len: data.len(),
                        data,
                        source: Box::new(error),
                    });
                }
            }
        }
        self.state = SessionState::Ready;
        Ok(data)
    }
    pub(super) async fn write_data(&mut self, bytes: &[u8]) -> Result<()> {
        let length = u32::try_from(bytes.len())
            .ok()
            .and_then(NonZeroU32::new)
            .ok_or(Error::InvalidLength)?;
        self.start()?;
        self.io
            .control(
                RequestKind::WriteData,
                envelope(RequestKind::WriteData, length.get(), self.info.id),
            )
            .await?;
        self.io.write(bytes).await?;
        self.state = SessionState::Ready;
        Ok(())
    }
    pub(super) async fn empty_response(&mut self) -> Result<Vec<u8>> {
        check_limit(32, self.max_reply_bytes)?;
        self.start()?;
        self.io
            .control(
                RequestKind::EmptyResponse,
                envelope(RequestKind::EmptyResponse, 32, self.info.id),
            )
            .await?;
        let bytes = self.io.read(32).await?;
        if !matches!(bytes.len(), 4 | 32) {
            return Err(ProtocolError::ReplyLength {
                expected: 32,
                actual: bytes.len(),
            }
            .into());
        }
        self.state = SessionState::Ready;
        Ok(bytes)
    }
}
