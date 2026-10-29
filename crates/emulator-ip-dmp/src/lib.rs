//! DMP intellectual-property implementations.
pub mod fill;
pub mod gles;
pub mod pica;
pub mod smaph;

#[cfg(target_os = "emscripten")]
static WEBGPU_BRIDGE_ENABLED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Select asynchronous host WebGPU rendering for DMP command lists.
pub fn configure_webgpu_bridge(enabled: bool) {
    #[cfg(target_os = "emscripten")]
    WEBGPU_BRIDGE_ENABLED.store(enabled, std::sync::atomic::Ordering::SeqCst);
    #[cfg(not(target_os = "emscripten"))]
    let _ = enabled;
}

pub(crate) fn webgpu_bridge_enabled() -> bool {
    #[cfg(target_os = "emscripten")]
    {
        WEBGPU_BRIDGE_ENABLED.load(std::sync::atomic::Ordering::SeqCst)
    }
    #[cfg(not(target_os = "emscripten"))]
    {
        false
    }
}
