//! Runtime-independent graphics data-path operations.
//!
//! These modules parse and render declared graphics command data. Device
//! register laws are wired outside this module.
pub mod batch;
pub mod clear;
pub mod commands;
pub mod gamma;
pub mod geometry;
pub mod init;
pub mod raster;
pub mod regs;
pub mod render;
pub mod stream;
