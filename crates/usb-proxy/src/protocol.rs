//! Versioned, camera-independent binary messages with opaque USB payloads.
//!
//! Every data-path message is one postcard value (see PIPELINE.md): unsigned
//! varints, length-prefixed strings and byte strings, one-varint enum tags.
//! Variant and field order is the wire format; append, never reorder.
use serde::{Deserialize, Serialize};

pub const VERSION: u32 = 5;
pub const MAX_TRANSFER: usize = 8 * 1024 * 1024;
/// One message: a whole transfer plus its framing and text fields.
pub const MAX_MESSAGE: usize = MAX_TRANSFER + 65536;
/// Connect and disconnect reasons are shown to the device owner; longer text is cut.
pub const MAX_REASON: usize = 512;
/// A controller keeps at most this many operations, and this many payload
/// bytes (OUT data plus requested IN lengths), in flight. A single larger
/// operation may still run alone.
pub const MAX_IN_FLIGHT: usize = 4096;
pub const MAX_IN_FLIGHT_BYTES: usize = 1024 * 1024;

/// Cut a reason to MAX_REASON bytes on a character boundary.
pub fn bounded_reason(text: &str) -> String {
    let mut end = text.len().min(MAX_REASON);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Device {
    /// Relay route: exporter connection ID / browser-local device ID.
    pub id: String,
    pub vendor_id: u16,
    pub product_id: u16,
    pub manufacturer: Option<String>,
    pub product: Option<String>,
    pub serial: Option<String>,
    pub interfaces: Vec<Interface>,
    /// Shared devices only: whether the camera is present, and its last battery report.
    #[serde(default)]
    pub status: Option<DeviceStatus>,
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DeviceStatus {
    pub online: bool,
    /// Why the device is offline: "battery", "unplugged", "not responding".
    #[serde(default)]
    pub reason: Option<String>,
    /// Last battery level a controller reported, in the firmware's own names.
    #[serde(default)]
    pub battery: Option<String>,
    /// Milliseconds since that report.
    #[serde(default)]
    pub battery_age_ms: Option<u64>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Interface {
    pub configuration: u8,
    pub interface: u8,
    pub alternate: u8,
    pub endpoints: Vec<Endpoint>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Endpoint {
    pub address: u8,
    pub kind: String,
    pub packet_size: u16,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestType {
    Standard,
    Class,
    Vendor,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Recipient {
    Device,
    Interface,
    Endpoint,
    Other,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Setup {
    pub request_type: RequestType,
    pub recipient: Recipient,
    pub request: u8,
    pub value: u16,
    pub index: u16,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    /// Echo at the exporter without accessing or claiming USB.
    Probe,
    List,
    /// A neutral progress line for the device owner's log; never touches USB.
    OwnerStatus {
        message: String,
    },
    /// Owner banner and spoken ten-second countdown; completes only after it ends.
    OwnerAttention {
        message: String,
    },
    Open {
        configuration: u8,
        interface: u8,
        alternate: u8,
    },
    Close,
    ControlIn {
        setup: Setup,
        length: usize,
    },
    ControlOut {
        setup: Setup,
        #[serde(with = "serde_bytes")]
        data: Vec<u8>,
    },
    TransferIn {
        endpoint: u8,
        length: usize,
    },
    TransferOut {
        endpoint: u8,
        #[serde(with = "serde_bytes")]
        data: Vec<u8>,
    },
    ClearHalt {
        endpoint: u8,
    },
}
impl Operation {
    /// Payload bytes this operation moves: OUT data or the requested IN length.
    pub fn payload(&self) -> usize {
        match self {
            Self::ControlOut { data, .. } | Self::TransferOut { data, .. } => data.len(),
            Self::ControlIn { length, .. } | Self::TransferIn { length, .. } => *length,
            _ => 0,
        }
    }
    pub fn is_in(&self) -> bool {
        matches!(self, Self::ControlIn { .. } | Self::TransferIn { .. })
    }
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Completion {
    pub devices: Vec<Device>,
    #[serde(with = "serde_bytes")]
    pub data: Vec<u8>,
    pub transferred: usize,
    /// WebUSB status or DOMException name; partial bytes/counts remain available.
    pub error: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Message {
    Hello {
        version: u32,
        role: String,
        token: String,
        /// Why this controller connected; native tools send their command line.
        reason: Option<String>,
    },
    Ready {
        version: u32,
        id: String,
    },
    /// A controller's last message before it disconnects on purpose.
    Goodbye {
        reason: String,
    },
    /// Controller to owner, in pipeline order: a battery reading the owner
    /// cannot take itself while the camera is shared.
    Status {
        battery: String,
    },
    /// Controller to owner: a problem only the controller can see (for
    /// example, the camera stopped answering); the owner alerts its user.
    Attention {
        message: String,
    },
    /// Controller to owner: what the tool is doing now, shown above the
    /// owner's log footer until replaced; an empty title clears it.
    Activity {
        title: String,
    },
    /// Controller to owner: a progress bar under the owner's log, `total`
    /// units long, until ProgressDone. Shown when the exporter reaches it in
    /// the pipeline, so it tracks USB work rather than submission.
    ProgressStart {
        label: String,
        total: u64,
    },
    ProgressUpdate {
        done: u64,
    },
    ProgressDone,
    Inventory {
        devices: Vec<Device>,
    },
    /// Run `op` after every earlier operation, unless the pipeline has failed.
    Execute {
        seq: u64,
        device: String,
        timeout_ms: u32,
        op: Operation,
    },
    /// Fail the pipeline unless bytes `offset..` of IN result `of` equal
    /// `bytes` (both ANDed with `mask` when present).
    Expect {
        seq: u64,
        of: u64,
        offset: u32,
        #[serde(with = "serde_bytes")]
        bytes: Vec<u8>,
        mask: Option<serde_bytes::ByteBuf>,
        length: Option<u32>,
        alternatives: Vec<serde_bytes::ByteBuf>,
    },
    /// Answered with Synced once every earlier operation has finished; ends
    /// a failed pipeline's discarding.
    Sync {
        seq: u64,
    },
    Result {
        seq: u64,
        completion: Completion,
    },
    /// The compared bytes as found (empty when `of` had no such range).
    ExpectFailed {
        seq: u64,
        #[serde(with = "serde_bytes")]
        actual: Vec<u8>,
    },
    /// `discarded`: operations skipped without touching USB since the failure.
    Synced {
        seq: u64,
        discarded: u64,
    },
    /// Relay to exporter and back: one controller's stream on a shared socket.
    Routed {
        stream: u64,
        message: Box<Message>,
    },
}

impl Message {
    pub fn encode(&self) -> crate::Result<Vec<u8>> {
        let bytes = postcard::to_allocvec(self).map_err(|e| crate::Error(e.to_string()))?;
        if bytes.len() > MAX_MESSAGE {
            return Err(crate::Error("message exceeds limit".into()));
        }
        Ok(bytes)
    }
    pub fn decode(bytes: &[u8]) -> crate::Result<Self> {
        let invalid = || crate::Error("invalid binary proxy message".into());
        if bytes.len() > MAX_MESSAGE {
            return Err(invalid());
        }
        match postcard::take_from_bytes::<Self>(bytes) {
            Ok((message, [])) => Ok(message),
            _ => Err(invalid()),
        }
    }
    /// Notices the owner shows; they carry no reply.
    pub fn is_notice(&self) -> bool {
        matches!(
            self,
            Self::Status { .. }
                | Self::Attention { .. }
                | Self::Activity { .. }
                | Self::ProgressStart { .. }
                | Self::ProgressUpdate { .. }
                | Self::ProgressDone
        )
    }
    /// Reject unsupported extents before a message can reach USB.
    pub fn validate(&self) -> crate::Result<()> {
        match self {
            Self::Execute { op, timeout_ms, .. } => {
                if !(1..=120000).contains(timeout_ms) {
                    return Err(crate::Error("timeout must be 1–120000 ms".into()));
                }
                op.validate()
            }
            Self::Expect {
                bytes,
                mask,
                length,
                alternatives,
                ..
            } => {
                if bytes.len() > MAX_TRANSFER
                    || mask.as_ref().is_some_and(|m| m.len() != bytes.len())
                {
                    return Err(crate::Error(
                        "an expectation's mask must match its bytes".into(),
                    ));
                }
                if length.is_some_and(|n| n as usize > MAX_TRANSFER) {
                    return Err(crate::Error(
                        "expectation length exceeds transfer limit".into(),
                    ));
                }
                if alternatives.len() > 8 || alternatives.iter().any(|a| a.len() != bytes.len()) {
                    return Err(crate::Error(
                        "expectation alternatives must have equal lengths (at most eight)".into(),
                    ));
                }
                Ok(())
            }
            Self::Routed { message, .. } if matches!(**message, Self::Routed { .. }) => {
                Err(crate::Error("nested route".into()))
            }
            Self::Routed { message, .. } => message.validate(),
            _ => Ok(()),
        }
    }
}

impl Operation {
    /// Reject unsupported extents before a request can reach USB.
    pub fn validate(&self) -> crate::Result<()> {
        let invalid = |message: &str| crate::Error(message.into());
        match self {
            Self::OwnerStatus { message } | Self::OwnerAttention { message } => {
                if message.is_empty() || message.len() > MAX_REASON {
                    return Err(invalid("owner message must be 1–512 UTF-8 bytes"));
                }
            }
            Self::ControlIn { length, .. } if *length > MAX_TRANSFER => {
                return Err(invalid("transfer exceeds 8 MiB"));
            }
            Self::ControlOut { data, .. } => validate_data(data)?,
            Self::TransferIn { endpoint, length } => {
                validate_endpoint(*endpoint)?;
                if endpoint & 128 == 0 || *length > MAX_TRANSFER {
                    return Err(invalid("invalid IN transfer"));
                }
            }
            Self::TransferOut { endpoint, data } => {
                validate_endpoint(*endpoint)?;
                if endpoint & 128 != 0 {
                    return Err(invalid("invalid OUT transfer"));
                }
                validate_data(data)?;
            }
            Self::ClearHalt { endpoint } => validate_endpoint(*endpoint)?,
            _ => (),
        }
        fn validate_endpoint(endpoint: u8) -> crate::Result<()> {
            if endpoint & 0x70 != 0 || endpoint & 15 == 0 {
                return Err(crate::Error("invalid endpoint address".into()));
            }
            Ok(())
        }
        fn validate_data(data: &[u8]) -> crate::Result<()> {
            if data.len() > MAX_TRANSFER {
                return Err(crate::Error("transfer exceeds 8 MiB".into()));
            }
            Ok(())
        }
        Ok(())
    }
}

/// Compare an expectation against an IN result, as every exporter must.
pub fn expectation_holds(data: &[u8], offset: usize, bytes: &[u8], mask: Option<&[u8]>) -> bool {
    let Some(actual) = offset
        .checked_add(bytes.len())
        .and_then(|end| data.get(offset..end))
    else {
        return false;
    };
    match mask {
        None => actual == bytes,
        Some(mask) => actual
            .iter()
            .zip(bytes)
            .zip(mask)
            .all(|((a, b), m)| a & m == b & m),
    }
}

/// Byte-exact vectors the TypeScript codec must reproduce (web/wire.test.ts
/// reads web/vectors.json; `vectors_are_current` keeps that file in step).
#[cfg(test)]
pub(crate) fn vectors() -> Vec<(&'static str, Message)> {
    let setup = Setup {
        request_type: RequestType::Vendor,
        recipient: Recipient::Other,
        request: 0,
        value: 0x1234,
        index: 1,
    };
    let device = Device {
        id: "1".into(),
        vendor_id: 0x04cb,
        product_id: 0xff80,
        manufacturer: Some("FUJIFILM".into()),
        product: None,
        serial: None,
        interfaces: vec![Interface {
            configuration: 1,
            interface: 0,
            alternate: 0,
            endpoints: vec![Endpoint {
                address: 0x81,
                kind: "bulk".into(),
                packet_size: 512,
            }],
        }],
        status: Some(DeviceStatus {
            online: true,
            reason: None,
            battery: Some("full".into()),
            battery_age_ms: Some(300),
        }),
    };
    let execute = |seq, op| Message::Execute {
        seq,
        device: "camera".into(),
        timeout_ms: 5000,
        op,
    };
    vec![
        (
            "hello",
            Message::Hello {
                version: VERSION,
                role: "controller".into(),
                token: "t".into(),
                reason: Some("install".into()),
            },
        ),
        (
            "hello_without_reason",
            Message::Hello {
                version: VERSION,
                role: "exporter".into(),
                token: "t".into(),
                reason: None,
            },
        ),
        (
            "ready",
            Message::Ready {
                version: VERSION,
                id: "7".into(),
            },
        ),
        (
            "goodbye",
            Message::Goodbye {
                reason: "done".into(),
            },
        ),
        (
            "status",
            Message::Status {
                battery: "full".into(),
            },
        ),
        (
            "attention",
            Message::Attention {
                message: "Camera stopped answering".into(),
            },
        ),
        (
            "activity",
            Message::Activity {
                title: "Uploading".into(),
            },
        ),
        (
            "progress_start",
            Message::ProgressStart {
                label: "Uploading".into(),
                total: 93056,
            },
        ),
        ("progress_update", Message::ProgressUpdate { done: 16384 }),
        ("progress_done", Message::ProgressDone),
        (
            "inventory",
            Message::Inventory {
                devices: vec![device.clone()],
            },
        ),
        ("probe", execute(1, Operation::Probe)),
        ("list", execute(2, Operation::List)),
        (
            "owner_status",
            execute(
                3,
                Operation::OwnerStatus {
                    message: "stopped".into(),
                },
            ),
        ),
        (
            "owner_attention",
            execute(
                4,
                Operation::OwnerAttention {
                    message: "Film it".into(),
                },
            ),
        ),
        (
            "open",
            execute(
                5,
                Operation::Open {
                    configuration: 1,
                    interface: 0,
                    alternate: 0,
                },
            ),
        ),
        ("close", execute(6, Operation::Close)),
        (
            "control_in",
            execute(
                7,
                Operation::ControlIn {
                    setup: setup.clone(),
                    length: 64,
                },
            ),
        ),
        (
            "control_out",
            execute(
                8,
                Operation::ControlOut {
                    setup,
                    data: vec![1, 2, 3],
                },
            ),
        ),
        (
            "transfer_in",
            execute(
                9,
                Operation::TransferIn {
                    endpoint: 0x82,
                    length: 16384,
                },
            ),
        ),
        (
            "transfer_out",
            execute(
                300,
                Operation::TransferOut {
                    endpoint: 1,
                    data: vec![0, 255],
                },
            ),
        ),
        (
            "clear_halt",
            execute(11, Operation::ClearHalt { endpoint: 0x82 }),
        ),
        (
            "expect",
            Message::Expect {
                seq: 12,
                of: 9,
                offset: 4,
                bytes: vec![0xaa, 0xbb],
                mask: None,
                length: Some(8),
                alternatives: vec![vec![0xcc, 0xdd].into()],
            },
        ),
        (
            "expect_masked",
            Message::Expect {
                seq: 13,
                of: 9,
                offset: 0,
                bytes: vec![0x10],
                mask: Some(serde_bytes::ByteBuf::from(vec![0xf0])),
                length: None,
                alternatives: vec![],
            },
        ),
        ("sync", Message::Sync { seq: 14 }),
        (
            "result",
            Message::Result {
                seq: 9,
                completion: Completion {
                    devices: vec![],
                    data: vec![9, 8, 7],
                    transferred: 3,
                    error: None,
                },
            },
        ),
        (
            "result_error",
            Message::Result {
                seq: 10,
                completion: Completion {
                    error: Some("stall".into()),
                    ..Completion::default()
                },
            },
        ),
        (
            "result_devices",
            Message::Result {
                seq: 2,
                completion: Completion {
                    devices: vec![device],
                    ..Completion::default()
                },
            },
        ),
        (
            "expect_failed",
            Message::ExpectFailed {
                seq: 12,
                actual: vec![0xaa, 0xbc],
            },
        ),
        (
            "synced",
            Message::Synced {
                seq: 14,
                discarded: 2,
            },
        ),
        (
            "routed",
            Message::Routed {
                stream: 3,
                message: Box::new(Message::Sync { seq: 1 }),
            },
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }
    #[test]
    fn every_message_round_trips() {
        for (name, message) in vectors() {
            message.validate().unwrap();
            let bytes = message.encode().unwrap();
            assert_eq!(Message::decode(&bytes).unwrap(), message, "{name}");
        }
    }
    #[test]
    fn vectors_are_current() {
        let mut text = String::from("{\n");
        let all = vectors();
        for (index, (name, message)) in all.iter().enumerate() {
            let comma = if index + 1 == all.len() { "" } else { "," };
            text += &format!(
                "  \"{name}\": \"{}\"{comma}\n",
                hex(&message.encode().unwrap())
            );
        }
        text += "}\n";
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/web/vectors.json");
        if std::env::var_os("UPDATE_VECTORS").is_some() {
            std::fs::write(path, &text).unwrap();
        }
        assert_eq!(
            std::fs::read_to_string(path).unwrap_or_default(),
            text,
            "rerun with UPDATE_VECTORS=1 and commit web/vectors.json"
        );
    }
    #[test]
    fn small_operations_are_a_few_bytes() {
        let read = Message::Execute {
            seq: 300,
            device: "camera".into(),
            timeout_ms: 5000,
            op: Operation::TransferIn {
                endpoint: 0x82,
                length: 16384,
            },
        };
        // tag, seq varint, device string, timeout varint, op tag, endpoint, length varint.
        assert_eq!(read.encode().unwrap().len(), 1 + 2 + 7 + 2 + 1 + 1 + 3);
        let data: Vec<u8> = (0..=255).collect();
        let write = Message::Execute {
            seq: 1,
            device: "camera".into(),
            timeout_ms: 5000,
            op: Operation::TransferOut {
                endpoint: 1,
                data: data.clone(),
            },
        };
        let bytes = write.encode().unwrap();
        assert!(bytes.ends_with(&data));
        assert_eq!(bytes.len(), 1 + 1 + 7 + 2 + 1 + 1 + 2 + 256);
    }
    #[test]
    fn owner_requests_bound_their_text() {
        for message in [String::new(), "é".repeat(257)] {
            assert!(
                Operation::OwnerStatus {
                    message: message.clone()
                }
                .validate()
                .is_err()
            );
            assert!(Operation::OwnerAttention { message }.validate().is_err());
        }
        let long = "é".repeat(MAX_REASON);
        let cut = bounded_reason(&long);
        assert!(cut.len() <= MAX_REASON && long.starts_with(&cut));
    }
    #[test]
    fn expectations_compare_bytes_with_optional_mask() {
        let data = [1, 2, 3, 4];
        assert!(expectation_holds(&data, 1, &[2, 3], None));
        assert!(!expectation_holds(&data, 1, &[2, 4], None));
        assert!(!expectation_holds(&data, 3, &[4, 5], None));
        assert!(!expectation_holds(&data, usize::MAX, &[1], None));
        assert!(expectation_holds(&data, 3, &[0x14], Some(&[0x0f])));
        assert!(!expectation_holds(&data, 3, &[0x14], Some(&[0xf0])));
        let bad = Message::Expect {
            seq: 1,
            of: 0,
            offset: 0,
            bytes: vec![1, 2],
            mask: Some(vec![1].into()),
            length: None,
            alternatives: vec![],
        };
        assert!(bad.validate().is_err());
    }
    #[test]
    fn malformed_or_oversized_messages_fail_before_execution() {
        let bytes = Message::Sync { seq: 1 }.encode().unwrap();
        assert!(Message::decode(&[]).is_err());
        assert!(Message::decode(&[bytes.as_slice(), &[0]].concat()).is_err());
        assert!(Message::decode(&[200]).is_err());
        // A version 3 JSON frame is not a message.
        assert!(Message::decode(b"FU\x03\0\x02\0\0\0{}").is_err());
        let huge = Message::Result {
            seq: 1,
            completion: Completion {
                data: vec![0; MAX_MESSAGE],
                ..Completion::default()
            },
        };
        assert!(huge.encode().is_err());
        let execute = |timeout_ms, op| Message::Execute {
            seq: 1,
            device: String::new(),
            timeout_ms,
            op,
        };
        assert!(execute(0, Operation::Probe).validate().is_err());
        assert!(execute(120001, Operation::Probe).validate().is_err());
        assert!(
            execute(
                10,
                Operation::TransferIn {
                    endpoint: 2,
                    length: 1
                }
            )
            .validate()
            .is_err()
        );
        assert!(
            execute(
                10,
                Operation::TransferOut {
                    endpoint: 0x82,
                    data: vec![]
                }
            )
            .validate()
            .is_err()
        );
        assert!(
            execute(10, Operation::ClearHalt { endpoint: 0x10 })
                .validate()
                .is_err()
        );
        let nested = Message::Routed {
            stream: 1,
            message: Box::new(Message::Routed {
                stream: 2,
                message: Box::new(Message::ProgressDone),
            }),
        };
        assert!(nested.validate().is_err());
    }
}
