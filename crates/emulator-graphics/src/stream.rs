//! Byte-lane command execution and draw-fence sampling.
//!
//! Sequence numbers can start anywhere and wrap FFFF to 1. Geometry and
//! rendering admission are independent of the packet framing decoder.

use super::commands::{self, Command};
use super::regs::Registers;
use fujimod_emulator_devices::record::{Result, Unsupported};

pub const UI_RAMP_VALUES: u32 = 0x10000;
pub const UI_RAMP_LANES: u32 = 0x10100;
pub const UI_RAMP_CURSOR: u32 = 0x10200;
pub type Geometry = (u32, u32, Vec<f64>);

#[derive(Clone, Debug, PartialEq)]
pub struct StreamDraw {
    pub sequence: u64,
    pub registers: Registers,
    pub geometry: Option<Vec<Geometry>>,
}

fn known(path: bool, index: u32) -> bool {
    if path {
        matches!(index, 0x10 | 0x20 | 0x30 | 0x60 | 0x69 | 0x6a | 0x7b | 0x7c | 0x80 | 0x81 | 0x8b | 0x8c
            | 0x8d | 0x8e | 0x40..=0x48 | 0x61..=0x68 | 0x84..=0x88)
    } else {
        matches!(index, 0x18 | 0x28 | 0x31 | 0xa0 | 0xa2 | 0xad | 0x100 | 0x115 | 0x117 | 0x118 | 0x11b
            | 0x140 | 0x143 | 0x144 | 0x147 | 0x14d | 0x14e | 0x152..=0x157 | 0x16c | 0x168 | 0x16b
            | 0x169 | 0x16a | 0x16d | 0x178 | 0x184 | 0x185 | 0x187 | 0x188 | 0x190 | 0x1b0 | 0x1c0 | 0xa4..=0xa8 | 0xc0..=0xdf
            | 0x106..=0x114 | 0x180..=0x183 | 0x1a0..=0x1a5 | 0x1a7..=0x1aa)
    }
}

fn fail<T>(message: impl Into<String>) -> Result<T> {
    Err(Unsupported(message.into()))
}

pub fn parse(
    words: &[u32],
    path: bool,
    initial: Option<&Registers>,
) -> Result<(Vec<StreamDraw>, Registers)> {
    let mut registers = initial.cloned().unwrap_or_default();
    let mut draws = Vec::new();
    let mut geometry = Vec::new();
    let mut geometry_started = false;
    let mut geometry_original: [Option<Option<u64>>; 12] = [None; 12];
    let (mut previous, mut pending) = (None::<u64>, None::<u64>);
    const STATE_KEYS: [u32; 12] = [
        0x40, 0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x60, 0x69, 0x6a,
    ];
    let mut semantic_error = None;
    let mut tail = [None; 3];
    let mut command_count = 0usize;
    let mut process = |command| -> Result<()> {
        match command {
            Command::Write(write) => {
                let (index, lanes, value) = (write.index, write.lanes, u64::from(write.value));
                if !known(path, index) {
                    return fail(format!(
                        "Graphics unknown {} register {index:#x}",
                        if path { "path" } else { "fill" }
                    ));
                }
                let (previous_value, changed) = registers.write_masked(index, write.mask(), value);
                if changed
                    && geometry_started
                    && let Some(state_index) = STATE_KEYS.iter().position(|&key| key == index)
                    && geometry_original[state_index].is_none()
                {
                    geometry_original[state_index] = Some(previous_value);
                }
                if !path && (index == 0x16b || index == 0x16c) {
                    if registers.get(0x140).unwrap_or(0) & 0x400 != 0 {
                        if index == 0x16b {
                            let selector = registers.at(index);
                            if lanes != 15 || selector & !0x1007f != 0 {
                                return fail("Graphics UI gradient ramp selector is unsupported");
                            }
                            registers.set(UI_RAMP_CURSOR, selector);
                        } else {
                            let cursor = registers.get(UI_RAMP_CURSOR);
                            if cursor.is_none_or(|c| c & !0x1007f != 0)
                                || ![3, 12, 15].contains(&lanes)
                            {
                                return fail(
                                    "Graphics UI gradient ramp upload has no bounded index/lanes",
                                );
                            }
                            let cursor = cursor.unwrap();
                            let slot = ((cursor >> 16) * 128 + (cursor & 127)) as u32;
                            let mask = write.mask();
                            registers.set(
                                UI_RAMP_VALUES + slot,
                                registers.get(UI_RAMP_VALUES + slot).unwrap_or(0) & !mask
                                    | value & mask,
                            );
                            registers.set(
                                UI_RAMP_LANES + slot,
                                registers.get(UI_RAMP_LANES + slot).unwrap_or(0) | u64::from(lanes),
                            );
                            registers.set(UI_RAMP_CURSOR, cursor + 1);
                        }
                    } else if index == 0x16c && write.apply(0) != 0x40001 & write.mask() {
                        // Written lanes must agree with the only supported value; a
                        // draw refuses any other retained value (see batch.rs).
                        return fail("Graphics UI non-gradient ramp/filter write is unsupported");
                    }
                }
                let marker = match (path, index) {
                    (true, 0x10 | 0x20) | (false, 0x18 | 0x28) => Some(0x12345678),
                    (true, 0x30) | (false, 0x31) => Some(0xf0),
                    _ => None,
                };
                if marker.is_some_and(|m| registers.get(index) != Some(m)) {
                    return fail("Graphics UI unknown stream flush marker");
                }
                if (path && index == 0x8d) || (!path && index == 0xad) {
                    let mut sequence = registers.at(index);
                    if sequence >> 16 != 1 || sequence & 0xffff == 0 {
                        return fail("Graphics UI unknown synchronization word");
                    }
                    sequence &= 0xffff;
                    if let Some(previous) = previous
                        && sequence != if previous == 0xffff { 1 } else { previous + 1 }
                    {
                        return fail("Graphics UI nonconsecutive draw fences");
                    }
                    previous = Some(sequence);
                    if path {
                        if geometry.is_empty() {
                            return fail("Graphics path draw has no geometry");
                        }
                        if geometry_started
                            && STATE_KEYS.iter().enumerate().any(|(index, &key)| {
                                geometry_original[index]
                                    .is_some_and(|previous| registers.get(key) != previous)
                            })
                        {
                            return fail(
                                "Graphics UI path transform/style changed during geometry",
                            );
                        }
                        draws.push(StreamDraw {
                            sequence,
                            registers: registers.clone(),
                            geometry: Some(std::mem::take(&mut geometry)),
                        });
                        geometry_started = false;
                        geometry_original = [None; 12];
                    } else {
                        if pending.is_some() {
                            return fail("Graphics UI fill fence is missing its draw");
                        }
                        pending = Some(sequence);
                    }
                } else if !path && index == 0x1a9 {
                    if pending.is_none() || registers.get(index) != Some(3) {
                        return fail(
                            "Graphics UI draw requires one preceding fill fence and control3",
                        );
                    }
                    draws.push(StreamDraw {
                        sequence: pending.take().unwrap(),
                        registers: registers.clone(),
                        geometry: None,
                    });
                }
            }
            Command::Geometry {
                code,
                flags,
                operands,
            } => {
                let values: Vec<f64> = operands
                    .iter()
                    .map(|&word| f64::from(f32::from_bits(word)))
                    .collect();
                if values
                    .iter()
                    .any(|v| !(v.is_finite() || code == 0x2c && v.is_nan()) || v.abs() > 16777216.0)
                {
                    return fail("Graphics UI geometry is nonfinite or outside the declared bound");
                }
                if !geometry_started {
                    geometry_started = true;
                    geometry_original = [None; 12];
                }
                geometry.push((code, flags, values));
            }
        }
        Ok(())
    };
    commands::visit(words, path, |command| {
        tail.copy_within(1..3, 0);
        tail[2] = Some(command);
        command_count += 1;
        if semantic_error.is_none()
            && let Err(error) = process(command)
        {
            semantic_error = Some(error);
        }
    })?;
    drop(process);
    if !path {
        // These are three flush register operations, not an opaque footer blob.
        // Unselected value bits are ignored by the same lane law as any write.
        let flush = [
            (0x28, 15, 0x12345678),
            (0x31, 1, 0xf0),
            (0x18, 15, 0x12345678),
        ];
        if command_count < 3
            || tail.iter().copied().zip(flush).any(|(command, (index, lanes, value))| {
                !matches!(command, Some(Command::Write(write)) if write.index == index && write.lanes == lanes && write.apply(0) == value)
            })
        {
            return fail("Graphics fill list requires all three terminal flush operations");
        }
    }
    if let Some(error) = semantic_error {
        return Err(error);
    }
    if !geometry.is_empty() || pending.is_some() {
        return fail("Graphics UI list ends before its draw fence");
    }
    Ok((draws, registers))
}
