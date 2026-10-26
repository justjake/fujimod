//! Event-log records and effects returned by device models.
use std::{borrow::Cow, sync::Arc};

/// A structured scalar, sequence or mapping in an event-log record.
#[derive(Clone, Debug, serde::Deserialize)]
pub enum Value {
    None,
    Int(i128),
    Float(f64),
    Bool(bool),
    Str(String),
    /// A constant string (evidence, limits, provenance), without allocation.
    Static(Cow<'static, str>),
    List(Vec<Value>),
    Tuple(Vec<Value>),
    Bytes(Vec<u8>),
    /// Ordered dictionaries in declared endpoint data and transaction records.
    Dict(Vec<(Value, Value)>),
    /// A hexadecimal integer formatted only when a record is serialized.
    Hex(u128),
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::None, Self::None) => true,
            (Self::Int(a), Self::Int(b)) => a == b,
            (Self::Hex(a), Self::Hex(b)) => a == b,
            (Self::Hex(a), Self::Str(b)) | (Self::Str(b), Self::Hex(a)) => b == &format!("{a:#x}"),
            (Self::Float(a), Self::Float(b)) => a == b,
            (Self::Bool(a), Self::Bool(b)) => a == b,
            (Self::Str(a), Self::Str(b)) => a == b,
            (Self::Static(a), Self::Static(b)) => a == b,
            (Self::List(a), Self::List(b)) | (Self::Tuple(a), Self::Tuple(b)) => a == b,
            (Self::Bytes(a), Self::Bytes(b)) => a == b,
            (Self::Dict(a), Self::Dict(b)) => a == b,
            _ => false,
        }
    }
}

impl serde::Serialize for Value {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        match self {
            Self::None => serializer.serialize_unit_variant("Value", 0, "None"),
            Self::Int(value) => serializer.serialize_newtype_variant("Value", 1, "Int", value),
            Self::Float(value) => serializer.serialize_newtype_variant("Value", 2, "Float", value),
            Self::Bool(value) => serializer.serialize_newtype_variant("Value", 3, "Bool", value),
            Self::Str(value) => serializer.serialize_newtype_variant("Value", 4, "Str", value),
            Self::Hex(value) => {
                let value = format!("{value:#x}");
                serializer.serialize_newtype_variant("Value", 4, "Str", &value)
            }
            Self::Static(value) => {
                serializer.serialize_newtype_variant("Value", 5, "Static", value)
            }
            Self::List(value) => serializer.serialize_newtype_variant("Value", 6, "List", value),
            Self::Tuple(value) => serializer.serialize_newtype_variant("Value", 7, "Tuple", value),
            Self::Bytes(value) => serializer.serialize_newtype_variant("Value", 8, "Bytes", value),
            Self::Dict(value) => serializer.serialize_newtype_variant("Value", 9, "Dict", value),
        }
    }
}

impl Value {
    /// A source label is borrowed during execution and owned after decoding;
    /// restoring metadata never leaks strings into a global intern table.
    pub fn constant(value: &'static str) -> Self {
        Self::Static(Cow::Borrowed(value))
    }

    /// Python's `hex(value)`.
    pub fn hex(value: u128) -> Value {
        Value::Hex(value)
    }
    pub fn int(value: impl Into<i128>) -> Value {
        Value::Int(value.into())
    }
    pub fn opt(value: Option<i64>) -> Value {
        value.map_or(Value::None, |v| Value::Int(v as i128))
    }
    pub fn str(value: &str) -> Value {
        Value::Str(value.to_owned())
    }
    /// True for a string key equal to `key` (dict lookups by name).
    pub fn is_key(&self, key: &str) -> bool {
        matches!(self, Value::Str(s) if s == key)
            || matches!(self, Value::Static(s) if s.as_ref() == key)
    }
    /// A dict with constant string keys, in the reference's field order.
    pub fn dict(fields: Vec<(&'static str, Value)>) -> Value {
        Value::Dict(
            fields
                .into_iter()
                .map(|(k, v)| (Value::constant(k), v))
                .collect(),
        )
    }
}

// Runtimes may move devices across threads: record values must stay Send.
const _: () = {
    const fn assert_send<T: Send>() {}
    assert_send::<Value>();
    assert_send::<Action>();
};

/// One named observation with fields in insertion order.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Record {
    pub name: Cow<'static, str>,
    /// Published observations are immutable; whole-board captures share their
    /// field trees instead of recursively copying every retained event.
    pub fields: Arc<[(Cow<'static, str>, Value)]>,
}

impl Record {
    pub fn new<K: Into<Cow<'static, str>>>(
        name: impl Into<Cow<'static, str>>,
        fields: impl IntoIterator<Item = (K, Value)>,
    ) -> Self {
        Self {
            name: name.into(),
            fields: fields.into_iter().map(|(k, v)| (k.into(), v)).collect(),
        }
    }
}

/// An effect leaving the device, applied by the host in order.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// `board.queue_irq(cpu, line)`.
    QueueIrq(u8, u8),
    /// The device's own `_record(name, **fields)`: its trace and the
    /// board's event log.
    Record(Record),
    /// `board.record(name, **fields)`: the board's event log only.
    Log(Record),
    /// Call the device's `complete(token)` `delay` ticks from now, ordered
    /// with every other scheduled callback as `board.schedule(delay, cb)`
    /// orders them.
    Schedule { delay: u64, token: u64 },
}

/// A modeled refusal (`UnsupportedDevice` with this message).
#[derive(Clone, Debug, PartialEq)]
pub struct Unsupported(pub String);

impl From<&str> for Unsupported {
    fn from(message: &str) -> Self {
        Unsupported(message.to_owned())
    }
}

impl From<String> for Unsupported {
    fn from(message: String) -> Self {
        Unsupported(message)
    }
}

pub type Result<T> = std::result::Result<T, Unsupported>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_event_fields_preserve_json_and_binary_checkpoint_layout() {
        #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
        struct OwnedRecord {
            name: Cow<'static, str>,
            fields: Vec<(Cow<'static, str>, Value)>,
        }
        let owned = OwnedRecord {
            name: Cow::Owned("Completed graphics batch".into()),
            fields: vec![
                (Cow::Borrowed("token"), Value::Int(17)),
                (
                    Cow::Owned("destination".into()),
                    Value::dict(vec![
                        ("bytes", Value::Bytes((0..64).collect())),
                        (
                            "bounds",
                            Value::Tuple(vec![Value::Int(720), Value::Int(480)]),
                        ),
                    ]),
                ),
            ],
        };
        let record = Record::new(owned.name.clone(), owned.fields.clone());
        assert_eq!(
            serde_json::to_vec(&record).unwrap(),
            serde_json::to_vec(&owned).unwrap()
        );
        let mut before = Vec::new();
        let mut after = Vec::new();
        ciborium::into_writer(&owned, &mut before).unwrap();
        ciborium::into_writer(&record, &mut after).unwrap();
        assert_eq!(before, after);
        let restored: Record = ciborium::from_reader(before.as_slice()).unwrap();
        assert_eq!(restored, record);
        let decoded: OwnedRecord = ciborium::from_reader(after.as_slice()).unwrap();
        assert_eq!(decoded, owned);
    }

    #[test]
    fn hexadecimal_values_keep_the_existing_serialized_string_shape() {
        let lazy = Value::hex(0x12345);
        let eager = Value::Str("0x12345".into());
        assert_eq!(
            serde_json::to_vec(&lazy).unwrap(),
            serde_json::to_vec(&eager).unwrap()
        );
        let mut lazy_cbor = Vec::new();
        let mut eager_cbor = Vec::new();
        ciborium::into_writer(&lazy, &mut lazy_cbor).unwrap();
        ciborium::into_writer(&eager, &mut eager_cbor).unwrap();
        assert_eq!(lazy_cbor, eager_cbor);
        let restored: Value = ciborium::from_reader(lazy_cbor.as_slice()).unwrap();
        assert_eq!(restored, lazy);
    }
}
