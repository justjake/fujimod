//! Direct debug log, event and console commands. No hidden polling.
use super::{memory_request, parameter_reply, status, word};
use crate::{
    Error, Result,
    error::{ProtocolError, check_limit},
};
use std::num::NonZeroU32;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EventName(String);
impl EventName {
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        if value.is_empty() || !value.bytes().all(|b| b.is_ascii_graphic()) {
            return Err(Error::InvalidToken);
        }
        Ok(Self(value))
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyState {
    On,
    Off,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CameraMode {
    Record,
    Playback,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DebugEvent {
    Key {
        name: EventName,
        state: KeyState,
    },
    Mode(CameraMode),
    /// Stock CMD event is distinct from submitting a console command.
    Command(String),
}
impl DebugEvent {
    fn data(&self) -> Result<Vec<u8>> {
        let text = match self {
            Self::Key { name, state } => format!(
                "{} {}\n",
                name.0,
                match state {
                    KeyState::On => "ON",
                    KeyState::Off => "OFF",
                }
            ),
            Self::Mode(mode) => format!(
                "MODE {}\n",
                match mode {
                    CameraMode::Record => "REC",
                    CameraMode::Playback => "PLAY",
                }
            ),
            Self::Command(argument) => {
                if argument.is_empty()
                    || !argument.is_ascii()
                    || argument.contains(['\0', '\r', '\n'])
                {
                    return Err(Error::InvalidToken);
                }
                format!("CMD {argument}\n")
            }
        };
        Ok(text.into_bytes())
    }
    pub(crate) fn encode(&self, limit: NonZeroU32) -> Result<(Vec<u8>, [u8; 16])> {
        let data = self.data()?;
        let length = u32::try_from(data.len()).map_err(|_| Error::InvalidLength)?;
        check_limit(length, limit.get())?;
        let mut parameters = [0; 16];
        parameters[12..].copy_from_slice(&length.to_le_bytes());
        Ok((memory_request(0x200001, 5, &parameters, &data), parameters))
    }
    pub(crate) fn decode(bytes: Vec<u8>, parameters: &[u8; 16]) -> Result<Vec<u8>> {
        parameter_reply(bytes, 0x200001, 5, parameters, 0)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConsoleCommand(Vec<u8>);
impl ConsoleCommand {
    /// The caller supplies delimiters intentionally; no newline is appended.
    pub fn new(bytes: Vec<u8>, limit: NonZeroU32) -> Result<Self> {
        let length = u32::try_from(bytes.len())
            .ok()
            .and_then(NonZeroU32::new)
            .ok_or(Error::InvalidLength)?;
        check_limit(length.get(), limit.get())?;
        if !bytes.is_ascii() || bytes.contains(&0) {
            return Err(Error::InvalidToken);
        }
        Ok(Self(bytes))
    }
    pub(crate) fn encode(&self) -> Vec<u8> {
        let mut bytes = vec![0; 24];
        bytes[..4].copy_from_slice(&0x200001u32.to_le_bytes());
        bytes[16..20].copy_from_slice(&6u32.to_le_bytes());
        bytes[20..24].copy_from_slice(&(self.0.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&self.0);
        bytes.extend_from_slice(&[0; 4]);
        bytes
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConsoleProgress {
    Complete,
    Pending,
    Unknown(u32),
}
#[derive(Debug)]
pub struct ConsoleReply {
    pub progress: ConsoleProgress,
    pub status: u32,
    pub output: Vec<u8>,
    pub raw: Vec<u8>,
}

pub(crate) fn console_poll(capacity: NonZeroU32) -> Vec<u8> {
    let mut bytes = vec![0; 32];
    bytes[..4].copy_from_slice(&0x200001u32.to_le_bytes());
    bytes[16..20].copy_from_slice(&7u32.to_le_bytes());
    bytes[20..24].copy_from_slice(&capacity.get().to_le_bytes());
    bytes
}
pub(crate) fn console_header(bytes: &[u8], function: u32) -> Result<()> {
    if bytes.len() >= 20 && word(bytes, 16) != function {
        return Err(ProtocolError::FunctionEcho {
            expected: function,
            actual: word(bytes, 16),
        }
        .into());
    }
    status(bytes, 0x200001, 20)
}
pub(crate) fn decode_console(bytes: Vec<u8>, capacity: NonZeroU32) -> Result<ConsoleReply> {
    console_header(&bytes, 7)?;
    if bytes.len() < 32 {
        return Err(ProtocolError::ReplyLength {
            expected: 32,
            actual: bytes.len(),
        }
        .into());
    }
    let size = word(&bytes, 28) as usize;
    if size > bytes.len() - 32 || size > capacity.get() as usize {
        return Err(ProtocolError::DataLength {
            available: (bytes.len() - 32).min(capacity.get() as usize),
            declared: size,
        }
        .into());
    }
    let progress = match word(&bytes, 20) {
        0 => ConsoleProgress::Complete,
        1 => ConsoleProgress::Pending,
        n => ConsoleProgress::Unknown(n),
    };
    Ok(ConsoleReply {
        progress,
        status: word(&bytes, 24),
        output: bytes[32..32 + size].to_vec(),
        raw: bytes,
    })
}

#[derive(Debug)]
pub struct LogReply {
    pub status: u32,
    pub data: Vec<u8>,
    pub raw: Vec<u8>,
}
pub(crate) fn log_request(capacity: NonZeroU32) -> Vec<u8> {
    buffered_read_request(1, capacity)
}
pub(crate) fn decode_log(bytes: Vec<u8>, capacity: NonZeroU32) -> Result<LogReply> {
    decode_buffered_read(bytes, capacity, 1)
}

/// Memory operation 4 reads the secondary buffered debug source. It shares the
/// operation-1 request and reply shape (a single capacity parameter and a
/// status/size header); only the operation code differs. Exposed so callers
/// that must address operation 4 directly have a first-class command for it
/// rather than a raw `research` request.
pub(crate) fn drain_request(capacity: NonZeroU32) -> Vec<u8> {
    buffered_read_request(4, capacity)
}
pub(crate) fn decode_drain(bytes: Vec<u8>, capacity: NonZeroU32) -> Result<LogReply> {
    decode_buffered_read(bytes, capacity, 4)
}

fn buffered_read_request(function: u32, capacity: NonZeroU32) -> Vec<u8> {
    let mut parameters = [0; 16];
    parameters[..4].copy_from_slice(&capacity.get().to_le_bytes());
    memory_request(0x200001, function, &parameters, &[])
}
fn decode_buffered_read(bytes: Vec<u8>, capacity: NonZeroU32, function: u32) -> Result<LogReply> {
    console_header(&bytes, function)?;
    if bytes.len() < 28 {
        return Err(ProtocolError::ReplyLength {
            expected: 28,
            actual: bytes.len(),
        }
        .into());
    }
    let size = word(&bytes, 24) as usize;
    if size > bytes.len() - 28 || size > capacity.get() as usize {
        return Err(ProtocolError::DataLength {
            available: (bytes.len() - 28).min(capacity.get() as usize),
            declared: size,
        }
        .into());
    }
    Ok(LogReply {
        status: word(&bytes, 20),
        data: bytes[28..28 + size].to_vec(),
        raw: bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn event_names_cannot_inject_second_commands() {
        assert!(EventName::new("S1 ON\nMODE REC").is_err());
        let event = DebugEvent::Key {
            name: EventName::new("S1").unwrap(),
            state: KeyState::On,
        };
        let (bytes, _) = event.encode(NonZeroU32::new(256).unwrap()).unwrap();
        assert_eq!(&bytes[36..], b"S1 ON\n");
    }
    #[test]
    fn drain_operation_four_mirrors_the_log_wire_shape_with_its_own_code() {
        let capacity = NonZeroU32::new(248).unwrap();
        let request = drain_request(capacity);
        // Command 0x200001, operation code 4 at +0x10, capacity at +0x14.
        assert_eq!(word(&request, 0), 0x200001);
        assert_eq!(word(&request, 16), 4);
        assert_eq!(word(&request, 20), 248);
        // Identical to the operation-1 log request apart from the operation code.
        let mut as_log = request.clone();
        as_log[16..20].copy_from_slice(&1u32.to_le_bytes());
        assert_eq!(as_log, log_request(capacity));
    }
    #[test]
    fn drain_decode_requires_operation_four_echo_and_bounds_the_payload() {
        let capacity = NonZeroU32::new(8).unwrap();
        // Reply header: command echo, operation-4 echo, zero status, size at +24.
        let mut reply = vec![0u8; 28];
        reply[..4].copy_from_slice(&0x200001u32.to_le_bytes());
        reply[16..20].copy_from_slice(&4u32.to_le_bytes());
        reply[24..28].copy_from_slice(&3u32.to_le_bytes());
        reply.extend_from_slice(b"abc");
        let decoded = decode_drain(reply.clone(), capacity).unwrap();
        assert_eq!(decoded.data, b"abc");
        assert_eq!(decoded.status, 0);
        // An operation-1 log decode rejects a reply that echoes operation 4.
        assert!(decode_log(reply.clone(), capacity).is_err());
        // A declared size past the payload is rejected.
        reply[24..28].copy_from_slice(&9u32.to_le_bytes());
        assert!(decode_drain(reply, capacity).is_err());
    }
    #[test]
    fn console_unknown_progress_is_retained_and_length_checked() {
        let capacity = NonZeroU32::new(10).unwrap();
        let mut bytes = console_poll(capacity);
        bytes[20..24].copy_from_slice(&42u32.to_le_bytes());
        bytes[28..32].copy_from_slice(&3u32.to_le_bytes());
        bytes.extend_from_slice(b"abc");
        assert_eq!(
            decode_console(bytes.clone(), capacity).unwrap().progress,
            ConsoleProgress::Unknown(42)
        );
        bytes[28..32].copy_from_slice(&11u32.to_le_bytes());
        assert!(decode_console(bytes, capacity).is_err());
    }
}
