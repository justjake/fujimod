//! XP5 display, OSD, renderer and panel interfaces.
pub mod display;
pub mod lz;
pub mod osd;
pub mod panel_clock;
pub mod panel_serial;
pub mod renderer;
#[cfg(test)]
mod lz_osd_tests;
