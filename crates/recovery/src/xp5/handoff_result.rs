//! The result file that the XP5 SD loader's stop 5 writes.
//!
//! The card carries a preallocated `FFRESULT.BIN`. Every fresh sector is
//! [`MAGIC`], its little-endian index and `0xa5` in bytes 12-511; the loader
//! writes a sector only after reading that whole pattern, because a written
//! header starts with the same magic and index. Sector 0 is the header, sectors 1-2 echo the
//! requested ranges, and each range's words start on their own sector from
//! sector 3, zero-padded.
use crate::image::{Result, require};
use crate::xp4::crc32;
use serde::{Deserialize, Serialize};

pub const NAME: &str = "FFRESULT.BIN";
pub const BYTES: usize = 1 << 20;
pub const MAGIC: [u8; 8] = *b"FFRSLT1\0";
/// The loader's range table holds this many entries.
pub const RANGES: usize = 64;
const SECTOR: usize = 512;
const ECHO_SECTOR: usize = 1;
const DATA_SECTOR: usize = 3;
const VERSION: u32 = 1;
const ECHOED: u32 = 2;
const DONE: u32 = 3;
const COMPLETE: u32 = 1;

/// Consecutive 32-bit words that stop 5 reads with 32-bit loads.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HandoffRange {
    pub address: u64,
    pub words: u32,
}

fn sectors(words: u32) -> usize {
    (words as usize).div_ceil(SECTOR / 4)
}

/// Check ranges against the loader's table and the file's capacity.
pub(crate) fn check(ranges: &[HandoffRange]) -> Result<()> {
    require(ranges.len() <= RANGES, "At most 64 XP5 handoff ranges")?;
    let mut total = DATA_SECTOR;
    for range in ranges {
        require(
            range.words > 0
                && range.address.is_multiple_of(4)
                && range.address + 4 * u64::from(range.words) <= 1 << 32,
            "Invalid XP5 handoff range",
        )?;
        total += sectors(range.words);
    }
    require(
        total * SECTOR <= BYTES,
        "XP5 handoff ranges exceed the result file",
    )
}

/// A fresh result file.
pub fn pattern() -> Vec<u8> {
    let mut bytes = vec![0xa5; BYTES];
    for (index, sector) in bytes.as_chunks_mut::<SECTOR>().0.iter_mut().enumerate() {
        sector[..8].copy_from_slice(&MAGIC);
        sector[8..12].copy_from_slice(&(index as u32).to_le_bytes());
    }
    bytes
}

/// One range the loader completed, checked against its CRC32.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct HandoffRangeResult {
    pub address: u64,
    pub words: Vec<u32>,
}

/// A result file as the loader left it. `ranges` holds the completed ranges
/// in request order; with `complete` false, `current` is the range whose
/// read had begun.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct HandoffResult {
    pub complete: bool,
    pub stage: u32,
    pub error: i32,
    pub range_count: u32,
    pub completed: u32,
    pub current: u32,
    pub ocr: u32,
    pub rca: u32,
    pub capacity: u64,
    pub image_bytes: u64,
    pub image_crc32: u32,
    pub current_el: u64,
    pub sctlr: u64,
    pub arrivals: [u32; 4],
    pub ranges: Vec<HandoffRangeResult>,
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}
fn u64_at(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap())
}

impl HandoffResult {
    pub fn parse(file: &[u8]) -> Result<Self> {
        require(file.len() == BYTES, "Result file has the wrong size")?;
        let header = &file[..SECTOR];
        require(
            header[..8] == MAGIC && u32_at(header, 8) == 0,
            "Result header is missing",
        )?;
        require(
            u32_at(header, 508) == crc32(&header[..508]),
            "Result header was never written or is damaged",
        )?;
        require(u32_at(header, 12) == VERSION, "Unsupported result version")?;
        let (stage, flags) = (u32_at(header, 16), u32_at(header, 24));
        let (range_count, completed, current) =
            (u32_at(header, 28), u32_at(header, 32), u32_at(header, 36));
        require(
            range_count as usize <= RANGES
                && completed <= current
                && current <= range_count
                && (stage >= ECHOED || completed == 0),
            "Invalid result progress",
        )?;
        let mut ranges = Vec::new();
        let mut first = DATA_SECTOR;
        for k in 0..completed as usize {
            let echo = ECHO_SECTOR * SECTOR + 16 * k;
            let (address, words) = (u64_at(file, echo), u32_at(file, echo + 8));
            require(
                words > 0 && u32_at(file, echo + 12) as usize == first,
                "Result request echo is inconsistent",
            )?;
            let start = first * SECTOR;
            let end = start + 4 * words as usize;
            require(end <= BYTES, "Result range exceeds the file")?;
            let data = &file[start..end];
            require(
                crc32(data) == u32_at(header, 104 + 4 * k),
                "Result range CRC32 differs",
            )?;
            ranges.push(HandoffRangeResult {
                address,
                words: data
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|w| u32::from_le_bytes(*w))
                    .collect(),
            });
            first += sectors(words);
        }
        Ok(Self {
            complete: stage == DONE && flags & COMPLETE != 0 && completed == range_count,
            stage,
            error: u32_at(header, 20) as i32,
            range_count,
            completed,
            current,
            ocr: u32_at(header, 40),
            rca: u32_at(header, 44),
            capacity: u64_at(header, 48),
            image_bytes: u64_at(header, 56),
            image_crc32: u32_at(header, 64),
            current_el: u64_at(header, 72),
            sctlr: u64_at(header, 80),
            arrivals: std::array::from_fn(|n| u32_at(header, 88 + 4 * n)),
            ranges,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lay out a file the way the loader writes it.
    fn written(ranges: &[(u64, Vec<u32>)], completed: usize, done: bool) -> Vec<u8> {
        let mut file = pattern();
        let mut header = vec![0u8; SECTOR];
        header[..8].copy_from_slice(&MAGIC);
        let mut set =
            |at: usize, value: u32| header[at..at + 4].copy_from_slice(&value.to_le_bytes());
        set(12, VERSION);
        set(16, if done { DONE } else { ECHOED });
        set(24, u32::from(done));
        set(28, ranges.len() as u32);
        set(32, completed as u32);
        set(36, completed as u32);
        set(88, 0xffff_ffff);
        let mut first = DATA_SECTOR;
        for (k, (address, words)) in ranges.iter().enumerate() {
            let echo = ECHO_SECTOR * SECTOR + 16 * k;
            file[echo..echo + 8].copy_from_slice(&address.to_le_bytes());
            file[echo + 8..echo + 12].copy_from_slice(&(words.len() as u32).to_le_bytes());
            file[echo + 12..echo + 16].copy_from_slice(&(first as u32).to_le_bytes());
            if k < completed {
                let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
                let at = first * SECTOR;
                file[at..at + sectors(words.len() as u32) * SECTOR].fill(0);
                file[at..at + bytes.len()].copy_from_slice(&bytes);
                set(104 + 4 * k, crc32(&bytes));
            }
            first += sectors(words.len() as u32);
        }
        let crc = crc32(&header[..508]);
        header[508..].copy_from_slice(&crc.to_le_bytes());
        file[..SECTOR].copy_from_slice(&header);
        file
    }

    #[test]
    fn complete_file_returns_every_range_in_request_order() {
        let ranges = vec![
            (0xff90e104, vec![1, 2, 3]),
            (0xffa11000, (0..200).collect()),
        ];
        let result = HandoffResult::parse(&written(&ranges, 2, true)).unwrap();
        assert!(result.complete);
        assert_eq!(result.arrivals, [0xffff_ffff, 0, 0, 0]);
        assert_eq!(result.ranges.len(), 2);
        assert_eq!(
            result.ranges[0],
            HandoffRangeResult {
                address: 0xff90e104,
                words: vec![1, 2, 3]
            }
        );
        assert_eq!(result.ranges[1].words, (0..200).collect::<Vec<u32>>());
    }

    #[test]
    fn interrupted_file_returns_only_completed_ranges() {
        let ranges = vec![(0x1000, vec![7]), (0x2000, vec![8])];
        let result = HandoffResult::parse(&written(&ranges, 1, false)).unwrap();
        assert!(!result.complete);
        assert_eq!((result.completed, result.current), (1, 1));
        assert_eq!(
            result.ranges,
            vec![HandoffRangeResult {
                address: 0x1000,
                words: vec![7]
            }]
        );
    }

    #[test]
    fn fresh_or_corrupted_files_are_refused() {
        assert!(HandoffResult::parse(&pattern()).is_err());
        let mut file = written(&[(0x1000, vec![7])], 1, true);
        file[DATA_SECTOR * SECTOR] ^= 1;
        assert!(HandoffResult::parse(&file).is_err());
        assert!(HandoffResult::parse(&file[..SECTOR]).is_err());
    }

    #[test]
    fn ranges_are_bounded_by_the_table_and_the_file() {
        let one = HandoffRange {
            address: 0xff90e104,
            words: 3,
        };
        assert!(check(std::slice::from_ref(&one)).is_ok());
        assert!(check(&vec![one.clone(); 65]).is_err());
        for broken in [
            HandoffRange {
                address: 0xff90e106,
                words: 1,
            },
            HandoffRange {
                address: 0x1000,
                words: 0,
            },
            HandoffRange {
                address: 0xffff_fffc,
                words: 2,
            },
            HandoffRange {
                address: 0,
                words: (BYTES / 4) as u32,
            },
        ] {
            assert!(check(&[broken]).is_err());
        }
        assert_eq!(pattern()[SECTOR + 8..SECTOR + 12], 1u32.to_le_bytes());
    }
}
