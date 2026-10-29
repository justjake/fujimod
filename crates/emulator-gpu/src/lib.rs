//! Shared GPU context, coverage rasterization, resident surfaces, and guest-memory transfers.

#[cfg(not(target_os = "emscripten"))]
mod coverage;
#[cfg(not(target_os = "emscripten"))]
mod surfaces;
#[cfg(not(target_os = "emscripten"))]
pub use coverage::{
    CoverageBatch, CoverageEdge, CoverageKernel, CoverageRaster, CoverageRequest, EdgeCoordinates,
    FillRule, SampleGrid,
};
#[cfg(not(target_os = "emscripten"))]
pub use surfaces::{
    GpuContext, GpuSurface, PendingSurfaceReadbacks, PixelWriteMask, SurfaceExtent, SurfaceFormat,
    SurfaceKey, SurfaceLayout, SurfaceReadback, context, context_async,
};
