//! MULTI-SHOT STACK merge for X-Trans RAF bursts.
//!
//! A capture plan of nested loops (burst frames, exposure steps, focus steps)
//! is merged by a list of stages; see [`plan`]. This crate implements the
//! resolution stage: handheld multi-frame super-resolution in the manner of
//! Wronski et al., "Handheld Multi-Frame Super-Resolution" (2019), on X-Trans
//! CFA data.
//!
//! - [`raf`] reads uncompressed X-Trans RAFs.
//! - [`align`] registers each frame to a reference on block-luma pyramids.
//! - [`merge`] accumulates every frame's CFA samples onto a finer output grid
//!   with robustness weights, strip by strip.
//! - [`dng`] and [`preview`] write the result; [`measure`] and [`demosaic`]
//!   quantify it against a single demosaiced frame.

pub mod align;
pub mod demosaic;
pub mod dng;
pub mod fit;
pub mod jpeg;
pub mod local;
pub mod measure;
pub mod merge;
pub mod noise;
pub mod plan;
pub mod plane;
pub mod preview;
pub mod raf;
pub mod source;
pub mod synthetic;
pub mod tiff;
pub mod xtrans;
