use super::{memory_request, parameter_reply};
use crate::{Error, Result, error::check_limit};
use std::num::NonZeroU32;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Address(pub u32);

/// Nonempty CPU address range; constructed once and checked for overflow.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MemoryRange {
    address: Address,
    length: NonZeroU32,
}
impl MemoryRange {
    pub fn new(address: Address, length: NonZeroU32) -> Result<Self> {
        if u64::from(address.0) + u64::from(length.get()) > (1u64 << 32) {
            return Err(Error::AddressOverflow);
        }
        Ok(Self { address, length })
    }
    pub fn address(self) -> Address {
        self.address
    }
    pub fn length(self) -> NonZeroU32 {
        self.length
    }
}

/// Limits from the selected firmware definition, not universal Fuji constants.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DebugMemoryLimits {
    pub read: NonZeroU32,
    pub write: NonZeroU32,
}

#[derive(Clone, Copy, Debug)]
pub struct ReadMemory {
    range: MemoryRange,
}
impl ReadMemory {
    pub fn new(range: MemoryRange, limits: DebugMemoryLimits) -> Result<Self> {
        check_limit(range.length.get(), limits.read.get())?;
        Ok(Self { range })
    }
    pub fn range(self) -> MemoryRange {
        self.range
    }
    pub(crate) fn parameters(self) -> [u8; 16] {
        parameters(self.range)
    }
    pub(crate) fn encode(self) -> Vec<u8> {
        memory_request(0x200001, 2, &self.parameters(), &[])
    }
    pub(crate) fn decode(self, response: Vec<u8>) -> Result<Vec<u8>> {
        parameter_reply(
            response,
            0x200001,
            2,
            &self.parameters(),
            self.range.length.get() as usize,
        )
    }
}

#[derive(Debug)]
pub struct WriteMemory {
    range: MemoryRange,
    data: Vec<u8>,
}
impl WriteMemory {
    pub fn new(address: Address, data: Vec<u8>, limits: DebugMemoryLimits) -> Result<Self> {
        let length = u32::try_from(data.len())
            .ok()
            .and_then(NonZeroU32::new)
            .ok_or(Error::InvalidLength)?;
        check_limit(length.get(), limits.write.get())?;
        Ok(Self {
            range: MemoryRange::new(address, length)?,
            data,
        })
    }
    pub fn range(&self) -> MemoryRange {
        self.range
    }
    pub(crate) fn encode(&self) -> Vec<u8> {
        memory_request(0x200001, 3, &parameters(self.range), &self.data)
    }
    pub(crate) fn decode(&self, response: Vec<u8>) -> Result<()> {
        parameter_reply(response, 0x200001, 3, &parameters(self.range), 0).map(|_| ())
    }
}

fn parameters(range: MemoryRange) -> [u8; 16] {
    let mut parameters = [0; 16];
    parameters[8..12].copy_from_slice(&range.address.0.to_le_bytes());
    parameters[12..].copy_from_slice(&range.length.get().to_le_bytes());
    parameters
}

#[cfg(test)]
mod tests {
    use super::*;
    fn nz(n: u32) -> NonZeroU32 {
        NonZeroU32::new(n).unwrap()
    }
    fn limits() -> DebugMemoryLimits {
        DebugMemoryLimits {
            read: nz(256),
            write: nz(128),
        }
    }

    #[test]
    fn address_space_end_is_inclusive() {
        assert!(MemoryRange::new(Address(u32::MAX), nz(1)).is_ok());
        assert!(matches!(
            MemoryRange::new(Address(u32::MAX), nz(2)),
            Err(Error::AddressOverflow)
        ));
    }
    #[test]
    fn direct_commands_reject_oversize_instead_of_splitting() {
        assert!(ReadMemory::new(MemoryRange::new(Address(0), nz(257)).unwrap(), limits()).is_err());
        assert!(WriteMemory::new(Address(0), vec![0; 129], limits()).is_err());
        assert!(WriteMemory::new(Address(0), vec![], limits()).is_err());
    }
    #[test]
    fn memory_packet_matches_python_wire_layout() {
        let request = ReadMemory::new(
            MemoryRange::new(Address(0x12345678), nz(4)).unwrap(),
            limits(),
        )
        .unwrap();
        assert_eq!(
            request.encode(),
            vec![
                1, 0, 0x20, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 0x78, 0x56, 0x34, 0x12, 4, 0, 0, 0
            ]
        );
        let write = WriteMemory::new(Address(0x12345678), vec![9, 8, 7, 6], limits()).unwrap();
        let mut expected = request.encode();
        expected[16] = 3;
        expected.extend_from_slice(&[9, 8, 7, 6]);
        assert_eq!(write.encode(), expected);
    }
    #[test]
    fn malformed_acknowledgments_are_rejected() {
        let read = ReadMemory::new(MemoryRange::new(Address(3), nz(1)).unwrap(), limits()).unwrap();
        let mut response = read.encode();
        response.push(99);
        assert_eq!(read.decode(response.clone()).unwrap(), [99]);
        response[28] ^= 1;
        assert!(read.decode(response).is_err());
    }
}
