//! Checks an `XT5_SCENE` source without the emulator: starts the scene reader, waits, reports
//! how many frames arrived, and writes the 4:2:2 still the JPEG shortcut would encode, converted
//! back to RGB, as a binary PPM.
//!
//! ```sh
//! XT5_SCENE='video:lavfi:testsrc2=size=640x480:rate=30@320x240' \
//!   cargo run -p fujimod-emulator-imaging --example scene_probe -- 3 /tmp/scene.ppm
//! ```
use fujimod_emulator_imaging::frame::{Scene, host_scene, host_still_422};
use std::time::Duration;

fn main() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let seconds: f64 = args
        .next()
        .map_or(Ok(3.0), |s| s.parse())
        .map_err(|_| "seconds")?;
    let output = args.next();
    let scene = host_scene().ok_or("XT5_SCENE is not set or not readable")?;
    std::thread::sleep(Duration::from_secs_f64(seconds));
    match scene {
        Scene::Live(feed) => {
            let (w, h) = feed.size();
            println!("live {w}x{h}: {} frames in {seconds} s", feed.frames());
            if feed.latest().is_none() {
                return Err("no frame arrived".into());
            }
        }
        Scene::Bars => println!("colour bars"),
        Scene::Picture { width, height, .. } => println!("picture {width}x{height}"),
    }
    if let Some(path) = output {
        let (w, h) = (640usize, 426usize);
        let yuv = host_still_422(w, h).ok_or("no still")?;
        let mut ppm = format!("P6\n{w} {h}\n255\n").into_bytes();
        for group in yuv.chunks_exact(4) {
            let (cb, cr) = (f64::from(group[2]) - 128.0, f64::from(group[3]) - 128.0);
            for &y in &group[..2] {
                let y = (f64::from(y) - 16.0) * 255.0 / 219.0;
                let k = 255.0 / 224.0;
                let rgb = [
                    y + 1.402 * k * cr,
                    y - 0.344 * k * cb - 0.714 * k * cr,
                    y + 1.772 * k * cb,
                ];
                ppm.extend(rgb.map(|v| v.round().clamp(0.0, 255.0) as u8));
            }
        }
        std::fs::write(&path, ppm).map_err(|e| format!("{path}: {e}"))?;
        println!("wrote {path} ({w}x{h})");
    }
    Ok(())
}
