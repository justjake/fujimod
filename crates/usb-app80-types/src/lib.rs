//! App80 command and value types independent of the USB carrier.

/// A firmware service-table identifier. Zero is a valid identifier on some bodies.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct CommandCode(pub u8);

/// A firmware address value carried by an App80 command.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Address(pub u32);

/// A firmware request, before choosing how to carry it to the camera.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Request {
    code: CommandCode,
    arguments: Vec<u8>,
}
impl Request {
    /// Unused argument bytes are zeroed; oversize input is rejected, never truncated.
    pub fn new(code: CommandCode, arguments: &[u8]) -> Result<Self, RequestError> {
        if arguments.len() > 32 {
            return Err(RequestError::TooManyArguments(arguments.len()));
        }
        let mut padded = [0; 32];
        padded[..arguments.len()].copy_from_slice(arguments);
        Ok(Self::from_codec(code, padded.to_vec()))
    }
    /// Build the arguments for a reviewed firmware codec with its declared length.
    pub fn from_codec(code: CommandCode, arguments: Vec<u8>) -> Self {
        Self { code, arguments }
    }
    pub fn code(&self) -> CommandCode {
        self.code
    }
    pub fn arguments(&self) -> &[u8] {
        &self.arguments
    }
    pub fn encoded_len(&self) -> usize {
        1 + self.arguments.len()
    }
    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = vec![0; 1 + self.arguments.len()];
        bytes[0] = self.code.0;
        bytes[1..].copy_from_slice(&self.arguments);
        bytes
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum RequestError {
    #[error("App80 request has {0} argument bytes; the limit is 32")]
    TooManyArguments(usize),
}

/// Exact mailbox bytes. Command-specific results are decoded by firmware APIs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Reply {
    raw: Vec<u8>,
}
impl Reply {
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        Self::decode_length(bytes, 33)
    }
    pub fn decode_length(bytes: &[u8], expected: usize) -> Result<Self, DecodeError> {
        if bytes.len() != expected {
            return Err(DecodeError::ReplyLength {
                expected,
                actual: bytes.len(),
            });
        }
        Ok(Self {
            raw: bytes.to_vec(),
        })
    }
    pub fn raw(&self) -> &[u8] {
        &self.raw
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum DecodeError {
    #[error("reply has {actual} bytes; expected {expected}")]
    ReplyLength { expected: usize, actual: usize },
    #[error("reply command echo is {actual}; expected {expected}")]
    CommandEcho { expected: u8, actual: u8 },
}

pub mod catalog;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_keep_full_command_byte_and_pad_arguments() {
        let request = Request::new(CommandCode(0), &[1, 2]).unwrap();
        assert_eq!(request.code(), CommandCode(0));
        assert_eq!(&request.encode()[..4], &[0, 1, 2, 0]);
        assert!(request.encode()[3..].iter().all(|&b| b == 0));
        assert_eq!(
            Request::new(CommandCode(255), &[42; 32]).unwrap().encode()[0],
            255
        );
        assert!(Request::new(CommandCode(1), &[0; 33]).is_err());
    }

    #[test]
    fn reply_is_an_exact_opaque_mailbox_not_a_pending_status() {
        for first in [0, 1, 0x80, 0xff] {
            let mut bytes = [42; 33];
            bytes[0] = first;
            assert_eq!(Reply::decode(&bytes).unwrap().raw(), &bytes);
        }
        assert!(Reply::decode(&[0; 32]).is_err());
        assert!(Reply::decode(&[0; 34]).is_err());
    }
}
