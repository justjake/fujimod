//! Framing for byte-lane register writes and path geometry commands.
//!
//! The decoder handles packet boundaries, operand counts and trailing alignment.
//! Register laws and supported rendering modes are checked by the consumers.

use fujimod_emulator_devices::record::{Result, Unsupported};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Write {
    pub index: u32,
    pub lanes: u32,
    pub value: u32,
}

impl Write {
    pub fn mask(self) -> u64 {
        (u64::from(self.lanes & 1 != 0) * 0xff)
            | (u64::from(self.lanes & 2 != 0) * 0xff00)
            | (u64::from(self.lanes & 4 != 0) * 0xff0000)
            | (u64::from(self.lanes & 8 != 0) * 0xff000000)
    }

    pub fn apply(self, previous: u64) -> u64 {
        let mask = self.mask();
        previous & !mask | u64::from(self.value) & mask
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command<'a> {
    Write(Write),
    Geometry {
        code: u32,
        flags: u32,
        operands: &'a [u32],
    },
}

pub fn geometry_words(code: u32) -> Option<usize> {
    match code {
        0x28..=0x2c => Some(2),
        0x30 => Some(4),
        0x31 => Some(6),
        0x32 | 0x33 => Some(8),
        _ => None,
    }
}

pub fn decode(words: &[u32], path: bool) -> Result<Vec<Command<'_>>> {
    let mut commands = Vec::new();
    visit(words, path, |command| commands.push(command))?;
    Ok(commands)
}

/// Validate framing and visit commands without allocating a command list.
pub fn visit<'a>(words: &'a [u32], path: bool, mut visitor: impl FnMut(Command<'a>)) -> Result<()> {
    let fail = |message: &str| Unsupported(format!("Graphics {message}"));
    if words.is_empty() || !words.len().is_multiple_of(16) {
        return Err(fail("commands require complete 16-word-aligned lists"));
    }
    let mut position = 0;
    while position < words.len() {
        let header = words[position];
        if header == 0 {
            if words.len() - position > 15 || words[position..].iter().any(|&word| word != 0) {
                return Err(fail("commands contain interior/excessive zero padding"));
            }
            break;
        }
        position += 1;
        if header & 0xfff00000 == 0x40000000 {
            let lanes = header >> 16 & 15;
            let Some(&value) = words.get(position) else {
                return Err(fail("register write has no value"));
            };
            if lanes == 0 {
                return Err(fail("register write selects no byte lanes"));
            }
            visitor(Command::Write(Write {
                index: header & 0xffff,
                lanes,
                value,
            }));
            position += 1;
        } else if let Some(count) = geometry_words(header >> 24).filter(|_| path) {
            if header & 0xffff != 0 || header >> 16 & 0xc0 != 0 || words.len() - position < count {
                return Err(fail(
                    "geometry has unsupported flags or incomplete operands",
                ));
            }
            visitor(Command::Geometry {
                code: header >> 24,
                flags: header >> 16 & 0x3f,
                operands: &words[position..position + count],
            });
            position += count;
        } else {
            return Err(Unsupported(format!("Graphics unknown command {header:#x}")));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn aligned(mut words: Vec<u32>) -> Vec<u32> {
        words.resize(words.len().next_multiple_of(16), 0);
        words
    }

    #[test]
    fn lane_writes_preserve_unselected_bytes_and_ignore_unused_value_bits() {
        let words = aligned(vec![0x40050060, 0x12345678]);
        let decoded = decode(&words, true).unwrap();
        let Command::Write(write) = decoded[0] else {
            panic!("expected write")
        };
        assert_eq!(write.apply(0xaabbccdd), 0xaa34cc78);
    }

    #[test]
    fn geometry_operands_are_data_even_when_they_look_like_headers() {
        let words = aligned(vec![0x28000000, 0x400f008d, 0]);
        let commands = decode(&words, true).unwrap();
        assert_eq!(
            commands,
            vec![Command::Geometry {
                code: 0x28,
                flags: 0,
                operands: &words[1..3]
            }]
        );
        assert!(decode(&words, false).is_err());
    }

    #[test]
    fn malformed_framing_refuses_without_indexing_past_the_list() {
        assert!(decode(&[], true).is_err());
        assert!(decode(&[0; 17], true).is_err());
        assert!(decode(&aligned(vec![0x40000060, 1]), true).is_err());
        assert!(decode(&aligned(vec![0, 0x400f0060, 1]), true).is_err());
        assert!(decode(&[0; 16], true).is_err());
        let mut truncated = vec![0x28000000, 0, 0];
        for _ in 0..6 {
            truncated.extend([0x400f0060, 1]);
        }
        truncated.push(0x33000000);
        assert!(decode(&truncated, true).is_err());
        assert!(decode(&aligned(vec![0x28400000, 0, 0]), true).is_err());
    }
}
