//! Single configuration bytes, with an immediate readback for writes.
use crate::{
    Result,
    error::ProtocolError,
    ff80::{
        Session,
        commands::configuration::{
            ConfigurationLocation, ConfigurationView, ReadConfiguration, WriteConfiguration,
        },
    },
};
use std::num::NonZeroU32;

fn location(view: ConfigurationView, offset: u32) -> ConfigurationLocation {
    ConfigurationLocation {
        view,
        page: 0,
        offset,
    }
}
/// Read one byte at `offset` (page 0) of the selected configuration view.
pub async fn read_byte(
    session: &mut Session,
    view: ConfigurationView,
    offset: u32,
    limit: NonZeroU32,
) -> Result<u8> {
    let command = ReadConfiguration::new(location(view, offset), NonZeroU32::MIN, limit)?;
    let bytes = session.read_configuration(command).await?;
    bytes
        .first()
        .copied()
        .ok_or_else(|| ProtocolError::ExpectationMismatch.into())
}
/// Write one byte, then read it back; a different value is an error. Which
/// offsets and values may be written is the caller's policy.
pub async fn write_byte_verified(
    session: &mut Session,
    view: ConfigurationView,
    offset: u32,
    value: u8,
    limit: NonZeroU32,
) -> Result<()> {
    let command = WriteConfiguration::new(location(view, offset), vec![value], limit)?;
    session.write_configuration(command).await?;
    if read_byte(session, view, offset, limit).await? != value {
        return Err(ProtocolError::ExpectationMismatch.into());
    }
    Ok(())
}
