//! Fuji's 12-byte vendor-control envelope. This is not USB packet framing.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum RequestKind {
    Reclaim = 0x1d,
    Open = 0xc0,
    Ping = 0xc2,
    Close = 0xc4,
    WriteCommand = 0xd0,
    ReadCommand = 0xd1,
    WriteData = 0xe0,
    ReadData = 0xe1,
    EmptyResponse = 0xe8,
}

pub(crate) fn envelope(kind: RequestKind, size: u32, session: u16) -> [u8; 12] {
    let mut bytes = [0; 12];
    bytes[0] = kind as u8;
    bytes[6..10].copy_from_slice(&size.to_be_bytes());
    bytes[10..12].copy_from_slice(&session.to_be_bytes());
    bytes
}

pub(crate) fn data_envelope(kind: RequestKind, offset: u32, size: u32, session: u16) -> [u8; 12] {
    let mut bytes = envelope(kind, size, session);
    bytes[2..6].copy_from_slice(&offset.to_be_bytes());
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixed_endian_session_envelope() {
        assert_eq!(
            envelope(RequestKind::Open, 8, 0),
            [0xc0, 0, 0, 0, 0, 0, 0, 0, 0, 8, 0, 0]
        );
        assert_eq!(
            envelope(RequestKind::ReadCommand, 0x12345678, 0x9abc),
            [0xd1, 0, 0, 0, 0, 0, 0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc]
        );
    }
}
