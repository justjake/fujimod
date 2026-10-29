use crate::{Error, Result, decompress, header_value};

/// Replace a method-4 stream's decoded bytes, retaining valid original matches.
///
/// A changed match becomes a literal run. Later matches are checked against the
/// replacement bytes, including overlapping copies, so edits cannot propagate
/// accidentally through the original dictionary. Unchanged inputs stay identical.
pub fn recompress(source: &[u8], replacement: &[u8]) -> Result<Vec<u8>> {
    let (_, original) = decompress(source)?;
    header_value(
        0,
        "replacement size",
        replacement.len() as u64,
        original.unpacked_size.into(),
    )?;
    let mut output = source[..20].to_vec();
    let mut cursor = 20;
    let mut written = 0;
    while cursor < original.packed_size as usize {
        let token = source[cursor];
        cursor += 1;
        let count;
        if token < 128 {
            count = token as usize;
            output.push(token);
            output.extend_from_slice(&replacement[written..written + count]);
            cursor += count;
        } else {
            let second = source[cursor];
            cursor += 1;
            count = (second & 15) as usize;
            let distance = ((token as usize & 127) << 4) | (second as usize >> 4);
            if (0..count).all(|n| replacement[written + n] == replacement[written + n - distance]) {
                output.extend_from_slice(&[token, second]);
            } else {
                output.push(count as u8);
                output.extend_from_slice(&replacement[written..written + count]);
            }
        }
        written += count;
    }
    let packed = u32::try_from(output.len()).map_err(|_| Error::Overflow {
        offset: 0,
        size: output.len() as u64,
    })?;
    output[4..8].copy_from_slice(&packed.to_le_bytes());
    output[8..12].copy_from_slice(&packed.div_ceil(0x4000).to_le_bytes());
    Ok(output)
}
