//! ARMv7-M Thumb exclusive encodings, DUI0553B §3.4.8/9. No doubleword form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Instruction {
    Clear,
    Transfer {
        load: bool,
        base: u8,
        data: u8,
        status: u8,
        bytes: u8,
        offset: u16,
    },
}
pub fn decode(first: u16, second: u16) -> Result<Option<Instruction>, String> {
    if first == 0xf3bf && second == 0x8f2f {
        return Ok(Some(Instruction::Clear));
    }
    let base = (first & 15) as u8;
    let data = (second >> 12) as u8;
    let (load, status, bytes, offset) = match first & 0xfff0 {
        0xe850 if second & 0x0f00 == 0x0f00 => (true, 0, 4, (second & 255) * 4),
        0xe840 => (false, ((second >> 8) & 15) as u8, 4, (second & 255) * 4),
        0xe8d0 if second & 0x0fff == 0x0f4f => (true, 0, 1, 0),
        0xe8d0 if second & 0x0fff == 0x0f5f => (true, 0, 2, 0),
        0xe8c0 if second & 0x0ff0 == 0x0f40 => (false, (second & 15) as u8, 1, 0),
        0xe8c0 if second & 0x0ff0 == 0x0f50 => (false, (second & 15) as u8, 2, 0),
        0xe8d0 if second & 0x0fff == 0x0f7f => return Err("ARMv7-M has no LDREXD".into()),
        0xe8c0 if second & 0x0ff0 == 0x0f70 => return Err("ARMv7-M has no STREXD".into()),
        _ => return Ok(None),
    };
    if base == 15
        || matches!(data, 13 | 15)
        || (!load && (matches!(status, 13 | 15) || status == base || status == data))
    {
        return Err("unpredictable M-profile exclusive register".into());
    }
    Ok(Some(Instruction::Transfer {
        load,
        base,
        data,
        status,
        bytes,
        offset,
    }))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn typed_widths_and_invalid_registers() {
        for (a, b, width, load, status) in [
            (0xe850, 0x1f01, 4, true, 0),
            (0xe840, 0x1201, 4, false, 2),
            (0xe8d0, 0x1f4f, 1, true, 0),
            (0xe8c0, 0x1f42, 1, false, 2),
            (0xe8d0, 0x1f5f, 2, true, 0),
            (0xe8c0, 0x1f52, 2, false, 2),
        ] {
            let Some(Instruction::Transfer {
                bytes,
                load: l,
                status: s,
                ..
            }) = decode(a, b).unwrap()
            else {
                panic!()
            };
            assert_eq!((bytes, l, s), (width, load, status));
        }
        assert!(decode(0xe85f, 0x1f00).is_err());
        assert!(decode(0xe840, 0x1100).is_err());
        assert!(decode(0xe8d0, 0x1f7f).is_err());
        assert_eq!(decode(0xf3bf, 0x8f2f).unwrap(), Some(Instruction::Clear));
    }
}
