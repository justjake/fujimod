//! Frames as row-addressable CFA sources.
//!
//! The merge reads each frame a band of rows at a time, so a frame can live in
//! a file, in memory or (on a camera) in a capture buffer.

use crate::xtrans::XTrans;
use serde::{Deserialize, Serialize};
use std::io;

/// Converts raw counts to linear values where the white level is 1.0.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Calibration {
    /// Black level per CFA tile position, `[y % 6][x % 6]` in active-area
    /// coordinates.
    pub black: [[f32; 6]; 6],
    /// Raw count of a clipped site.
    pub white: f32,
    /// White-balance multipliers `[R, G, B]` with green at 1.0, used for
    /// luma, previews and the DNG neutral; merged data stays camera-native.
    pub wb: [f32; 3],
}

impl Calibration {
    pub fn uniform(black: f32, white: f32, wb: [f32; 3]) -> Self {
        Self {
            black: [[black; 6]; 6],
            white,
            wb,
        }
    }

    /// Linear value of a raw count at active-area site `(x, y)`, and whether
    /// the site is clipped.
    #[inline(always)]
    pub fn linear(&self, raw: u16, x: usize, y: usize) -> (f32, bool) {
        let black = self.black[y % 6][x % 6];
        let v = (raw as f32 - black) / (self.white - black);
        (v, raw as f32 >= self.white * 0.995 - 0.5)
    }
}

/// Exposure metadata recorded with a frame.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Exposure {
    pub seconds: Option<f64>,
    pub f_number: Option<f64>,
    pub iso: Option<u32>,
    pub bias_ev: Option<f64>,
    pub focal_mm: Option<f64>,
    pub datetime: Option<String>,
    pub subsec: Option<String>,
}

/// One CFA frame. Coordinates are active-area coordinates.
pub trait FrameSource: Sync {
    fn label(&self) -> String;
    fn width(&self) -> usize;
    fn height(&self) -> usize;
    /// CFA layout with `(0, 0)` at the active area's top-left site.
    fn cfa(&self) -> XTrans;
    fn calibration(&self) -> &Calibration;
    fn exposure(&self) -> Exposure {
        Exposure::default()
    }
    /// Fills `out` with whole rows starting at row `y0`; `out.len()` is a
    /// multiple of the width.
    fn read_rows(&self, y0: usize, out: &mut [u16]) -> io::Result<()>;
}

/// A frame held in memory.
pub struct MemoryFrame {
    pub label: String,
    pub width: usize,
    pub height: usize,
    pub cfa: XTrans,
    pub calibration: Calibration,
    pub data: Vec<u16>,
}

impl FrameSource for MemoryFrame {
    fn label(&self) -> String {
        self.label.clone()
    }
    fn width(&self) -> usize {
        self.width
    }
    fn height(&self) -> usize {
        self.height
    }
    fn cfa(&self) -> XTrans {
        self.cfa
    }
    fn calibration(&self) -> &Calibration {
        &self.calibration
    }
    fn read_rows(&self, y0: usize, out: &mut [u16]) -> io::Result<()> {
        let start = y0 * self.width;
        let src = self
            .data
            .get(start..start + out.len())
            .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "rows past frame end"))?;
        out.copy_from_slice(src);
        Ok(())
    }
}
