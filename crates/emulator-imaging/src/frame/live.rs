//! Live host scenes: a video device, a video file or URL, or a raw RGB FIFO, read on a
//! background thread into a latest-frame buffer.
//!
//! `XT5_SCENE` forms handled here (see [`super::host_scene`]):
//!
//! - `video:avf:N[@WxH[@FPS]]`: AVFoundation video device `N` (an index or a device name, as
//!   `ffmpeg -f avfoundation -list_devices true -i ""` lists them): a webcam, OBS Virtual
//!   Camera, an iPhone Continuity Camera or a screen ("Capture screen 0").
//! - `video:lavfi:GRAPH[@WxH[@FPS]]`: an ffmpeg lavfi source graph, such as
//!   `testsrc2=size=640x480:rate=30`, paced in real time.
//! - `video:PATH_OR_URL[@WxH[@FPS]]`: a video file, looped forever and paced in real time, or a
//!   network URL (`rtsp://...`, `srt://...`).
//! - `fifo:PATH@WxH`: raw `rgb24` frames of exactly `W * H * 3` bytes, top row first, from any
//!   producer that writes into the FIFO (or a regular file) at `PATH`. The reader holds the
//!   FIFO open for reading and writing, so producers can stop and restart; a producer that dies
//!   in the middle of a frame misaligns the frames after it. A regular file is read again from
//!   the start when it ends.
//!
//! `video:` spawns `ffmpeg` (`XT5_FFMPEG`, else `ffmpeg` on `PATH`, else the Homebrew paths)
//! with `-nostdin -loglevel error`, scaled to cover `W x H` with the aspect ratio kept and the
//! centre cropped, as `rgb24` raw video on its standard output. The default size is 1080x720
//! (the X-T5's 3:2) and the default rate 30. The device is opened with `-framerate FPS
//! -pixel_format nv12`; a device that does not offer that rate refuses to open.
//!
//! The emulator thread never waits for the producer: it takes the newest complete frame (an
//! `Arc` clone under a lock that the reader holds only to swap a pointer), and frames the
//! emulator does not use are dropped. Until the first frame arrives, and when the producer fails
//! before its first frame, the scene renders colour bars; the failure is logged once. When the
//! producer stops after some frames, the last frame stays. The child process is killed when the
//! feed is dropped; at process exit it ends on its next write into the closed pipe.
use std::io::Read;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// One RGB frame: `width * height * 3` bytes (R, G, B), top row first.
pub struct RgbFrame {
    pub width: usize,
    pub height: usize,
    pub rgb: Vec<u8>,
}

/// Where a live scene comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// An AVFoundation video device, by index or name.
    Device(String),
    /// An ffmpeg lavfi source graph.
    Lavfi(String),
    /// A video file (looped) or a URL.
    Input(String),
    /// Raw rgb24 frames from a FIFO or file.
    Fifo(PathBuf),
}

/// A parsed live scene selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spec {
    pub source: Source,
    pub width: usize,
    pub height: usize,
    pub fps: u32,
}

pub const DEFAULT_WIDTH: usize = 1080;
pub const DEFAULT_HEIGHT: usize = 720;
pub const DEFAULT_FPS: u32 = 30;

fn size(text: &str) -> Option<(usize, usize)> {
    let (w, h) = text.split_once('x')?;
    let (w, h) = (w.parse::<usize>().ok()?, h.parse::<usize>().ok()?);
    ((1..=8192).contains(&w) && (1..=8192).contains(&h)).then_some((w, h))
}

/// Splits a trailing `@WxH` or `@WxH@FPS` from `text`. A body may itself contain `@`
/// (`rtsp://user@host`); only a well-formed suffix is taken.
fn geometry(text: &str) -> Result<(&str, Option<(usize, usize)>, Option<u32>), String> {
    if let Some((head, tail)) = text.rsplit_once('@') {
        if let Some(wh) = size(tail) {
            return Ok((head, Some(wh), None));
        }
        if let (Ok(fps), Some((body, wh))) = (tail.parse::<u32>(), head.rsplit_once('@'))
            && let Some(wh) = size(wh)
        {
            if !(1..=240).contains(&fps) {
                return Err(format!("frame rate {fps} is outside 1..=240"));
            }
            return Ok((body, Some(wh), Some(fps)));
        }
    }
    Ok((text, None, None))
}

/// Parses the text after `video:`.
pub fn parse_video(text: &str) -> Result<Spec, String> {
    let (body, wh, fps) = geometry(text)?;
    let source = if let Some(device) = body.strip_prefix("avf:") {
        Source::Device(device.to_string())
    } else if let Some(graph) = body.strip_prefix("lavfi:") {
        Source::Lavfi(graph.to_string())
    } else {
        Source::Input(body.to_string())
    };
    if matches!(&source, Source::Device(s) | Source::Lavfi(s) | Source::Input(s) if s.is_empty()) {
        return Err("video: needs avf:N, lavfi:GRAPH or a file path or URL".into());
    }
    let (width, height) = wh.unwrap_or((DEFAULT_WIDTH, DEFAULT_HEIGHT));
    Ok(Spec {
        source,
        width,
        height,
        fps: fps.unwrap_or(DEFAULT_FPS),
    })
}

/// Parses the text after `fifo:`; the size is required.
pub fn parse_fifo(text: &str) -> Result<Spec, String> {
    let (body, wh, fps) = geometry(text)?;
    let (width, height) = wh.ok_or("fifo: needs PATH@WxH")?;
    if body.is_empty() {
        return Err("fifo: needs PATH@WxH".into());
    }
    Ok(Spec {
        source: Source::Fifo(PathBuf::from(body)),
        width,
        height,
        fps: fps.unwrap_or(DEFAULT_FPS),
    })
}

/// The ffmpeg arguments for a `video:` spec; `None` for a FIFO.
pub fn ffmpeg_args(spec: &Spec) -> Option<Vec<String>> {
    let mut args: Vec<String> = ["-nostdin", "-hide_banner", "-loglevel", "error"]
        .into_iter()
        .map(String::from)
        .collect();
    let fps = spec.fps.to_string();
    match &spec.source {
        Source::Device(device) => {
            // Video only: "N" alone would also be read as video, but a name needs ":none".
            let input = if device.contains(':') {
                device.clone()
            } else {
                format!("{device}:none")
            };
            args.extend(
                [
                    "-f",
                    "avfoundation",
                    "-framerate",
                    &fps,
                    "-pixel_format",
                    "nv12",
                ]
                .map(String::from),
            );
            args.extend(["-i".into(), input]);
        }
        Source::Lavfi(graph) => {
            args.extend(["-re", "-f", "lavfi", "-i"].map(String::from));
            args.push(graph.clone());
        }
        Source::Input(input) => {
            args.push("-re".into());
            if !input.contains("://") {
                args.extend(["-stream_loop", "-1"].map(String::from));
            }
            args.extend(["-i".into(), input.clone()]);
        }
        Source::Fifo(_) => return None,
    }
    let (w, h) = (spec.width, spec.height);
    args.extend([
        "-an".into(),
        "-vf".into(),
        format!("scale={w}:{h}:force_original_aspect_ratio=increase,crop={w}:{h},setsar=1"),
        "-pix_fmt".into(),
        "rgb24".into(),
        "-f".into(),
        "rawvideo".into(),
        "-".into(),
    ]);
    Some(args)
}

type Latest = Arc<Mutex<Option<Arc<RgbFrame>>>>;

/// A running live scene: the newest frame its reader thread has completed.
pub struct LiveFeed {
    label: String,
    width: usize,
    height: usize,
    latest: Latest,
    frames: Arc<AtomicU64>,
    child: Mutex<Option<Child>>,
}

impl LiveFeed {
    /// Starts reading `spec`: spawns ffmpeg for `video:` or opens the FIFO on the reader thread.
    pub fn start(spec: &Spec, label: &str) -> Result<Self, String> {
        let mut feed = Self::idle(label, spec.width, spec.height);
        match &spec.source {
            Source::Fifo(path) => {
                let path = path.clone();
                feed.spawn_reader(move || open_fifo(&path), true)?;
            }
            _ => {
                let args = ffmpeg_args(spec).expect("video spec");
                let mut child = spawn_ffmpeg(&args)?;
                let stdout = child.stdout.take().ok_or("ffmpeg has no standard output")?;
                *feed.child.get_mut().unwrap() = Some(child);
                let mut stdout = Some(stdout);
                feed.spawn_reader(
                    move || {
                        stdout
                            .take()
                            .map(|out| Box::new(out) as Box<dyn Read + Send>)
                            .ok_or_else(|| "ffmpeg exited".to_string())
                    },
                    false,
                )?;
            }
        }
        Ok(feed)
    }

    /// Reads `width x height` rgb24 frames from `reader` until it ends (for tests and embedders).
    pub fn from_reader(
        reader: impl Read + Send + 'static,
        width: usize,
        height: usize,
        label: &str,
    ) -> Result<Self, String> {
        let feed = Self::idle(label, width, height);
        let mut reader = Some(reader);
        feed.spawn_reader(
            move || {
                reader
                    .take()
                    .map(|r| Box::new(r) as Box<dyn Read + Send>)
                    .ok_or_else(|| "reader ended".to_string())
            },
            false,
        )?;
        Ok(feed)
    }

    fn idle(label: &str, width: usize, height: usize) -> Self {
        Self {
            label: label.to_string(),
            width,
            height,
            latest: Arc::new(Mutex::new(None)),
            frames: Arc::new(AtomicU64::new(0)),
            child: Mutex::new(None),
        }
    }

    /// Runs the reader thread: `open` yields the next stream; with `reopen`, an ended stream is
    /// followed by another `open` (FIFO producers that restart).
    fn spawn_reader(
        &self,
        mut open: impl FnMut() -> Result<Box<dyn Read + Send>, String> + Send + 'static,
        reopen: bool,
    ) -> Result<(), String> {
        let (latest, frames, label) =
            (self.latest.clone(), self.frames.clone(), self.label.clone());
        let bytes = self.width * self.height * 3;
        let (width, height) = (self.width, self.height);
        std::thread::Builder::new()
            .name("xt5-scene-reader".into())
            .spawn(move || loop {
                let mut stream = match open() {
                    Ok(stream) => stream,
                    Err(error) => {
                        if frames.load(Ordering::Relaxed) == 0 {
                            eprintln!("XT5_SCENE: {label}: {error}; showing colour bars");
                        }
                        return;
                    }
                };
                loop {
                    let mut rgb = vec![0u8; bytes];
                    if stream.read_exact(&mut rgb).is_err() {
                        break;
                    }
                    let frame = Arc::new(RgbFrame { width, height, rgb });
                    *latest.lock().unwrap_or_else(|e| e.into_inner()) = Some(frame);
                    if frames.fetch_add(1, Ordering::Relaxed) == 0 {
                        eprintln!("XT5_SCENE: {label}: first frame {width}x{height}");
                    }
                }
                if !reopen {
                    match frames.load(Ordering::Relaxed) {
                        0 => eprintln!("XT5_SCENE: {label}: no frame from the producer; showing colour bars"),
                        n => eprintln!("XT5_SCENE: {label}: producer ended after {n} frames; keeping the last one"),
                    }
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            })
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    /// The newest complete frame, or `None` before the first one. Never waits for the producer.
    pub fn latest(&self) -> Option<Arc<RgbFrame>> {
        self.latest
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// The number of frames read so far.
    pub fn frames(&self) -> u64 {
        self.frames.load(Ordering::Relaxed)
    }

    /// The frame size the reader expects.
    pub fn size(&self) -> (usize, usize) {
        (self.width, self.height)
    }
}

impl Drop for LiveFeed {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.get_mut().ok().and_then(Option::take) {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Opens a FIFO read-write, so the reader never sees end of file between producers and a
/// producer that opens while no frame is being read loses nothing; a regular file is opened
/// read-only and read again from the start when it ends.
fn open_fifo(path: &std::path::Path) -> Result<Box<dyn Read + Send>, String> {
    #[cfg(unix)]
    let fifo = {
        use std::os::unix::fs::FileTypeExt;
        std::fs::metadata(path).is_ok_and(|m| m.file_type().is_fifo())
    };
    #[cfg(not(unix))]
    let fifo = false;
    std::fs::OpenOptions::new()
        .read(true)
        .write(fifo)
        .open(path)
        .map(|file| Box::new(file) as Box<dyn Read + Send>)
        .map_err(|error| format!("{}: {error}", path.display()))
}

fn spawn_ffmpeg(args: &[String]) -> Result<Child, String> {
    let mut candidates: Vec<String> = Vec::new();
    if let Ok(path) = std::env::var("XT5_FFMPEG") {
        candidates.push(path);
    } else {
        candidates.extend(
            [
                "ffmpeg",
                "/opt/homebrew/bin/ffmpeg",
                "/usr/local/bin/ffmpeg",
            ]
            .map(String::from),
        );
    }
    let mut last = String::new();
    for program in &candidates {
        match Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(mut child) => {
                // Forward ffmpeg's messages from inside this process: when the emulator exits,
                // the forwarder goes with it, and ffmpeg's complaints about the closed output
                // pipe have nowhere to go.
                if let Some(stderr) = child.stderr.take() {
                    let _ = std::thread::Builder::new()
                        .name("xt5-scene-ffmpeg-log".into())
                        .spawn(move || {
                            use std::io::BufRead;
                            for line in std::io::BufReader::new(stderr)
                                .lines()
                                .map_while(Result::ok)
                            {
                                eprintln!("XT5_SCENE ffmpeg: {line}");
                            }
                        });
                }
                return Ok(child);
            }
            Err(error) => last = format!("{program}: {error}"),
        }
    }
    Err(format!("cannot start ffmpeg ({last})"))
}

/// Bilinear sample of `frame` at output pixel (`x`, `y`) of a `w x h` picture, with pixel
/// centres aligned.
pub(super) struct Sampler<'a> {
    frame: &'a RgbFrame,
    columns: Vec<(usize, usize, u32)>,
}

fn taps(output: usize, input: usize, at: usize) -> (usize, usize, u32) {
    // Source position in 1/256 pixel: (at + 0.5) * input / output - 0.5, clamped.
    let p = ((2 * at + 1) * input * 256 / (2 * output)).saturating_sub(128);
    let (i, f) = (p / 256, (p % 256) as u32);
    let i = i.min(input - 1);
    (i, (i + 1).min(input - 1), f)
}

impl<'a> Sampler<'a> {
    pub(super) fn new(frame: &'a RgbFrame, width: usize) -> Self {
        let columns = (0..width).map(|x| taps(width, frame.width, x)).collect();
        Self { frame, columns }
    }

    /// One output row's pixel fetcher for row `y` of `height`.
    pub(super) fn row(&self, y: usize, height: usize) -> impl Fn(usize) -> [u8; 3] + '_ {
        let (y0, y1, fy) = taps(height, self.frame.height, y);
        let stride = self.frame.width * 3;
        let (top, bottom) = (
            &self.frame.rgb[y0 * stride..][..stride],
            &self.frame.rgb[y1 * stride..][..stride],
        );
        move |x| {
            let (x0, x1, fx) = self.columns[x];
            let mut out = [0u8; 3];
            for (c, value) in out.iter_mut().enumerate() {
                let blend = |row: &[u8]| {
                    u32::from(row[x0 * 3 + c]) * (256 - fx) + u32::from(row[x1 * 3 + c]) * fx
                };
                let v = blend(top) * (256 - fy) + blend(bottom) * fy;
                *value = ((v + (1 << 15)) >> 16) as u8;
            }
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn wait(feed: &LiveFeed, frames: u64) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while feed.frames() < frames {
            assert!(Instant::now() < deadline, "only {} frames", feed.frames());
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn specs_parse_devices_files_urls_and_fifos() {
        let s = parse_video("avf:1").unwrap();
        assert_eq!(
            s,
            Spec {
                source: Source::Device("1".into()),
                width: 1080,
                height: 720,
                fps: 30
            }
        );
        let s = parse_video("avf:1@640x480@15").unwrap();
        assert_eq!(
            (s.source, s.width, s.height, s.fps),
            (Source::Device("1".into()), 640, 480, 15)
        );
        let s = parse_video("/tmp/a@b.mp4@320x240").unwrap();
        assert_eq!(
            (s.source, s.width, s.fps),
            (Source::Input("/tmp/a@b.mp4".into()), 320, 30)
        );
        let s = parse_video("rtsp://user@host/x").unwrap();
        assert_eq!(s.source, Source::Input("rtsp://user@host/x".into()));
        let s = parse_video("lavfi:testsrc2=size=640x480:rate=30@64x48").unwrap();
        assert_eq!(
            s.source,
            Source::Lavfi("testsrc2=size=640x480:rate=30".into())
        );
        let s = parse_fifo("/tmp/f@8x4").unwrap();
        assert_eq!(
            (s.source, s.width, s.height),
            (Source::Fifo("/tmp/f".into()), 8, 4)
        );
        assert!(parse_fifo("/tmp/f").is_err());
        assert!(parse_video("avf:").is_err());
        assert!(parse_video("avf:1@64x48@0").is_err());
        let args = ffmpeg_args(&parse_video("avf:1@640x480").unwrap()).unwrap();
        assert!(args.windows(2).any(|w| w == ["-i", "1:none"]));
        assert!(args.windows(2).any(|w| w == ["-framerate", "30"]));
        let args = ffmpeg_args(&parse_video("clip.mov").unwrap()).unwrap();
        assert!(args.windows(2).any(|w| w == ["-stream_loop", "-1"]));
        assert_eq!(args.last().unwrap(), "-");
    }

    #[test]
    fn a_reader_keeps_only_the_newest_frame() {
        let (w, h) = (4, 2);
        let mut bytes = Vec::new();
        for n in 0..5u8 {
            bytes.extend(std::iter::repeat_n(n * 10, w * h * 3));
        }
        bytes.extend([1, 2, 3]); // a partial trailing frame is not taken
        let feed = LiveFeed::from_reader(std::io::Cursor::new(bytes), w, h, "test").unwrap();
        wait(&feed, 5);
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(feed.frames(), 5);
        let frame = feed.latest().unwrap();
        assert_eq!((frame.width, frame.height), (w, h));
        assert!(frame.rgb.iter().all(|&b| b == 40));
    }

    #[test]
    fn bilinear_sampling_keeps_flat_colours_and_interpolates_edges() {
        let frame = RgbFrame {
            width: 2,
            height: 1,
            rgb: vec![0, 0, 0, 200, 100, 50],
        };
        let sampler = Sampler::new(&frame, 8);
        let row = sampler.row(0, 4);
        assert_eq!(row(0), [0, 0, 0]);
        assert_eq!(row(7), [200, 100, 50]);
        let middle = row(4);
        assert!(middle[0] > 50 && middle[0] < 150, "{middle:?}");
        // Same size: exact copy.
        let sampler = Sampler::new(&frame, 2);
        let row = sampler.row(0, 1);
        assert_eq!((row(0), row(1)), ([0, 0, 0], [200, 100, 50]));
    }

    #[cfg(unix)]
    #[test]
    fn a_fifo_producer_feeds_the_scene_and_may_restart() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("xt5-scene-fifo-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("scene.rgb");
        let _ = std::fs::remove_file(&path);
        assert!(
            Command::new("mkfifo")
                .arg(&path)
                .status()
                .unwrap()
                .success()
        );
        let spec = parse_fifo(&format!("{}@4x2", path.display())).unwrap();
        let feed = LiveFeed::start(&spec, "fifo test").unwrap();
        assert!(feed.latest().is_none());
        for (round, value) in [(1u64, 7u8), (2, 9)] {
            // Each producer opens, writes one frame and closes.
            let mut producer = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
            producer.write_all(&[value; 4 * 2 * 3]).unwrap();
            drop(producer);
            wait(&feed, round);
            assert!(feed.latest().unwrap().rgb.iter().all(|&b| b == value));
        }
        let picture = super::super::render_422(&super::super::Scene::Live(feed), 8, 4).unwrap();
        let [y, cb, cr] = super::super::ycbcr([9, 9, 9]);
        assert_eq!(&picture[..4], &[y as u8, y as u8, cb as u8, cr as u8]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Runs ffmpeg's synthetic `testsrc2` through the full `video:` path (no camera).
    /// `cargo test -p fujimod-emulator-imaging -- --ignored ffmpeg_lavfi`
    #[test]
    #[ignore = "needs ffmpeg"]
    fn ffmpeg_lavfi_testsrc2_feeds_frames() {
        let spec = parse_video("lavfi:testsrc2=size=640x480:rate=30@160x120@30").unwrap();
        let feed = LiveFeed::start(&spec, "testsrc2").unwrap();
        wait(&feed, 10);
        let frame = feed.latest().unwrap();
        assert_eq!(
            (frame.width, frame.height, frame.rgb.len()),
            (160, 120, 160 * 120 * 3)
        );
        let distinct: std::collections::BTreeSet<_> =
            frame.rgb.chunks(3).map(|p| p.to_vec()).collect();
        assert!(distinct.len() > 16, "testsrc2 is colourful");
    }
}
