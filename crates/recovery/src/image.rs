//! Checked initialized mappings, zero regions, word edits and SD containers.
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Firmware(#[from] fujimod_firmware::Error),
    #[error("{0}")]
    Invalid(String),
}
pub type Result<T> = std::result::Result<T, Error>;
pub(crate) fn require(ok: bool, message: &str) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(Error::Invalid(message.into()))
    }
}
pub(crate) fn hash(bytes: &[u8], expected: &str) -> Result<()> {
    let actual = fujimod_firmware::sha256(bytes);
    require(
        actual == expected,
        &format!("Source SHA-256 differs from recipe: expected {expected}, got {actual}"),
    )
}
pub(crate) fn extent(data: &[u8], at: usize, size: usize) -> Result<&[u8]> {
    Ok(fujimod_firmware::extent(data, at as u64, size as u64)?)
}
pub(crate) fn word(data: &[u8], at: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(extent(data, at, 4)?.try_into().unwrap()))
}
pub(crate) fn put(data: &mut [u8], at: usize, value: u32) -> Result<()> {
    extent(data, at, 4)?;
    data[at..at + 4].copy_from_slice(&value.to_le_bytes());
    Ok(())
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Prefix {
    pub size: usize,
    pub sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Compression {
    Raw,
    Method4,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub dat_offset: usize,
    pub size: usize,
    pub compression: Compression,
    pub decoded_size: usize,
    pub sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initialized_prefix: Option<Prefix>,
}
impl Source {
    pub fn read(&self, inverted: &[u8]) -> Result<Vec<u8>> {
        let raw = extent(inverted, self.dat_offset, self.size)?;
        let mut decoded = match self.compression {
            Compression::Raw => raw.to_vec(),
            Compression::Method4 => fujimod_firmware::decompress(raw)?.0,
        };
        require(
            decoded.len() == self.decoded_size,
            "Decoded source length differs",
        )?;
        hash(&decoded, &self.sha256)?;
        if let Some(prefix) = &self.initialized_prefix {
            hash(extent(&decoded, 0, prefix.size)?, &prefix.sha256)?;
            require(
                decoded[prefix.size..].iter().all(|v| *v == 0),
                "Initialized source suffix is nonzero",
            )?;
            decoded.truncate(prefix.size);
        }
        Ok(decoded)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Region {
    pub name: String,
    pub address: usize,
    pub source: Source,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bss {
    pub name: String,
    pub address: usize,
    pub size: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Edit {
    pub address: usize,
    pub old: u32,
    pub new: u32,
    pub what: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub size: usize,
    pub entry: usize,
    pub regions: Vec<Region>,
    #[serde(default)]
    pub bss: Vec<Bss>,
    #[serde(default)]
    pub edits: Vec<Edit>,
}
fn span(at: usize, len: usize, size: usize) -> Result<std::ops::Range<usize>> {
    let end = at
        .checked_add(len)
        .ok_or_else(|| Error::Invalid("Extent overflow".into()))?;
    require(end <= size, "Region exceeds image")?;
    Ok(at..end)
}
pub fn edits(image: &mut [u8], edits: &[Edit]) -> Result<()> {
    let mut seen = BTreeSet::new();
    // Validate the whole set before mutating, including duplicate no-op edits.
    for edit in edits {
        require(
            edit.address.is_multiple_of(4) && seen.insert(edit.address),
            "Unaligned or duplicate word edit",
        )?;
        require(
            word(image, edit.address)? == edit.old,
            &format!(
                "{}: expected old word differs at {:#x}",
                edit.what, edit.address
            ),
        )?;
    }
    for edit in edits {
        put(image, edit.address, edit.new)?;
    }
    Ok(())
}
pub fn transform(inverted: &[u8], plan: &Plan) -> Result<Vec<u8>> {
    require(
        plan.size > 0 && plan.size <= 0x40000000 && plan.entry < plan.size,
        "Invalid RAM image size or entry",
    )?;
    let mut output = vec![0; plan.size];
    let mut spans: Vec<std::ops::Range<usize>> = Vec::new();
    for region in &plan.regions {
        let bytes = region.source.read(inverted)?;
        let range = span(region.address, bytes.len(), plan.size)?;
        require(
            !spans
                .iter()
                .any(|s| s.start < range.end && range.start < s.end),
            "Overlapping initialized regions",
        )?;
        output[range.clone()].copy_from_slice(&bytes);
        spans.push(range);
    }
    for edit in &plan.edits {
        require(
            spans.iter().any(|s| {
                s.contains(&edit.address)
                    && edit.address.checked_add(4).is_some_and(|end| end <= s.end)
            }),
            "Word edit outside initialized data",
        )?;
    }
    for bss in &plan.bss {
        let range = span(bss.address, bss.size, plan.size)?;
        require(
            !spans
                .iter()
                .any(|s| s.start < range.end && range.start < s.end),
            "BSS overlaps another region",
        )?;
        spans.push(range);
    }
    edits(&mut output, &plan.edits)?;
    Ok(output)
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Part {
    pub dat_offset: usize,
    pub size: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeaderWord {
    pub offset: usize,
    pub value: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Packaging {
    pub header_parts: Vec<Part>,
    pub header_sha256: String,
    pub header_words: Vec<HeaderWord>,
    #[serde(default)]
    pub edits: Vec<Edit>,
    pub header_filename: String,
    pub image_filename: String,
    pub entry: usize,
    pub load_address: usize,
}
impl Packaging {
    pub fn header(&self, inverted: &[u8]) -> Result<Vec<u8>> {
        let mut bytes = Vec::new();
        for part in &self.header_parts {
            bytes.extend_from_slice(extent(inverted, part.dat_offset, part.size)?);
        }
        hash(&bytes, &self.header_sha256)?;
        for check in &self.header_words {
            require(
                word(&bytes, check.offset)? == check.value,
                "SD header word differs",
            )?;
        }
        edits(&mut bytes, &self.edits)?;
        Ok(bytes)
    }
    pub fn package(&self, inverted: &[u8], plan: &Plan, image: Vec<u8>) -> Result<Vec<File>> {
        require(image.len() == plan.size, "SD image length differs")?;
        self.package_bytes(inverted, plan.entry, image)
    }
    pub fn package_bytes(
        &self,
        inverted: &[u8],
        entry: usize,
        image: Vec<u8>,
    ) -> Result<Vec<File>> {
        require(
            self.entry == entry && self.load_address == 0,
            "SD image handoff differs",
        )?;
        require(
            !self
                .header_filename
                .eq_ignore_ascii_case(&self.image_filename),
            "Duplicate output filename",
        )?;
        Ok(vec![
            File::new(&self.header_filename, self.header(inverted)?)?,
            File::new(&self.image_filename, image)?,
        ])
    }
}
#[derive(Debug)]
pub struct File {
    pub name: String,
    pub bytes: Vec<u8>,
}
impl File {
    pub fn new(name: &str, bytes: Vec<u8>) -> Result<Self> {
        require(
            !name.is_empty()
                && name != "."
                && name != ".."
                && !name.contains(['/', '\\', ':', '\0'])
                && name.is_ascii()
                && !name.bytes().any(|b| b < 32 || b == 127),
            "Output filename must be an ASCII basename",
        )?;
        Ok(Self {
            name: name.into(),
            bytes,
        })
    }
}
