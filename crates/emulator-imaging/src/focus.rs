//! Virtual focus: the defocus between the attached lens and the host scene.
//!
//! The scene of [`crate::frame`] is a flat picture. This module gives it one focus distance,
//! expressed as the lens focus position at which it is sharp, so the image models can show
//! what an autofocus system needs: a picture that blurs with defocus, and phase-detect pixel
//! pairs whose two views slide apart in proportion to it.
//!
//! # Inputs
//!
//! - The lens. The board attaches the virtual lens with [`attach_lens`]: a function from board
//!   time to the focus motor position, and the motor's travel. Models that have a board time
//!   call [`sample`] before they render, so the picture follows the motor while it moves.
//! - `XT5_SCENE_FOCUS` (a host input outside the run definition and the checkpoint, like
//!   `XT5_SCENE`): the lens position at which the scene is in focus. `N` is a position in lens
//!   units, `N%` a fraction of the travel from the low end. A position outside the travel
//!   cannot be reached, which is how a run asks for an autofocus failure. Unset: the scene is
//!   in focus at every lens position (no blur, no phase shift), as before this module.
//! - `XT5_FOCUS_BLUR` (optional, default 6): the blur-circle diameter, as a percentage of the
//!   picture width, when the defocus equals the full lens travel.
//! - `XT5_FOCUS_PHASE` (optional, default 2): the separation of the two phase-detect views, as
//!   a percentage of the picture width, when the defocus equals the full lens travel.
//!
//! # Model
//!
//! Defocus is linear in the motor position: `lens position - scene position`, in lens units.
//! The blur-circle diameter and the phase separation are both proportional to it (thin lens,
//! small defocus). These are functional scenario values, not a calibrated lens.
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

/// Where the scene is in focus.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SceneFocus {
    /// A lens focus position in lens units.
    Position(f64),
    /// A fraction of the lens travel from its low end (1.0 is the high end).
    Travel(f64),
}

impl SceneFocus {
    /// `N` (lens units) or `N%` (percentage of the travel).
    pub fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim();
        let number = |s: &str| {
            s.trim()
                .parse::<f64>()
                .ok()
                .filter(|v| v.is_finite())
                .ok_or_else(|| format!("{text:?} is not a lens position (N) or a travel percentage (N%)"))
        };
        match text.strip_suffix('%') {
            Some(percent) => Ok(Self::Travel(number(percent)? / 100.)),
            None => Ok(Self::Position(number(text)?)),
        }
    }

    /// The position in lens units for a lens whose travel is `min..=max`.
    pub fn position(self, min: f64, max: f64) -> f64 {
        match self {
            Self::Position(position) => position,
            Self::Travel(fraction) => min + (max - min) * fraction,
        }
    }
}

/// The optical scenario: scene focus and the two proportionality constants.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Optics {
    pub scene: SceneFocus,
    /// Blur-circle diameter over picture width at a defocus of the full travel.
    pub blur_at_travel: f64,
    /// Phase-view separation over picture width at a defocus of the full travel.
    pub phase_at_travel: f64,
}

/// The defocus at one moment.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Defocus {
    /// Lens position minus the scene's focus position, in lens units.
    pub units: f64,
    /// The same as a fraction of the lens travel (signed).
    pub travel: f64,
    /// Blur-circle diameter as a fraction of the picture width.
    pub blur: f64,
    /// Signed separation of the two phase-detect views as a fraction of the picture width:
    /// the second view lies this far to the right of the first.
    pub phase: f64,
    /// Whether the scene's focus position lies inside the lens travel.
    pub reachable: bool,
}

impl Optics {
    /// The defocus of a lens at `position` whose travel is `min..=max`.
    pub fn defocus(&self, position: f64, min: f64, max: f64) -> Defocus {
        let travel = (max - min).max(1.);
        let target = self.scene.position(min, max);
        let units = position - target;
        let fraction = units / travel;
        Defocus {
            units,
            travel: fraction,
            blur: fraction.abs() * self.blur_at_travel,
            phase: fraction * self.phase_at_travel,
            reachable: (min..=max).contains(&target),
        }
    }
}

type Lens = Box<dyn Fn(u64) -> u16 + Send + Sync>;

struct Attached {
    position: Lens,
    min: f64,
    max: f64,
}

static LENS: Mutex<Option<Attached>> = Mutex::new(None);
static ACTIVE: AtomicBool = AtomicBool::new(false);
/// The newest sampled defocus: `blur` and `phase` as f64 bits, for readers without a board time.
static BLUR: AtomicU64 = AtomicU64::new(0);
static PHASE: AtomicU64 = AtomicU64::new(0);

/// The scenario from `XT5_SCENE_FOCUS`, `XT5_FOCUS_BLUR` and `XT5_FOCUS_PHASE`; `None` when
/// `XT5_SCENE_FOCUS` is unset or unreadable. Read once per process.
pub fn host_optics() -> Option<Optics> {
    static OPTICS: OnceLock<Option<Optics>> = OnceLock::new();
    *OPTICS.get_or_init(|| {
        let text = std::env::var("XT5_SCENE_FOCUS").ok().filter(|v| !v.trim().is_empty())?;
        let scene = match SceneFocus::parse(&text) {
            Ok(scene) => scene,
            Err(error) => {
                eprintln!("XT5_SCENE_FOCUS: {error}; the scene stays in focus");
                return None;
            }
        };
        let percent = |name: &str, default: f64| {
            std::env::var(name)
                .ok()
                .and_then(|v| v.trim().parse::<f64>().ok())
                .filter(|v| v.is_finite() && *v >= 0.)
                .unwrap_or(default)
                / 100.
        };
        let optics = Optics {
            scene,
            blur_at_travel: percent("XT5_FOCUS_BLUR", 6.),
            phase_at_travel: percent("XT5_FOCUS_PHASE", 2.),
        };
        eprintln!(
            "XT5_SCENE_FOCUS: scene in focus at {text} (blur {:.1}% and phase separation {:.1}% of the width per full travel)",
            optics.blur_at_travel * 100.,
            optics.phase_at_travel * 100.
        );
        Some(optics)
    })
}

/// Attaches the virtual lens: `position(now)` is the focus motor position at board time `now`,
/// and `min..=max` its travel. The newest attachment wins (one lens per board).
pub fn attach_lens(position: impl Fn(u64) -> u16 + Send + Sync + 'static, min: u16, max: u16) {
    *LENS.lock().unwrap_or_else(|e| e.into_inner()) = Some(Attached {
        position: Box::new(position),
        min: f64::from(min),
        max: f64::from(max),
    });
}

/// Removes the lens (a board without one, or a test).
pub fn detach_lens() {
    *LENS.lock().unwrap_or_else(|e| e.into_inner()) = None;
    store(Defocus::default());
}

fn store(defocus: Defocus) {
    BLUR.store(defocus.blur.to_bits(), Ordering::Relaxed);
    PHASE.store(defocus.phase.to_bits(), Ordering::Relaxed);
    ACTIVE.store(defocus.blur != 0. || defocus.phase != 0., Ordering::Relaxed);
}

/// The defocus at board time `now` with the given scenario; `None` without a lens.
pub fn defocus_with(optics: &Optics, now: u64) -> Option<Defocus> {
    let lens = LENS.lock().unwrap_or_else(|e| e.into_inner());
    let lens = lens.as_ref()?;
    Some(optics.defocus(f64::from((lens.position)(now)), lens.min, lens.max))
}

/// Samples the lens at board time `now` and makes the result the current defocus, which
/// [`blur`] and [`phase`] report to renderers. `None` (and no defocus) without a lens or
/// without `XT5_SCENE_FOCUS`.
pub fn sample(now: u64) -> Option<Defocus> {
    let defocus = host_optics().and_then(|optics| defocus_with(&optics, now));
    store(defocus.unwrap_or_default());
    defocus
}

/// The current blur-circle diameter as a fraction of the picture width (0 when in focus).
pub fn blur() -> f64 {
    if !ACTIVE.load(Ordering::Relaxed) {
        return 0.;
    }
    f64::from_bits(BLUR.load(Ordering::Relaxed))
}

/// The current phase-view separation as a signed fraction of the picture width.
pub fn phase() -> f64 {
    if !ACTIVE.load(Ordering::Relaxed) {
        return 0.;
    }
    f64::from_bits(PHASE.load(Ordering::Relaxed))
}

/// Blurs an sRGB raster (`width * height * 3` bytes) in place with a blur circle of
/// `diameter` pixels: two box passes in each direction, whose combined width matches the
/// circle. Edges repeat the border pixel. A diameter under one pixel changes nothing.
pub fn blur_rgb(rgb: &mut [u8], width: usize, height: usize, diameter: f64) {
    if width == 0 || height == 0 || rgb.len() != width * height * 3 || !(diameter >= 1.) {
        return;
    }
    // Two boxes of width b convolve to a triangle of base 2b - 1; take b = diameter / 2.
    let radius = ((diameter / 2. - 1.) / 2.).round().max(0.) as usize;
    let radius = radius.max(usize::from(diameter >= 2.));
    if radius == 0 {
        return;
    }
    let mut line = vec![0u32; width.max(height) + 1];
    let mut pass = |rgb: &mut [u8], count: usize, step: usize, start: usize| {
        for c in 0..3 {
            line[0] = 0;
            for i in 0..count {
                line[i + 1] = line[i] + u32::from(rgb[start + i * step + c]);
            }
            let (first, last) = (
                u32::from(rgb[start + c]),
                u32::from(rgb[start + (count - 1) * step + c]),
            );
            let window = (2 * radius + 1) as u32;
            for i in 0..count {
                let low = i.saturating_sub(radius);
                let high = (i + radius + 1).min(count);
                let below = (radius - (i - low)) as u32;
                let above = ((i + radius + 1) - high) as u32;
                let sum = line[high] - line[low] + below * first + above * last;
                rgb[start + i * step + c] = ((sum + window / 2) / window) as u8;
            }
        }
    };
    for _ in 0..2 {
        for y in 0..height {
            pass(rgb, width, 3, y * width * 3);
        }
        for x in 0..width {
            pass(rgb, height, width * 3, x * 3);
        }
    }
}

/// One row blurred by a box of `width` samples (any width of at least one sample; the box may
/// cover fractions of samples), borders repeated. Two passes of the half-width box make the
/// smooth profile of a blur circle of that diameter.
fn box_row(row: &[f64], width: f64, out: &mut Vec<f64>, prefix: &mut Vec<f64>) {
    let n = row.len();
    prefix.clear();
    prefix.push(0.);
    for value in row {
        prefix.push(prefix[prefix.len() - 1] + value);
    }
    // Integral of the row from 0 to `t` (samples are unit-wide steps; borders repeat).
    let integral = |t: f64| -> f64 {
        if t <= 0. {
            return t * row[0];
        }
        if t >= n as f64 {
            return prefix[n] + (t - n as f64) * row[n - 1];
        }
        let i = t.floor() as usize;
        prefix[i] + (t - i as f64) * row[i]
    };
    out.clear();
    for x in 0..n {
        let centre = x as f64 + 0.5;
        out.push((integral(centre + width / 2.) - integral(centre - width / 2.)) / width);
    }
}

/// Contrast (focus) evaluation of `levels`, `height` rows of `width` linear samples: for each
/// cell of a `columns` x `rows` grid (row-major) and each entry `s` of `scales`, the sum over
/// the cell of `|L(x) - (L(x - s) + L(x + s)) / 2|`, the response of a horizontal band-pass
/// filter of period about `4 s` samples. Before the filter each row is blurred by a blur circle
/// of `diameter` samples (fractions allowed), so the sums fall as the picture defocuses: a
/// band of scale `s` keeps its level while the circle is smaller than `s` and then falls
/// roughly as `s / diameter`. One vector of `columns * rows` sums per scale.
pub fn contrast_grid(
    levels: &[f64],
    width: usize,
    height: usize,
    columns: usize,
    rows: usize,
    scales: &[usize],
    diameter: f64,
) -> Vec<Vec<f64>> {
    let mut sums = vec![vec![0.; columns * rows]; scales.len()];
    if width == 0 || height == 0 || columns == 0 || rows == 0 || levels.len() != width * height {
        return sums;
    }
    let (mut once, mut twice, mut prefix) = (Vec::new(), Vec::new(), Vec::new());
    for (y, row) in levels.chunks_exact(width).enumerate() {
        let row: &[f64] = if diameter > 1. {
            // Two boxes of half the diameter: a triangle whose base is the diameter.
            let half = (diameter / 2.).max(1.);
            box_row(row, half, &mut once, &mut prefix);
            box_row(&once, half, &mut twice, &mut prefix);
            &twice
        } else {
            row
        };
        let cell_row = y * rows / height;
        for (band, &s) in scales.iter().enumerate() {
            if s == 0 || 2 * s >= width {
                continue;
            }
            for x in s..width - s {
                let response = (row[x] - (row[x - s] + row[x + s]) / 2.).abs();
                sums[band][cell_row * columns + x * columns / width] += response;
            }
        }
    }
    sums
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_focus_parses_positions_and_travel_percentages() {
        assert_eq!(SceneFocus::parse("1500"), Ok(SceneFocus::Position(1500.)));
        assert_eq!(SceneFocus::parse(" 25% "), Ok(SceneFocus::Travel(0.25)));
        assert_eq!(SceneFocus::parse("-300"), Ok(SceneFocus::Position(-300.)));
        assert!(SceneFocus::parse("near").is_err());
        assert_eq!(SceneFocus::Travel(0.25).position(0., 2000.), 500.);
        assert_eq!(SceneFocus::Position(700.).position(0., 2000.), 700.);
    }

    #[test]
    fn defocus_is_linear_and_signed() {
        let optics = Optics {
            scene: SceneFocus::Position(1500.),
            blur_at_travel: 0.06,
            phase_at_travel: 0.02,
        };
        let focused = optics.defocus(1500., 0., 2000.);
        assert_eq!((focused.units, focused.blur, focused.phase), (0., 0., 0.));
        assert!(focused.reachable);
        let near = optics.defocus(500., 0., 2000.);
        assert_eq!(near.units, -1000.);
        assert!((near.blur - 0.03).abs() < 1e-12);
        assert!((near.phase + 0.01).abs() < 1e-12);
        let far = optics.defocus(2000., 0., 2000.);
        assert!((far.blur - 0.015).abs() < 1e-12);
        assert!((far.phase - 0.005).abs() < 1e-12);
        let unreachable = Optics { scene: SceneFocus::Travel(1.5), ..optics }.defocus(2000., 0., 2000.);
        assert!(!unreachable.reachable);
        assert_eq!(unreachable.units, -1000.);
    }

    #[test]
    fn lens_attachment_follows_board_time() {
        let optics = Optics {
            scene: SceneFocus::Position(1000.),
            blur_at_travel: 0.1,
            phase_at_travel: 0.1,
        };
        attach_lens(|now| (now / 10) as u16, 0, 2000);
        assert_eq!(defocus_with(&optics, 10_000).unwrap().units, 0.);
        assert_eq!(defocus_with(&optics, 5_000).unwrap().units, -500.);
        detach_lens();
        assert_eq!(defocus_with(&optics, 5_000), None);
        assert_eq!((blur(), phase()), (0., 0.));
    }

    fn edge(width: usize, height: usize) -> Vec<u8> {
        let mut rgb = vec![0u8; width * height * 3];
        for y in 0..height {
            for x in width / 2..width {
                rgb[(y * width + x) * 3..][..3].copy_from_slice(&[200, 200, 200]);
            }
        }
        rgb
    }

    /// Sum of absolute horizontal differences: a sharpness measure.
    fn contrast(rgb: &[u8], width: usize) -> u64 {
        rgb.chunks_exact(width * 3)
            .map(|row| {
                (1..width)
                    .map(|x| u64::from(row[x * 3].abs_diff(row[x * 3 - 3])).pow(2))
                    .sum::<u64>()
            })
            .sum()
    }

    #[test]
    fn blur_spreads_an_edge_and_keeps_the_mean() {
        let (w, h) = (64, 8);
        let sharp = edge(w, h);
        let mut small = sharp.clone();
        blur_rgb(&mut small, w, h, 0.5);
        assert_eq!(small, sharp, "a sub-pixel circle changes nothing");
        let mut soft = sharp.clone();
        blur_rgb(&mut soft, w, h, 6.);
        let mut softer = sharp.clone();
        blur_rgb(&mut softer, w, h, 16.);
        assert!(contrast(&soft, w) < contrast(&sharp, w));
        assert!(contrast(&softer, w) < contrast(&soft, w));
        // Far from the edge the levels are unchanged; the edge centre is mid-grey.
        assert_eq!(softer[0], 0);
        assert_eq!(softer[(w - 1) * 3], 200);
        let middle = (softer[(w / 2 - 1) * 3] as u32 + softer[(w / 2) * 3] as u32) / 2;
        assert!((95..=105).contains(&middle), "{middle}");
        // The transition is monotonic.
        let row = &softer[..w * 3];
        assert!((1..w).all(|x| row[x * 3] >= row[x * 3 - 3]));
    }

    /// Rows of vertical bars: `period` samples per bar, alternating 0 and 1000.
    fn bars(width: usize, height: usize, period: usize) -> Vec<f64> {
        (0..width * height).map(|i| if (i % width) / period % 2 == 0 { 0. } else { 1000. }).collect()
    }

    #[test]
    fn contrast_falls_smoothly_with_the_blur_circle() {
        let (w, h) = (512, 8);
        let levels = bars(w, h, 32);
        let total = |diameter: f64, band: usize| -> f64 {
            contrast_grid(&levels, w, h, 8, 2, &[1, 2, 4], diameter)[band].iter().sum()
        };
        // In focus: every edge gives `step * s / 2` on each side, so the bands are in ratio 1:2:4.
        let sharp = [total(0., 0), total(0., 1), total(0., 2)];
        assert!(sharp[0] > 0.);
        assert!((sharp[1] / sharp[0] - 2.).abs() < 0.2 && (sharp[2] / sharp[0] - 4.).abs() < 0.6);
        // Every band falls monotonically with the circle, the fine band first.
        let mut previous = sharp;
        for diameter in [1.5, 2., 3., 4.5, 6., 9., 14., 22.] {
            let now = [total(diameter, 0), total(diameter, 1), total(diameter, 2)];
            for band in 0..3 {
                assert!(now[band] <= previous[band] + 1e-6, "band {band} at {diameter}");
            }
            previous = now;
        }
        assert!(total(4., 0) / sharp[0] < total(4., 2) / sharp[2]);
        assert!(total(22., 2) < 0.4 * sharp[2]);
        // A flat picture has no contrast at any focus.
        let flat = vec![500.; w * h];
        assert!(contrast_grid(&flat, w, h, 8, 2, &[1, 2, 4], 0.).iter().flatten().all(|v| *v == 0.));
    }

    #[test]
    fn contrast_lands_in_the_cell_of_the_edge() {
        let (w, h) = (64, 4);
        // One edge at x = 40: cells are 16 wide, so only column 2 of each row sees it.
        let levels: Vec<f64> = (0..w * h).map(|i| if i % w < 40 { 0. } else { 800. }).collect();
        let grid = &contrast_grid(&levels, w, h, 4, 2, &[1], 0.)[0];
        assert_eq!(grid.len(), 8);
        for (cell, value) in grid.iter().enumerate() {
            assert_eq!(*value > 0., cell % 4 == 2, "cell {cell}");
        }
        // The same for both grid rows.
        assert_eq!(grid[2], grid[6]);
    }
}
