//! DMP intellectual-property implementations.

#![recursion_limit = "256"]

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

/// Serialize host GPU submissions for the duration of `body`.
///
/// The shared submission lock only orders command lists; it carries no state
/// of its own. Releasing it before a panic resumes keeps a failed command list
/// from poisoning every later submission in the process.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn with_submission<T>(
    context: &fujimod_emulator_gpu::GpuContext,
    body: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let guard = context.acquire()?;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(body));
    drop(guard);
    match result {
        Ok(value) => value,
        Err(payload) => std::panic::resume_unwind(payload),
    }
}
