//! Portable software-codec regression: the same eight-frame stream on native
//! and Wasm. An optional native output path saves the generated AVC for decoding.
use fujimod_emulator_image_codecs::h264::{Backend, Codec, Config};
use sha2::{Digest, Sha256};

fn clip() -> Vec<u8> {
    let config = Config {
        width: 64,
        height: 32,
        frames_per_second: 30,
        bitrate: 1_000_000,
        max_qp: 30,
        gop: 4,
    };
    let mut encoder = Codec::new(Backend::Software, config, &[]).unwrap();
    let mut stream = Vec::new();
    for i in 0..8 {
        let mut frame = vec![128; config.frame_bytes()];
        for row in 0..config.height {
            for col in 0..config.width {
                frame[row * config.width + col] = if (col < 32) ^ (i % 2 == 1) { 40 } else { 200 };
            }
        }
        stream.extend(encoder.encode(config, &frame).unwrap());
    }
    stream
}
#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn h264_digest_word(index: u32) -> u32 {
    let digest = Sha256::digest(clip());
    let offset = (index as usize) * 4;
    u32::from_le_bytes(digest[offset..offset + 4].try_into().unwrap())
}
fn main() {
    let stream = clip();
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(path) = std::env::args_os().nth(1) {
        std::fs::write(path, &stream).unwrap();
    }
    println!(
        "{{\"backend\":\"software\",\"frames\":8,\"width\":64,\"height\":32,\"sha256\":\"{:x}\"}}",
        Sha256::digest(stream)
    );
}
