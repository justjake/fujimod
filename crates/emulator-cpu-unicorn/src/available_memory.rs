//! Emscripten heap capacity used to fit browser translation caches.

#[cfg(target_os = "emscripten")]
pub fn remaining_heap_bytes() -> Result<u64, String> {
    unsafe extern "C" {
        fn emscripten_get_heap_size() -> usize;
        fn emscripten_get_heap_max() -> usize;
    }
    let heap_size = unsafe { emscripten_get_heap_size() };
    let heap_max = unsafe { emscripten_get_heap_max() };
    heap_max
        .checked_sub(heap_size)
        .map(|bytes| bytes as u64)
        .ok_or_else(|| "Emscripten heap size exceeds its maximum".into())
}
