//! One device-9 bus exchange, carried by the service command. The two prefix
//! bytes and up to 29 data bytes are firmware fields, not host retry policy.
use super::{ArgumentKind, CommandError};
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Device9Count(u8);
impl Device9Count {
    pub fn new(value: u8) -> Result<Self, CommandError> {
        if value <= 29 {
            Ok(Self(value))
        } else {
            Err(CommandError::Argument(ArgumentKind::Device9Count))
        }
    }
    pub fn value(self) -> u8 {
        self.0
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Device9Payload {
    bytes: [u8; 29],
    length: u8,
}
impl Device9Payload {
    pub fn new(data: &[u8]) -> Result<Self, CommandError> {
        if data.len() > 29 {
            return Err(CommandError::Argument(ArgumentKind::Device9Count));
        }
        let mut bytes = [0; 29];
        bytes[..data.len()].copy_from_slice(data);
        Ok(Self {
            bytes,
            length: data.len() as u8,
        })
    }
    pub fn data(&self) -> &[u8] {
        &self.bytes[..usize::from(self.length)]
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Device9Action {
    Read {
        first: u8,
        second: u8,
        count: Device9Count,
    },
    Write {
        first: u8,
        second: u8,
        payload: Device9Payload,
    },
}
impl Device9Action {
    pub(super) fn encode(self, put: &mut impl FnMut(usize, u8)) {
        match self {
            Self::Read {
                first,
                second,
                count,
            } => {
                put(0, 0);
                put(1, first);
                put(2, second);
                put(3, count.value());
                for i in 0..usize::from(count.value()) {
                    put(4 + i, 255);
                }
            }
            Self::Write {
                first,
                second,
                payload,
            } => {
                put(0, 1);
                put(1, first);
                put(2, second);
                put(3, payload.length);
                for (i, &byte) in payload.data().iter().enumerate() {
                    put(4 + i, byte);
                }
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{Catalog, Command, ReplyValue};
    use fujimod_catalog_firmware_identity::FirmwareIdentity;
    use fujimod_catalog_service::{Definition, ServiceModes};
    #[test]
    fn exchanges_keep_twenty_ninth_byte_and_reject_thirty_before_io() {
        assert!(Device9Count::new(30).is_err());
        assert!(Device9Payload::new(&[0; 30]).is_err());
        for reverse in [false, true] {
            let slots: Vec<_> = (0..33).map(|i| if reverse { 32 - i } else { i }).collect();
            let doc = serde_json::json!({"schema":1,"firmware":{"id":"device9","model":"Synthetic","version":"1","sha256":"ab".repeat(32)},"mailbox":{"page":3,"offset":128,"configuration_limit":512},"entries":[{"code":229,"codec":"device9_exchange","label":"device9","slots":slots,"enabled_modes":4,"next_mode":0,"firmware_kind":1,"handler":0,"parameter_table":0}]});
            let c = Catalog::new(
                Definition::from_json(
                    &doc.to_string(),
                    &FirmwareIdentity::new("device9", &"ab".repeat(32)).unwrap(),
                )
                .unwrap(),
            );
            for count in [0, 1, 28, 29] {
                let p = c
                    .prepare(
                        ServiceModes::new(4).unwrap(),
                        Command::Device9 {
                            action: Device9Action::Read {
                                first: 0x12,
                                second: 0x34,
                                count: Device9Count::new(count).unwrap(),
                            },
                        },
                    )
                    .unwrap();
                assert_eq!(p.request().encode().len(), 34);
                let mut raw = p.request().encode();
                for i in 0..usize::from(count) {
                    raw[1 + slots[4 + i]] = i as u8;
                }
                let reply = p.decode_bytes(&raw).unwrap();
                assert_eq!(
                    reply.value(),
                    &ReplyValue::Device9Data {
                        data: (0..count).collect()
                    }
                );
                assert!(p.decode_bytes(&raw[..33]).is_err());
                let payload = Device9Payload::new(&(0..count).collect::<Vec<_>>()).unwrap();
                let p = c
                    .prepare(
                        ServiceModes::new(4).unwrap(),
                        Command::Device9 {
                            action: Device9Action::Write {
                                first: 1,
                                second: 2,
                                payload,
                            },
                        },
                    )
                    .unwrap();
                assert_eq!(p.request().arguments()[slots[3]], count);
                for i in 0..usize::from(count) {
                    assert_eq!(p.request().arguments()[slots[4 + i]], i as u8);
                }
                assert_eq!(
                    p.decode_bytes(&p.request().encode()).unwrap().value(),
                    &ReplyValue::NoPayload
                );
            }
        }
    }
}
