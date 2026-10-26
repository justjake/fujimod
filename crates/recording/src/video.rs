//! Host encoders consume shared raw RGB. No PNG intermediate or frame files.
use crate::{Frame, Result};
use std::{
    fs::File,
    io::{BufWriter, Write},
    path::Path,
    process::{Child, ChildStdin, Command, Stdio},
    time::{Duration, Instant},
};

struct Encoder {
    child: Child,
    input: Option<BufWriter<ChildStdin>>,
}
impl Encoder {
    fn open(output: &Path, name: &str, frame: &Frame, fps: u32, log: &File) -> Result<Self> {
        let mut child = Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "rawvideo",
                "-pixel_format",
                "rgb24",
                "-video_size",
            ])
            .arg(format!("{}x{}", frame.width(), frame.height()))
            .args([
                "-framerate",
                &fps.to_string(),
                "-i",
                "-",
                "-an",
                "-c:v",
                "libx264",
                "-preset",
                "fast",
                "-crf",
                "20",
                "-pix_fmt",
                "yuv420p",
                "-movflags",
                "+faststart",
            ])
            .arg(output.join(name))
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(log.try_clone().map_err(|e| e.to_string())?)
            .spawn()
            .map_err(|e| format!("Start walkthrough ffmpeg: {e}"))?;
        let input = child.stdin.take().ok_or("Missing video encoder input")?;
        Ok(Self {
            child,
            input: Some(BufWriter::with_capacity(1 << 20, input)),
        })
    }
    fn write(&mut self, bytes: &[u8]) -> Result<()> {
        self.input
            .as_mut()
            .ok_or("Video encoder is closed")?
            .write_all(bytes)
            .map_err(|e| format!("Video encoding failed: {e}; see ffmpeg.log"))
    }
    fn close(&mut self) -> Result<()> {
        if let Some(mut input) = self.input.take() {
            input.flush().map_err(|e| e.to_string())?;
        }
        Ok(())
    }
    fn wait(&mut self) -> Result<()> {
        let start = Instant::now();
        loop {
            if let Some(status) = self.child.try_wait().map_err(|e| e.to_string())? {
                return if status.success() {
                    Ok(())
                } else {
                    Err("Video encoding failed; see ffmpeg.log".into())
                };
            }
            if start.elapsed() > Duration::from_secs(30) {
                return Err("Video encoding timed out; see ffmpeg.log".into());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
impl Drop for Encoder {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub struct Encoders {
    annotated: Encoder,
    clean: Encoder,
}
impl Encoders {
    pub fn check() -> Result<()> {
        let result = Command::new("ffmpeg")
            .args(["-version"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|e| format!("Walkthrough video requires ffmpeg: {e}"))?;
        if result.success() {
            Ok(())
        } else {
            Err("Walkthrough video requires working ffmpeg".into())
        }
    }
    pub fn open(output: &Path, frame: &Frame, fps: u32) -> Result<Self> {
        if !frame.width().is_multiple_of(2) || !frame.height().is_multiple_of(2) {
            return Err("Video frame dimensions must be even".into());
        }
        let log = File::create(output.join("ffmpeg.log")).map_err(|e| e.to_string())?;
        let annotated = Encoder::open(output, "video.mp4", frame, fps, &log)?;
        let clean = Encoder::open(output, "video-clean.mp4", frame, fps, &log)?;
        Ok(Self { annotated, clean })
    }
    pub fn write(&mut self, annotated: &[u8], clean: &[u8]) -> Result<[f64; 2]> {
        let start = Instant::now();
        self.annotated.write(annotated)?;
        let first = start.elapsed().as_secs_f64();
        let start = Instant::now();
        self.clean.write(clean)?;
        Ok([first, start.elapsed().as_secs_f64()])
    }
    pub fn finish(&mut self) -> Result<()> {
        // Close both inputs before waiting; either encoder can have buffered frames.
        let first = self.annotated.close();
        let second = self.clean.close();
        let third = self.annotated.wait();
        let fourth = self.clean.wait();
        first.and(second).and(third).and(fourth)
    }
}
