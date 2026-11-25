//! X-Processor 5 imaging peripherals and attached functional optics.
#[path = "imaging_focus.rs"]
pub mod focus;
#[path = "imaging_iris.rs"]
pub mod iris;

#[path = "imaging_buffer.rs"]
pub mod buffer;
#[path = "imaging_input.rs"]
pub mod input;
#[path = "imaging_paf.rs"]
pub mod paf;

#[path = "imaging_work.rs"]
pub mod work;

#[path = "imaging_preview.rs"]
pub mod preview;

pub const STATISTICS_BASE: u64 = 0xff800000;
pub const STATISTICS_BANK_SIZE: usize = 0x75a0;

#[path = "imaging_still.rs"]
pub mod still;

#[path = "imaging_movie.rs"]
pub mod movie;
#[path = "imaging_fcnv.rs"]
pub mod fcnv;

#[path = "imaging_mix.rs"]
pub mod mix;

#[path = "imaging_colour.rs"]
pub mod colour;
