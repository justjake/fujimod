//! Bind the selected firmware's codecs to its declared ff80 mailbox.
use super::*;
use fujimod_catalog_service::{Definition, ServiceModes};
use fujimod_usb_app80_types::catalog::{
    Catalog, Command, CommandError, CommandReply, PreparedCommand,
};
use fujimod_usb_app80_types::{CommandCode, DecodeError};

#[derive(Clone, Debug)]
pub struct Service {
    catalog: Catalog,
    mailbox: Mailbox,
}
impl Service {
    pub fn new(definition: Definition) -> Result<Self> {
        let location = definition.mailbox();
        let mailbox = Mailbox::new(
            location.page(),
            location.offset(),
            location.configuration_limit(),
        )?;
        Ok(Self {
            catalog: Catalog::new(definition),
            mailbox,
        })
    }
    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }
    pub fn prepare(
        &self,
        modes: ServiceModes,
        command: Command,
    ) -> std::result::Result<Invocation, CommandError> {
        Ok(Invocation {
            command: self.catalog.prepare(modes, command)?,
            mailbox: self.mailbox,
        })
    }
    pub fn prepare_code(
        &self,
        code: CommandCode,
        modes: ServiceModes,
        command: Command,
    ) -> std::result::Result<Invocation, CommandError> {
        Ok(Invocation {
            command: self.catalog.prepare_code(code, modes, command)?,
            mailbox: self.mailbox,
        })
    }
    pub fn prepare_research(
        &self,
        code: CommandCode,
        modes: ServiceModes,
        arguments: &[u8],
    ) -> std::result::Result<Invocation, CommandError> {
        Ok(Invocation {
            command: self.catalog.prepare_research(code, modes, arguments)?,
            mailbox: self.mailbox,
        })
    }
}
/// Its request and carrier originate from one firmware definition. Preparing an
/// invocation does no I/O and never changes service modes on the camera.
#[derive(Clone, Debug)]
pub struct Invocation {
    command: PreparedCommand,
    mailbox: Mailbox,
}
impl Invocation {
    /// Bind a reviewed command to an explicit App80 mailbox for one session exchange.
    pub fn new(command: PreparedCommand, mailbox: Mailbox) -> Self {
        Self { command, mailbox }
    }
    pub fn command(&self) -> &PreparedCommand {
        &self.command
    }
    pub fn configuration_limit(&self) -> NonZeroU32 {
        self.mailbox.limit
    }
    /// Own the ff80 stage/publish/read exchange and validate the nested reply.
    pub async fn execute(&self, session: &mut Session) -> Result<CommandReply> {
        let reply = self
            .mailbox
            .exchange(session, self.command.request())
            .await?;
        match self.command.decode(reply).map_err(|error| match error {
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
        }) {
            Ok(reply) => Ok(reply),
            Err(error) => {
                session.invalidate_completion();
                Err(error)
            }
        }
    }
}
