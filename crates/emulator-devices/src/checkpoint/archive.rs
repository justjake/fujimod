//! Self-contained CBOR envelopes with content-addressed binary storage.
//! Device validators still check model identity, geometry and typed state.
use super::Checkpoint;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Archive<B> {
    schema: u32,
    root: Node,
    blobs: Vec<B>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Node {
    kind: String,
    state: usize,
    blobs: Vec<usize>,
    children: Vec<Node>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Blob {
    sha256: [u8; 32],
    #[serde(with = "super::shared_bytes")]
    data: Arc<[u8]>,
}
#[derive(Serialize)]
struct BlobRef<'a> {
    sha256: [u8; 32],
    #[serde(with = "super::bytes")]
    data: &'a [u8],
}
#[derive(Default)]
struct Builder<'a> {
    blobs: Vec<BlobRef<'a>>,
    by_hash: BTreeMap<[u8; 32], usize>,
    // The input tree retains these immutable allocations for the entire export.
    // Pointer reuse only avoids repeated hashing; it never enters the archive.
    by_address: BTreeMap<(usize, usize), usize>,
}
impl<'a> Builder<'a> {
    fn blob(&mut self, bytes: &'a Arc<[u8]>) -> Result<usize, String> {
        let address = (bytes.as_ptr() as usize, bytes.len());
        if let Some(&index) = self.by_address.get(&address) {
            return Ok(index);
        }
        let sha256: [u8; 32] = Sha256::digest(bytes).into();
        let index = if let Some(&index) = self.by_hash.get(&sha256) {
            if self.blobs[index].data != &**bytes {
                return Err("snapshot archive hash collision".into());
            }
            index
        } else {
            let index = self.blobs.len();
            self.blobs.push(BlobRef {
                sha256,
                data: bytes,
            });
            self.by_hash.insert(sha256, index);
            index
        };
        self.by_address.insert(address, index);
        Ok(index)
    }
    fn node(&mut self, saved: &'a Checkpoint) -> Result<Node, String> {
        Ok(Node {
            kind: saved.kind.to_string(),
            state: self.blob(&saved.bytes)?,
            blobs: saved
                .blobs
                .iter()
                .map(|b| self.blob(b))
                .collect::<Result<_, _>>()?,
            children: saved
                .children
                .iter()
                .map(|c| self.node(c))
                .collect::<Result<_, _>>()?,
        })
    }
}
fn node(saved: Node, blobs: &[Blob], used: &mut [bool]) -> Result<Checkpoint, String> {
    fn blob(index: usize, blobs: &[Blob], used: &mut [bool]) -> Result<Arc<[u8]>, String> {
        let bytes = blobs
            .get(index)
            .ok_or("snapshot archive blob reference is out of range")?;
        used[index] = true;
        Ok(bytes.data.clone())
    }
    Ok(Checkpoint {
        kind: saved.kind.into(),
        bytes: blob(saved.state, blobs, used)?,
        blobs: saved
            .blobs
            .into_iter()
            .map(|i| blob(i, blobs, used))
            .collect::<Result<_, _>>()?,
        children: saved
            .children
            .into_iter()
            .map(|c| node(c, blobs, used))
            .collect::<Result<_, _>>()?,
    })
}

impl Checkpoint {
    /// Serialize metadata, binary blobs and every child into one byte stream.
    /// Equal immutable data is stored once, even across unrelated child nodes.
    pub fn to_bytes(&self) -> Result<Vec<u8>, String> {
        let mut builder = Builder::default();
        let root = builder.node(self)?;
        let archive = Archive {
            schema: 1,
            root,
            blobs: builder.blobs,
        };
        let mut bytes = Vec::new();
        ciborium::into_writer(&archive, &mut bytes)
            .map_err(|e| format!("snapshot archive encode: {e}"))?;
        Ok(bytes)
    }

    /// Decode and verify a complete envelope without touching a live machine.
    /// The caller applies its archive size limit before decoding and validates
    /// the result against the receiving machine before installing any values.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        let mut input = bytes;
        let archive: Archive<Blob> = ciborium::from_reader(&mut input)
            .map_err(|e| format!("snapshot archive decode: {e}"))?;
        if !input.is_empty() {
            return Err("snapshot archive has trailing bytes".into());
        }
        if archive.schema != 1 {
            return Err("unknown snapshot archive schema".into());
        }
        let mut hashes = BTreeSet::new();
        for blob in &archive.blobs {
            let actual: [u8; 32] = Sha256::digest(&blob.data).into();
            if actual != blob.sha256 {
                return Err("snapshot archive blob hash differs".into());
            }
            if !hashes.insert(blob.sha256) {
                return Err("snapshot archive has duplicate blobs".into());
            }
        }
        let mut used = vec![false; archive.blobs.len()];
        let restored = node(archive.root, &archive.blobs, &mut used)?;
        if used.contains(&false) {
            return Err("snapshot archive has unreferenced blobs".into());
        }
        Ok(restored)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_archive_outlives_original_blobs_and_deduplicates_by_content() {
        let encoded = {
            let pixels: Vec<u8> = (0..65536).map(|i| i as u8).collect();
            // Distinct allocations deliberately contain the same data.
            let child = Checkpoint::encode("child-v1", &(123u64, "input"))
                .unwrap()
                .with_blobs(vec![pixels.clone().into()]);
            Checkpoint::encode("root-v1", &[42u32, 99])
                .unwrap()
                .with_blobs(vec![pixels.into()])
                .with_children(vec![child])
                .to_bytes()
                .unwrap()
        };
        // Byte strings stay binary, and the repeated 64 KiB block appears once.
        assert!(encoded.len() < 65536 + 512);
        let restored = Checkpoint::from_bytes(&encoded).unwrap();
        assert_eq!(restored.decode::<[u32; 2]>("root-v1").unwrap(), [42, 99]);
        assert_eq!(
            restored.children()[0]
                .decode::<(u64, String)>("child-v1")
                .unwrap(),
            (123, "input".into())
        );
        assert_eq!(
            &*restored.blobs()[0],
            &(0..65536).map(|i| i as u8).collect::<Vec<_>>()
        );
        assert!(Arc::ptr_eq(
            &restored.blobs()[0],
            &restored.children()[0].blobs()[0]
        ));
        assert_eq!(restored.to_bytes().unwrap(), encoded);
    }

    #[test]
    fn truncated_extended_or_unknown_envelopes_are_refused() {
        let encoded = Checkpoint::encode("root-v1", &42u32)
            .unwrap()
            .to_bytes()
            .unwrap();
        for end in [0, 1, encoded.len() / 2, encoded.len() - 1] {
            assert!(Checkpoint::from_bytes(&encoded[..end]).is_err());
        }
        let mut extra = encoded.clone();
        extra.push(0);
        assert!(Checkpoint::from_bytes(&extra).is_err());
        let mut value: ciborium::value::Value = ciborium::from_reader(&encoded[..]).unwrap();
        let ciborium::value::Value::Map(fields) = &mut value else {
            unreachable!()
        };
        fields.push((
            ciborium::value::Value::Text("extra".into()),
            ciborium::value::Value::Bool(true),
        ));
        let mut extra = Vec::new();
        ciborium::into_writer(&value, &mut extra).unwrap();
        assert!(Checkpoint::from_bytes(&extra).is_err());
    }

    #[test]
    fn changed_bytes_bad_references_duplicate_blocks_and_unknown_versions_are_refused() {
        let encoded = Checkpoint::encode("root-v1", &42u32)
            .unwrap()
            .to_bytes()
            .unwrap();
        let mut archive: Archive<Blob> = ciborium::from_reader(&encoded[..]).unwrap();
        archive.blobs[0].data = vec![0].into();
        let mut bad = Vec::new();
        ciborium::into_writer(&archive, &mut bad).unwrap();
        assert!(
            Checkpoint::from_bytes(&bad)
                .err()
                .unwrap()
                .contains("hash differs")
        );
        for corrupt in 0..4 {
            let mut archive: Archive<Blob> = ciborium::from_reader(&encoded[..]).unwrap();
            match corrupt {
                0 => archive.root.state = usize::MAX,
                1 => archive.blobs.push(archive.blobs[0].clone()),
                2 => archive.schema = 2,
                _ => archive.blobs.push(Blob {
                    sha256: Sha256::digest([0]).into(),
                    data: vec![0].into(),
                }),
            }
            let mut bad = Vec::new();
            ciborium::into_writer(&archive, &mut bad).unwrap();
            assert!(Checkpoint::from_bytes(&bad).is_err());
        }
    }
}
