//! FAT long-name records: Microsoft FAT Specification, August 30, 2005, §7.
use super::{BTreeSet, err, short_name, u16_at};
const POSITIONS: [usize; 13] = [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30];
fn checksum(name: &[u8]) -> u8 {
    name.iter()
        .fold(0u8, |sum, &b| sum.rotate_right(1).wrapping_add(b))
}
pub(super) fn valid(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && name != "."
        && name != ".."
        && !name.ends_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'~' | b'.'))
}
pub(super) fn records(
    name: &str,
    reserved: &mut BTreeSet<[u8; 11]>,
) -> Result<([u8; 11], Vec<[u8; 32]>), String> {
    if let Ok(short) = short_name(name) {
        return Ok((short, vec![]));
    }
    if !valid(name) {
        return err("Invalid bounded ASCII long name");
    }
    let (stem, ext) = name.rsplit_once('.').unwrap_or((name, ""));
    let stem: String = stem.chars().filter(|c| *c != '.').collect();
    for n in 1..=4096 {
        let suffix = format!("~{n}");
        let base = format!("{}{}", &stem[..stem.len().min(8 - suffix.len())], suffix);
        let alias = if ext.is_empty() {
            base
        } else {
            format!("{base}.{}", &ext[..ext.len().min(3)])
        };
        let short = short_name(&alias)?;
        if !reserved.insert(short) {
            continue;
        }
        let chars: Vec<_> = name.encode_utf16().collect();
        let count = chars.len().div_ceil(13);
        let mut entries = vec![];
        for i in (0..count).rev() {
            let mut raw = [0; 32];
            raw[0] = (i + 1) as u8 | if i + 1 == count { 0x40 } else { 0 };
            raw[11] = 15;
            raw[13] = checksum(&short);
            for (j, offset) in POSITIONS.into_iter().enumerate() {
                let index = i * 13 + j;
                let value = chars
                    .get(index)
                    .copied()
                    .unwrap_or(if index == chars.len() { 0 } else { 0xffff });
                raw[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
            }
            entries.push(raw);
        }
        return Ok((short, entries));
    }
    err("FAT short-name alias bound exhausted")
}
#[derive(Default)]
pub(super) struct Pending {
    next: u8,
    checksum: u8,
    chars: Vec<u16>,
}
impl Pending {
    pub fn push(&mut self, raw: &[u8; 32]) -> Result<(), String> {
        let ordinal = raw[0] & 0x3f;
        if raw[0] & 0x40 != 0 {
            if self.next != 0 || ordinal == 0 || ordinal > 20 {
                return err("Invalid FAT long-name sequence");
            }
            self.next = ordinal;
            self.checksum = raw[13];
            self.chars = vec![0xffff; ordinal as usize * 13];
        }
        if ordinal == 0
            || ordinal != self.next
            || raw[0] & 0x80 != 0
            || raw[12] != 0
            || u16_at(raw, 26) != 0
            || raw[13] != self.checksum
        {
            return err("Invalid FAT long-name record");
        }
        for (j, offset) in POSITIONS.into_iter().enumerate() {
            self.chars[(ordinal as usize - 1) * 13 + j] = u16_at(raw, offset);
        }
        self.next -= 1;
        Ok(())
    }
    pub fn finish(&mut self, short: &[u8]) -> Result<Option<String>, String> {
        if self.chars.is_empty() {
            return Ok(None);
        }
        if self.next != 0 || self.checksum != checksum(short) {
            return err("FAT long-name checksum or sequence mismatch");
        }
        let end = self
            .chars
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(self.chars.len());
        if self.chars[end..].iter().skip(1).any(|&c| c != 0xffff) {
            return err("Invalid FAT long-name padding");
        }
        let name =
            String::from_utf16(&self.chars[..end]).map_err(|_| "Invalid FAT long-name UTF-16")?;
        if !valid(&name) {
            return err("Unsupported FAT long name");
        }
        *self = Self::default();
        Ok(Some(name.to_ascii_uppercase()))
    }
    pub fn clear(&mut self) {
        *self = Self::default();
    }
}
