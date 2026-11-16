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
/// Bytes placed in a RAM image extent that no initialized section or BSS covers.
#[derive(Clone, Debug)]
pub struct OverlayRegion {
    pub name: String,
    pub address: usize,
    pub bytes: Vec<u8>,
}
/// Additions to a transformed RAM image: new regions in its unused extent and checked word
/// edits of its initialized bytes.
#[derive(Clone, Debug, Default)]
pub struct Overlay {
    pub regions: Vec<OverlayRegion>,
    pub edits: Vec<Edit>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OverlayFileRegion {
    name: String,
    address: usize,
    size: usize,
    file: String,
    sha256: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OverlayFile {
    regions: Vec<OverlayFileRegion>,
    edits: Vec<Edit>,
}
impl Overlay {
    /// Read an overlay description. Each region names a binary file in the description's own
    /// directory, with its exact size and SHA-256.
    pub fn read(path: &std::path::Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| Error::Invalid(format!("Cannot read RAM overlay: {error}")))?;
        let file: OverlayFile = serde_json::from_str(&text)
            .map_err(|error| Error::Invalid(format!("Invalid RAM overlay: {error}")))?;
        let directory = path.parent().unwrap_or(std::path::Path::new("."));
        let mut regions = Vec::new();
        for region in file.regions {
            require(
                !region.file.is_empty()
                    && std::path::Path::new(&region.file).file_name()
                        == Some(std::ffi::OsStr::new(&region.file)),
                "RAM overlay region file must be a basename",
            )?;
            let bytes = std::fs::read(directory.join(&region.file)).map_err(|error| {
                Error::Invalid(format!("Cannot read RAM overlay region {}: {error}", region.name))
            })?;
            require(
                bytes.len() == region.size && fujimod_firmware::sha256(&bytes) == region.sha256,
                &format!("RAM overlay region {} differs from its description", region.name),
            )?;
            regions.push(OverlayRegion {
                name: region.name,
                address: region.address,
                bytes,
            });
        }
        Ok(Self {
            regions,
            edits: file.edits,
        })
    }
}
/// Apply an overlay to an image `transform` produced from `plan`. A region must lie inside the
/// image, in bytes that are zero there, outside every initialized region and outside every
/// BSS extent, which the stock startup may clear after the image is loaded. An edit must
/// replace an expected word of initialized bytes outside the overlay regions. Nothing changes
/// unless every check passes.
pub fn overlay(image: &mut [u8], plan: &Plan, overlay: &Overlay) -> Result<()> {
    require(image.len() == plan.size, "RAM overlay image length differs from its plan")?;
    let initialized = plan
        .regions
        .iter()
        .map(|region| {
            let size = region
                .source
                .initialized_prefix
                .as_ref()
                .map_or(region.source.decoded_size, |prefix| prefix.size);
            Ok((region.name.as_str(), span(region.address, size, plan.size)?))
        })
        .collect::<Result<Vec<_>>>()?;
    let bss = plan
        .bss
        .iter()
        .map(|bss| Ok((bss.name.as_str(), span(bss.address, bss.size, plan.size)?)))
        .collect::<Result<Vec<_>>>()?;
    let mut placed: Vec<std::ops::Range<usize>> = Vec::new();
    for region in &overlay.regions {
        require(
            !region.bytes.is_empty(),
            &format!("RAM overlay region {} is empty", region.name),
        )?;
        let range = span(region.address, region.bytes.len(), plan.size).map_err(|_| {
            Error::Invalid(format!("RAM overlay region {} exceeds the image", region.name))
        })?;
        let overlaps = |other: &std::ops::Range<usize>| other.start < range.end && range.start < other.end;
        if let Some((name, _)) = initialized.iter().find(|(_, other)| overlaps(other)) {
            return Err(Error::Invalid(format!(
                "RAM overlay region {} overlaps initialized {name}",
                region.name
            )));
        }
        if let Some((name, _)) = bss.iter().find(|(_, other)| overlaps(other)) {
            return Err(Error::Invalid(format!(
                "RAM overlay region {} lies in {name} BSS, which the stock startup may clear",
                region.name
            )));
        }
        require(
            !placed.iter().any(overlaps),
            &format!("RAM overlay region {} overlaps another overlay region", region.name),
        )?;
        require(
            image[range.clone()].iter().all(|byte| *byte == 0),
            &format!("RAM overlay region {} replaces nonzero bytes", region.name),
        )?;
        placed.push(range);
    }
    for edit in &overlay.edits {
        let end = edit.address.checked_add(4);
        require(
            end.is_some_and(|end| {
                initialized
                    .iter()
                    .any(|(_, s)| s.start <= edit.address && end <= s.end)
                    && !placed.iter().any(|s| s.start < end && edit.address < s.end)
            }),
            &format!(
                "{}: RAM overlay edit at {:#x} is outside initialized stock bytes",
                edit.what, edit.address
            ),
        )?;
    }
    // `edits` checks every word before writing any; the region writes cannot fail.
    edits(image, &overlay.edits)?;
    for (region, range) in overlay.regions.iter().zip(&placed) {
        image[range.clone()].copy_from_slice(&region.bytes);
    }
    Ok(())
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
#[cfg(test)]
mod overlay_tests {
    use super::*;
    fn plan() -> Plan {
        let source = |decoded_size| Source {
            dat_offset: 0,
            size: decoded_size,
            compression: Compression::Raw,
            decoded_size,
            sha256: String::new(),
            initialized_prefix: None,
        };
        Plan {
            size: 0x400,
            entry: 0,
            regions: vec![Region { name: "code".into(), address: 0, source: source(0x100) }],
            bss: vec![Bss { name: "data".into(), address: 0x100, size: 0x100 }],
            edits: Vec::new(),
        }
    }
    fn image() -> Vec<u8> {
        let mut image = vec![0; 0x400];
        image[..0x100].fill(0x11);
        image
    }
    fn region(address: usize, bytes: &[u8]) -> OverlayRegion {
        OverlayRegion { name: "mods".into(), address, bytes: bytes.to_vec() }
    }
    fn edit(address: usize, old: u32) -> Edit {
        Edit { address, old, new: 0x9400_0000, what: "hook".into() }
    }
    #[test]
    fn places_regions_in_unused_zero_bytes_and_applies_checked_edits() {
        let mut ram = image();
        let overlay = Overlay { regions: vec![region(0x200, &[1, 2, 3, 4])], edits: vec![edit(0x10, 0x1111_1111)] };
        super::overlay(&mut ram, &plan(), &overlay).unwrap();
        assert_eq!(&ram[0x200..0x204], &[1, 2, 3, 4]);
        assert_eq!(word(&ram, 0x10).unwrap(), 0x9400_0000);
        let mut expected = image();
        expected[0x200..0x204].copy_from_slice(&[1, 2, 3, 4]);
        put(&mut expected, 0x10, 0x9400_0000).unwrap();
        assert_eq!(ram, expected);
    }
    #[test]
    fn refuses_bss_initialized_nonzero_overlapping_and_outside_regions_and_bad_edits() {
        let mut nonzero = image();
        nonzero[0x300] = 1;
        let cases: Vec<(Vec<u8>, Overlay, &str)> = vec![
            (image(), Overlay { regions: vec![region(0x180, &[1])], edits: vec![] }, "lies in data BSS"),
            (image(), Overlay { regions: vec![region(0xfc, &[1; 8])], edits: vec![] }, "overlaps initialized code"),
            (image(), Overlay { regions: vec![region(0x3fe, &[1; 4])], edits: vec![] }, "exceeds the image"),
            (nonzero, Overlay { regions: vec![region(0x300, &[1])], edits: vec![] }, "replaces nonzero bytes"),
            (image(), Overlay { regions: vec![region(0x200, &[1; 8]), region(0x204, &[1])], edits: vec![] },
             "overlaps another overlay region"),
            (image(), Overlay { regions: vec![], edits: vec![edit(0x10, 0)] }, "expected old word differs"),
            (image(), Overlay { regions: vec![], edits: vec![edit(0x200, 0)] }, "outside initialized stock bytes"),
            (image(), Overlay { regions: vec![], edits: vec![edit(0x10, 0x1111_1111), edit(0x10, 0x1111_1111)] },
             "duplicate word edit"),
        ];
        for (mut ram, overlay, message) in cases {
            let before = ram.clone();
            let error = super::overlay(&mut ram, &plan(), &overlay).unwrap_err().to_string();
            assert!(error.contains(message), "{error} does not contain {message}");
            assert_eq!(ram, before, "a refused overlay changed the image: {message}");
        }
        // A refused edit after valid regions leaves the regions unwritten as well.
        let mut ram = image();
        let overlay = Overlay { regions: vec![region(0x200, &[1])], edits: vec![edit(0x10, 0)] };
        assert!(super::overlay(&mut ram, &plan(), &overlay).is_err());
        assert_eq!(ram, image());
    }
    #[test]
    fn reads_region_files_beside_the_description_and_checks_their_hashes() {
        let directory = std::env::temp_dir().join(format!("fujimod-overlay-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("region.bin"), [1, 2, 3, 4]).unwrap();
        let describe = |sha256: &str, file: &str| {
            serde_json::json!({
                "regions": [{"name": "mods", "address": 0x200, "size": 4, "file": file, "sha256": sha256}],
                "edits": [{"address": 0x10, "old": 0x1111_1111u32, "new": 0x9400_0000u32, "what": "hook"}]
            })
            .to_string()
        };
        let path = directory.join("overlay.json");
        std::fs::write(&path, describe(&fujimod_firmware::sha256(&[1, 2, 3, 4]), "region.bin")).unwrap();
        let read = Overlay::read(&path).unwrap();
        let regions: Vec<_> = read.regions.iter().map(|r| (r.name.as_str(), r.address, r.bytes.as_slice())).collect();
        assert_eq!(regions, vec![("mods", 0x200, &[1u8, 2, 3, 4][..])]);
        let edits: Vec<_> = read.edits.iter().map(|e| (e.address, e.old, e.new, e.what.as_str())).collect();
        assert_eq!(edits, vec![(0x10, 0x1111_1111, 0x9400_0000, "hook")]);
        std::fs::write(&path, describe(&"0".repeat(64), "region.bin")).unwrap();
        assert!(Overlay::read(&path).unwrap_err().to_string().contains("differs from its description"));
        std::fs::write(&path, describe(&fujimod_firmware::sha256(&[1, 2, 3, 4]), "../region.bin")).unwrap();
        assert!(Overlay::read(&path).unwrap_err().to_string().contains("must be a basename"));
        std::fs::remove_dir_all(&directory).unwrap();
    }
}
