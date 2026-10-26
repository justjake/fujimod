//! Interfaces shared by the EXR II image processing blocks (in
//! `fujimod-emulator-exr2-isp`) and the devices they exchange data with.
//! EXR II image processing blocks. Register names and masks follow the
//! corresponding register interfaces; pixel approximations and
//! timing are declared explicitly by each model, independently of firmware.
pub mod contrast;
pub mod dma;
pub mod memory_outputs;
pub mod options;
pub mod output_channels;
pub mod parameters;

pub fn word(
    cx: &mut fujimod_emulator_devices::Cx<'_>,
    address: u64,
    size: u8,
    base: u64,
    end: usize,
) -> Option<usize> {
    let Some(offset) = address.checked_sub(base) else {
        cx.unmodeled();
        return None;
    };
    if size != 4 || offset % 4 != 0 || offset >= end as u64 {
        cx.unmodeled();
        None
    } else {
        Some(offset as usize)
    }
}

pub fn due(now: u64, delay: u64) -> Result<u64, String> {
    if delay == 0 {
        return Err("image engine delay must be positive".into());
    }
    now.checked_add(delay)
        .ok_or_else(|| "image engine clock overflow".into())
}
pub mod stream;
