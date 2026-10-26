use super::{
    commands::{
        info::{DeviceInfo, InfoSelector},
        memory::{ReadMemory, WriteMemory},
    },
    framing::{RequestKind, envelope},
};
use crate::{
    Error, Result,
    connection::{Backend, Io},
    error::{ProtocolError, check_limit},
};
use std::num::NonZeroU32;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionState {
    Ready,
    /// A transfer failed, its reply was malformed, or its future was dropped.
    /// The device may still be executing and this session cannot issue commands.
    CompletionUnknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SessionInfo {
    pub id: u16,
    pub advertised_packet_bytes: NonZeroU32,
    pub raw_open_reply: [u8; 8],
}

/// Exclusive owner of a connection's command/reply exchanges.
///
/// All operations borrow mutably. Cancellation or a malformed response leaves
/// this session unusable; dropping it releases host resources without requesting
/// a camera reset, firmware cleanup or service-mode transition.
pub struct Session {
    core: Core<crate::trace::Observed<Backend>>,
}
impl Session {
    pub(crate) async fn open(
        io: Backend,
        max_reply_bytes: u32,
        trace: Option<crate::trace::Trace>,
    ) -> Result<Self> {
        Ok(Self {
            core: Core::open(crate::trace::Observed::new(io, trace), max_reply_bytes).await?,
        })
    }
    pub(crate) async fn replay(
        script: crate::testing::Script,
        max_reply_bytes: u32,
        trace: Option<crate::trace::Trace>,
    ) -> Result<Self> {
        Ok(Self {
            core: Core::open(
                crate::trace::Observed::new(Backend::Replay(script), trace),
                max_reply_bytes,
            )
            .await?,
        })
    }
    pub(crate) async fn from_peer(
        peer: crate::testing::Model,
        max_reply_bytes: u32,
        trace: Option<crate::trace::Trace>,
    ) -> Result<Self> {
        Ok(Self {
            core: Core::open(
                crate::trace::Observed::new(Backend::Model(peer), trace),
                max_reply_bytes,
            )
            .await?,
        })
    }
    pub fn trace(&self) -> Option<crate::trace::Trace> {
        self.core.io.trace()
    }
    pub fn info(&self) -> SessionInfo {
        self.core.info
    }
    pub fn state(&self) -> SessionState {
        self.core.state
    }
    /// A composition observed an invalid nested response after its USB exchange.
    /// This host-side fence sends no I/O and cannot restore a usable session.
    pub fn invalidate_completion(&mut self) {
        self.core.state = SessionState::CompletionUnknown;
    }
    pub fn max_reply_bytes(&self) -> NonZeroU32 {
        NonZeroU32::new(self.core.max_reply_bytes).expect("validated before session open")
    }
    /// Returns the two raw ping reply bytes; their meaning is not inferred.
    pub async fn ping(&mut self) -> Result<[u8; 2]> {
        self.core.ping().await
    }
    pub async fn device_info(&mut self, selector: InfoSelector) -> Result<DeviceInfo> {
        self.core.device_info(selector).await
    }
    pub async fn read_memory(&mut self, command: ReadMemory) -> Result<Vec<u8>> {
        self.core.read_memory(command).await
    }
    pub async fn write_memory(&mut self, command: WriteMemory) -> Result<()> {
        self.core.write_memory(command).await
    }
    /// Execute a bounded sequence, checking each reply before the next command
    /// reaches USB. A condition failure is terminal, including on local USB.
    pub async fn sequence(
        &mut self,
        commands: Vec<super::sequence::SequenceCommand>,
        limits: super::commands::memory::DebugMemoryLimits,
    ) -> Result<Vec<Vec<u8>>> {
        use super::sequence::CheckedCommand;
        check_limit(
            commands
                .len()
                .try_into()
                .map_err(|_| Error::InvalidLength)?,
            512,
        )?;
        let commands: Vec<CheckedCommand> = commands
            .into_iter()
            .map(|c| CheckedCommand::new(c, limits))
            .collect::<Result<_>>()?;
        for command in &commands {
            check_limit(command.reply_size, self.core.max_reply_bytes)?;
            check_limit(
                command.reply_size,
                fujimod_usb_proxy::protocol::MAX_TRANSFER as u32,
            )?;
            check_limit(
                command
                    .request
                    .len()
                    .try_into()
                    .map_err(|_| Error::InvalidLength)?,
                fujimod_usb_proxy::protocol::MAX_TRANSFER as u32,
            )?;
            if command.request.len() >= 36 {
                check_limit(
                    command
                        .reply_size
                        .saturating_sub(36)
                        .max(command.request.len().saturating_sub(36) as u32),
                    self.info().advertised_packet_bytes.get(),
                )?;
            }
        }
        if commands.is_empty() {
            return Ok(Vec::new());
        }
        if !matches!(self.core.io.inner_mut(), Backend::Proxy(_)) {
            let mut replies = Vec::with_capacity(commands.len());
            for command in commands {
                replies.push(
                    self.core
                        .command(command.request.clone(), command.reply_size, |r| {
                            command.decode(r)
                        })
                        .await?,
                );
            }
            return Ok(replies);
        }
        let framed = commands
            .iter()
            .map(|c| crate::proxy::CheckedExchange {
                write: envelope(
                    RequestKind::WriteCommand,
                    c.request.len() as u32,
                    self.core.info.id,
                ),
                bytes: c.request.clone(),
                read: envelope(RequestKind::ReadCommand, c.reply_size, self.core.info.id),
                reply: c.reply_size as usize,
                expected: c.expected.clone(),
            })
            .collect();
        self.core.start()?;
        let trace = self.core.io.trace();
        let spans: Vec<_> = commands
            .iter()
            .map(|c| {
                trace.as_ref().map(|t| {
                    t.protocol(
                        crate::trace::Layer::Ff80,
                        u32::from_le_bytes(c.request[..4].try_into().expect("command header")),
                        &c.request,
                    )
                })
            })
            .collect();
        let Backend::Proxy(io) = self.core.io.inner_mut() else {
            unreachable!()
        };
        let replies = io.checked_commands(framed).await;
        if replies.is_err() {
            for span in spans.into_iter().flatten() {
                span.complete(&replies, &[]);
            }
            return replies;
        }
        let mut result = Vec::with_capacity(commands.len());
        for ((command, raw), span) in commands.iter().zip(replies?).zip(spans) {
            let decoded = command.decode(raw.clone());
            if let Some(span) = span {
                span.complete(&decoded, &raw);
            }
            result.push(decoded?);
        }
        self.core.state = SessionState::Ready;
        Ok(result)
    }
    /// Several reads; over a device share they stream with one wait.
    pub async fn read_memory_many(&mut self, commands: Vec<ReadMemory>) -> Result<Vec<Vec<u8>>> {
        if !matches!(self.core.io.inner_mut(), Backend::Proxy(_)) {
            let mut replies = Vec::with_capacity(commands.len());
            for command in commands {
                replies.push(self.core.read_memory(command).await?);
            }
            return Ok(replies);
        }
        let mut framed = Vec::with_capacity(commands.len());
        for command in &commands {
            let size = command.range().length().get();
            check_limit(size, self.core.info.advertised_packet_bytes.get())?;
            framed.push((
                command.encode(),
                size.checked_add(36).ok_or(Error::InvalidLength)?,
            ));
        }
        let replies = self.many(framed).await?;
        self.decode_many(
            commands
                .into_iter()
                .zip(replies)
                .map(|(c, r)| move || c.decode(r)),
        )
    }
    /// Several writes; over a device share they stream with one wait.
    pub async fn write_memory_many(&mut self, commands: Vec<WriteMemory>) -> Result<()> {
        if !matches!(self.core.io.inner_mut(), Backend::Proxy(_)) {
            for command in commands {
                self.core.write_memory(command).await?;
            }
            return Ok(());
        }
        let mut framed = Vec::with_capacity(commands.len());
        for command in &commands {
            check_limit(
                command.range().length().get(),
                self.core.info.advertised_packet_bytes.get(),
            )?;
            framed.push((command.encode(), 36));
        }
        let replies = self.many(framed).await?;
        self.decode_many(
            commands
                .into_iter()
                .zip(replies)
                .map(|(c, r)| move || c.decode(r)),
        )?;
        Ok(())
    }
    /// Raw replies for framed commands as one share pipeline. A failure leaves
    /// the session's completion unknown until every reply decodes.
    async fn many(&mut self, commands: Vec<(Vec<u8>, u32)>) -> Result<Vec<Vec<u8>>> {
        let id = self.core.info.id;
        let max_reply = self.core.max_reply_bytes;
        let mut framed = Vec::with_capacity(commands.len());
        for (bytes, size) in commands {
            check_limit(size, max_reply)?;
            let length = u32::try_from(bytes.len()).map_err(|_| Error::InvalidLength)?;
            framed.push(crate::proxy::FramedCommand {
                write: envelope(RequestKind::WriteCommand, length, id),
                bytes,
                read: envelope(RequestKind::ReadCommand, size, id),
                reply: size as usize,
            });
        }
        let sizes: Vec<usize> = framed.iter().map(|f| f.reply).collect();
        self.core.start()?;
        let Backend::Proxy(io) = self.core.io.inner_mut() else {
            unreachable!()
        };
        let replies = io.commands(framed).await?;
        for (reply, size) in replies.iter().zip(&sizes) {
            if reply.len() > *size {
                return Err(ProtocolError::ReplyLength {
                    expected: *size,
                    actual: reply.len(),
                }
                .into());
            }
        }
        Ok(replies)
    }
    /// Decode replies in order; device status errors still leave a usable session.
    fn decode_many<T>(
        &mut self,
        decoders: impl Iterator<Item = impl FnOnce() -> Result<T>>,
    ) -> Result<Vec<T>> {
        let mut values = Vec::new();
        for decode in decoders {
            let result = decode();
            if result.is_ok() || matches!(result, Err(Error::DeviceStatus { .. })) {
                self.core.state = SessionState::Ready;
            }
            values.push(result?);
        }
        self.core.state = SessionState::Ready;
        Ok(values)
    }
    /// A neutral progress line in the share owner's log. No-op for local USB.
    pub async fn owner_status(&mut self, message: &str) -> Result<()> {
        match self.core.io.inner_mut() {
            crate::connection::Backend::Proxy(io) => {
                io.owner_message(fujimod_usb_proxy::Operation::OwnerStatus {
                    message: message.into(),
                })
                .await
            }
            _ => Ok(()),
        }
    }
    /// Wait for the share owner's spoken ten-second countdown. No USB traffic.
    pub async fn owner_attention(&mut self, message: &str) -> Result<()> {
        match self.core.io.inner_mut() {
            crate::connection::Backend::Proxy(io) => {
                io.owner_message(fujimod_usb_proxy::Operation::OwnerAttention {
                    message: message.into(),
                })
                .await
            }
            _ => Err(
                fujimod_usb_proxy::Error("attention requires a shared Dev Tools page".into())
                    .into(),
            ),
        }
    }
    /// Tell a share owner the camera's battery level (a firmware level name).
    /// Only shared (proxy) sessions have an owner; elsewhere this does nothing.
    pub async fn report_battery(&mut self, level: &str) -> Result<()> {
        self.notify_owner(fujimod_usb_proxy::Message::Status {
            battery: level.into(),
        })
        .await
    }
    /// Ask a share owner's user to fix a problem only this controller can see.
    /// Only shared (proxy) sessions have an owner; elsewhere this does nothing.
    pub async fn request_attention(&mut self, message: &str) -> Result<()> {
        self.notify_owner(fujimod_usb_proxy::Message::Attention {
            message: message.into(),
        })
        .await
    }
    /// What the tool is doing now, above the share owner's log footer; an
    /// empty title clears it. No-op for local USB.
    pub async fn owner_activity(&mut self, title: &str) -> Result<()> {
        self.notify_owner(fujimod_usb_proxy::Message::Activity {
            title: fujimod_usb_proxy::protocol::bounded_reason(title),
        })
        .await
    }
    /// Start a `total`-unit progress bar under the share owner's log.
    pub async fn owner_progress_start(&mut self, label: &str, total: u64) -> Result<()> {
        self.notify_owner(fujimod_usb_proxy::Message::ProgressStart {
            label: fujimod_usb_proxy::protocol::bounded_reason(label),
            total,
        })
        .await
    }
    pub async fn owner_progress_update(&mut self, done: u64) -> Result<()> {
        self.notify_owner(fujimod_usb_proxy::Message::ProgressUpdate { done })
            .await
    }
    pub async fn owner_progress_done(&mut self) -> Result<()> {
        self.notify_owner(fujimod_usb_proxy::Message::ProgressDone)
            .await
    }
    async fn notify_owner(&mut self, message: fujimod_usb_proxy::Message) -> Result<()> {
        match self.core.io.inner_mut() {
            crate::connection::Backend::Proxy(io) => io.notify_owner(message).await,
            _ => Ok(()),
        }
    }
    /// Sends C4 once. This consumes the session even if the close fails.
    pub async fn close(mut self) -> Result<[u8; 2]> {
        let reply = self.core.close().await?;
        self.core.io.release().await?;
        Ok(reply)
    }
}

pub(crate) struct Core<I> {
    io: I,
    info: SessionInfo,
    state: SessionState,
    max_reply_bytes: u32,
}

impl<I: Io> Core<I> {
    async fn open(mut io: I, max_reply_bytes: u32) -> Result<Self> {
        check_limit(8, max_reply_bytes)?;
        let bytes: [u8; 8] = exact(Self::lifecycle_io(&mut io, RequestKind::Open, 8, 0).await?)?;
        let packet = NonZeroU32::new(u32::from_be_bytes(bytes[4..8].try_into().unwrap()))
            .ok_or(ProtocolError::ZeroPacketLimit)?;
        Ok(Self {
            io,
            info: SessionInfo {
                id: u16::from_le_bytes([bytes[0], bytes[1]]),
                advertised_packet_bytes: packet,
                raw_open_reply: bytes,
            },
            state: SessionState::Ready,
            max_reply_bytes,
        })
    }
    fn start(&mut self) -> Result<()> {
        if self.state != SessionState::Ready {
            return Err(Error::CompletionUnknown);
        }
        // Set before the first await; dropping any in-flight future preserves it.
        self.state = SessionState::CompletionUnknown;
        Ok(())
    }
    async fn lifecycle_io(io: &mut I, kind: RequestKind, size: u32, id: u16) -> Result<Vec<u8>> {
        let bytes = envelope(kind, size, id);
        let span = io
            .trace()
            .map(|t| t.protocol(crate::trace::Layer::Ff80, bytes[0].into(), &bytes));
        let mut reply = Vec::new();
        let result = async {
            io.control(kind, bytes).await?;
            reply = io.read(size as usize).await?;
            if reply.len() != size as usize {
                return Err(ProtocolError::ReplyLength {
                    expected: size as usize,
                    actual: reply.len(),
                }
                .into());
            }
            Ok(())
        }
        .await;
        if let Some(span) = span {
            span.complete(&result, &reply);
        }
        result.map(|()| reply)
    }
    async fn lifecycle(&mut self, kind: RequestKind) -> Result<[u8; 2]> {
        self.start()?;
        let reply = exact(Self::lifecycle_io(&mut self.io, kind, 2, self.info.id).await?)?;
        self.state = SessionState::Ready;
        Ok(reply)
    }
    async fn ping(&mut self) -> Result<[u8; 2]> {
        self.lifecycle(RequestKind::Ping).await
    }
    async fn close(&mut self) -> Result<[u8; 2]> {
        self.lifecycle(RequestKind::Close).await
    }

    async fn command<T>(
        &mut self,
        bytes: Vec<u8>,
        reply_size: u32,
        decode: impl FnOnce(Vec<u8>) -> Result<T>,
    ) -> Result<T> {
        check_limit(reply_size, self.max_reply_bytes)?;
        let size = u32::try_from(bytes.len()).map_err(|_| Error::InvalidLength)?;
        self.start()?;
        let code = u32::from_le_bytes(bytes[..4].try_into().expect("encoded command header"));
        let span = self
            .io
            .trace()
            .map(|t| t.protocol(crate::trace::Layer::Ff80, code, &bytes));
        let mut raw = Vec::new();
        let result = async {
            self.io
                .control(
                    RequestKind::WriteCommand,
                    envelope(RequestKind::WriteCommand, size, self.info.id),
                )
                .await?;
            self.io.write(&bytes).await?;
            self.io
                .control(
                    RequestKind::ReadCommand,
                    envelope(RequestKind::ReadCommand, reply_size, self.info.id),
                )
                .await?;
            let response = self.io.read(reply_size as usize).await?;
            if span.is_some() {
                raw = response.clone();
            }
            if response.len() > reply_size as usize {
                return Err(ProtocolError::ReplyLength {
                    expected: reply_size as usize,
                    actual: response.len(),
                }
                .into());
            }
            let result = decode(response);
            // A framed device error is a completed reply, not an async disposition.
            if result.is_ok() || matches!(result, Err(Error::DeviceStatus { .. })) {
                self.state = SessionState::Ready;
            }
            result
        }
        .await;
        if let Some(span) = span {
            span.complete(&result, &raw);
        }
        result
    }
    async fn device_info(&mut self, selector: InfoSelector) -> Result<DeviceInfo> {
        self.command(selector.encode(), 128, |r| selector.decode(r))
            .await
    }
    async fn read_memory(&mut self, command: ReadMemory) -> Result<Vec<u8>> {
        let size = command.range().length().get();
        check_limit(size, self.info.advertised_packet_bytes.get())?;
        let reply_size = size.checked_add(36).ok_or(Error::InvalidLength)?;
        self.command(command.encode(), reply_size, |r| command.decode(r))
            .await
    }
    async fn write_memory(&mut self, command: WriteMemory) -> Result<()> {
        check_limit(
            command.range().length().get(),
            self.info.advertised_packet_bytes.get(),
        )?;
        self.command(command.encode(), 36, |r| command.decode(r))
            .await
    }
}

fn exact<const N: usize>(data: Vec<u8>) -> Result<[u8; N]> {
    let actual = data.len();
    data.try_into().map_err(|_| {
        ProtocolError::ReplyLength {
            expected: N,
            actual,
        }
        .into()
    })
}

#[cfg(test)]
mod tests;

mod commands;
