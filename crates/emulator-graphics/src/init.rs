//! State-only GPU submissions. Admission depends on decoded register values,
//! supported modes and bounded scissor records, not a writer's instruction order.

use super::batch::{affine, float24};
use super::clear::Descriptor;
use super::commands::{self, Command};
use super::regs::Registers;
use super::stream;
use fujimod_emulator_devices::record::{Result, Unsupported, Value};

fn fail<T>(message: &str) -> Result<T> {
    Err(Unsupported(message.into()))
}

pub(crate) fn writes(commands: &[Command<'_>], index: u32) -> bool {
    commands
        .iter()
        .any(|command| matches!(command, Command::Write(write) if write.index == index))
}

pub(crate) fn dimensions(path: &Registers, max_bytes: u64) -> Result<(u64, u64, u64)> {
    let dimensions = path.get(0x7b).unwrap_or(0);
    if dimensions > u64::from(u32::MAX) {
        return fail("Graphics initializer dimensions exceed the register width");
    }
    let (width, height) = (dimensions >> 16, dimensions & 0xffff);
    let frame_bytes = width * height * 4;
    if width == 0
        || height == 0
        || width & 7 != 0
        || height & 7 != 0
        || width * height > 0x100000
        || frame_bytes > max_bytes
    {
        return fail("Graphics initializer has unsupported framebuffer dimensions");
    }
    Ok((width, height, frame_bytes))
}

pub(crate) fn validate_fill(fill: &Registers, width: u64, height: u64) -> Result<u64> {
    // These controls select the implemented unmasked, inactive-sampler state.
    // Upload values are separately checked by the stream decoder.
    for (index, expected) in [
        (0x140, 0x50000),
        (0x190, 0),
        (0x1c0, 0),
        (0x14d, 0),
        (0x143, 0),
        (0x153, 0),
        (0x144, 0),
        (0x154, 0),
        (0x147, 1),
        (0x157, 1),
        (0x178, 1),
        (0x16c, 0x40001),
        (0x187, 0),
        (0x1a2, 0),
    ] {
        if fill.get(index) != Some(expected) {
            return fail("Graphics initializer has unsupported mask, paint or sampler state");
        }
    }
    if fill.get(0x1a3) != Some(height << 16 | width) {
        return fail("Graphics initializer path/fill dimensions disagree");
    }
    let format = fill
        .get(0x1a7)
        .ok_or_else(|| Unsupported("Graphics initializer target format is missing".into()))?;
    if format & 0xffff > 0x101 || format & 0xff00 != 0x100 {
        return fail("Graphics initializer target format/order is unsupported");
    }
    Ok(format & 255)
}

pub fn initialize(
    path_words: &[u32],
    fill_words: &[u32],
    initial_path: Option<&Registers>,
    initial_fill: Option<&Registers>,
    max_bytes: u64,
) -> Result<(Descriptor, (Registers, Registers))> {
    let (path_jobs, path_bank) = stream::parse(path_words, true, initial_path)?;
    let (fill_jobs, fill_bank) = stream::parse(fill_words, false, initial_fill)?;
    if !path_jobs.is_empty() || !fill_jobs.is_empty() {
        return fail("Graphics state-only initialization cannot contain draws");
    }
    let path_commands = commands::decode(path_words, true)?;
    let fill_commands = commands::decode(fill_words, false)?;
    super::gamma::validate_upload(&fill_commands)?;
    let (width, height, frame_bytes) = dimensions(&path_bank, max_bytes)?;
    let mode = path_bank.get(0x60).unwrap_or(255) & 255;
    if mode > 2
        || path_bank
            .get(0x81)
            .is_none_or(|v| v & 0xff0000ff != 0x1000001)
    {
        return fail("Graphics initializer sample mode or path controls are unsupported");
    }
    let matrix_refresh = (0x40..=0x48).any(|index| writes(&path_commands, index));
    if matrix_refresh {
        affine(&path_bank, 0x40, false, true)?;
    }
    let order = validate_fill(&fill_bank, width, height)?;
    let count_refresh = writes(&fill_commands, 0x100);
    let mode_refresh = writes(&fill_commands, 0x115);
    if mode_refresh
        && fill_bank
            .get(0x115)
            .is_none_or(|v| !matches!(v & !1, 0 | 0x10000 | 0x1000000))
    {
        return fail("Graphics initializer scissor enable is unsupported");
    }
    let mut clips = Vec::new();
    let has_records = (0xc0..=0xdf).any(|index| writes(&fill_commands, index));
    if has_records || count_refresh {
        let count = fill_bank
            .get(0x100)
            .filter(|&count| count <= 16)
            .ok_or_else(|| {
                Unsupported(
                    "Graphics initializer scissor count is missing or outside its bound".into(),
                )
            })?;
        for index in 0..count as u32 {
            let horizontal = fill_bank.get(0xc0 + 2 * index);
            let vertical = fill_bank.get(0xc1 + 2 * index);
            let (Some(horizontal), Some(vertical)) = (horizontal, vertical) else {
                return fail("Graphics initializer scissor record is incomplete");
            };
            let (x0, y0, x1, y1) = (
                horizontal & 0xffff,
                vertical & 0xffff,
                (horizontal >> 16) + 1,
                (vertical >> 16) + 1,
            );
            if x0 >= x1 || y0 >= y1 {
                return fail("Graphics initializer scissor has reversed endpoints");
            }
            if writes(&fill_commands, 0xc0 + 2 * index) || writes(&fill_commands, 0xc1 + 2 * index)
            {
                clips.push((x0, y0, x1, y1));
            }
        }
        if (0xc0 + count as u32 * 2..=0xdf).any(|index| writes(&fill_commands, index)) {
            return fail("Graphics initializer scissor writes exceed the declared count");
        }
    }
    // Validate every matrix operand even in a partial update. Other entries
    // can legitimately come from the preceding command bank.
    for command in &path_commands {
        if let Command::Write(write) = *command
            && (0x40..=0x48).contains(&write.index)
        {
            float24(path_bank.at(write.index) & 0xffffff)?;
        }
    }
    let descriptor = vec![
        ("operation", Value::str("initialize")),
        ("first_global", Value::Bool(false)),
        ("width", Value::int(width)),
        ("height", Value::int(height)),
        ("frame_bytes", Value::int(frame_bytes)),
        ("path_mode", Value::int(mode)),
        ("order", Value::int(order)),
        ("matrix_refresh", Value::Bool(matrix_refresh)),
        ("scissor_count_refresh", Value::Bool(count_refresh)),
        ("scissor_enable_refresh", Value::Bool(mode_refresh)),
        (
            "refreshed_scissors",
            Value::List(
                clips
                    .iter()
                    .map(|&(a, b, c, d)| {
                        Value::Tuple(vec![
                            Value::int(a),
                            Value::int(b),
                            Value::int(c),
                            Value::int(d),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "pixel_effect",
            Value::str(
                "None: validated repeated initializer/dirty state branches contain no geometry or draw",
            ),
        ),
    ];
    Ok((descriptor, (path_bank, fill_bank)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scissor_lane_preserves_supported_paint_modes() {
        // Image/gradient commands independently write lanes 8/4 while lane 1
        // carries the scissor mode. These banks exercise the register law.
        let mut path = Registers::new();
        path.set(0x7b, 8 << 16 | 8);
        path.set(0x60, 0);
        path.set(0x81, 0x1000001);
        let mut fill = Registers::new();
        for (index, value) in [
            (0x140, 0x50000),
            (0x190, 0),
            (0x1c0, 0),
            (0x14d, 0),
            (0x143, 0),
            (0x153, 0),
            (0x144, 0),
            (0x154, 0),
            (0x147, 1),
            (0x157, 1),
            (0x178, 1),
            (0x16c, 0x40001),
            (0x187, 0),
            (0x1a2, 0),
            (0x1a3, 8 << 16 | 8),
            (0x1a7, 0x100),
        ] {
            fill.set(index, value);
        }
        let mut path_words = vec![0x400f007b, 8 << 16 | 8];
        path_words.resize(16, 0);
        let mut fill_words = vec![
            0x40010115, 1, 0x400f0028, 0x12345678, 0x40010031, 0xf0, 0x400f0018, 0x12345678,
        ];
        fill_words.resize(16, 0);
        for mode in [0, 0x10000, 0x1000000] {
            fill.set(0x115, mode);
            let (_, (_, result)) =
                initialize(&path_words, &fill_words, Some(&path), Some(&fill), 256).unwrap();
            assert_eq!(result.get(0x115), Some(mode | 1));
        }
        for mode in [0x100, 0x20000, 0x2000000, 0x1010000] {
            fill.set(0x115, mode);
            assert!(initialize(&path_words, &fill_words, Some(&path), Some(&fill), 256).is_err());
        }
    }
}
