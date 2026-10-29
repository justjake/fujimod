//! PICA200 shader and fragment operations available to DMP rendering.

pub mod blend;
pub mod fog;
pub mod format;
pub mod isa;
pub mod texture;

mod glsl;
mod shader;

pub use glsl::{
    VERTEX_OUTPUT_SEMANTICS, translate_vertex_to_wgsl, translate_vertex_to_wgsl_for_draw,
};
