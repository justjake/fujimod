//! Typed binary device state. Diagnostic JSON is never a restore source.
mod archive;
use serde::{Serialize, de::DeserializeOwned};
use std::sync::Arc;

#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Checkpoint {
    kind: std::borrow::Cow<'static, str>,
    #[serde(with = "shared_bytes")]
    bytes: Arc<[u8]>,
    #[serde(with = "shared_blobs")]
    blobs: Vec<Arc<[u8]>>,
    children: Vec<Checkpoint>,
}
impl Checkpoint {
    pub fn encode<T: Serialize>(kind: &'static str, state: &T) -> Result<Self, String> {
        let mut bytes = Vec::new();
        ciborium::into_writer(state, &mut bytes)
            .map_err(|e| format!("{kind} snapshot encode: {e}"))?;
        Ok(Self {
            kind: std::borrow::Cow::Borrowed(kind),
            bytes: bytes.into(),
            blobs: Vec::new(),
            children: Vec::new(),
        })
    }
    pub fn decode<T: DeserializeOwned>(&self, kind: &str) -> Result<T, String> {
        if self.kind != kind {
            return Err("device snapshot kind differs".into());
        }
        let mut input = &*self.bytes;
        let value = ciborium::from_reader(&mut input)
            .map_err(|e| format!("{kind} snapshot decode: {e}"))?;
        if !input.is_empty() {
            return Err("device snapshot has trailing bytes".into());
        }
        Ok(value)
    }
    pub fn with_children(mut self, children: Vec<Checkpoint>) -> Self {
        self.children = children;
        self
    }
    pub fn children(&self) -> &[Checkpoint] {
        &self.children
    }
    pub fn with_blobs(mut self, blobs: Vec<Arc<[u8]>>) -> Self {
        self.blobs = blobs;
        self
    }
    pub fn blobs(&self) -> &[Arc<[u8]>] {
        &self.blobs
    }
    pub fn bytes(&self) -> usize {
        self.bytes.len()
            + self.blobs.iter().map(|b| b.len()).sum::<usize>()
            + self.children.iter().map(Self::bytes).sum::<usize>()
    }
    pub fn share_unchanged(&mut self, previous: &Self) {
        if self.kind == previous.kind && self.bytes == previous.bytes {
            self.bytes = previous.bytes.clone();
        }
    }
}

/// Complete owned fields. Shared wires use explicit implementations so restore
/// keeps their existing handles; this macro never substitutes diagnostic JSON.
/// Optional immutable byte blobs retain their allocation across captures and
/// restores; typed metadata records and validates each blob's exact length.
#[macro_export]
macro_rules! checkpoint_fields {
    ($( $(#[$meta:meta])* $field:ident : $state:ty),+ $(,)? $(; blobs: [$($blob:ident),+ $(,)?])?) => {
        fn checkpoint(&self) -> std::result::Result<$crate::checkpoint::Checkpoint, String> {
            #[derive(serde::Serialize)]
            struct Snapshot<'a> { $( $(#[$meta])* $field: &'a $state, )+ $( $($blob: usize,)+ )? }
            let saved = $crate::checkpoint::Checkpoint::encode(self.name(), &Snapshot {
                $( $field: &self.$field, )+ $( $($blob: self.$blob.len(),)+ )?
            })?;
            Ok(saved.with_blobs(vec![$($(self.$blob.clone()),+)?]))
        }
        fn validate_checkpoint(&self, saved: &$crate::checkpoint::Checkpoint) -> std::result::Result<(), String> {
            #[allow(dead_code)]
            #[derive(serde::Deserialize)]
            struct Snapshot { $( $(#[$meta])* $field: $state, )+ $( $($blob: usize,)+ )? }
            let Snapshot { $( $field: _, )+ $( $($blob,)+ )? } = saved.decode(self.name())?;
            let lengths: &[usize] = &[$($($blob),+)?];
            if lengths.len() != saved.blobs().len()
                || lengths.iter().zip(saved.blobs()).any(|(length, blob)| *length != blob.len()) {
                return Err("device snapshot blob lengths differ".into());
            }
            Ok(())
        }
        fn restore_checkpoint(&mut self, saved: &$crate::checkpoint::Checkpoint) -> std::result::Result<(), String> {
            #[derive(serde::Deserialize)]
            struct Snapshot { $( $(#[$meta])* $field: $state, )+ $( $($blob: usize,)+ )? }
            let Snapshot { $( $field, )+ $( $($blob,)+ )? } = saved.decode(self.name())?;
            let lengths: &[usize] = &[$($($blob),+)?];
            if lengths.len() != saved.blobs().len()
                || lengths.iter().zip(saved.blobs()).any(|(length, blob)| *length != blob.len()) {
                return Err("device snapshot blob lengths differ".into());
            }
            $( self.$field = $field; )+
            $( let mut blobs = saved.blobs().iter(); $(self.$blob = blobs.next().unwrap().clone();)+ )?
            Ok(())
        }
    };
}

/// Serde support for fixed hardware banks larger than Serde's built-in arrays.
pub mod array {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    pub fn serialize<T: Serialize, S: Serializer, const N: usize>(
        value: &[T; N],
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        value.as_slice().serialize(serializer)
    }
    pub fn deserialize<'de, T: Deserialize<'de>, D: Deserializer<'de>, const N: usize>(
        deserializer: D,
    ) -> Result<[T; N], D::Error> {
        let value = Vec::<T>::deserialize(deserializer)?;
        value.try_into().map_err(|v: Vec<T>| {
            serde::de::Error::custom(format!("expected {N} hardware words, got {}", v.len()))
        })
    }
}

/// Encode contiguous byte storage as a CBOR byte string. JSON retains its
/// ordinary integer-array representation, including diagnostic fingerprints.
pub mod bytes {
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(value: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bytes(value)
    }
    pub fn deserialize<'de, D: Deserializer<'de>, B: From<Vec<u8>>>(
        deserializer: D,
    ) -> Result<B, D::Error> {
        Ok(Vec::<u8>::deserialize(deserializer)?.into())
    }
}

pub mod shared_bytes {
    use super::*;
    pub fn serialize<S: serde::Serializer>(bytes: &Arc<[u8]>, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_bytes(bytes)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Arc<[u8]>, D::Error> {
        struct Bytes;
        impl<'de> serde::de::Visitor<'de> for Bytes {
            type Value = Arc<[u8]>;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("shared bytes")
            }
            fn visit_bytes<E: serde::de::Error>(self, bytes: &[u8]) -> Result<Self::Value, E> {
                Ok(bytes.into())
            }
            fn visit_byte_buf<E: serde::de::Error>(self, bytes: Vec<u8>) -> Result<Self::Value, E> {
                Ok(bytes.into())
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Self::Value, A::Error> {
                let mut bytes = Vec::new();
                while let Some(byte) = seq.next_element::<u8>()? {
                    bytes.push(byte);
                }
                Ok(bytes.into())
            }
        }
        // Internally tagged input records buffer CBOR byte strings before
        // decoding the frame; accept that byte representation as well as JSON.
        d.deserialize_byte_buf(Bytes)
    }
}

mod shared_blobs {
    use super::*;
    pub fn serialize<S: serde::Serializer>(v: &[Arc<[u8]>], s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq;
        struct Bytes<'a>(&'a [u8]);
        impl serde::Serialize for Bytes<'_> {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_bytes(self.0)
            }
        }
        let mut out = s.serialize_seq(Some(v.len()))?;
        for b in v {
            out.serialize_element(&Bytes(b))?;
        }
        out.end()
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Arc<[u8]>>, D::Error> {
        use serde::Deserialize;
        Ok(Vec::<Vec<u8>>::deserialize(d)?
            .into_iter()
            .map(Into::into)
            .collect())
    }
}

/// Fixed banks of fixed-width rows (for packet and pixel staging).
pub mod matrix {
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<T: serde::Serialize, S: Serializer, const N: usize, const M: usize>(
        v: &[[T; M]; N],
        s: S,
    ) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq;
        let mut rows = s.serialize_seq(Some(N))?;
        for row in v {
            rows.serialize_element(row.as_slice())?;
        }
        rows.end()
    }
    pub fn deserialize<
        'de,
        T: Deserialize<'de>,
        D: Deserializer<'de>,
        const N: usize,
        const M: usize,
    >(
        d: D,
    ) -> Result<[[T; M]; N], D::Error> {
        let rows = Vec::<Vec<T>>::deserialize(d)?;
        let rows = rows
            .into_iter()
            .map(|row| {
                row.try_into()
                    .map_err(|_| serde::de::Error::custom("snapshot row width differs"))
            })
            .collect::<Result<Vec<[T; M]>, D::Error>>()?;
        rows.try_into()
            .map_err(|_| serde::de::Error::custom("snapshot row count differs"))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn byte_buffers_are_bulk_cbor_and_preserve_json_and_owned_storage() {
        #[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug)]
        struct Buffers {
            #[serde(with = "super::bytes")]
            vector: Vec<u8>,
            #[serde(with = "super::bytes")]
            boxed: Box<[u8]>,
        }
        let expected = Buffers {
            vector: (0..=255).cycle().take(4096).collect(),
            boxed: vec![0xff; 4096].into_boxed_slice(),
        };
        let snapshot = super::Checkpoint::encode("buffers", &expected).unwrap();
        assert!(snapshot.bytes() < 8250);
        let decoded: Buffers = snapshot.decode("buffers").unwrap();
        assert_eq!(decoded, expected);
        assert_eq!(
            serde_json::to_value(&decoded).unwrap(),
            serde_json::json!({
                "vector": expected.vector, "boxed": expected.boxed,
            })
        );
    }
    use super::*;
    use crate::{Device, Record, Value};
    use crate::{exr2::clock, test_host::*};
    #[test]
    fn checkpoint_records_keep_owned_labels_and_value_variants() {
        let original = Record::new(
            "Wire transaction",
            [
                ("part", Value::constant("lens")),
                ("register", Value::Int(0x12)),
                ("payload", Value::Bytes(vec![1, 2, 3])),
            ],
        );
        let saved = Checkpoint::encode("record", &original).unwrap();
        let restored: Record = saved.decode("record").unwrap();
        assert_eq!(restored, original);
        assert!(matches!(
            restored.fields[0].1,
            Value::Static(std::borrow::Cow::Owned(_))
        ));
        assert!(saved.decode::<Record>("wrong-kind").is_err());
    }
    #[test]
    fn checkpoint_clock_restores_its_existing_output_without_a_tick() {
        let mut clock = clock::Clocks::new(
            clock::Options {
                output_settle_ticks: 5,
                reset_ready: true,
            },
            (),
        )
        .unwrap();
        let output = clock.output();
        let mut host = TestHost::new();
        write(&mut clock, &mut host, clock::BASE + 0x48, 1 << 28);
        let saved = clock.checkpoint().unwrap();
        let state = clock.fingerprint().unwrap();
        advance(&mut clock, &mut host, 5);
        assert!(output.ready());
        clock.restore_checkpoint(&saved).unwrap();
        assert!(!output.ready());
        assert_eq!(clock.fingerprint().unwrap(), state);
        advance(&mut clock, &mut host, 4);
        assert!(!output.ready());
        advance(&mut clock, &mut host, 5);
        assert!(output.ready());
    }
}
