//! X-Processor 5 imaging peripherals and attached functional optics.
#[path = "imaging_focus.rs"]
pub mod focus;
#[path = "imaging_iris.rs"]
pub mod iris;

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
