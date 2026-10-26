//! Direct configuration commands. Storage reads and RAM writes are explicit.
use super::{memory_request, parameter_reply, status, word};
use crate::{
    Error, Result,
    error::{ProtocolError, check_limit},
};
use std::num::NonZeroU32;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum ConfigurationView {
    Storage = 0,
    Ram = 1,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConfigurationLocation {
    pub view: ConfigurationView,
    pub page: u32,
    pub offset: u32,
}
impl ConfigurationLocation {
    pub(crate) fn parameters(self, length: u32) -> Result<[u8; 16]> {
        self.offset
            .checked_add(length)
            .ok_or(Error::AddressOverflow)?;
        let mut bytes = [0; 16];
        for (chunk, value) in bytes.as_chunks_mut::<4>().0.iter_mut().zip([
            self.view as u32,
            self.page,
            self.offset,
            length,
        ]) {
            chunk.copy_from_slice(&value.to_le_bytes());
        }
        Ok(bytes)
    }
}

#[derive(Debug)]
pub struct ConfigurationGeometry {
    pub capacity: u32,
    pub page_bytes: NonZeroU32,
    pub raw: Vec<u8>,
}
impl ConfigurationView {
    pub(crate) fn encode_geometry(self) -> Vec<u8> {
        let mut params = [0; 16];
        params[..4].copy_from_slice(&(self as u32).to_le_bytes());
        memory_request(0x1a0002, 1, &params, &[])
    }
    pub(crate) fn decode_geometry(self, response: Vec<u8>) -> Result<ConfigurationGeometry> {
        if response.len() < 20 {
            return Err(ProtocolError::ReplyLength {
                expected: 20,
                actual: response.len(),
            }
            .into());
        }
        if word(&response, 16) != 1 {
            return Err(ProtocolError::FunctionEcho {
                expected: 1,
                actual: word(&response, 16),
            }
            .into());
        }
        status(&response, 0x1a0002, 20)?;
        if response.len() < 36 {
            return Err(ProtocolError::ReplyLength {
                expected: 36,
                actual: response.len(),
            }
            .into());
        }
        if word(&response, 20) != self as u32 || word(&response, 24) != 0 {
            return Err(ProtocolError::ParameterEcho.into());
        }
        let page_bytes =
            NonZeroU32::new(word(&response, 32)).ok_or(ProtocolError::ZeroPacketLimit)?;
        Ok(ConfigurationGeometry {
            capacity: word(&response, 28),
            page_bytes,
            raw: response,
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ReadConfiguration {
    pub(crate) params: [u8; 16],
    pub(crate) length: NonZeroU32,
}
impl ReadConfiguration {
    pub fn new(
        location: ConfigurationLocation,
        length: NonZeroU32,
        limit: NonZeroU32,
    ) -> Result<Self> {
        check_limit(length.get(), limit.get())?;
        Ok(Self {
            params: location.parameters(length.get())?,
            length,
        })
    }
    pub fn encode(self) -> Vec<u8> {
        memory_request(0x1a0002, 2, &self.params, &[])
    }
    pub(crate) fn decode(self, bytes: Vec<u8>) -> Result<Vec<u8>> {
        parameter_reply(bytes, 0x1a0002, 2, &self.params, self.length.get() as usize)
    }
}

#[derive(Debug)]
pub struct WriteConfiguration {
    params: [u8; 16],
    data: Vec<u8>,
}
impl WriteConfiguration {
    /// Sends one configuration write. Persistence depends on the selected view
    /// and firmware; no separate save command is issued.
    pub fn new(location: ConfigurationLocation, data: Vec<u8>, limit: NonZeroU32) -> Result<Self> {
        let length = u32::try_from(data.len())
            .ok()
            .and_then(NonZeroU32::new)
            .ok_or(Error::InvalidLength)?;
        check_limit(length.get(), limit.get())?;
        Ok(Self {
            params: location.parameters(length.get())?,
            data,
        })
    }
    pub fn encode(&self) -> Vec<u8> {
        memory_request(0x1a0002, 3, &self.params, &self.data)
    }
    pub(crate) fn decode(&self, bytes: Vec<u8>) -> Result<()> {
        parameter_reply(bytes, 0x1a0002, 3, &self.params, 0).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn nz(n: u32) -> NonZeroU32 {
        NonZeroU32::new(n).unwrap()
    }
    #[test]
    fn reads_distinguish_storage_from_ram_and_check_echoes() {
        for view in [ConfigurationView::Storage, ConfigurationView::Ram] {
            let command = ReadConfiguration::new(
                ConfigurationLocation {
                    view,
                    page: 3,
                    offset: 8,
                },
                nz(2),
                nz(256),
            )
            .unwrap();
            let encoded = command.encode();
            assert_eq!(word(&encoded, 0), 0x1a0002);
            assert_eq!(word(&encoded, 20), view as u32);
            assert_eq!(word(&encoded, 24), 3);
            let mut response = encoded.clone();
            response.extend_from_slice(&[9, 10]);
            assert_eq!(command.decode(response.clone()).unwrap(), [9, 10]);
            response[24] = 4;
            assert!(command.decode(response).is_err());
        }
    }
    #[test]
    fn geometry_keeps_unknown_tail_without_imposing_one_body_limit() {
        let mut response = ConfigurationView::Ram.encode_geometry();
        response[28..32].copy_from_slice(&0x420000u32.to_le_bytes());
        response[32..36].copy_from_slice(&512u32.to_le_bytes());
        response.extend_from_slice(&[0x42; 12]);
        let info = ConfigurationView::Ram.decode_geometry(response).unwrap();
        assert_eq!(info.capacity, 0x420000);
        assert_eq!(info.page_bytes.get(), 512);
        assert_eq!(info.raw.len(), 48);
    }
    #[test]
    fn writes_are_single_commands_with_bounded_data() {
        let location = ConfigurationLocation {
            view: ConfigurationView::Ram,
            page: 0,
            offset: 0xf7,
        };
        let write = WriteConfiguration::new(location, vec![1], nz(256)).unwrap();
        assert_eq!(write.encode()[36..], [1]);
        assert!(WriteConfiguration::new(location, vec![0; 257], nz(256)).is_err());
        assert!(WriteConfiguration::new(location, vec![], nz(256)).is_err());
    }
}
