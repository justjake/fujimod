//! X-Processor Pro register blocks with no work in flight.
//!
//! Several imaging blocks of X-T20 2.11 are configured from section 5 with
//! plain word writes, often through the table copier 0xc41d30, and read back
//! with read-modify-write updates. Some have handshakes that wait for a
//! status bit after a stop or start: 0xc3c0c0 writes 0 to 0xfff3b000 and
//! waits until bit 0 reads set; 0xc3a954 does the same for the units at
//! 0xfff55000 + 0x1000*n, and their stop 0xc3be40 waits for +0xd8 bit 8.
//! Each block is a register file: words read back their last write and start
//! at 0, and the declared idle bits always read set, as no work is ever in
//! flight. What the blocks compute and any data they move are not modeled.
//!
//! A block may declare [`Frames`]: while its start bit is set it finishes a
//! frame at every multiple of the frame period and raises one interrupt edge;
//! its stopped bit reads set only while the start bit is clear. The live-view
//! output units at 0xfff55000 + 0x1000*n start with +0xc8 bit 28 (0xc3a98c),
//! their stop clears it and waits for +0xd8 bit 8, and their edge handlers
//! (GIC 107 + n) report the frame to the imaging dispatcher.
//!
//! A live-view output unit may declare a uniform image: before each frame
//! interrupt it writes `lines` lines of packed 4:2:2 pixel pairs (Y0 Y1 Cb Cr)
//! to the buffer at +0xc0, +0xc4 pixels wide (two bytes per pixel). The
//! firmware moves +0xc0 through its buffers between frames (unit 1:
//! 0xa7d6000 + 0x180000*k, 720 pixels) and the display shows the buffer the
//! unit finished last. Other data the unit or the rest of the pipeline moves
//! is not modeled. Instead of the uniform image a unit may declare the
//! virtual post-ISP scene ([`fujimod_virtual_scene::PROFILE`]: color bars, a
//! ramp and a moving square, BT.601 limited 4:2:2), sampled over the whole
//! scene into the programmed width and `lines`, one phase per frame.
//!
//! A block may instead declare a [`Job`]: each write that sets its start bit
//! while no job is in flight finishes one job after the declared ticks. The
//! job sets the status bit, the interrupt line is high while a status bit is
//! set, and writing a status bit clears it, as for the face detection engine.
//! The block at 0xfff7f000 (driver 0x1558424..0x1558a94) starts with +0x04
//! bit 0 (0x15589c4); its handler 0x1558108 reads +0x44, writes 1 to it and
//! calls the job's callback. A job without a status bit instead declares a
//! stop bit: its interrupt stays high until a write clears that bit (the
//! development engine 0xfff98000, whose handler 0x1560744 writes 0 to +0x00
//! and waits for bit 0 through 0x1560de8). Idle bits may share the status
//! word when they do not overlap the status bit (0xfffc4000: +0x3c bit 20
//! idle, bit 0 done; 0xfffc3000: +0xdc 0x01000100 idle, bit 0 done, which
//! its handler 0xcce510 clears through 0xc6d580). What a job computes is not
//! modeled.
//!
//! A block may declare [`Command`]s: writing exactly a command's value to its
//! word sets the declared field of a status word, which otherwise reads back
//! its last write. After a still, reccont takes the second DRAM controller
//! 0xfff0f000 through 0xc6c64c (+0xb8 = 0x6a020202, then waits for +0xbc bits
//! 5..0 = 0x27) and back through 0xc6c700 (0x01020202, waits for 0x20); these
//! are its only +0xb8 commands. What the states mean is not known.
//!
//! A job may declare [`Work`]: when it finishes, before its interrupt rises,
//! the block develops or scales a still from the registers the firmware
//! programmed, with the declared approximation of [`crate::development`].
//! A block may publish its words for another block to read: the development
//! engine reads the color unit 0xfff47000 this way when its film law is
//! declared ([`crate::development::FILM_PROFILE`]).
//!
//! A job may declare a [`MovieOutput`]: when it finishes, the padded I420 frame
//! its registers point at is converted to RGBA and published to one host
//! scanout (the X-T20 2.11 playback display engine 0xfffd5000).
use fujimod_emulator_devices::graphics::scanout::Frame;
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

/// Words a block publishes: its current register file.
pub type Published = Arc<Mutex<BTreeMap<u64, u32>>>;

/// A block's published words as a PL330 source. A DMA load reads the same
/// words the block returns to the CPU, zero where nothing was written. The
/// audio register window also accepts DMA stores; other windows stay
/// read-only.
pub struct SourceWindow {
    base: u64,
    size: u64,
    words: Published,
    writable: bool,
}
impl SourceWindow {
    pub fn new(base: u64, size: u64, words: Published) -> Result<Self, String> {
        Self::with_writable(base, size, words, false)
    }
    pub fn new_writable(base: u64, size: u64, words: Published) -> Result<Self, String> {
        Self::with_writable(base, size, words, true)
    }
    fn with_writable(
        base: u64,
        size: u64,
        words: Published,
        writable: bool,
    ) -> Result<Self, String> {
        if base & 0xfff != 0 || size == 0 || size > 0x1000 {
            return Err("Invalid X-Processor Pro block source window".into());
        }
        Ok(Self {
            base,
            size,
            words,
            writable,
        })
    }
}
impl fujimod_emulator_ip_arm::pl330::Source for SourceWindow {
    fn contains(&self, address: u64, len: usize) -> bool {
        address >= self.base
            && address
                .checked_add(len as u64)
                .is_some_and(|end| end <= self.base + self.size)
    }
    fn read(&self, address: u64, len: usize) -> Result<Vec<u8>, String> {
        let words = self
            .words
            .lock()
            .map_err(|_| "Register block words poisoned")?;
        let start = address - self.base;
        Ok((start..start + len as u64)
            .map(|offset| {
                let word = words.get(&(offset & !3)).copied().unwrap_or(0);
                (word >> (8 * (offset & 3))) as u8
            })
            .collect())
    }
    fn writable(&self) -> bool {
        self.writable
    }
    fn write(&self, address: u64, bytes: &[u8]) -> Result<(), String> {
        if !self.writable {
            return Err("PL330 store into a read-only source".into());
        }
        let mut words = self
            .words
            .lock()
            .map_err(|_| "Register block words poisoned")?;
        for (index, byte) in bytes.iter().enumerate() {
            let offset = address + index as u64 - self.base;
            let word = words.entry(offset & !3).or_insert(0);
            let shift = 8 * (offset & 3);
            *word = (*word & !(0xffu32 << shift)) | ((*byte as u32) << shift);
        }
        Ok(())
    }
}

/// Frame law of a block: (offset, mask) of the start and stopped bits, the
/// interrupt line and the frame period in ticks.
#[derive(Clone, Copy, Debug)]
pub struct Frames {
    pub start: (u64, u32),
    pub stopped: (u64, u32),
    pub irq: u32,
    pub period: u64,
}
/// Job law of a block: (offset, mask) of the start and status bits (or,
/// without a status bit, the stop bit that a write clears to acknowledge),
/// the level interrupt line and the ticks a job takes.
#[derive(Clone, Copy, Debug)]
pub struct Job {
    pub start: (u64, u32),
    pub status: Option<(u64, u32)>,
    pub stop: Option<(u64, u32)>,
    pub irq: u32,
    pub ticks: u64,
}
/// X-T20 2.11 movie display engine: a job that scans a padded I420 frame to a
/// host output when it finishes. The frame planes are the luma and chroma
/// bases; the programmed width and height are padded up to the engine's
/// 16-line alignment.
#[derive(Clone, Copy, Debug)]
pub struct MovieOutput {
    pub output: usize,
}
/// X-T20 2.11 playback display engine register window.
pub const MOVIE_OUTPUT_BASE: u64 = 0xfffd_5000;
/// Host output index of the movie display, separate from the panel OSD output.
pub const MOVIE_OUTPUT: usize = 1;
const MOVIE_LUMA: u64 = 0x008;
const MOVIE_CHROMA: u64 = 0x00c;
const MOVIE_WIDTH: u64 = 0x010;
const MOVIE_PITCH: u64 = 0x014;
const MOVIE_GEOMETRY: u64 = 0x018;
/// Command law of a block: writing `value` to `word` sets the bits `mask` of
/// the status word `status` to `state`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Command {
    pub word: u64,
    pub value: u32,
    pub status: u64,
    pub mask: u32,
    pub state: u32,
}
/// Data a job moves when it finishes (see [`crate::development`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Work {
    /// The development engine; the X-Trans site (line, sample) of the first
    /// source sample.
    Develop { phase: (u8, u8) },
    /// The resizer.
    Resize,
}
const DEVELOP_SOURCE: u64 = 0x08;
const DEVELOP_SOURCE_STRIDE: u64 = 0x0c;
const DEVELOP_SIZE: u64 = 0x14;
const DEVELOP_DESTINATION: u64 = 0x1c;
const DEVELOP_DESTINATION_STRIDE: u64 = 0x20;
const RESIZE_INPUT: u64 = 0x08;
const RESIZE_OUTPUT: u64 = 0x14;
const RESIZE_STEP_X: u64 = 0x18;
const RESIZE_STEP_Y: u64 = 0x1c;
const RESIZE_SOURCE: u64 = 0xc0;
const RESIZE_SOURCE_STRIDE: u64 = 0xc4;
const RESIZE_DESTINATION: u64 = 0xc8;
const RESIZE_DESTINATION_STRIDE: u64 = 0xcc;
const RESIZE_FORMAT: u64 = 0x100;
/// Packed 4:2:2 in and out, the format of a shot's jobs.
const RESIZE_PACKED_422: u32 = 0x1100_0002;
/// The same with bits 16 and 20 set: X-T20 2.11 playback (0x1548040) sets
/// them from request bytes +0x32 and +0x31 when it magnifies the decoded
/// picture to the display. Their effect is not modeled; the layout is
/// declared packed 4:2:2 as well.
const RESIZE_PACKED_422_PLAYBACK: u32 = 0x1111_0002;
/// Picture a live-view output unit writes at each frame.
#[derive(Clone, Debug, PartialEq)]
enum Picture {
    /// One packed pixel pair repeated.
    Uniform([u8; 4]),
    /// The declared virtual scene.
    Scene,
    /// A host RGB image: width, height and rows of red, green, blue bytes.
    Host(u64, u64, Arc<[u8]>),
    /// One 14-bit linear (R, G, B) developed through the film law and white
    /// balance of the color unit's programmed words, with the packed pair
    /// written while the unit is still unprogrammed. A unit not programmed
    /// yet completes no frame.
    Film([u16; 3], [u8; 4]),
}
/// Virtual scene size: its normalized coordinates.
const SCENE_WIDTH: u64 = 1024;
const SCENE_HEIGHT: u64 = 768;
/// The whole of a scene, unscaled.
fn whole(scene: fujimod_virtual_scene::Scene) -> fujimod_virtual_scene::Crop {
    fujimod_virtual_scene::Crop {
        x: 0,
        y: 0,
        width: scene.width,
        height: scene.height,
    }
}
/// Tightly packed I420 (Y, Cb, Cr) to RGBA, BT.601 limited range, in the host
/// scanout's red-first byte order.
fn i420_to_rgba(width: usize, height: usize, y_plane: &[u8], uv: &[u8]) -> Result<Vec<u8>, String> {
    let pixels = width
        .checked_mul(height)
        .ok_or("Movie frame size overflows")?;
    let chroma = pixels / 4;
    if width & 1 != 0 || height & 1 != 0 || y_plane.len() != pixels || uv.len() != chroma * 2 {
        return Err("Invalid X-Processor Pro movie I420 frame".into());
    }
    let (cb, cr) = uv.split_at(chroma);
    let cw = width / 2;
    let clamp = |value: i32| value.clamp(0, 255) as u8;
    let mut rgba = Vec::with_capacity(pixels * 4);
    for row in 0..height {
        let y_row = &y_plane[row * width..][..width];
        let c_row = (row / 2) * cw;
        for column in 0..width {
            let y = i32::from(y_row[column]);
            let u = i32::from(cb[c_row + column / 2]) - 128;
            let v = i32::from(cr[c_row + column / 2]) - 128;
            let c = 298 * (y - 16);
            rgba.extend_from_slice(&[
                clamp((c + 409 * v + 128) >> 8),
                clamp((c - 100 * u - 208 * v + 128) >> 8),
                clamp((c + 516 * u + 128) >> 8),
                255,
            ]);
        }
    }
    Ok(rgba)
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    words: BTreeMap<u64, u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    due: Option<u64>,
    #[serde(default, skip_serializing_if = "is_zero")]
    frames: u64,
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    status: u32,
    #[serde(default, skip_serializing_if = "is_zero")]
    jobs: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    movie_frames: u64,
}
const BUFFER: u64 = 0xc0;
const WIDTH: u64 = 0xc4;
fn is_zero(n: &u64) -> bool {
    *n == 0
}
fn is_zero_u32(n: &u32) -> bool {
    *n == 0
}
pub struct Block {
    base: u64,
    size: u64,
    /// (offset, mask) of status bits that always read set.
    idle: Vec<(u64, u32)>,
    frames: Option<Frames>,
    job: Option<Job>,
    work: Option<Work>,
    movie: Option<MovieOutput>,
    /// Lines and the Y0 Y1 Cb Cr pixel pair of a uniform live-view image.
    image: Option<(u32, Picture)>,
    /// (offset, value) of words that read a fixed value whatever is written.
    fixed: Vec<(u64, u32)>,
    commands: Vec<Command>,
    /// Where this block publishes its words.
    published: Option<Published>,
    /// The color unit's words, for the film law of a development job, and
    /// the declared reference white balance.
    color: Option<(Published, [u32; 3])>,
    /// The still tone unit's words (matrix, curves and brightness table of the
    /// still law), for a development job.
    tone: Option<Published>,
    /// The published integration value and scene-light scale the film
    /// picture's light follows.
    film_exposure: Option<crate::statistics::Exposure>,
    film_light: Option<crate::statistics::SceneLight>,
    /// A host picture the film develops pixel by pixel: width, height, rows
    /// of sRGB bytes.
    film_picture: Option<(u64, u64, Arc<[u8]>)>,
    /// The last developed picture and what it was developed from; derived
    /// output only, never part of the block's state.
    film_frame: Mutex<Option<FilmFrame>>,
    state: State,
}
/// One developed film picture and its inputs.
struct FilmFrame {
    words: BTreeMap<u64, u32>,
    factor: u64,
    width: u64,
    lines: u64,
    image: Vec<u8>,
}
impl Block {
    pub fn new(
        base: u64,
        size: u64,
        idle: Vec<(u64, u32)>,
        frames: Option<Frames>,
    ) -> Result<Self, String> {
        let bit = |(o, mask): (u64, u32)| o & 3 != 0 || o >= size || mask == 0;
        if base & 0xfff != 0
            || size == 0
            || size > 0x1000
            || size & 3 != 0
            || idle.iter().any(|&b| bit(b))
            || frames.is_some_and(|f| {
                bit(f.start)
                    || bit(f.stopped)
                    || !(32..512).contains(&f.irq)
                    || f.period == 0
                    || idle.contains(&f.stopped)
            })
        {
            return Err("Invalid X-Processor Pro register block".into());
        }
        Ok(Self {
            base,
            size,
            idle,
            frames,
            job: None,
            work: None,
            movie: None,
            image: None,
            fixed: Vec::new(),
            commands: Vec::new(),
            published: None,
            color: None,
            tone: None,
            film_exposure: None,
            film_light: None,
            film_picture: None,
            film_frame: Mutex::new(None),
            state: State::default(),
        })
    }
    /// One job per start, finished after the declared ticks.
    pub fn with_job(mut self, job: Job) -> Result<Self, String> {
        let bit = |(o, mask): (u64, u32)| o & 3 != 0 || o >= self.size || mask == 0;
        if self.frames.is_some()
            || bit(job.start)
            || job.status.is_some() == job.stop.is_some()
            || job.stop.is_some_and(bit)
            || job.status.is_some_and(|status| {
                bit(status)
                    || job.start.0 == status.0
                    || self
                        .idle
                        .iter()
                        .any(|&(o, mask)| o == status.0 && mask & status.1 != 0)
            })
            || !(32..512).contains(&job.irq)
            || job.ticks == 0
        {
            return Err("Invalid X-Processor Pro register block job".into());
        }
        self.job = Some(job);
        Ok(self)
    }
    /// Scan the job's padded I420 frame to `output` when it finishes.
    pub fn with_movie_output(mut self, output: usize) -> Result<Self, String> {
        if self.job.is_none() || self.size <= MOVIE_GEOMETRY || output > 8 {
            return Err("Invalid X-Processor Pro movie output".into());
        }
        self.movie = Some(MovieOutput { output });
        Ok(self)
    }
    /// Data the job moves when it finishes.
    pub fn with_work(mut self, work: Work) -> Result<Self, String> {
        let words = match work {
            Work::Develop { phase } if phase.0 < 6 && phase.1 < 6 => DEVELOP_DESTINATION_STRIDE,
            Work::Resize => RESIZE_FORMAT,
            Work::Develop { .. } => return Err("Invalid X-Trans phase".into()),
        };
        if self.job.is_none() || words >= self.size {
            return Err("Invalid X-Processor Pro register block work".into());
        }
        self.work = Some(work);
        Ok(self)
    }
    /// Publish this block's words to `words`.
    pub fn with_published(mut self, words: Published) -> Self {
        *words.lock().unwrap() = self.state.words.clone();
        self.published = Some(words);
        self
    }
    /// Develop with the film law and white balance of the color unit's
    /// published words, relative to the `reference` (R, G, B) gains.
    pub fn with_color(mut self, words: Published, reference: [u32; 3]) -> Result<Self, String> {
        if !matches!(self.work, Some(Work::Develop { .. })) || reference.contains(&0) {
            return Err("Only a development job reads a color unit".into());
        }
        self.color = Some((words, reference));
        Ok(self)
    }
    /// Develop with the matrix and curves of the still tone unit's published
    /// words; widths, chroma, luma bands and white balance stay the color
    /// unit's.
    pub fn with_tone(mut self, words: Published) -> Result<Self, String> {
        if !matches!(self.work, Some(Work::Develop { .. })) || self.color.is_none() {
            return Err("Only a development job reads a still tone unit".into());
        }
        self.tone = Some(words);
        Ok(self)
    }
    fn publish(&self) {
        if let Some(words) = &self.published {
            *words.lock().unwrap() = self.state.words.clone();
        }
    }
    fn run_work(&self, cx: &mut Cx<'_>, work: Work) -> Result<(), String> {
        let word = |o| self.state.words.get(&o).copied().unwrap_or(0);
        let address = |o| u64::from(word(o));
        let stride = |o| word(o) as usize * 2;
        let (rows, destination, destination_stride) = match work {
            Work::Develop { phase } => {
                let (lines, width) = crate::development::size(word(DEVELOP_SIZE));
                let source_stride = stride(DEVELOP_SOURCE_STRIDE);
                let raw = cx.ram_bytes(
                    address(DEVELOP_SOURCE),
                    source_stride * (lines - 1) + 2 * width,
                )?;
                let phase = (usize::from(phase.0), usize::from(phase.1));
                let film = match &self.color {
                    Some((words, reference)) => {
                        let words = words.lock().unwrap();
                        let still = self.tone.as_ref().map(|tone| tone.lock().unwrap());
                        Some((
                            crate::development::film(still.as_deref(), &words)?,
                            crate::development::white_balance(&words, *reference)?,
                        ))
                    }
                    None => None,
                };
                let rows = crate::development::develop(
                    &raw,
                    source_stride,
                    lines,
                    width,
                    phase,
                    film.as_ref().map(|(film, factors)| (film, *factors)),
                )?;
                (
                    rows,
                    address(DEVELOP_DESTINATION),
                    stride(DEVELOP_DESTINATION_STRIDE),
                )
            }
            Work::Resize => {
                if ![RESIZE_PACKED_422, RESIZE_PACKED_422_PLAYBACK].contains(&word(RESIZE_FORMAT)) {
                    return Err(format!(
                        "Unmodeled resizer format {:#x}",
                        word(RESIZE_FORMAT)
                    ));
                }
                let (lines, width) = crate::development::size(word(RESIZE_INPUT));
                let (out_lines, out_width) = crate::development::size(word(RESIZE_OUTPUT));
                let (Some(step_x), Some(step_y)) = (
                    crate::development::step(word(RESIZE_STEP_X)),
                    crate::development::step(word(RESIZE_STEP_Y)),
                ) else {
                    return Err("Unmodeled resizer step".into());
                };
                let scale = crate::development::Scale {
                    stride: stride(RESIZE_SOURCE_STRIDE),
                    lines,
                    width,
                    out_lines,
                    out_width,
                    step_x,
                    step_y,
                };
                let source = cx.ram_bytes(
                    address(RESIZE_SOURCE),
                    scale.stride * (lines - 1) + 2 * width,
                )?;
                let rows = crate::development::resample(&source, scale)?;
                (
                    rows,
                    address(RESIZE_DESTINATION),
                    stride(RESIZE_DESTINATION_STRIDE),
                )
            }
        };
        if let Some(row) = rows.first().filter(|row| row.len() > destination_stride) {
            return Err(format!(
                "Still picture line of {} bytes wider than its destination stride {destination_stride} ({} lines to {destination:#x})",
                row.len(),
                rows.len()
            ));
        }
        for (y, row) in rows.iter().enumerate() {
            cx.ram_write(destination + (y * destination_stride) as u64, row)
                .map_err(|e| format!("Still picture outside RAM: {e}"))?;
        }
        Ok(())
    }
    /// Words that read a fixed value: ready words a driver compares for
    /// equality before a run (0xfffc3000 +0xdc and 0xfff7e000 +0x1c read
    /// 0x01000100 while the firmware writes 1 there).
    pub fn with_fixed(mut self, fixed: Vec<(u64, u32)>) -> Result<Self, String> {
        let job_offsets = self
            .job
            .map(|j| [Some(j.start.0), j.status.map(|s| s.0)])
            .unwrap_or_default();
        if fixed.iter().any(|&(o, _)| {
            o & 3 != 0
                || o >= self.size
                || self.idle.iter().any(|&(i, _)| i == o)
                || job_offsets.contains(&Some(o))
                || self
                    .frames
                    .is_some_and(|f| f.start.0 == o || f.stopped.0 == o)
        }) {
            return Err("Invalid X-Processor Pro register block fixed word".into());
        }
        self.fixed = fixed;
        Ok(self)
    }
    /// Commands that set a field of a status word.
    pub fn with_commands(mut self, commands: Vec<Command>) -> Result<Self, String> {
        let word = |o: u64| o & 3 != 0 || o >= self.size;
        let special = |o: u64| {
            self.idle.iter().any(|&(i, _)| i == o)
                || self.fixed.iter().any(|&(f, _)| f == o)
                || self.job.is_some_and(|j| {
                    j.start.0 == o
                        || j.status.is_some_and(|s| s.0 == o)
                        || j.stop.is_some_and(|s| s.0 == o)
                })
                || self
                    .frames
                    .is_some_and(|f| f.start.0 == o || f.stopped.0 == o)
        };
        if commands.iter().any(|c| {
            word(c.word)
                || word(c.status)
                || c.word == c.status
                || c.mask == 0
                || c.state & !c.mask != 0
                || special(c.word)
                || special(c.status)
        }) {
            return Err("Invalid X-Processor Pro register block command".into());
        }
        self.commands = commands;
        Ok(self)
    }
    /// A uniform live-view image written at each frame: lines and Y, Cb, Cr.
    pub fn with_live_view_image(self, lines: u32, ycbcr: [u8; 3]) -> Result<Self, String> {
        let [y, cb, cr] = ycbcr;
        self.with_picture(lines, Picture::Uniform([y, y, cb, cr]))
    }
    /// A uniform scene, given as 14-bit linear light at the `reference` white
    /// balance, developed at each frame with the film law and white balance of
    /// the color unit's published `words` (the live-view pipeline reprograms
    /// the unit every frame). Until the firmware programs it, `unprogrammed`
    /// (Y, Cb, Cr) is written.
    pub fn with_live_view_film(
        mut self,
        lines: u32,
        linear: [u16; 3],
        unprogrammed: [u8; 3],
        words: Published,
        reference: [u32; 3],
    ) -> Result<Self, String> {
        if self.work.is_some() || reference.contains(&0) || linear.iter().any(|&v| v > 16383) {
            return Err("Invalid X-Processor Pro live-view film".into());
        }
        let [y, cb, cr] = unprogrammed;
        self = self.with_picture(lines, Picture::Film(linear, [y, y, cb, cr]))?;
        self.color = Some((words, reference));
        Ok(self)
    }
    /// The declared film light follows the published integration value, as the
    /// statistics scene's light does (`crate::statistics::Exposure`).
    pub fn with_film_exposure(
        mut self,
        exposure: crate::statistics::Exposure,
    ) -> Result<Self, String> {
        if !matches!(self.image, Some((_, Picture::Film(..)))) || exposure.reference == 0 {
            return Err("Invalid X-Processor Pro live-view film exposure".into());
        }
        self.film_exposure = Some(exposure);
        Ok(self)
    }
    /// The declared film light follows a published scene-light scale
    /// (`crate::statistics::SceneLight`).
    pub fn with_film_scene_light(
        mut self,
        light: crate::statistics::SceneLight,
    ) -> Result<Self, String> {
        if !matches!(self.image, Some((_, Picture::Film(..)))) {
            return Err("Invalid X-Processor Pro live-view film scene light".into());
        }
        self.film_light = Some(light);
        Ok(self)
    }
    /// A host picture developed through the film law in place of the uniform
    /// film light, scaled by the same exposure and scene light.
    pub fn with_film_picture(
        mut self,
        width: u64,
        height: u64,
        rgb: Vec<u8>,
    ) -> Result<Self, String> {
        let scene = fujimod_virtual_scene::Scene {
            width,
            height,
            frame: 0,
        };
        if !matches!(self.image, Some((_, Picture::Film(..))))
            || fujimod_virtual_scene::validate_crop(scene, whole(scene)).is_err()
            || width.checked_mul(height).and_then(|n| n.checked_mul(3)) != Some(rgb.len() as u64)
        {
            return Err("Invalid X-Processor Pro live-view film picture".into());
        }
        self.film_picture = Some((width, height, rgb.into()));
        Ok(self)
    }
    /// The published exposure and scene-light factors applied to film light.
    fn film_factor(&self) -> f64 {
        let exposure = self.film_exposure.as_ref().map_or(1., |e| e.factor());
        let light = self.film_light.as_ref().map_or(1., |l| l.factor());
        exposure * light
    }
    /// The film picture developed with `words` at `width` x `lines`, reused
    /// while the words, the light and the size are unchanged. `None` while
    /// the unit is not programmed yet.
    fn developed_picture(
        &self,
        words: &BTreeMap<u64, u32>,
        reference: [u32; 3],
        width: u64,
        lines: u64,
    ) -> Option<Result<Vec<u8>, fujimod_virtual_scene::SceneError>> {
        let (w, h, rgb) = self.film_picture.as_ref()?;
        let factor = self.film_factor();
        let mut cached = self.film_frame.lock().unwrap();
        if let Some(frame) = cached.as_ref().filter(|f| {
            f.factor == factor.to_bits()
                && f.width == width
                && f.lines == lines
                && &f.words == words
        }) {
            return Some(Ok(frame.image.clone()));
        }
        let film = crate::development::film(None, words).ok()?;
        let balance = crate::development::white_balance(words, reference).ok()?;
        let light: Vec<u16> = (0..=255u8)
            .map(|v| {
                (crate::statistics::linear(v) * 16383. * factor)
                    .round()
                    .clamp(0., 16383.) as u16
            })
            .collect();
        let scene = fujimod_virtual_scene::Scene {
            width: *w,
            height: *h,
            frame: 0,
        };
        let develop = |x: u64, y: u64| {
            let i = ((y * w + x) * 3) as usize;
            let level =
                |c: usize| crate::development::balanced(light[usize::from(rgb[i + c])], balance[c]);
            film.develop([level(0), level(1), level(2)])
        };
        let image = fujimod_virtual_scene::output_pairs(
            scene,
            whole(scene),
            width,
            lines,
            None,
            |x0, x1, y| fujimod_emulator_imaging::film::Film::pack(develop(x0, y), develop(x1, y)),
        );
        if let Ok(image) = &image {
            *cached = Some(FilmFrame {
                words: words.clone(),
                factor: factor.to_bits(),
                width,
                lines,
                image: image.clone(),
            });
        }
        Some(image)
    }
    /// The declared film light scaled by the published factors, in the 14-bit
    /// sensor range.
    fn scaled_film_light(&self, linear: [u16; 3]) -> [u16; 3] {
        let factor = self.film_factor();
        linear.map(|value| (f64::from(value) * factor).round().clamp(0., 16383.) as u16)
    }
    /// The declared virtual scene written at each frame, over `lines` lines.
    pub fn with_live_view_scene(self, lines: u32) -> Result<Self, String> {
        self.with_picture(lines, Picture::Scene)
    }
    /// A host RGB image (post-ISP) sampled at each frame, over `lines` lines.
    pub fn with_live_view_host_image(
        self,
        lines: u32,
        width: u64,
        height: u64,
        rgb: Vec<u8>,
    ) -> Result<Self, String> {
        let scene = fujimod_virtual_scene::Scene {
            width,
            height,
            frame: 0,
        };
        if fujimod_virtual_scene::validate_crop(scene, whole(scene)).is_err()
            || width.checked_mul(height).and_then(|n| n.checked_mul(3)) != Some(rgb.len() as u64)
        {
            return Err("Invalid X-Processor Pro live-view host image".into());
        }
        self.with_picture(lines, Picture::Host(width, height, rgb.into()))
    }
    fn with_picture(mut self, lines: u32, picture: Picture) -> Result<Self, String> {
        if self.frames.is_none() || self.size <= WIDTH || !(1..=4096).contains(&lines) {
            return Err("Invalid X-Processor Pro live-view image".into());
        }
        self.image = Some((lines, picture));
        Ok(self)
    }
    fn write_image(&self, cx: &mut Cx<'_>) {
        let Some((lines, picture)) = &self.image else {
            return;
        };
        let word = |o| self.state.words.get(&o).copied().unwrap_or(0);
        let (address, width) = (word(BUFFER), word(WIDTH));
        if width == 0 {
            return;
        }
        if width & 1 != 0 || width > 0x2000 {
            return cx.fail("Unsupported live-view line width");
        }
        let (width, lines) = (u64::from(width), u64::from(*lines));
        let image = match picture {
            Picture::Uniform(pair) => Ok(pair.repeat((width / 2 * lines) as usize)),
            Picture::Film(_, _)
                if self.film_picture.is_some()
                    && self
                        .color
                        .as_ref()
                        .is_some_and(|(w, _)| !w.lock().unwrap().is_empty()) =>
            {
                let (words, reference) = self.color.as_ref().unwrap();
                let words = words.lock().unwrap().clone();
                match self.developed_picture(&words, *reference, width, lines) {
                    Some(image) => image,
                    // A unit not programmed yet completes no frame; a unit
                    // between two words develops with the words as they are.
                    None => return,
                }
            }
            Picture::Film(linear, unprogrammed) => {
                let linear = self.scaled_film_light(*linear);
                let pair = match &self.color {
                    Some((words, reference)) if !words.lock().unwrap().is_empty() => {
                        let words = words.lock().unwrap();
                        let developed = crate::development::film(None, &words).and_then(|film| {
                            let factors = crate::development::white_balance(&words, *reference)?;
                            let level =
                                |c: usize| crate::development::balanced(linear[c], factors[c]);
                            Ok(film.develop([level(0), level(1), level(2)]))
                        });
                        match developed {
                            Ok(pixel) => fujimod_emulator_imaging::film::Film::pack(pixel, pixel),
                            // A unit not programmed yet completes no frame.
                            Err(_) => return,
                        }
                    }
                    _ => *unprogrammed,
                };
                Ok(pair.repeat((width / 2 * lines) as usize))
            }
            Picture::Scene => {
                let scene = fujimod_virtual_scene::Scene {
                    width: SCENE_WIDTH,
                    height: SCENE_HEIGHT,
                    frame: self.state.frames,
                };
                fujimod_virtual_scene::output(scene, whole(scene), width, lines, None)
            }
            Picture::Host(w, h, rgb) => {
                let scene = fujimod_virtual_scene::Scene {
                    width: *w,
                    height: *h,
                    frame: 0,
                };
                fujimod_virtual_scene::output_rgb(
                    scene,
                    whole(scene),
                    width,
                    lines,
                    None,
                    |x, y| {
                        let i = ((y * w + x) * 3) as usize;
                        [rgb[i], rgb[i + 1], rgb[i + 2]]
                    },
                )
            }
        };
        let Ok(image) = image else {
            return cx.fail("Live-view scene output refused");
        };
        if cx.ram_write(u64::from(address), &image).is_err() {
            cx.fail("Live-view buffer outside RAM");
        }
    }
    fn started(&self, frames: &Frames) -> bool {
        let (offset, mask) = frames.start;
        self.state.words.get(&offset).is_some_and(|w| w & mask != 0)
    }
    /// Register window size in bytes.
    pub fn window(&self) -> u64 {
        self.size
    }
    /// The stopped bit reads the run state, not the last write.
    fn stopped_mask(&self, offset: u64) -> u32 {
        self.frames
            .filter(|f| f.stopped.0 == offset)
            .map_or(0, |f| f.stopped.1)
    }
    fn offset(&self, address: u64, size: u8) -> Option<u64> {
        let offset = address.checked_sub(self.base)?;
        (size == 4 && offset & 3 == 0 && offset < self.size).then_some(offset)
    }
    /// Any 1/2/4-byte aligned offset within the block. Sub-word accesses reach
    /// the same word storage as a four-byte access.
    fn access_offset(&self, address: u64, size: u8) -> Option<u64> {
        let offset = address.checked_sub(self.base)?;
        let width = u64::from(size);
        ((1..=4).contains(&size)
            && offset % width == 0
            && offset.checked_add(width)? <= self.size)
            .then_some(offset)
    }
    /// The four-byte word at a four-byte-aligned offset, with its fixed, idle
    /// and frame-stop bits applied.
    fn read_word(&self, offset: u64) -> u32 {
        if let Some(&(_, value)) = self.fixed.iter().find(|(o, _)| *o == offset) {
            return value;
        }
        let mut idle = self
            .idle
            .iter()
            .filter(|(o, _)| *o == offset)
            .fold(0, |bits, (_, mask)| bits | mask);
        if let Some((_, mask)) = self.job.and_then(|j| j.status).filter(|s| s.0 == offset) {
            return self.state.status & mask | idle;
        }
        if let Some(frames) = self.frames.filter(|f| f.stopped.0 == offset) {
            idle = idle & !frames.stopped.1
                | if self.started(&frames) {
                    0
                } else {
                    frames.stopped.1
                };
        }
        self.state.words.get(&offset).copied().unwrap_or(0) & !self.stopped_mask(offset) | idle
    }
    /// Publish the movie engine's padded I420 frame as one host scanout.
    fn scan_movie(&mut self, cx: &mut Cx<'_>, movie: MovieOutput) -> Result<(), String> {
        let word = |offset| self.state.words.get(&offset).copied().unwrap_or(0);
        let width = u64::from(word(MOVIE_WIDTH));
        let pitch = u64::from(word(MOVIE_PITCH));
        let height = u64::from(word(MOVIE_GEOMETRY) >> 16);
        if width == 0 || height == 0 {
            return Ok(());
        }
        if width > 0x2000 || height > 0x2000 || pitch != width {
            return Err("Invalid X-Processor Pro movie geometry".into());
        }
        // The descriptor aligns the frame height up to the engine's 16-line
        // block; the player buffer holds the padded planes.
        let lines = (height + 0xf) & !0xf;
        let pixels = width
            .checked_mul(lines)
            .ok_or("Movie frame size overflows")?;
        let luma = u64::from(word(MOVIE_LUMA));
        let chroma = u64::from(word(MOVIE_CHROMA));
        let y = cx.ram_bytes(luma, pixels as usize)?;
        let uv = cx.ram_bytes(chroma, (pixels / 2) as usize)?;
        let rgba = i420_to_rgba(width as usize, lines as usize, &y, &uv)?;
        self.state.movie_frames += 1;
        let sequence = self.state.movie_frames;
        cx.output_status(
            movie.output,
            json!({
                "active": true, "format": "rgba8888-premultiplied",
                "width": width, "height": lines, "sequence": sequence,
                "source": format!("0x{luma:x}"), "chroma": format!("0x{chroma:x}"),
                "capture": "movie-i420",
            }),
        )?;
        cx.scanout(Frame {
            output: movie.output,
            sequence,
            width: width as u32,
            height: lines as u32,
            rgba,
        })
    }
}
impl Device for Block {
    fn name(&self) -> &'static str {
        "xpp-register-block"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let Some(offset) = self.access_offset(address, size) else {
            cx.unmodeled();
            return 0;
        };
        if size == 4 {
            return u64::from(self.read_word(offset));
        }
        let shift = (offset & 3) * 8;
        let width = u32::from(size) * 8;
        (u64::from(self.read_word(offset & !3)) >> shift) & ((1u64 << width) - 1)
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        if size != 4 {
            let Some(offset) = self.access_offset(address, size) else {
                return cx.unmodeled();
            };
            let base = offset & !3;
            let shift = (offset & 3) * 8;
            let width = u32::from(size) * 8;
            let mask = ((1u32 << width) - 1) << shift;
            let merged = (self.read_word(base) & !mask) | (((value as u32) << shift) & mask);
            if merged == 0 {
                self.state.words.remove(&base);
            } else {
                self.state.words.insert(base, merged);
            }
            return;
        }
        if let (Some(job), Some(offset), Ok(value)) =
            (self.job, self.offset(address, size), u32::try_from(value))
        {
            if job
                .stop
                .is_some_and(|(o, mask)| o == offset && value & mask == 0)
                && self.state.status != 0
            {
                self.state.status = 0;
                cx.set_irq(job.irq, false);
            }
            if job.status.is_some_and(|s| s.0 == offset) {
                self.state.status &= !value;
                if self.state.status == 0 {
                    cx.set_irq(job.irq, false);
                }
                return;
            }
            if offset == job.start.0 && value & job.start.1 != 0 && self.state.due.is_none() {
                self.state.due = cx.now().checked_add(job.ticks);
                if self.state.due.is_none() {
                    cx.fail("Register block job deadline overflow");
                }
            }
        }
        match (self.offset(address, size), u32::try_from(value)) {
            (Some(offset), Ok(0)) => {
                self.state.words.remove(&offset);
            }
            (Some(offset), Ok(value)) => {
                self.state.words.insert(offset, value);
            }
            _ => return cx.unmodeled(),
        }
        if let Some(c) = self
            .commands
            .iter()
            .find(|c| Some(c.word) == self.offset(address, size) && u64::from(c.value) == value)
        {
            let word = self.state.words.get(&c.status).copied().unwrap_or(0) & !c.mask | c.state;
            if word == 0 {
                self.state.words.remove(&c.status);
            } else {
                self.state.words.insert(c.status, word);
            }
        }
        self.publish();
        if let Some(frames) = self.frames {
            self.state.due = match (self.started(&frames), self.state.due) {
                (false, _) => None,
                (true, Some(at)) => Some(at),
                (true, None) => (cx.now() / frames.period)
                    .checked_add(1)
                    .and_then(|n| n.checked_mul(frames.period)),
            };
        }
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        if let Some(job) = self.job {
            if self.state.due.is_some_and(|at| at <= cx.now()) {
                self.state.due = None;
                if let Some(work) = self.work
                    && let Err(e) = self.run_work(cx, work)
                {
                    return cx.fail(e);
                }
                if let Some(movie) = self.movie
                    && let Err(e) = self.scan_movie(cx, movie)
                {
                    return cx.fail(e);
                }
                self.state.jobs += 1;
                self.state.status |= job.status.map_or(1, |(_, mask)| mask);
                cx.set_irq(job.irq, true);
            }
            return;
        }
        let (Some(frames), Some(due)) = (self.frames, self.state.due) else {
            return;
        };
        if cx.now() < due {
            return;
        }
        self.write_image(cx);
        cx.set_irq(frames.irq, true);
        cx.set_irq(frames.irq, false);
        self.state.frames += 1;
        let step = ((cx.now() - due) / frames.period + 1).checked_mul(frames.period);
        self.state.due = step.and_then(|step| due.checked_add(step));
        if self.state.due.is_none() {
            cx.fail("Register block frame deadline overflow");
        }
    }
    fn next_deadline(&self, _now: u64) -> Option<u64> {
        self.state.due
    }
    fn save(&self) -> Value {
        serde_json::to_value(&self.state).unwrap_or(Value::Null)
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let state: State = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        self.check(&state)?;
        self.state = state;
        self.publish();
        Ok(())
    }
    fn checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        fujimod_emulator_devices::checkpoint::Checkpoint::encode(self.name(), &self.state)
    }
    fn validate_checkpoint(
        &self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<(), String> {
        if !saved.children().is_empty() || !saved.blobs().is_empty() {
            return Err("Register block checkpoint has unexpected parts".into());
        }
        self.check(&saved.decode(self.name())?)
    }
    /// The published words are shared with the color unit's reader and are
    /// refreshed in place from the restored register file.
    fn restore_checkpoint(
        &mut self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        self.state = saved.decode(self.name())?;
        self.publish();
        Ok(())
    }
    fn fingerprint(&self) -> Result<Value, String> {
        Ok(self.checkpoint()?.state())
    }
}
impl Block {
    fn check(&self, state: &State) -> Result<(), String> {
        let started = self.frames.is_some_and(|f| {
            let (offset, mask) = f.start;
            state.words.get(&offset).is_some_and(|w| w & mask != 0)
        });
        if state
            .words
            .iter()
            .any(|(o, v)| o & 3 != 0 || *o >= self.size || *v == 0)
            || self.frames.is_some() && started != state.due.is_some()
            || self.frames.is_none() && self.job.is_none() && state.due.is_some()
            || self.job.is_none() && (state.status != 0 || state.jobs != 0)
            || self.movie.is_none() && state.movie_frames != 0
            || self.job.is_some_and(|j| match j.status {
                Some((offset, mask)) => {
                    state.status & !mask != 0 || state.words.contains_key(&offset)
                }
                None => state.status > 1,
            })
            || self
                .frames
                .zip(state.due)
                .is_some_and(|(f, at)| at % f.period != 0)
        {
            return Err("Invalid register block checkpoint".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::test_host::{TestHost, read, write};

    const BASE: u64 = 0xfff55000;

    #[test]
    fn i420_converts_black_and_gray_with_bt601_limited_range() {
        // 2 x 2: black luma, neutral chroma.
        let rgba = i420_to_rgba(2, 2, &[16, 16, 16, 16], &[128, 128]).unwrap();
        assert_eq!(
            rgba,
            [0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255]
        );
        // Mid luma keeps the neutral chroma gray.
        let rgba = i420_to_rgba(2, 2, &[128, 128, 128, 128], &[128, 128]).unwrap();
        assert_eq!(rgba, [130, 130, 130, 255].repeat(4));
        assert!(i420_to_rgba(3, 2, &[0; 6], &[128; 3]).is_err());
    }

    #[test]
    fn a_movie_job_scans_its_padded_i420_frame_to_the_declared_output() {
        use fujimod_emulator_devices::test_host::advance;
        let mut unit = Block::new(MOVIE_OUTPUT_BASE, 0x1000, vec![(0, 0x8000_0000)], None)
            .unwrap()
            .with_job(Job {
                start: (0, 1),
                status: Some((0x64, 1)),
                stop: None,
                irq: 143,
                ticks: 100,
            })
            .unwrap()
            .with_fixed(vec![(0x60, 1)])
            .unwrap()
            .with_movie_output(MOVIE_OUTPUT)
            .unwrap();
        assert!(
            Block::new(MOVIE_OUTPUT_BASE, 0x1000, vec![], None)
                .unwrap()
                .with_movie_output(MOVIE_OUTPUT)
                .is_err()
        );
        let mut h = TestHost::new();
        // 2 x 2 black frame padded to the engine's 16-line block.
        h.ram[0x1000..0x1020].fill(16);
        h.ram[0x2000..0x2010].fill(128);
        write(&mut unit, &mut h, MOVIE_OUTPUT_BASE + MOVIE_LUMA, 0x1000);
        write(&mut unit, &mut h, MOVIE_OUTPUT_BASE + MOVIE_CHROMA, 0x2000);
        write(&mut unit, &mut h, MOVIE_OUTPUT_BASE + MOVIE_WIDTH, 2);
        write(&mut unit, &mut h, MOVIE_OUTPUT_BASE + MOVIE_PITCH, 2);
        write(
            &mut unit,
            &mut h,
            MOVIE_OUTPUT_BASE + MOVIE_GEOMETRY,
            (2 << 16) | 0x80,
        );
        write(&mut unit, &mut h, MOVIE_OUTPUT_BASE, 1);
        advance(&mut unit, &mut h, 100);
        assert_eq!(read(&mut unit, &mut h, MOVIE_OUTPUT_BASE + 0x64) & 1, 1);
        assert_eq!(unit.state.movie_frames, 1);
        assert_eq!(h.scanouts.len(), 1);
        let frame = &h.scanouts[0];
        assert_eq!(frame.output, MOVIE_OUTPUT);
        assert_eq!((frame.width, frame.height), (2, 16));
        assert_eq!(frame.rgba.len(), 2 * 16 * 4);
        assert_eq!(&frame.rgba[..4], &[0, 0, 0, 255]);
    }

    #[test]
    fn byte_accesses_merge_into_the_word() {
        let mut unit = Block::new(BASE, 0x200, vec![], None).unwrap();
        let mut h = TestHost::new();
        // A byte store writes one lane and leaves the others; a byte read
        // returns just that lane. 108d8e4 writes gate words this way.
        {
            let mut cx = h.cx();
            unit.write(&mut cx, BASE + 0x18, 1, 0x5a);
            assert_eq!(cx.refusal(), None);
        }
        assert_eq!(read(&mut unit, &mut h, BASE + 0x18), 0x5a);
        {
            let mut cx = h.cx();
            unit.write(&mut cx, BASE + 0x19, 1, 0xa5);
            assert_eq!(cx.refusal(), None);
        }
        assert_eq!(read(&mut unit, &mut h, BASE + 0x18), 0xa55a);
        let mut cx = h.cx();
        assert_eq!(unit.read(&mut cx, BASE + 0x19, 1), 0xa5);
        assert_eq!(cx.refusal(), None);
    }

    #[test]
    fn words_read_back_and_idle_bits_read_set() {
        let mut unit = Block::new(BASE, 0x200, vec![(0, 1), (0xd8, 0x100)], None).unwrap();
        let mut h = TestHost::new();
        // 0xc3a954: stop, then wait for bit 0.
        write(&mut unit, &mut h, BASE, 0);
        assert_eq!(read(&mut unit, &mut h, BASE), 1);
        // 0xc3ab78 sets +0xd8 bit 0; 0xc3be40 waits for bit 8.
        write(&mut unit, &mut h, BASE + 0xd8, 1);
        assert_eq!(read(&mut unit, &mut h, BASE + 0xd8), 0x101);
        write(&mut unit, &mut h, BASE + 0xc0, 0xa7d_6000);
        let saved = unit.save();
        write(&mut unit, &mut h, BASE + 0xc0, 0);
        unit.restore(&saved).unwrap();
        assert_eq!(read(&mut unit, &mut h, BASE + 0xc0), 0xa7d_6000);
        let mut cx = h.cx();
        unit.read(&mut cx, BASE + 0x200, 4);
        assert!(cx.is_unmodeled());
        assert!(Block::new(BASE, 0x200, vec![(0x200, 1)], None).is_err());
    }

    #[test]
    fn x_t20_auto_mode_unit_reads_the_status_bits_its_driver_waits_for() {
        // 0xfffd4000: 0x155d0ec polls +0x00 bit 0, 0x155d40c polls +0x088 bits 0 and 16.
        let mut h = TestHost::new();
        let mut pipeline =
            Block::new(0xfffd4000, 0x1000, vec![(0, 1), (0x88, 1 << 16 | 1)], None).unwrap();
        write(&mut pipeline, &mut h, 0xfffd4000, 0);
        assert_eq!(read(&mut pipeline, &mut h, 0xfffd4000) & 1, 1);
        assert_eq!(
            read(&mut pipeline, &mut h, 0xfffd4088) & (1 << 16 | 1),
            1 << 16 | 1
        );
    }

    #[test]
    fn started_blocks_finish_a_frame_each_period() {
        use fujimod_emulator_devices::test_host::advance;
        let frames = Frames {
            start: (0xc8, 1 << 28),
            stopped: (0xd8, 0x100),
            irq: 107,
            period: 10,
        };
        assert!(Block::new(BASE, 0x200, vec![(0xd8, 0x100)], Some(frames)).is_err());
        let mut unit = Block::new(BASE, 0x200, vec![(0, 1)], Some(frames)).unwrap();
        let mut h = TestHost::new();
        assert_eq!(read(&mut unit, &mut h, BASE + 0xd8), 0x100);
        assert_eq!(unit.next_deadline(0), None);
        h.now = 3;
        write(&mut unit, &mut h, BASE + 0xc8, 1 << 28 | 1 << 24);
        assert_eq!(read(&mut unit, &mut h, BASE + 0xd8), 0);
        assert_eq!(unit.next_deadline(3), Some(10));
        // 0xc3abe8 sets bit 0 for each frame; bit 8 keeps reading the run state.
        write(&mut unit, &mut h, BASE + 0xd8, 0x101);
        assert_eq!(read(&mut unit, &mut h, BASE + 0xd8), 1);
        advance(&mut unit, &mut h, 25);
        assert_eq!(unit.state.frames, 1);
        assert_eq!(unit.next_deadline(25), Some(30));
        let saved = unit.save();
        write(&mut unit, &mut h, BASE + 0xc8, 1 << 24);
        assert_eq!(read(&mut unit, &mut h, BASE + 0xd8), 0x101);
        assert_eq!(unit.next_deadline(25), None);
        unit.restore(&saved).unwrap();
        assert_eq!(unit.next_deadline(25), Some(30));
        assert!(
            unit.restore(&serde_json::json!({ "words": [] , "due": 30 }))
                .is_err()
        );
    }

    #[test]
    fn a_started_job_finishes_once_and_holds_its_level_interrupt() {
        use fujimod_emulator_devices::test_host::advance;
        const ENGINE: u64 = 0xfff7f000;
        let job = Job {
            start: (0x04, 1),
            status: Some((0x44, 1)),
            stop: None,
            irq: 128,
            ticks: 100,
        };
        let block = || Block::new(ENGINE, 0x1000, vec![(0, 1), (0x40, 0x1_0001)], None).unwrap();
        assert!(block().with_job(Job { ticks: 0, ..job }).is_err());
        assert!(
            block()
                .with_job(Job {
                    status: Some((0x40, 1)),
                    ..job
                })
                .is_err()
        );
        let frames = Frames {
            start: (0xc8, 1 << 28),
            stopped: (0xd8, 0x100),
            irq: 107,
            period: 10,
        };
        assert!(
            Block::new(ENGINE, 0x1000, vec![], Some(frames))
                .unwrap()
                .with_job(job)
                .is_err()
        );
        let mut engine = block().with_job(job).unwrap();
        let mut h = TestHost::new();
        // 0x1558468 writes 1 to the status; 0x15589c4 waits for +0x40 and
        // sets +0x04 bit 0.
        write(&mut engine, &mut h, ENGINE + 0x44, 1);
        assert_eq!(read(&mut engine, &mut h, ENGINE + 0x40), 0x1_0001);
        h.now = 5;
        write(&mut engine, &mut h, ENGINE + 0x04, 1);
        assert_eq!(engine.next_deadline(5), Some(105));
        // A second start while the job runs does not move it.
        write(&mut engine, &mut h, ENGINE + 0x04, 1);
        assert_eq!(engine.next_deadline(5), Some(105));
        advance(&mut engine, &mut h, 104);
        assert_eq!(read(&mut engine, &mut h, ENGINE + 0x44), 0);
        advance(&mut engine, &mut h, 105);
        assert_eq!(read(&mut engine, &mut h, ENGINE + 0x44), 1);
        assert_eq!(engine.state.status, 1);
        assert_eq!(engine.next_deadline(105), None);
        let saved = engine.save();
        // 0x1558108 reads the status and writes 1 to acknowledge it.
        write(&mut engine, &mut h, ENGINE + 0x44, 1);
        assert_eq!(engine.state.status, 0);
        assert_eq!(read(&mut engine, &mut h, ENGINE + 0x44), 0);
        assert_eq!(read(&mut engine, &mut h, ENGINE + 0x04), 1);
        engine.restore(&saved).unwrap();
        assert_eq!(read(&mut engine, &mut h, ENGINE + 0x44), 1);
        assert!(block().restore(&saved).is_err());
        write(&mut engine, &mut h, ENGINE + 0x04, 1);
        advance(&mut engine, &mut h, 205);
        assert_eq!(engine.state.jobs, 2);
    }

    #[test]
    fn live_view_units_write_the_declared_image_each_frame() {
        use fujimod_emulator_devices::test_host::advance;
        let frames = Frames {
            start: (0xc8, 1 << 28),
            stopped: (0xd8, 0x100),
            irq: 108,
            period: 10,
        };
        let block = || Block::new(BASE, 0x200, vec![(0, 1)], Some(frames)).unwrap();
        assert!(
            Block::new(BASE, 0x200, vec![], None)
                .unwrap()
                .with_live_view_image(480, [73, 114, 138])
                .is_err()
        );
        assert!(block().with_live_view_image(0, [73, 114, 138]).is_err());
        let mut unit = block().with_live_view_image(3, [73, 114, 138]).unwrap();
        let mut h = TestHost::new();
        write(&mut unit, &mut h, BASE + BUFFER, 0x1000);
        write(&mut unit, &mut h, BASE + WIDTH, 4);
        write(&mut unit, &mut h, BASE + 0xc8, 1 << 28);
        advance(&mut unit, &mut h, 10);
        assert_eq!(h.ram[0x1000..0x1008], [73, 73, 114, 138, 73, 73, 114, 138]);
        assert_eq!(h.ram[0x1014..0x1018], [73, 73, 114, 138]);
        assert_eq!(h.ram[0x1018], 0);
        write(&mut unit, &mut h, BASE + BUFFER, 0x2000);
        advance(&mut unit, &mut h, 20);
        assert_eq!(h.ram[0x2000..0x2004], [73, 73, 114, 138]);
        write(&mut unit, &mut h, BASE + WIDTH, 3);
        h.now = 30;
        let mut cx = h.cx();
        unit.advance(&mut cx);
        assert_eq!(cx.refusal(), Some("Unsupported live-view line width"));
    }

    #[test]
    fn a_live_view_film_picture_is_developed_pixel_by_pixel() {
        use crate::development::tests::provia;
        use fujimod_emulator_devices::test_host::advance;
        let frames = Frames {
            start: (0xc8, 1 << 28),
            stopped: (0xd8, 0x100),
            irq: 108,
            period: 10,
        };
        let reference = [1138, 604, 1096];
        let words = Published::default();
        let film = || {
            Block::new(BASE, 0x200, vec![(0, 1)], Some(frames))
                .unwrap()
                .with_live_view_film(
                    1,
                    [1015, 764, 577],
                    [16, 128, 128],
                    words.clone(),
                    reference,
                )
                .unwrap()
        };
        // Only a film unit takes a picture, and only one of its declared size.
        assert!(
            Block::new(BASE, 0x200, vec![(0, 1)], Some(frames))
                .unwrap()
                .with_film_picture(2, 1, vec![0; 6])
                .is_err()
        );
        assert!(film().with_film_picture(2, 1, vec![0; 5]).is_err());
        // A dark left half and a bright right half.
        let mut unit = film()
            .with_film_picture(2, 1, vec![20, 20, 20, 230, 230, 230])
            .unwrap();
        let mut h = TestHost::new();
        write(&mut unit, &mut h, BASE + BUFFER, 0x1000);
        write(&mut unit, &mut h, BASE + WIDTH, 4);
        write(&mut unit, &mut h, BASE + 0xc8, 1 << 28);
        advance(&mut unit, &mut h, 10);
        // Unprogrammed, the declared pair.
        assert_eq!(h.ram[0x1000..0x1008], [16, 16, 128, 128, 16, 16, 128, 128]);
        let mut all = provia();
        all.insert(0x100, reference[0] | reference[1] << 16);
        all.insert(0x104, reference[2]);
        *words.lock().unwrap() = all;
        h.ram[0x1000..0x1008].fill(0);
        advance(&mut unit, &mut h, 20);
        let (dark, bright) = (h.ram[0x1000], h.ram[0x1004]);
        assert!(dark < bright, "{dark} {bright}");
        // A neutral picture stays near neutral at the reference white balance.
        for c in [h.ram[0x1006], h.ram[0x1007]] {
            assert!(c.abs_diff(128) <= 8, "{c}");
        }
        // The same inputs give the same frame.
        let first = h.ram[0x1000..0x1008].to_vec();
        h.ram[0x1000..0x1008].fill(0);
        advance(&mut unit, &mut h, 30);
        assert_eq!(h.ram[0x1000..0x1008], first[..]);
    }

    #[test]
    fn a_live_view_film_unit_develops_its_scene_with_the_color_units_words() {
        use crate::development::tests::provia;
        use fujimod_emulator_devices::test_host::advance;
        let frames = Frames {
            start: (0xc8, 1 << 28),
            stopped: (0xd8, 0x100),
            irq: 108,
            period: 10,
        };
        let block = || Block::new(BASE, 0x200, vec![(0, 1)], Some(frames)).unwrap();
        let reference = [1138, 604, 1096];
        let words = Published::default();
        // The color unit's words with the gains of the reference white balance.
        let program = |matrix: Option<[u32; 6]>, gains: [u32; 3]| {
            let mut all = provia();
            for (i, word) in matrix.into_iter().flatten().enumerate() {
                all.insert(0x300 + 4 * i as u64, word);
            }
            all.insert(0x100, gains[0] | gains[1] << 16);
            all.insert(0x104, gains[2]);
            *words.lock().unwrap() = all;
        };
        assert!(
            block()
                .with_live_view_film(
                    3,
                    [1015, 764, 577],
                    [16, 128, 128],
                    words.clone(),
                    [0, 1, 1]
                )
                .is_err()
        );
        assert!(
            block()
                .with_live_view_film(3, [20000, 0, 0], [16, 128, 128], words.clone(), reference)
                .is_err()
        );
        let mut unit = block()
            .with_live_view_film(
                3,
                [1015, 764, 577],
                [16, 128, 128],
                words.clone(),
                reference,
            )
            .unwrap();
        let mut h = TestHost::new();
        write(&mut unit, &mut h, BASE + BUFFER, 0x1000);
        write(&mut unit, &mut h, BASE + WIDTH, 4);
        write(&mut unit, &mut h, BASE + 0xc8, 1 << 28);
        // Until the firmware programs the color unit the declared pair is written.
        advance(&mut unit, &mut h, 10);
        assert_eq!(h.ram[0x1000..0x1004], [16, 16, 128, 128]);
        let frame = |unit: &mut Block, h: &mut TestHost, now| {
            h.ram[0x1000..0x1100].fill(0);
            advance(unit, h, now);
            h.ram[0x1000..0x1004].to_vec()
        };
        program(None, reference);
        let provia_frame = frame(&mut unit, &mut h, 20);
        // PROVIA at the reference white balance gives the measured wall.
        assert_eq!(provia_frame, [73, 73, 114, 138]);
        // The same words give the same frame; the whole line is one pair.
        assert_eq!(frame(&mut unit, &mut h, 30), provia_frame);
        assert_eq!(h.ram[0x1004..0x1008], provia_frame[..]);
        // Another film matrix and another white balance each change it.
        let velvia = [0x1f71040b, 0x84, 0x04b61fb6, 0x1f94, 0x1f84002c, 0x450];
        program(Some(velvia), reference);
        let velvia_frame = frame(&mut unit, &mut h, 40);
        assert_ne!(velvia_frame, provia_frame);
        program(None, [3917, 604, 2776]);
        let warm_frame = frame(&mut unit, &mut h, 50);
        assert_ne!(warm_frame, provia_frame);
        assert_ne!(warm_frame, velvia_frame);
        // Unequal curve copies are a legitimate per-channel program: each
        // output channel develops through its own slot with the words the
        // unit holds, so the frame is not empty.
        program(None, reference);
        words.lock().unwrap().insert(0x440, 0);
        assert_ne!(frame(&mut unit, &mut h, 60), [0; 4]);
    }

    #[test]
    fn a_live_view_film_follows_the_published_exposure_and_scene_light() {
        use crate::statistics::{Exposure, LIGHT_UNIT, SceneLight};
        use fujimod_emulator_devices::test_host::advance;
        use std::sync::atomic::{AtomicU32, Ordering};
        let frames = Frames {
            start: (0xc8, 1 << 28),
            stopped: (0xd8, 0x100),
            irq: 108,
            period: 10,
        };
        let reference = [1138, 604, 1096];
        let words = Published::default();
        let value = std::sync::Arc::new(AtomicU32::new(0));
        let light = std::sync::Arc::new(AtomicU32::new(LIGHT_UNIT));
        let block = || Block::new(BASE, 0x200, vec![(0, 1)], Some(frames)).unwrap();
        let film = || {
            block()
                .with_live_view_film(
                    3,
                    [1015, 764, 577],
                    [16, 128, 128],
                    words.clone(),
                    reference,
                )
                .unwrap()
                .with_film_exposure(Exposure {
                    value: value.clone(),
                    reference: 1566,
                })
                .unwrap()
                .with_film_scene_light(SceneLight {
                    value: light.clone(),
                })
                .unwrap()
        };
        let mut unit = film();
        // Not yet published: the declared light.
        assert_eq!(
            unit.scaled_film_light([1000, 2000, 16383]),
            [1000, 2000, 16383]
        );
        // Twice the reference doubles it, clamped to the 14-bit range.
        value.store(2 * 1566, Ordering::Relaxed);
        assert_eq!(
            unit.scaled_film_light([1000, 2000, 16383]),
            [2000, 4000, 16383]
        );
        // The scene light is a second scale on the same light.
        value.store(0, Ordering::Relaxed);
        light.store(LIGHT_UNIT / 2, Ordering::Relaxed);
        assert_eq!(
            unit.scaled_film_light([1000, 2000, 16383]),
            [500, 1000, 8192]
        );
        // The developed frame brightens with the exposure and keeps its
        // declared light while the value is 0.
        let mut all = crate::development::tests::provia();
        all.insert(0x100, reference[0] | reference[1] << 16);
        all.insert(0x104, reference[2]);
        *words.lock().unwrap() = all;
        let mut h = TestHost::new();
        write(&mut unit, &mut h, BASE + BUFFER, 0x1000);
        write(&mut unit, &mut h, BASE + WIDTH, 4);
        write(&mut unit, &mut h, BASE + 0xc8, 1 << 28);
        let frame = |unit: &mut Block, h: &mut TestHost, now| {
            h.ram[0x1000..0x1100].fill(0);
            advance(unit, h, now);
            h.ram[0x1000]
        };
        light.store(LIGHT_UNIT, Ordering::Relaxed);
        let declared = frame(&mut unit, &mut h, 10);
        value.store(2 * 1566, Ordering::Relaxed);
        let brighter = frame(&mut unit, &mut h, 20);
        assert!(brighter > declared, "{brighter} vs {declared}");
        value.store(0, Ordering::Relaxed);
        assert_eq!(frame(&mut unit, &mut h, 30), declared);
        // Only a film picture takes the two scales.
        let plain = block().with_live_view_image(3, [73, 114, 138]).unwrap();
        assert!(
            plain
                .with_film_exposure(Exposure {
                    value: value.clone(),
                    reference: 1566
                })
                .is_err()
        );
        assert!(
            block()
                .with_live_view_image(3, [73, 114, 138])
                .unwrap()
                .with_film_scene_light(SceneLight {
                    value: light.clone()
                })
                .is_err()
        );
    }

    #[test]
    fn a_live_view_scene_unit_writes_the_moving_virtual_scene() {
        use fujimod_emulator_devices::test_host::advance;
        let frames = Frames {
            start: (0xc8, 1 << 28),
            stopped: (0xd8, 0x100),
            irq: 108,
            period: 10,
        };
        let mut unit = Block::new(BASE, 0x200, vec![(0, 1)], Some(frames))
            .unwrap()
            .with_live_view_scene(4)
            .unwrap();
        let mut h = TestHost::new();
        write(&mut unit, &mut h, BASE + BUFFER, 0x1000);
        write(&mut unit, &mut h, BASE + WIDTH, 8);
        write(&mut unit, &mut h, BASE + 0xc8, 1 << 28);
        advance(&mut unit, &mut h, 10);
        // The white and yellow bars at the top left (eight pixels sample one
        // bar each), BT.601 limited, chroma averaged over the pair.
        assert_eq!(h.ram[0x1000..0x1004], [235, 210, 72, 137]);
        let first = h.ram[0x1000..0x1040].to_vec();
        advance(&mut unit, &mut h, 20);
        // The ramp moves with the frame phase.
        assert_ne!(h.ram[0x1000..0x1040], first[..]);
        assert_eq!(h.ram[0x1040], 0);
    }

    #[test]
    fn a_live_view_host_image_unit_samples_the_image_into_the_buffer() {
        use fujimod_emulator_devices::test_host::advance;
        let frames = Frames {
            start: (0xc8, 1 << 28),
            stopped: (0xd8, 0x100),
            irq: 108,
            period: 10,
        };
        let unit = || Block::new(BASE, 0x200, vec![(0, 1)], Some(frames)).unwrap();
        let (red, blue) = ([255, 0, 0], [0, 0, 255]);
        assert!(
            unit()
                .with_live_view_host_image(2, 2, 1, red.to_vec())
                .is_err()
        );
        let mut unit = unit()
            .with_live_view_host_image(2, 2, 1, [red, blue].concat())
            .unwrap();
        let mut h = TestHost::new();
        write(&mut unit, &mut h, BASE + BUFFER, 0x1000);
        write(&mut unit, &mut h, BASE + WIDTH, 4);
        write(&mut unit, &mut h, BASE + 0xc8, 1 << 28);
        advance(&mut unit, &mut h, 10);
        // Each image pixel covers two output pixels, on both lines.
        let row = [[81, 81, 90, 240], [41, 41, 240, 110]].concat();
        assert_eq!(h.ram[0x1000..0x1010], [row.clone(), row].concat()[..]);
        assert_eq!(h.ram[0x1010], 0);
    }

    #[test]
    fn development_engine_jobs_share_idle_status_words_or_have_no_status() {
        use fujimod_emulator_devices::test_host::advance;
        // 0xfffc4000: start +0x00 bit 0, done +0x3c bit 0 beside idle bit 20.
        const ENGINE: u64 = 0xfffc4000;
        let job = Job {
            start: (0, 1),
            status: Some((0x3c, 1)),
            stop: None,
            irq: 132,
            ticks: 10,
        };
        let block = |idle| Block::new(ENGINE, 0x1000, idle, None).unwrap();
        assert!(block(vec![(0x3c, 1 << 20 | 1)]).with_job(job).is_err());
        let mut engine = block(vec![(0, 1 << 28), (0x3c, 1 << 20)])
            .with_job(job)
            .unwrap();
        let mut h = TestHost::new();
        assert_eq!(read(&mut engine, &mut h, ENGINE + 0x3c), 1 << 20);
        write(&mut engine, &mut h, ENGINE, 1);
        advance(&mut engine, &mut h, 10);
        assert_eq!(read(&mut engine, &mut h, ENGINE + 0x3c), 1 << 20 | 1);
        write(&mut engine, &mut h, ENGINE + 0x3c, 1);
        assert_eq!(read(&mut engine, &mut h, ENGINE + 0x3c), 1 << 20);
        // 0xfff98000: start +0x04 bit 0, no status; the handler stops it
        // (+0x00 = 0) to acknowledge interrupt 130.
        let job = Job {
            start: (4, 1),
            status: None,
            stop: Some((0, 1)),
            irq: 130,
            ticks: 10,
        };
        let block = || Block::new(0xfff98000, 0x1000, vec![(0, 1)], None).unwrap();
        assert!(block().with_job(Job { stop: None, ..job }).is_err());
        let mut engine = block().with_job(job).unwrap();
        write(&mut engine, &mut h, 0xfff98004, 1);
        assert_eq!(engine.next_deadline(10), Some(20));
        advance(&mut engine, &mut h, 20);
        assert_eq!(engine.state.status, 1);
        assert_eq!(engine.state.jobs, 1);
        assert_eq!(engine.next_deadline(20), None);
        let saved = engine.save();
        write(&mut engine, &mut h, 0xfff98000, 0);
        assert_eq!(engine.state.status, 0);
        assert_eq!(read(&mut engine, &mut h, 0xfff98000), 1);
        engine.restore(&saved).unwrap();
        assert_eq!(engine.state.status, 1);
    }

    #[test]
    fn a_development_job_and_a_resizer_job_move_the_programmed_pictures() {
        use fujimod_emulator_devices::test_host::advance;
        let mut h = TestHost::new();
        let job = Job {
            start: (4, 1),
            status: None,
            stop: Some((0, 1)),
            irq: 130,
            ticks: 10,
        };
        let block = |base| Block::new(base, 0x1000, vec![(0, 1)], None).unwrap();
        assert!(block(0xfff98000).with_work(Work::Resize).is_err());
        assert!(
            block(0xfff98000)
                .with_job(job)
                .unwrap()
                .with_work(Work::Develop { phase: (6, 0) })
                .is_err()
        );
        // A gray X-Trans frame of 12 lines x 12 samples, stride 32 bytes.
        let gray = crate::imaging::BLACK + (crate::imaging::WHITE - crate::imaging::BLACK) / 5;
        h.ram[0x1000..0x1000 + 32 * 12].copy_from_slice(&gray.to_le_bytes().repeat(16 * 12));
        let mut engine = block(0xfff98000)
            .with_job(job)
            .unwrap()
            .with_work(Work::Develop { phase: (2, 2) })
            .unwrap();
        for (offset, value) in [
            (0x08, 0x1000),
            (0x0c, 16),
            (0x14, 0xb_000b),
            (0x1c, 0x4000),
            (0x20, 16),
        ] {
            write(&mut engine, &mut h, 0xfff98000 + offset, value);
        }
        write(&mut engine, &mut h, 0xfff98004, 1);
        advance(&mut engine, &mut h, 10);
        assert_eq!(engine.state.status, 1);
        let developed = h.ram[0x4000..0x4000 + 32 * 12].to_vec();
        let pixel = &developed[..4];
        assert!(pixel[0] > 100 && pixel[0] == pixel[1] && pixel[2] == 128 && pixel[3] == 128);
        assert!(
            developed
                .chunks(32)
                .all(|line| line[..24].chunks(4).all(|p| p == pixel))
        );
        // The resizer halves it: 12 x 12 to 6 x 6 at 0x8000, stride 12 bytes.
        let mut resizer = block(0xfffc3000)
            .with_job(Job { irq: 131, ..job })
            .unwrap()
            .with_work(Work::Resize)
            .unwrap();
        for (offset, value) in [
            (0x08, 0xb_000b),
            (0x14, 0x5_0005),
            (0x18, 0x1_1000),
            (0x1c, 0x1_1000),
            (0xc0, 0x4000),
            (0xc4, 16),
            (0xc8, 0x8000),
            (0xcc, 6),
        ] {
            write(&mut resizer, &mut h, 0xfffc3000 + offset, value);
        }
        write(&mut resizer, &mut h, 0xfffc3004, 1);
        h.now = 20;
        let mut cx = h.cx();
        resizer.advance(&mut cx);
        assert!(cx.refusal().is_some_and(|f| f.contains("format")));
        drop(cx);
        write(&mut resizer, &mut h, 0xfffc3100, RESIZE_PACKED_422);
        write(&mut resizer, &mut h, 0xfffc3004, 1);
        advance(&mut resizer, &mut h, 40);
        let scaled = &h.ram[0x8000..0x8000 + 12 * 6];
        assert!(scaled.chunks(4).all(|p| p == pixel));
        // Playback's format magnifies: 6x6 to 12x12 at half-pixel steps.
        h.ram[0x8000..0x8000 + 24 * 12].fill(0);
        for (offset, value) in [
            (0x08, 0x5_0005),
            (0x14, 0xb_000b),
            (0x18, 0x800),
            (0x1c, 0x800),
            (0xc0, 0x4000),
            (0xcc, 12),
            (0x100, RESIZE_PACKED_422_PLAYBACK),
        ] {
            write(&mut resizer, &mut h, 0xfffc3000 + offset, value);
        }
        write(&mut resizer, &mut h, 0xfffc3004, 1);
        advance(&mut resizer, &mut h, 60);
        let magnified = &h.ram[0x8000..0x8000 + 24 * 12];
        assert!(magnified.chunks(4).all(|p| p == pixel));
    }

    #[test]
    fn commands_set_their_status_field() {
        let dram = |commands| {
            Block::new(0xfff0f000, 0x1000, vec![(0x14c, 8)], None)?.with_commands(commands)
        };
        let enter = Command {
            word: 0xb8,
            value: 0x6a02_0202,
            status: 0xbc,
            mask: 0x3f,
            state: 0x27,
        };
        let leave = Command {
            value: 0x0102_0202,
            state: 0x20,
            ..enter
        };
        assert!(
            dram(vec![Command {
                state: 0x40,
                ..enter
            }])
            .is_err()
        );
        assert!(
            dram(vec![Command {
                status: 0x14c,
                ..enter
            }])
            .is_err()
        );
        let mut block = dram(vec![enter, leave]).unwrap();
        let base = 0xfff0f000;
        let mut h = TestHost::new();
        write(&mut block, &mut h, base + 0xbc, 0x100);
        write(&mut block, &mut h, base + 0xb8, 0x6a02_0202);
        assert_eq!(read(&mut block, &mut h, base + 0xbc), 0x127);
        assert_eq!(read(&mut block, &mut h, base + 0xb8), 0x6a02_0202);
        write(&mut block, &mut h, base + 0xb8, 0x6a02_0203);
        assert_eq!(read(&mut block, &mut h, base + 0xbc), 0x127);
        write(&mut block, &mut h, base + 0xb8, 0x0102_0202);
        let saved = block.save();
        assert_eq!(read(&mut block, &mut h, base + 0xbc), 0x120);
        let mut other = dram(vec![enter, leave]).unwrap();
        other.restore(&saved).unwrap();
        assert_eq!(read(&mut other, &mut h, base + 0xbc), 0x120);
    }

    #[test]
    fn the_audio_window_reads_the_blocks_words_and_accepts_stores() {
        use fujimod_emulator_ip_arm::pl330::Source;
        let words = Published::default();
        let mut block = Block::new(0xfff1b000, 0x1000, vec![], None)
            .unwrap()
            .with_published(words.clone());
        let mut h = TestHost::new();
        write(&mut block, &mut h, 0xfff1b020, 0x1122_3344);
        let window = SourceWindow::new_writable(0xfff1b000, 0x100, words).unwrap();
        assert!(window.contains(0xfff1b020, 4));
        assert!(!window.contains(0xfff1afff, 2));
        assert!(!window.contains(0xfff1b0ff, 2));
        assert_eq!(
            window.read(0xfff1b020, 4).unwrap(),
            [0x44, 0x33, 0x22, 0x11]
        );
        // A word the CPU never wrote reads silent, so the movie channel's
        // load from +0x20 succeeds before an audio codec exists.
        assert_eq!(window.read(0xfff1b030, 4).unwrap(), [0; 4]);
        assert!(window.writable());
        window.write(0xfff1b020, &[0xaa, 0xbb, 0xcc, 0xdd]).unwrap();
        assert_eq!(
            window.read(0xfff1b020, 4).unwrap(),
            [0xaa, 0xbb, 0xcc, 0xdd]
        );
        // The DMA store updates the published window. The audio block's
        // private register state is not synchronized until a codec model
        // exists; the movie path only needs the store to complete.
    }

    #[test]
    fn a_pl330_load_from_the_audio_window_copies_its_words() {
        use fujimod_emulator_ip_arm::pl330::{Pl330, Throughput};
        let words = Published::default();
        let mut block = Block::new(0xfff1b000, 0x1000, vec![], None)
            .unwrap()
            .with_published(words.clone());
        let mut h = TestHost::new();
        write(&mut block, &mut h, 0xfff1b020, 0x1122_3344);
        let window = SourceWindow::new(0xfff1b000, 0x100, words).unwrap();
        let mut dma = Pl330::new(
            0xfff18000,
            vec![207],
            vec![Arc::new(window)],
            Throughput {
                setup_ticks: 1,
                bytes_per_tick: 1,
            },
        )
        .unwrap();
        // DMAMOV SAR, 0xfff1b020; DAR, 0x1000; CCR 16-byte bursts;
        // DMALDP 0; DMAST; DMAEND.
        let program = [
            0xbc, 0, 0x20, 0xb0, 0xf1, 0xff, 0xbc, 2, 0x00, 0x10, 0x00, 0x00, 0xbc, 1, 0x17, 0xc0,
            0x05, 0x00, 0x25, 0x00, 0x08, 0x00,
        ];
        h.ram[0x100..0x100 + program.len()].copy_from_slice(&program);
        write(&mut dma, &mut h, 0xfff18000 + 0xd0c, 0x100);
        write(&mut dma, &mut h, 0xfff18000 + 0xd08, 0xa2_0000);
        write(&mut dma, &mut h, 0xfff18000 + 0xd04, 0);
        h.now = 17;
        {
            let mut cx = h.cx();
            dma.event(&mut cx, 0);
            assert_eq!(cx.refusal(), None);
        }
        assert_eq!(h.ram[0x1000..0x1004], 0x1122_3344_u32.to_le_bytes());
        assert_eq!(h.ram[0x1004..0x1010], [0; 12]);
    }

    #[test]
    fn fixed_words_read_their_value_whatever_is_written() {
        let block = || Block::new(0xfffc3000, 0x1000, vec![(0, 1)], None).unwrap();
        assert!(block().with_fixed(vec![(0, 1)]).is_err());
        let mut engine = block().with_fixed(vec![(0xdc, 0x0100_0100)]).unwrap();
        let mut h = TestHost::new();
        write(&mut engine, &mut h, 0xfffc30dc, 1);
        assert_eq!(read(&mut engine, &mut h, 0xfffc30dc), 0x0100_0100);
        write(&mut engine, &mut h, 0xfffc30d8, 4);
        assert_eq!(read(&mut engine, &mut h, 0xfffc30d8), 4);
    }
}
