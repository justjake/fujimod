//! Update container mechanisms. All mappings are explicit inputs; no body database,
//! filesystem, execution, or signature verification is involved.
use sha2::{Digest, Sha256};

mod error;
pub use error::Error;
pub type Result<T> = std::result::Result<T, Error>;
fn require(ok: bool, error: Error) -> Result<()> {
    if ok { Ok(()) } else { Err(error) }
}
fn header_value(offset: u64, field: &'static str, value: u64, expected: u64) -> Result<()> {
    require(
        value == expected,
        Error::HeaderValue {
            offset,
            field,
            value,
            expected,
        },
    )
}
fn verify_hash(data: &[u8], offset: u64, expected: &str) -> Result<()> {
    let actual = sha256(data);
    if actual == expected {
        Ok(())
    } else {
        Err(Error::Hash {
            offset,
            size: data.len() as u64,
            expected: expected.into(),
            actual,
        })
    }
}
pub fn sha256(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
pub fn invert(data: &[u8]) -> Vec<u8> {
    data.iter().map(|b| !b).collect()
}
pub fn extent(data: &[u8], offset: u64, size: u64) -> Result<&[u8]> {
    let end = offset
        .checked_add(size)
        .ok_or(Error::Overflow { offset, size })?;
    require(
        end <= data.len() as u64,
        Error::Extent {
            offset,
            size,
            available: data.len() as u64,
        },
    )?;
    Ok(&data[offset as usize..end as usize])
}
fn word(data: &[u8], offset: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(
        extent(data, offset as u64, 4)?.try_into().unwrap(),
    ))
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InnerHeader {
    pub word_bits: u8,
    pub base: u64,
    pub initialized_size: u64,
    pub entry: u64,
}
pub fn inner_header(data: &[u8]) -> Option<InnerHeader> {
    for width in [4, 8] {
        if data.len() < width * 8 {
            continue;
        }
        let values: Vec<u64> = data[..width * 8]
            .chunks_exact(width)
            .map(|v| {
                let mut bytes = [0; 8];
                bytes[..width].copy_from_slice(v);
                u64::from_le_bytes(bytes)
            })
            .collect();
        if values[0] == 0 && values[7] == 0xaaaaffff {
            return Some(InnerHeader {
                word_bits: (width * 8) as u8,
                base: values[1],
                initialized_size: values[2],
                entry: values[3],
            });
        }
    }
    None
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Compression {
    pub packed_size: u32,
    pub unpacked_size: u32,
    pub page_size: u32,
    pub page_count: u32,
    pub method: u32,
    pub literals: usize,
    pub matches: usize,
}
pub fn decompress(source: &[u8]) -> Result<(Vec<u8>, Compression)> {
    let unpacked = word(source, 0)?;
    let packed = word(source, 4)?;
    let pages = word(source, 8)?;
    header_value(12, "page size", u64::from(word(source, 12)?), 0x4000)?;
    header_value(16, "compression method", u64::from(word(source, 16)?), 4)?;
    require(
        packed >= 20 && packed as usize <= source.len(),
        Error::Extent {
            offset: 0,
            size: packed.into(),
            available: source.len() as u64,
        },
    )?;
    header_value(
        8,
        "compressed page count",
        pages.into(),
        packed.div_ceil(0x4000).into(),
    )?;
    let body = &source[20..packed as usize];
    // A token produces at most fifteen bytes from two encoded bytes. Bound before
    // allocation so a tiny untrusted header cannot request gigabytes of memory.
    require(
        u64::from(unpacked) <= body.len() as u64 * 15,
        Error::DecodedSize {
            offset: 0,
            actual: body.len() as u64 * 15,
            declared: unpacked.into(),
        },
    )?;
    let mut output = Vec::new();
    output
        .try_reserve_exact(unpacked as usize)
        .map_err(|_| Error::Allocation {
            size: unpacked.into(),
        })?;
    let (mut pos, mut literals, mut matches) = (0, 0, 0);
    while pos < body.len() {
        let command = body[pos];
        pos += 1;
        if command < 0x80 {
            let count = command as usize;
            require(
                count > 0 && pos + count <= body.len(),
                Error::Literal {
                    offset: (pos + 19) as u64,
                    length: count,
                    remaining: body.len() - pos,
                },
            )?;
            require(
                output.len() + count <= unpacked as usize,
                Error::DecodedSize {
                    offset: (pos + 20) as u64,
                    actual: (output.len() + count) as u64,
                    declared: unpacked.into(),
                },
            )?;
            output.extend_from_slice(&body[pos..pos + count]);
            pos += count;
            literals += 1;
        } else {
            require(
                pos < body.len(),
                Error::Match {
                    offset: (pos + 19) as u64,
                    distance: None,
                    length: None,
                    output_size: output.len(),
                },
            )?;
            let second = body[pos];
            pos += 1;
            let distance = ((command as usize & 0x7f) << 4) | (second as usize >> 4);
            let length = second as usize & 15;
            require(
                distance > 0 && distance <= output.len() && length > 0,
                Error::Match {
                    offset: (pos + 18) as u64,
                    distance: Some(distance),
                    length: Some(length),
                    output_size: output.len(),
                },
            )?;
            require(
                output.len() + length <= unpacked as usize,
                Error::DecodedSize {
                    offset: (pos + 20) as u64,
                    actual: (output.len() + length) as u64,
                    declared: unpacked.into(),
                },
            )?;
            for _ in 0..length {
                output.push(output[output.len() - distance]);
            }
            matches += 1;
        }
    }
    require(
        output.len() == unpacked as usize,
        Error::DecodedSize {
            offset: packed.into(),
            actual: output.len() as u64,
            declared: unpacked.into(),
        },
    )?;
    Ok((
        output,
        Compression {
            packed_size: packed,
            unpacked_size: unpacked,
            page_size: 0x4000,
            page_count: pages,
            method: 4,
            literals,
            matches,
        },
    ))
}
#[derive(Debug)]
pub struct Candidate {
    pub offset: usize,
    pub decoded: Vec<u8>,
    pub compression: Compression,
    pub header: Option<InnerHeader>,
}
pub fn scan(source: &[u8]) -> Vec<Candidate> {
    let data = invert(source);
    let mark = [0, 0x40, 0, 0, 4, 0, 0, 0];
    let mut result = Vec::new();
    let mut pos = 12;
    while pos + 8 <= data.len() {
        if data[pos..pos + 8] != mark {
            pos += 1;
            continue;
        }
        let offset = pos - 12;
        pos += 1;
        if let Ok((decoded, compression)) = decompress(&data[offset..]) {
            let header = inner_header(&decoded);
            pos = offset + compression.packed_size as usize + 12;
            result.push(Candidate {
                offset,
                decoded,
                compression,
                header,
            });
        }
    }
    result
}
/// A named initialized mapping, independent overlay bank, or data-only unit.
#[derive(Debug, Clone)]
pub struct Unit {
    pub name: String,
    pub base: u64,
    pub bytes: Vec<u8>,
    pub executable: bool,
}
#[derive(Debug)]
pub struct Image {
    pub source_sha256: String,
    pub units: Vec<Unit>,
}
#[derive(Debug, Clone)]
pub enum Encoding {
    Raw,
    Method4,
}
#[derive(Debug, Clone)]
pub struct Mapping {
    pub name: String,
    pub offset: u64,
    pub size: u64,
    pub base: u64,
    pub executable: bool,
    pub encoding: Encoding,
}
/// Decode explicit mappings. Inner headers remain separately inspectable, since
/// their initialized extent can differ from the actual decoded stream length.
pub fn decode(source: &[u8], mappings: &[Mapping]) -> Result<Image> {
    let data = invert(source);
    let mut units = Vec::new();
    for m in mappings {
        let bytes = match m.encoding {
            Encoding::Raw => extent(&data, m.offset, m.size)?.to_vec(),
            Encoding::Method4 => {
                // Validate the DAT offset before converting on Wasm32.
                extent(&data, m.offset, 0)?;
                let (mut bytes, _) = decompress(&data[m.offset as usize..])?;
                if m.size < bytes.len() as u64 {
                    bytes.truncate(m.size as usize);
                }
                bytes
            }
        };
        require(
            m.base.checked_add(bytes.len() as u64).is_some(),
            Error::Overflow {
                offset: m.base,
                size: bytes.len() as u64,
            },
        )?;
        units.push(Unit {
            name: m.name.clone(),
            base: m.base,
            bytes,
            executable: m.executable,
        });
    }
    Ok(Image {
        source_sha256: sha256(source),
        units,
    })
}
/// Verify a raw initialized mapping or explicit extent; never infer a load address.
#[derive(Debug)]
pub struct RawSpec<'a> {
    pub offset: u64,
    pub size: u64,
    pub base: u64,
    pub sha256: &'a str,
    pub header_bits: Option<u8>,
    pub entry: Option<u64>,
    pub architecture_bits: u8,
}
pub fn raw<'a>(data: &'a [u8], spec: &RawSpec<'_>) -> Result<&'a [u8]> {
    require(
        spec.size > 0 && spec.base.checked_add(spec.size).is_some(),
        Error::Overflow {
            offset: spec.base,
            size: spec.size,
        },
    )?;
    let bytes = extent(data, spec.offset, spec.size)?;
    verify_hash(bytes, spec.offset, spec.sha256)?;
    if let Some(bits) = spec.header_bits {
        let actual = inner_header(bytes);
        let valid = actual.as_ref().is_some_and(|h| {
            h.word_bits == bits
                && h.base == spec.base
                && h.initialized_size == spec.size
                && bits == spec.architecture_bits
        });
        if !valid {
            return Err(Error::RawHeader {
                offset: spec.offset,
                bits,
                base: spec.base,
                size: spec.size,
                architecture_bits: spec.architecture_bits,
                actual,
            });
        }
        let h = actual.unwrap();
        require(
            Some(h.entry) == spec.entry && h.entry >= spec.base && h.entry - spec.base < spec.size,
            Error::RawEntry {
                offset: spec.offset,
                entry: h.entry,
                expected: spec.entry,
                base: spec.base,
                size: spec.size,
            },
        )?;
    }
    Ok(bytes)
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Overlay {
    pub logical_id: u32,
    pub semaphore: u32,
    pub address: u64,
    pub size: u64,
    pub dat_offset: u64,
    pub sha256: String,
}
#[derive(Debug)]
pub struct OverlayTable<'a> {
    pub offset: u64,
    pub base: u64,
    pub size: u64,
    pub sha256: &'a str,
    pub alias: u64,
    pub flash_dat_bias: u64,
    pub section_id: u32,
    /// (load base, DAT offset) for every executable compressed stream.
    pub streams: &'a [(u64, u64)],
}
pub fn overlays(data: &[u8], spec: &OverlayTable<'_>) -> Result<Vec<Overlay>> {
    let table = extent(data, spec.offset, spec.size)?;
    verify_hash(table, spec.offset, spec.sha256)?;
    let words = |mut address: u64, count: usize| -> Result<Vec<u32>> {
        if spec.alias != 0 && address >= spec.alias {
            address -= spec.alias;
        }
        let pointer_error = || Error::DescriptorPointer {
            address,
            base: spec.base,
            size: spec.size,
            read_size: (count * 4) as u64,
        };
        let index = address.checked_sub(spec.base).ok_or_else(pointer_error)?;
        require(index % 4 == 0, pointer_error())?;
        let bytes = extent(table, index, (count * 4) as u64).map_err(|_| pointer_error())?;
        Ok(bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|v| u32::from_le_bytes(*v))
            .collect())
    };
    let roots = words(spec.base, 2)?;
    let mut sections = std::collections::BTreeMap::new();
    for i in 0..=spec.size / 36 {
        let address = u64::from(roots[0]) + i * 36;
        if words(address, 1)?[0] == u32::MAX {
            break;
        }
        let row = words(address, 9)?;
        require(
            !sections.contains_key(&row[0]),
            Error::DuplicateSection {
                address,
                id: row[0],
            },
        )?;
        sections.insert(row[0], row);
    }
    for &(base, dat) in spec.streams {
        let rows: Vec<_> = sections
            .values()
            .filter(|r| u64::from(r[1]) == base && r[8] == 1)
            .collect();
        require(
            rows.len() == 1 && u64::from(rows[0][5]).checked_add(spec.flash_dat_bias) == Some(dat),
            Error::StreamBias {
                base,
                dat_offset: dat,
                bias: spec.flash_dat_bias,
                flash_offsets: rows.iter().map(|r| r[5]).collect(),
            },
        )?;
    }
    let section = sections
        .get(&spec.section_id)
        .ok_or(Error::MissingSection {
            offset: spec.offset,
            id: spec.section_id,
        })?;
    require(
        section[8] == 0,
        Error::CompressedSection {
            id: spec.section_id,
            method: section[8],
        },
    )?;
    let group = words(u64::from(roots[1]), 3)?;
    require(
        group[2] == section[6],
        Error::OverlayGroup {
            address: roots[1].into(),
            group_size: group[2],
            section_size: section[6],
            base: group[1],
            flash_offset: section[5],
        },
    )?;
    let mut records = Vec::new();
    for i in 0..=spec.size / 24 {
        let address = u64::from(group[0]) + i * 24;
        if words(address, 1)?[0] == u32::MAX {
            return Ok(records);
        }
        let r = words(address, 6)?;
        let (run, length) = (u64::from(r[1]), u64::from(r[2]));
        require(
            length > 0
                && r[3] == 0
                && r[4] == 0
                && run >= u64::from(group[1])
                && run + length <= u64::from(group[1]) + u64::from(group[2]),
            Error::OverlayEntry {
                address,
                run,
                length,
                reserved: r[3],
                flags: r[4],
                base: group[1],
                size: group[2],
            },
        )?;
        let dat = spec
            .flash_dat_bias
            .checked_add(u64::from(section[5]))
            .and_then(|v| v.checked_add(run - u64::from(group[1])))
            .ok_or(Error::Overflow {
                offset: spec.flash_dat_bias,
                size: u64::from(section[5]) + run - u64::from(group[1]),
            })?;
        records.push(Overlay {
            logical_id: r[0],
            semaphore: r[5],
            address: run,
            size: length,
            dat_offset: dat,
            sha256: sha256(extent(data, dat, length)?),
        });
    }
    Err(Error::UnterminatedTable {
        address: group[0].into(),
        size: spec.size,
    })
}

#[cfg(test)]
mod tests;

/// Parsed outer container fields. Identity records and uninterpreted words stay
/// opaque. A DER trailer is located structurally; parsing does not authenticate it.
#[derive(Debug)]
pub struct OuterHeader<'a> {
    pub kind: u32,
    pub version_major: u32,
    pub version_minor: u32,
    pub header: &'a [u8],
    pub payload_offset: usize,
    pub signed_bytes: Option<&'a [u8]>,
    pub signature_der: Option<&'a [u8]>,
}
pub fn outer_header(source: &[u8]) -> Result<OuterHeader<'_>> {
    let kind = word(source, 0)?;
    let payload_offset = match kind {
        4 => 0x25c,
        6 => 0x27c,
        8 => 0x2bc,
        _ => {
            return Err(Error::OuterType {
                offset: 0,
                value: kind,
            });
        }
    };
    let header = extent(source, 0, payload_offset as u64)?;
    let (mut signed_bytes, mut signature_der) = (None, None);
    if kind == 8 {
        // ECDSA P-256 DER: SEQUENCE of two positive, minimally encoded INTEGERs.
        for length in 8..=72.min(source.len().saturating_sub(payload_offset)) {
            let offset = source.len() - length;
            let der = &source[offset..];
            if valid_signature_der(der) {
                signed_bytes = Some(&source[..offset]);
                signature_der = Some(der);
                break;
            }
        }
        require(
            signature_der.is_some(),
            Error::MissingSignature {
                offset: payload_offset as u64,
                size: (source.len() - payload_offset) as u64,
            },
        )?;
    }
    Ok(OuterHeader {
        kind,
        version_major: word(source, 0x204)?,
        version_minor: word(source, 0x208)?,
        header,
        payload_offset,
        signed_bytes,
        signature_der,
    })
}
fn valid_signature_der(data: &[u8]) -> bool {
    if data.len() < 8 || data[0] != 0x30 || data[1] as usize != data.len() - 2 {
        return false;
    }
    let mut pos = 2;
    for _ in 0..2 {
        if pos + 2 > data.len() || data[pos] != 2 {
            return false;
        }
        let len = data[pos + 1] as usize;
        pos += 2;
        if len == 0 || len > 33 || pos + len > data.len() {
            return false;
        }
        let v = &data[pos..pos + len];
        if v[0] & 0x80 != 0
            || (len > 1 && v[0] == 0 && v[1] & 0x80 == 0)
            || (len == 33 && v[0] != 0)
        {
            return false;
        }
        pos += len;
    }
    pos == data.len()
}
