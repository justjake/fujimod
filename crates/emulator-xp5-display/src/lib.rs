//! XP5 display, OSD, renderer and panel interfaces.
pub mod display;
pub mod lz;
#[cfg(test)]
mod lz_osd_tests;
pub mod panel_clock;
pub mod panel_serial;
pub mod renderer;

#[cfg(test)]
mod display_vsync_tests;
