//! App80 carried by ff80 configuration commands, above both command codecs.
use fujimod_usb::{
    Result,
    error::check_limit,
    ff80::{
        Session,
        commands::configuration::{
            ConfigurationLocation, ConfigurationView, ReadConfiguration, WriteConfiguration,
        },
    },
};
use fujimod_usb_app80_types::{DecodeError, Reply, Request};
use std::num::NonZeroU32;

/// An explicit firmware mailbox definition. No camera body or offset is selected
/// by default. The RAM command byte is followed by 32 argument/result bytes.
#[derive(Clone, Copy, Debug)]
pub struct Mailbox {
    location: ConfigurationLocation,
    parameters: ConfigurationLocation,
    limit: NonZeroU32,
}
impl Mailbox {
    pub fn new(page: u32, offset: u32, configuration_limit: NonZeroU32) -> Result<Self> {
        let location = ConfigurationLocation {
            view: ConfigurationView::Ram,
            page,
            offset,
        };
        // Validate the complete operation before any camera I/O is possible.
        ReadConfiguration::new(location, NonZeroU32::new(33).unwrap(), configuration_limit)?;
        let parameters = ConfigurationLocation {
            offset: offset + 1,
            ..location
        };
        Ok(Self {
            location,
            parameters,
            limit: configuration_limit,
        })
    }
    /// Stage arguments, publish the command, then read its result once.
    ///
    /// The publish write awaits its ff80 reply, including firmware's asynchronous
    /// completion. No callback return value is supplied by the host. A timeout or
    /// cancellation fences the session; this operation never polls or retries.
    pub async fn exchange(&self, session: &mut Session, request: &Request) -> Result<Reply> {
        // Both setup writes have smaller replies; reject an insufficient final
        // read budget before staging arguments or publishing the command.
        let length = request.encoded_len() as u32;
        check_limit(36 + length, session.max_reply_bytes().get())?;
        let stage =
            WriteConfiguration::new(self.parameters, request.arguments().to_vec(), self.limit)?;
        let publish = WriteConfiguration::new(self.location, request.encode(), self.limit)?;
        let read =
            ReadConfiguration::new(self.location, NonZeroU32::new(length).unwrap(), self.limit)?;
        let span = session.trace().map(|t| {
            t.protocol(
                fujimod_usb::trace::Layer::App80,
                request.code().0.into(),
                &request.encode(),
            )
        });
        let result = async {
            session.write_configuration(stage).await?;
            session.write_configuration(publish).await?;
            Reply::decode_length(&session.read_configuration(read).await?, length as usize).map_err(
                |error| match error {
                    DecodeError::ReplyLength { expected, actual } => {
                        fujimod_usb::error::ProtocolError::ReplyLength { expected, actual }.into()
                    }
                    DecodeError::CommandEcho { expected, actual } => {
                        fujimod_usb::error::ProtocolError::CommandEcho {
                            expected: expected.into(),
                            actual: actual.into(),
                        }
                        .into()
                    }
                },
            )
        }
        .await;
        if let Some(span) = span {
            span.complete(&result, result.as_ref().map(|r| r.raw()).unwrap_or(&[]));
        }
        result
    }
}

mod service;
pub use service::{Invocation, Service};

#[cfg(test)]
mod tests;
