//! Checked changes to a startup RAM image, applied before its CRC is taken.
use crate::image::{self, Result, extent, require, word};
use serde::Deserialize;
use std::path::Path;

/// One 32-bit word inside the stock image that must hold `old`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WordEdit {
    pub address: u64,
    pub old: u32,
    pub new: u32,
}

/// Bytes placed past the end of the stock image, or replacing a stock span
/// whose current bytes have the SHA-256 `replaces`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Payload {
    pub address: u64,
    pub bytes: Vec<u8>,
    pub replaces: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RamPatches {
    pub words: Vec<WordEdit>,
    pub payloads: Vec<Payload>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PayloadFile {
    address: u64,
    /// Relative to the patch file's directory.
    file: String,
    sha256: String,
    /// SHA-256 of the stock bytes this payload replaces, for a payload
    /// inside the stock image.
    #[serde(default)]
    replaces: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PatchFile {
    #[serde(default)]
    words: Vec<WordEdit>,
    #[serde(default)]
    payloads: Vec<PayloadFile>,
}

impl RamPatches {
    pub fn is_empty(&self) -> bool {
        self.words.is_empty() && self.payloads.is_empty()
    }

    /// Reads a patch file whose payload bytes live in hash-checked files
    /// beside it.
    pub fn read(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| image::Error::Invalid(format!("RAM patch file: {e}")))?;
        let file: PatchFile = serde_json::from_str(&text)
            .map_err(|e| image::Error::Invalid(format!("RAM patch file: {e}")))?;
        let directory = path.parent().unwrap_or(Path::new("."));
        let mut payloads = Vec::new();
        for row in file.payloads {
            require(
                Path::new(&row.file)
                    .components()
                    .all(|c| matches!(c, std::path::Component::Normal(_))),
                "RAM payload path leaves the patch directory",
            )?;
            let bytes = std::fs::read(directory.join(&row.file))
                .map_err(|e| image::Error::Invalid(format!("RAM payload {}: {e}", row.file)))?;
            image::hash(&bytes, &row.sha256)?;
            payloads.push(Payload {
                address: row.address,
                bytes,
                replaces: row.replaces,
            });
        }
        Ok(Self {
            words: file.words,
            payloads,
        })
    }

    /// Applies every edit to `ram` or none. Words must lie inside the stock
    /// image and hold their old value. A payload either replaces a stock
    /// span whose bytes match its `replaces` hash, or lies past the image's
    /// end, where the image grows with zeros. No two edits overlap.
    pub fn apply(&self, ram: &mut Vec<u8>) -> Result<()> {
        let stock = ram.len() as u64;
        let mut words: Vec<u64> = self.words.iter().map(|w| w.address).collect();
        words.sort_unstable();
        require(
            words.windows(2).all(|pair| pair[0] != pair[1]),
            "RAM patch edits one word twice",
        )?;
        for edit in &self.words {
            require(
                edit.address.is_multiple_of(4) && edit.address + 4 <= stock,
                "RAM word edit lies outside the stock image",
            )?;
            require(
                word(ram, edit.address as usize)? == edit.old,
                "RAM word edit does not match the stock word",
            )?;
        }
        let mut spans: Vec<(u64, u64)> = self
            .words
            .iter()
            .map(|w| (w.address, w.address + 4))
            .collect();
        for payload in &self.payloads {
            let end = payload.address.checked_add(payload.bytes.len() as u64);
            require(
                !payload.bytes.is_empty() && end.is_some_and(|end| end < 1 << 30),
                "RAM payload must be non-empty and end below 1 GiB",
            )?;
            let end = end.unwrap_or(u64::MAX);
            match &payload.replaces {
                Some(digest) => {
                    require(
                        end <= stock,
                        "Replacing RAM payload lies outside the stock image",
                    )?;
                    image::hash(&ram[payload.address as usize..end as usize], digest)?;
                }
                None => require(
                    payload.address >= stock,
                    "RAM payload inside the stock image must name the bytes it replaces",
                )?,
            }
            spans.push((payload.address, end));
        }
        spans.sort_unstable();
        require(
            spans.windows(2).all(|pair| pair[0].1 <= pair[1].0),
            "RAM patches overlap",
        )?;
        let end = spans.iter().map(|span| span.1).fold(stock, u64::max);
        ram.resize(end as usize, 0);
        for edit in &self.words {
            ram[edit.address as usize..edit.address as usize + 4]
                .copy_from_slice(&edit.new.to_le_bytes());
        }
        for payload in &self.payloads {
            let at = payload.address as usize;
            extent(ram, at, payload.bytes.len())?;
            ram[at..at + payload.bytes.len()].copy_from_slice(&payload.bytes);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image() -> Vec<u8> {
        (0..64u8).collect()
    }

    fn edit(address: u64, old: u32, new: u32) -> WordEdit {
        WordEdit { address, old, new }
    }

    fn payload(address: u64, bytes: &[u8], replaces: Option<&[u8]>) -> Payload {
        Payload {
            address,
            bytes: bytes.to_vec(),
            replaces: replaces.map(fujimod_firmware::sha256),
        }
    }

    #[test]
    fn words_and_payloads_apply_and_grow_the_image() {
        let mut ram = image();
        let stock = image();
        let patches = RamPatches {
            words: vec![edit(8, 0x0b0a0908, 0x94000000)],
            payloads: vec![
                payload(80, &[1, 2, 3], None),
                payload(16, &[9; 8], Some(&stock[16..24])),
            ],
        };
        patches.apply(&mut ram).unwrap();
        assert_eq!(ram.len(), 83);
        assert_eq!(word(&ram, 8).unwrap(), 0x94000000);
        assert_eq!(&ram[16..24], [9; 8]);
        assert_eq!(&ram[24..64], &stock[24..64]);
        assert!(ram[64..80].iter().all(|v| *v == 0));
        assert_eq!(&ram[80..], [1, 2, 3]);
    }

    #[test]
    fn mismatched_or_misplaced_edits_leave_the_image_unchanged() {
        let stock = image();
        let good = edit(8, 0x0b0a0908, 1);
        for patches in [
            RamPatches {
                words: vec![edit(8, 0, 1)],
                payloads: vec![],
            },
            RamPatches {
                words: vec![edit(6, 0x0b0a0908, 1)],
                payloads: vec![],
            },
            RamPatches {
                words: vec![edit(64, 0, 1)],
                payloads: vec![],
            },
            RamPatches {
                words: vec![good.clone(), good.clone()],
                payloads: vec![],
            },
            // A payload inside the image without the hash of what it replaces.
            RamPatches {
                words: vec![],
                payloads: vec![payload(60, &[1], None)],
            },
            // A replacement whose stock bytes differ.
            RamPatches {
                words: vec![],
                payloads: vec![payload(16, &[9; 8], Some(&[0; 8]))],
            },
            // A replacement that runs past the image.
            RamPatches {
                words: vec![],
                payloads: vec![payload(60, &[9; 8], Some(&[60, 61, 62, 63, 0, 0, 0, 0]))],
            },
            // A word edit inside a replaced span.
            RamPatches {
                words: vec![good.clone()],
                payloads: vec![payload(8, &[9; 8], Some(&stock[8..16]))],
            },
            RamPatches {
                words: vec![],
                payloads: vec![payload(64, &[1, 2], None), payload(65, &[3], None)],
            },
            RamPatches {
                words: vec![],
                payloads: vec![payload(64, &[], None)],
            },
            RamPatches {
                words: vec![],
                payloads: vec![payload(1 << 30, &[1], None)],
            },
        ] {
            let mut ram = image();
            assert!(patches.apply(&mut ram).is_err(), "{patches:?}");
            assert_eq!(ram, image());
        }
    }

    #[test]
    fn patch_files_read_hash_checked_payloads_beside_them() {
        let directory = std::env::temp_dir().join(format!("ram-patch-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("payload.bin"), [7u8; 200]).unwrap();
        let sha = fujimod_firmware::sha256(&[7u8; 200]);
        let write = |text: String| {
            std::fs::write(directory.join("patches.json"), text).unwrap();
            RamPatches::read(&directory.join("patches.json"))
        };
        let read = write(format!(
            r#"{{"words":[{{"address":8,"old":1,"new":2}}],"payloads":[{{"address":4096,"file":"payload.bin","sha256":"{sha}"}},{{"address":0,"file":"payload.bin","sha256":"{sha}","replaces":"{sha}"}}]}}"#
        ))
        .unwrap();
        assert_eq!(read.words, vec![edit(8, 1, 2)]);
        assert_eq!(
            read.payloads,
            vec![
                Payload {
                    address: 4096,
                    bytes: vec![7; 200],
                    replaces: None
                },
                Payload {
                    address: 0,
                    bytes: vec![7; 200],
                    replaces: Some(sha.clone())
                },
            ]
        );
        let zero = "0".repeat(64);
        assert!(
            write(format!(
                r#"{{"payloads":[{{"address":4096,"file":"payload.bin","sha256":"{zero}"}}]}}"#
            ))
            .is_err()
        );
        assert!(
            write(format!(
                r#"{{"payloads":[{{"address":4096,"file":"../payload.bin","sha256":"{sha}"}}]}}"#
            ))
            .is_err()
        );
        assert!(write(r#"{"words":[],"extra":1}"#.into()).is_err());
        std::fs::remove_dir_all(&directory).unwrap();
    }
}
