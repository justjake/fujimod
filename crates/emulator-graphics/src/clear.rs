//! Bounded rectangle and state-only admission over decoded GPU commands.
//!
//! Packet order and byte lanes are handled by the stream interpreter. This
//! module checks the rendering law, RAM ranges, scratch layout and paired fences.

use super::batch::affine;
use super::commands;
use super::init;
use super::regs::Registers;
use super::stream;
use fujimod_emulator_devices::record::{Result, Unsupported, Value};

fn fail<T>(message: impl Into<String>) -> Result<T> {
    Err(Unsupported(message.into()))
}

pub type Descriptor = Vec<(&'static str, Value)>;

pub fn get<'a>(descriptor: &'a Descriptor, key: &str) -> Option<&'a Value> {
    descriptor
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, value)| value)
}

pub fn int(descriptor: &Descriptor, key: &str) -> i128 {
    match get(descriptor, key) {
        Some(Value::Int(value)) => *value,
        Some(Value::Bool(value)) => i128::from(*value),
        _ => panic!("descriptor field {key} is not an int"),
    }
}

fn set(descriptor: &mut Descriptor, key: &'static str, value: Value) {
    match descriptor.iter_mut().find(|(k, _)| *k == key) {
        Some(slot) => slot.1 = value,
        None => descriptor.push((key, value)),
    }
}

fn bounded_ram(address: u64, count: u64, ram_bytes: u64, name: &str) -> Result<u64> {
    let alias = address & 0xc0000000;
    let physical = address - alias;
    if (alias != 0 && alias != 0x40000000)
        || address & 3 != 0
        || physical
            .checked_add(count)
            .is_none_or(|end| end > ram_bytes)
    {
        return fail(format!(
            "Graphics clear {name} has unresolved RAM range/alias/alignment"
        ));
    }
    Ok(physical)
}

fn header(
    operation: &str,
    first_global: Option<bool>,
    width: u64,
    height: u64,
    frame_bytes: u64,
) -> Descriptor {
    let mut descriptor = vec![("operation", Value::str(operation))];
    if let Some(first) = first_global {
        descriptor.push(("first_global", Value::Bool(first)));
    }
    descriptor.extend([
        ("width", Value::int(width)),
        ("height", Value::int(height)),
        ("frame_bytes", Value::int(frame_bytes)),
    ]);
    descriptor
}

pub fn path_descriptor(words: &[u32], ram_bytes: u64, max_clear_bytes: u64) -> Result<Descriptor> {
    let (draws, final_bank) = stream::parse(words, true, None)?;
    let commands = commands::decode(words, true)?;
    if draws.is_empty() {
        let (width, height, frame_bytes) = init::dimensions(&final_bank, max_clear_bytes)?;
        if final_bank.get(0x60) != Some(2) || final_bank.get(0x81) != Some(0x1000001) {
            return fail(
                "Graphics clear initializer requires sample mode2 and known path controls",
            );
        }
        let first_global = init::writes(&commands, 0x80);
        let matrix = (0x40..=0x48).any(|index| init::writes(&commands, index));
        if matrix && affine(&final_bank, 0x40, false, true)? != [1., 0., 0., 0., 1., 0.] {
            return fail("Graphics clear initializer requires an identity matrix");
        }
        if first_global && final_bank.get(0x80) != Some(0) {
            return fail("Graphics initializer scratch state is unsupported");
        }
        let mut descriptor = header("initialize", Some(first_global), width, height, frame_bytes);
        descriptor.push(("path_mode", Value::int(2)));
        if matrix && !first_global {
            descriptor.push(("identity_matrix_refresh", Value::Bool(true)));
        }
        descriptor.push((
            "pixel_effect",
            Value::str(if matrix && !first_global {
                "None: exact initialization/matrix/mode commands contain no geometry"
            } else {
                "None: exact initialization/state/flush commands contain no geometry"
            }),
        ));
        return Ok(descriptor);
    }
    if draws.len() != 1 {
        return fail("Graphics clear requires one rectangle draw");
    }
    let draw = &draws[0];
    let path = &draw.registers;
    let (width, height, frame_bytes) = init::dimensions(path, max_clear_bytes)?;
    if path.get(0x60) != Some(0x100)
        || path.get(0x81) != Some(0x1000101)
        || path.get(0x7c) != Some(1)
        || path.get(0x8c) != Some(1)
        || affine(path, 0x40, false, true)? != [1., 0., 0., 0., 1., 0.]
    {
        return fail("Graphics clear requires untransformed filled rectangle state");
    }
    let padded_height = height + (height & 8);
    let stride = padded_height * 16;
    let scratch = path
        .get(0x84)
        .ok_or_else(|| Unsupported("Graphics clear scratch address is missing".into()))?;
    bounded_ram(scratch, stride * 5, ram_bytes, "scratch")?;
    if path.get(0x80) != Some(padded_height << 16)
        || path.get(0x8e) != Some(height << 16)
        || (0..5).any(|index| path.get(0x84 + index) != Some(scratch + u64::from(index) * stride))
        || path.get(0x8b) != Some(scratch + stride * 5 - 8)
    {
        return fail("Graphics clear scratch layout is unsupported");
    }
    let geometry = draw.geometry.as_ref().unwrap();
    if geometry.len() != 4
        || geometry
            .iter()
            .zip([0x28, 0x29, 0x28, 0x29])
            .any(|((code, flags, values), expected)| {
                *code != expected
                    || *flags != 0
                    || values.len() != 2
                    || values.iter().any(|v| !v.is_finite() || v.trunc() != *v)
            })
    {
        return fail("Graphics clear requires integer finite rectangle edges");
    }
    let points: Vec<_> = geometry
        .iter()
        .map(|(_, _, v)| (v[0] as i64, v[1] as i64))
        .collect();
    let [(x0, y0), (left, y1), (x1, bottom), (right, top)] =
        [points[0], points[1], points[2], points[3]];
    if left != x0
        || bottom != y1
        || right != x1
        || top != y0
        || !(0 <= x0 && x0 < x1 && x1 <= width as i64)
        || !(0 <= y0 && y0 < y1 && y1 <= height as i64)
    {
        return fail("Graphics clear requires one clipped axis-aligned rectangle");
    }
    let mut descriptor = header("clear", None, width, height, frame_bytes);
    descriptor.extend([
        (
            "rectangle",
            Value::List([x0, y0, x1, y1].into_iter().map(Value::int).collect()),
        ),
        ("scratch", Value::int(scratch)),
        ("scratch_stride", Value::int(stride)),
        ("padded_height", Value::int(padded_height)),
        ("path_sequence", Value::int(draw.sequence)),
    ]);
    Ok(descriptor)
}

pub fn parse_clear(
    path_words: &[u32],
    fill_words: &[u32],
    ram_bytes: u64,
    max_clear_bytes: u64,
    initial_path: Option<&Registers>,
    initial_fill: Option<&Registers>,
) -> Result<(Descriptor, (Registers, Registers))> {
    let mut descriptor = path_descriptor(path_words, ram_bytes, max_clear_bytes)?;
    let (_, path) = stream::parse(path_words, true, initial_path)?;
    let (fills, fill) = stream::parse(fill_words, false, initial_fill)?;
    let commands = commands::decode(fill_words, false)?;
    super::gamma::validate_upload(&commands)?;
    let width = int(&descriptor, "width") as u64;
    let height = int(&descriptor, "height") as u64;
    let frame_bytes = int(&descriptor, "frame_bytes") as u64;
    let initialization =
        matches!(get(&descriptor,"operation"),Some(Value::Str(s)) if s == "initialize");
    if initialization {
        if !fills.is_empty() {
            return fail("Graphics initializer cannot contain a fill draw");
        }
        let order = init::validate_fill(&fill, width, height)?;
        let count_refresh = init::writes(&commands, 0x100);
        let mode_refresh = init::writes(&commands, 0x115);
        if count_refresh && fill.get(0x100) != Some(0) || mode_refresh && fill.get(0x115) != Some(0)
        {
            return fail("Graphics clear initializer requires disabled empty scissor state");
        }
        if init::writes(&commands, 0x1b0) {
            if fill.get(0x1b0) != Some(0xb) || fill.get(0x1a1) != Some(0) {
                return fail("Graphics initializer target controls are unsupported");
            }
            let target = fill
                .get(0x1a4)
                .ok_or_else(|| Unsupported("Graphics initializer target is missing".into()))?;
            let physical = bounded_ram(target, frame_bytes, ram_bytes, "initializer target")?;
            if fill.get(0x1a8) != Some(target + width * height - 8) {
                return fail("Graphics initializer target endpoint disagrees with dimensions");
            }
            set(&mut descriptor, "target", Value::int(physical));
            set(
                &mut descriptor,
                "target_register",
                Value::hex(u128::from(target)),
            );
        }
        set(&mut descriptor, "order", Value::int(order));
        if get(&descriptor, "identity_matrix_refresh") == Some(&Value::Bool(true))
            && count_refresh
            && mode_refresh
        {
            set(&mut descriptor, "zero_clip_mode_refresh", Value::Bool(true));
        }
        return Ok((descriptor, (path, fill)));
    }
    if fills.len() != 1 {
        return fail("Graphics clear requires one paired fill draw");
    }
    let draw = &fills[0];
    let state = &draw.registers;
    let order = init::validate_fill(state, width, height)?;
    for (index, expected) in [
        (0x115, 0),
        (0x184, 8),
        (0x1a0, 0),
        (0x1a1, 0),
        (0x1a5, 0),
        (0x1aa, 0),
    ] {
        if state.get(index) != Some(expected) {
            return fail("Graphics clear has unsupported scissor, blend or coverage state");
        }
    }
    if draw.sequence != int(&descriptor, "path_sequence") as u64 {
        return fail("Graphics clear path/fill synchronization does not match");
    }
    let scratch = int(&descriptor, "scratch") as u64;
    let stride = int(&descriptor, "scratch_stride") as u64;
    let padded_height = int(&descriptor, "padded_height") as u64;
    if (0..5).any(|index| state.get(0xa4 + index) != Some(scratch + u64::from(index) * stride))
        || state.get(0xa0) != Some(padded_height << 16 | 0x10)
        || state.get(0xa2) != Some(width - 1)
        || state.get(0x1a7) != Some(0x100 | order)
    {
        return fail("Graphics clear path/fill scratch or target state disagrees");
    }
    let destination = state
        .get(0x1a4)
        .ok_or_else(|| Unsupported("Graphics clear destination is missing".into()))?;
    let physical = bounded_ram(destination, frame_bytes, ram_bytes, "destination")?;
    let scratch_physical = bounded_ram(scratch, stride * 5, ram_bytes, "scratch")?;
    if physical < scratch_physical + stride * 5 && scratch_physical < physical + frame_bytes {
        return fail("Graphics clear overlapping framebuffer/scratch is unproved");
    }
    let canonical = state
        .get(0x185)
        .ok_or_else(|| Unsupported("Graphics clear color is missing".into()))?
        as u32;
    let [red, green, blue, alpha] = [0, 8, 16, 24].map(|shift| (canonical >> shift) & 255);
    let pixel = if order == 0 {
        red << 24 | green << 16 | blue << 8 | alpha
    } else {
        alpha << 24 | red << 16 | green << 8 | blue
    };
    descriptor.extend([
        ("destination", Value::int(physical)),
        ("destination_register", Value::hex(u128::from(destination))),
        ("canonical_rgba_word", Value::hex(u128::from(canonical))),
        ("pixel_word", Value::hex(u128::from(pixel))),
        ("order", Value::int(order)),
        ("fill_sequence", Value::int(draw.sequence)),
        ("pixel_bytes", Value::Bytes(pixel.to_le_bytes().to_vec())),
    ]);
    Ok((descriptor, (path, fill)))
}

/// Atomic software-derived Morton update preserving every other byte.
pub fn render_clear(source: &[u8], width: u64, rectangle: [i64; 4], pixel: &[u8]) -> Vec<u8> {
    let mut output = source.to_vec();
    let [x0, y0, x1, y1] = rectangle;
    for y in y0..y1 {
        let row = (y / 8) as u64 * (width / 8);
        for x in x0..x1 {
            let local = super::render::morton(8, (x & 7) as usize, (y & 7) as usize) as u64;
            let offset = (((row + (x / 8) as u64) * 64 + local) * 4) as usize;
            output[offset..offset + 4].copy_from_slice(pixel);
        }
    }
    output
}

#[doc(hidden)]
pub mod fixtures;
#[cfg(test)]
mod tests;
