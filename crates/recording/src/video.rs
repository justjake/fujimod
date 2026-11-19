//! Host encoders consume lossless RGB at each presentation change.
use crate::{Frame, Result, matroska::Matroska};
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
    stream: Matroska,
    fps: u64,
    next_frame: u64,
    last_packet: Option<(u64, Frame)>,
}
impl Encoder {
    fn open(output: &Path, name: &str, frame: &Frame, fps: u32, log: &File) -> Result<Self> {
        let mut child = Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-xerror",
                "-y",
                "-f",
                "matroska",
                "-i",
                "-",
                "-vf",
                &format!("setsar=0/1,format=yuv420p,fps={fps}"),
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
        let mut input = BufWriter::with_capacity(1 << 20, input);
        let stream = match Matroska::open(&mut input, frame, fps) {
            Ok(stream) => stream,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(encoding_error(error));
            }
        };
        Ok(Self {
            child,
            input: Some(input),
            stream,
            fps: u64::from(fps),
            next_frame: 0,
            last_packet: None,
        })
    }
    fn write(&mut self, frame: &Frame) -> Result<()> {
        let input = self.input.as_mut().ok_or("Video encoder is closed")?;
        // Bound the pending hold so FFmpeg continues encoding during long
        // stills. A changed frame otherwise ends the previous frame's hold.
        if self.last_packet.as_ref().is_none_or(|(index, last)| {
            self.next_frame - index >= self.fps || !last.same_pixels(frame)
        }) {
            self.stream
                .write(input, self.next_frame, frame)
                .map_err(encoding_error)?;
            self.last_packet = Some((self.next_frame, frame.clone()));
        }
        self.next_frame += 1;
        Ok(())
    }
    fn close(&mut self) -> Result<()> {
        if let Some(mut input) = self.input.take() {
            // FFmpeg extends the final packet by one frame at the declared
            // rate. Anchor it at the last presentation index to end the hold.
            if let Some((index, last)) = self.last_packet.take()
                && self.next_frame - 1 > index
            {
                self.stream
                    .write(&mut input, self.next_frame - 1, &last)
                    .map_err(encoding_error)?;
            }
            input.flush().map_err(encoding_error)?;
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
fn encoding_error(error: std::io::Error) -> String {
    format!("Video encoding failed: {error}; see ffmpeg.log")
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
        if !(1..=60).contains(&fps) {
            return Err("fps must be 1..60".into());
        }
        if !frame.width().is_multiple_of(2) || !frame.height().is_multiple_of(2) {
            return Err("Video frame dimensions must be even".into());
        }
        let log = File::create(output.join("ffmpeg.log")).map_err(|e| e.to_string())?;
        let annotated = Encoder::open(output, "video.mp4", frame, fps, &log)?;
        let clean = Encoder::open(output, "video-clean.mp4", frame, fps, &log)?;
        Ok(Self { annotated, clean })
    }
    pub fn write(&mut self, annotated: &Frame, clean: &Frame) -> Result<[f64; 2]> {
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
