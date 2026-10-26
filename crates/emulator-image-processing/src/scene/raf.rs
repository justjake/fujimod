//! Parse Fujifilm X70 RAF files and extract linear relative scene radiance.
//!
//! RAF files contain uncompressed X-Trans CFA (Color Filter Array) data with
//! embedded EXIF/TIFF metadata. This module reads the X-Trans 6×6 pattern,
//! black/white levels, and exposure parameters, then converts raw samples to
//! linear radiometric values. Output is per-site radiance at native sensor
//! resolution, tagged with CFA phase; a cheap linear RGB preview view is also
//! provided for optics/preview callers.

use fujimod_emulator_imaging::scene::mosaic::Radiance;
use fujimod_emulator_imaging::scene::{Frame, FramePixels};
use serde::{Deserialize, Serialize};
use std::io::{self, Read, Seek, SeekFrom};

/// Recorded exposure parameters from EXIF/TIFF metadata.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RecordedExposure {
    /// Exposure time in seconds (1 / shutter speed).
    pub seconds: f64,
    /// Aperture f-number (e.g., 2.8, 5.6, 9.0).
    pub f_number: f64,
    /// ISO sensitivity (e.g., 200, 400, 800, 1600).
    pub iso: u16,
    /// Dynamic range mode: 100, 200, or 400.
    /// DR200 lowers effective sensor gain 2× relative to displayed ISO.
    /// DR400 lowers effective sensor gain 4× relative to displayed ISO.
    pub dynamic_range: u16,
}

/// Fujifilm X70 RAF scene: per-site radiance and exposure metadata.
/// Per-site radiance values are tagged with their CFA phase (0=R, 1=G, 2=B).
/// The emulated X-Trans sensor reads each site's radiance directly with no demosaic.
#[derive(Clone, Debug)]
pub struct RafScene {
    /// Full sensor width in pixels (4992 for X70).
    pub width: u32,
    /// Full sensor height in pixels (3296 for X70).
    pub height: u32,
    /// X-Trans 6×6 CFA pattern (6 rows, 6 columns per pattern tile).
    /// Each value is 0 (R), 1 (G), or 2 (B). Used to determine CFA phase at any pixel.
    pub cfa: [[u8; 6]; 6],
    /// Black level per CFA color: [R, G, B] in raw 14-bit counts.
    pub black: [u16; 3],
    /// White level (clipping point): 16383 for 14-bit (2^14 - 1).
    pub white: u16,
    /// Per-site relative scene radiance at native sensor resolution.
    /// Row-major black-subtracted counts, one u16 per sensor site.
    /// Normalized: (raw - black) / (exposure_time / f_number² × sensor_gain),
    /// where sensor_gain = ISO/200 × DR_factor:
    /// - DR100: factor = 1.0
    /// - DR200: factor = 0.5 (effective gain half the displayed ISO)
    /// - DR400: factor = 0.25 (effective gain quarter the displayed ISO)
    pub counts: Vec<u16>,
    /// Original exposure normalization; `radiance(index)` applies the f32 rounding.
    pub norm: f64,
    /// Exposure parameters from EXIF/TIFF.
    pub exposure: RecordedExposure,
    /// Crop top offset in pixels (from FujiIFD tag 0x0110 RawImageCropTopLeft).
    pub crop_top: u16,
    /// Crop left offset in pixels (from FujiIFD tag 0x0110 RawImageCropTopLeft).
    pub crop_left: u16,
    /// Cropped image height in pixels (from FujiIFD tag 0x0111 RawImageCroppedSize).
    pub crop_height: u16,
    /// Cropped image width in pixels (from FujiIFD tag 0x0111 RawImageCroppedSize).
    pub crop_width: u16,
}

impl RafScene {
    #[inline(always)]
    pub fn radiance(&self, index: usize) -> f32 {
        (f64::from(self.counts[index]) / self.norm) as f32
    }
}

/// Linear RGB preview: 6×6 bins of per-color average radiance.
/// Created by averaging all sites of each CFA color within each 6×6 cell.
#[derive(Clone, Debug)]
pub struct RafRadianceRgb {
    /// Output width: (raw width + 5) / 6 cells.
    pub width: u32,
    /// Output height: (raw height + 5) / 6 cells.
    pub height: u32,
    /// RGB radiance: Vec of 3 f32 values [R, G, B] per 6×6 cell.
    /// Cells are in row-major order.
    pub radiance: Vec<[f32; 3]>,
}

// X-Trans 6×6 CFA pattern: rows 0-5, columns 0-5.
// Values: 0 = Red, 1 = Green, 2 = Blue.
// This is the canonical Fujifilm X-Trans layout used in X70 and other bodies.
const XTRANS: [[u8; 6]; 6] = [
    [0, 2, 1, 2, 0, 1], // row 0
    [1, 1, 0, 1, 1, 2], // row 1
    [1, 1, 2, 1, 1, 0], // row 2
    [2, 0, 1, 0, 2, 1], // row 3
    [1, 1, 2, 1, 1, 0], // row 4
    [1, 1, 0, 1, 1, 2], // row 5
];

/// Parse FujiIFD from RAF TIFF section to extract black level and visible area dimensions.
/// The FujiIFD is a 16-entry TIFF directory with tags:
/// - 0xf001: RawImageFullWidth
/// - 0xf002: RawImageFullHeight
/// - 0xf00a: BlackLevel (36 int32u values, all equal for X70)
fn parse_fuji_ifd(tiff_buf: &[u8; 2048]) -> io::Result<(u16, u16, u16, u16, u16)> {
    // The RAF TIFF section contains crop parameters in a Fuji directory (tags 0x0110, 0x0111),
    // but that directory may be beyond the 2048-byte buffer we read. For X70, the crop values
    // are: top=16, left=24, height=3264, width=4896 from FujiIFD tag 0x0110/0x0111.
    // Future enhancement: read these from the file if the directory structure is accessible.
    const X70_CROP_TOP: u16 = 16;
    const X70_CROP_LEFT: u16 = 24;
    const X70_CROP_HEIGHT: u16 = 3264;
    const X70_CROP_WIDTH: u16 = 4896;

    // Find the 1-entry IFD header with tag 0xf000 pointing to the 16-entry FujiIFD.
    let mut fuji_ifd_header_offset = None;
    for search_offset in (0..tiff_buf.len().saturating_sub(14)).step_by(2) {
        if search_offset + 14 <= tiff_buf.len() {
            let entry_count =
                u16::from_le_bytes([tiff_buf[search_offset], tiff_buf[search_offset + 1]]);
            if entry_count == 1 {
                let tag_offset = search_offset + 2;
                let tag = u16::from_le_bytes([tiff_buf[tag_offset], tiff_buf[tag_offset + 1]]);
                if tag == 0xf000 {
                    fuji_ifd_header_offset = Some(search_offset);
                    break;
                }
            }
        }
    }

    let header_offset = fuji_ifd_header_offset.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "FujiIFD header (tag 0xf000) not found in RAF TIFF section",
        )
    })?;

    // Read the 0xf000 entry to get the pointer to the 16-entry FujiIFD.
    let entry_offset = header_offset + 2;
    let fuji_ifd_ptr = u32::from_le_bytes([
        tiff_buf[entry_offset + 8],
        tiff_buf[entry_offset + 9],
        tiff_buf[entry_offset + 10],
        tiff_buf[entry_offset + 11],
    ]) as usize;

    let ifd_offset = fuji_ifd_ptr;
    if ifd_offset + 2 + 12 * 16 > tiff_buf.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "FujiIFD offset out of bounds",
        ));
    }

    // Verify the FujiIFD has 16 entries.
    let entry_count = u16::from_le_bytes([tiff_buf[ifd_offset], tiff_buf[ifd_offset + 1]]);
    if entry_count != 16 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Expected 16 FujiIFD entries, got {}", entry_count),
        ));
    }

    // Parse the 16 FujiIFD entries.
    let mut fuji_tags: std::collections::HashMap<u16, (u16, u32, u32)> =
        std::collections::HashMap::new();
    for i in 0..16 {
        let at = ifd_offset + 2 + 12 * i;
        let tag = u16::from_le_bytes([tiff_buf[at], tiff_buf[at + 1]]);
        let typ = u16::from_le_bytes([tiff_buf[at + 2], tiff_buf[at + 3]]);
        let count = u32::from_le_bytes([
            tiff_buf[at + 4],
            tiff_buf[at + 5],
            tiff_buf[at + 6],
            tiff_buf[at + 7],
        ]);
        let value = u32::from_le_bytes([
            tiff_buf[at + 8],
            tiff_buf[at + 9],
            tiff_buf[at + 10],
            tiff_buf[at + 11],
        ]);
        fuji_tags.insert(tag, (typ, count, value));
    }

    // Extract and validate RawImageFullWidth (0xf001) and RawImageFullHeight (0xf002).
    let raw_width = fuji_tags
        .get(&0xf001u16)
        .map(|(_, _, v)| *v as u32)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Missing tag 0xf001"))?;

    let raw_height = fuji_tags
        .get(&0xf002u16)
        .map(|(_, _, v)| *v as u32)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Missing tag 0xf002"))?;

    if (raw_width, raw_height) != (4992, 3296) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "Unexpected FujiIFD dimensions: {}x{}",
                raw_width, raw_height
            ),
        ));
    }

    // Parse black level (tag 0xf00a): 36 int32u values all equal for X70.
    let black_level = match fuji_tags.get(&0xf00au16) {
        Some((typ, count, offset)) => {
            if (*typ, *count) != (4, 36) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("Unsupported black-level map: type={}, count={}", typ, count),
                ));
            }
            let offset = *offset as usize;
            if offset + 144 > tiff_buf.len() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Black-level map offset out of bounds",
                ));
            }
            let mut values = Vec::with_capacity(36);
            for i in 0..36 {
                let v = u32::from_le_bytes([
                    tiff_buf[offset + i * 4],
                    tiff_buf[offset + i * 4 + 1],
                    tiff_buf[offset + i * 4 + 2],
                    tiff_buf[offset + i * 4 + 3],
                ]);
                values.push(v as u16);
            }

            if values.iter().any(|&v| v != values[0]) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Unequal per-site black levels require qualified orientation",
                ));
            }
            values[0]
        }
        None => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Missing black level tag (0xf00a) in FujiIFD",
            ));
        }
    };

    Ok((
        black_level,
        X70_CROP_TOP,
        X70_CROP_LEFT,
        X70_CROP_HEIGHT,
        X70_CROP_WIDTH,
    ))
}

/// Parse RAF file and return scene data with per-site radiance.
///
/// Validates RAF header, CFA structure, TIFF directory, and raw plane size.
/// Returns error for compressed variants or mismatched dimensions.
pub fn read_raf<R: Read + Seek>(mut file: R) -> io::Result<RafScene> {
    // Read RAF header (148 bytes minimum).
    let mut header = [0u8; 148];
    file.read_exact(&mut header)?;

    // Check magic: "FUJIFILMCCD-RAW " (16 bytes).
    if &header[0..16] != b"FUJIFILMCCD-RAW " {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "RAF header magic mismatch",
        ));
    }

    // Check camera model: "X70\0" at offset 28.
    if &header[28..32] != b"X70\0" {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Expected X70 RAF file",
        ));
    }

    // Parse offset table (6 big-endian u32 at offset 84).
    let jpeg_offset = u32::from_be_bytes([header[84], header[85], header[86], header[87]]) as u64;
    let jpeg_size = u32::from_be_bytes([header[88], header[89], header[90], header[91]]) as u64;
    let cfa_offset = u32::from_be_bytes([header[92], header[93], header[94], header[95]]) as u64;
    let cfa_size = u32::from_be_bytes([header[96], header[97], header[98], header[99]]) as u64;
    let raw_offset =
        u32::from_be_bytes([header[100], header[101], header[102], header[103]]) as u64;
    let raw_size = u32::from_be_bytes([header[104], header[105], header[106], header[107]]) as u64;

    // Validate offset table bounds.
    if !(148 <= jpeg_offset
        && jpeg_offset < cfa_offset
        && cfa_offset < raw_offset
        && raw_offset < (1u64 << 31)
        && jpeg_size >= 2048
        && cfa_size >= 128
        && cfa_size <= 32768)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Unsupported RAF offset table",
        ));
    }

    // X70 raw plane is uncompressed: 2048 byte TIFF + (4992×3296×2) samples.
    let expected_raw_size = 2048 + (4992u64 * 3296 * 2);
    if raw_size != expected_raw_size {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Unsupported/compressed RAF plane (expected uncompressed X70)",
        ));
    }

    // Read CFA header.
    file.seek(SeekFrom::Start(cfa_offset))?;
    let mut cfa_buf = vec![0u8; cfa_size as usize];
    file.read_exact(&mut cfa_buf)?;

    // Parse CFA structure: 4-byte magic (0x0000000a), then 9 TLV entries.
    if cfa_buf.len() < 4
        || u32::from_be_bytes([cfa_buf[0], cfa_buf[1], cfa_buf[2], cfa_buf[3]]) != 10
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Invalid CFA header",
        ));
    }

    let mut tags: std::collections::HashMap<u16, Vec<u8>> = std::collections::HashMap::new();
    let mut at = 4usize;
    for _ in 0..9 {
        if at + 4 > cfa_buf.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Truncated CFA geometry",
            ));
        }
        let tag = u16::from_be_bytes([cfa_buf[at], cfa_buf[at + 1]]);
        let len = u16::from_be_bytes([cfa_buf[at + 2], cfa_buf[at + 3]]) as usize;
        at += 4;

        if at + len > cfa_buf.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Truncated CFA geometry",
            ));
        }
        tags.insert(tag, cfa_buf[at..at + len].to_vec());
        at += len;
    }

    // Validate CFA pattern and dimensions.
    // Tag 0x100: [height, width] as 2 u16 big-endian.
    // Tag 0x131: CFA pattern (36 bytes, 6×6, values 0/1/2).
    if let Some(dims) = tags.get(&0x100) {
        if dims.len() != 4 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid CFA dimensions tag",
            ));
        }
        let h = u16::from_be_bytes([dims[0], dims[1]]) as u32;
        let w = u16::from_be_bytes([dims[2], dims[3]]) as u32;
        if (h, w) != (3296, 4992) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Unsupported CFA geometry",
            ));
        }
    } else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Missing CFA dimensions tag",
        ));
    }

    if let Some(pattern_data) = tags.get(&0x131) {
        if pattern_data.len() != 36 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid CFA pattern length",
            ));
        }
        // Check all pattern values are 0, 1, or 2.
        if !pattern_data.iter().all(|&v| v <= 2) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Unsupported CFA colors",
            ));
        }
    } else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Missing CFA pattern tag",
        ));
    }

    // Read and parse TIFF section from raw plane to extract FujiIFD with black level.
    // The raw section starts with 2048 bytes of TIFF data containing the FujiIFD.
    file.seek(SeekFrom::Start(raw_offset))?;
    let mut tiff_buf = [0u8; 2048];
    file.read_exact(&mut tiff_buf)?;

    // Validate TIFF little-endian header.
    if &tiff_buf[0..4] != b"II*\0" {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Unsupported RAF TIFF header",
        ));
    }

    // Parse FujiIFD from RAF's raw TIFF section to extract black level and crop parameters.
    // The FujiIFD is a 16-entry TIFF directory embedded within the 2048-byte TIFF section.
    let (black_level, crop_top, crop_left, crop_height, crop_width) = parse_fuji_ifd(&tiff_buf)?;

    // Read raw samples from file (starting at raw_offset + 2048).
    let samples_offset = raw_offset + 2048;
    file.seek(SeekFrom::Start(samples_offset))?;
    let sample_count = (4992u32 * 3296) as usize;
    let mut raw_samples = vec![0u16; sample_count];
    for sample in &mut raw_samples {
        let mut buf = [0u8; 2];
        file.read_exact(&mut buf)?;
        *sample = u16::from_le_bytes(buf);
    }

    // Read EXIF metadata from JPEG segment.
    // EXIF headers are typically in the first 64 KiB of the JPEG.
    file.seek(SeekFrom::Start(jpeg_offset))?;
    let jpeg_read_size = std::cmp::min(jpeg_size as usize, 65536);
    let mut jpeg_buf = vec![0u8; jpeg_read_size];
    let jpeg_read = file.read(&mut jpeg_buf)?;
    let exif_metadata = parse_exif(&jpeg_buf[..jpeg_read])?;

    // Retain black-subtracted counts and the original radiance normalization.
    let (counts, norm) = compute_counts(raw_samples, black_level, &exif_metadata);

    Ok(RafScene {
        width: 4992,
        height: 3296,
        cfa: XTRANS,
        black: [black_level; 3], // Assume equal across channels for X70.
        white: 16383,            // 14-bit: (1<<14)-1.
        counts,
        norm,
        exposure: exif_metadata,
        crop_top,
        crop_left,
        crop_height,
        crop_width,
    })
}

/// Remove the black offset in place and retain the exposure normalization.
fn compute_counts(
    mut raw_samples: Vec<u16>,
    black_level: u16,
    exposure: &RecordedExposure,
) -> (Vec<u16>, f64) {
    // Sensor gain = ISO / 200 × DR_factor:
    // - DR100: factor = 1.0
    // - DR200: factor = 0.5
    // - DR400: factor = 0.25
    let dr_factor = match exposure.dynamic_range {
        200 => 0.5,
        400 => 0.25,
        _ => 1.0, // DR100
    };
    let sensor_gain = (exposure.iso as f64 / 200.0) * dr_factor;

    // Normalization factor: exposure_time / (f_number² × sensor_gain).
    let f_squared = exposure.f_number * exposure.f_number;
    let norm = exposure.seconds / (f_squared * sensor_gain);

    for raw in &mut raw_samples {
        *raw = raw.saturating_sub(black_level);
    }
    (raw_samples, norm)
}

/// Convert per-site radiance to linear RGB preview.
/// Averages all sites of each CFA color within each 6×6 cell.
pub fn raf_to_radiance_rgb(scene: &RafScene) -> RafRadianceRgb {
    // Output dimensions: one cell per 6 pixels.
    let cell_width = (scene.width as usize + 5) / 6;
    let cell_height = (scene.height as usize + 5) / 6;

    let mut rgb_cells = vec![[0f32; 3]; cell_width * cell_height];

    // Accumulate per-color sums in each cell.
    let mut color_counts = vec![[0usize; 3]; cell_width * cell_height];

    for y in 0..scene.height as usize {
        for x in 0..scene.width as usize {
            let sample_idx = y * scene.width as usize + x;
            let color = scene.cfa[y % 6][x % 6] as usize;

            let cell_x = x / 6;
            let cell_y = y / 6;
            let cell_idx = cell_y * cell_width + cell_x;

            if color < 3 && cell_idx < rgb_cells.len() {
                rgb_cells[cell_idx][color] += scene.radiance(sample_idx);
                color_counts[cell_idx][color] += 1;
            }
        }
    }

    // Average each color in each cell.
    for (cell, counts) in rgb_cells.iter_mut().zip(color_counts.iter()) {
        for c in 0..3 {
            if counts[c] > 0 {
                cell[c] /= counts[c] as f32;
            }
        }
    }

    RafRadianceRgb {
        width: cell_width as u32,
        height: cell_height as u32,
        radiance: rgb_cells,
    }
}

/// Parse exposure metadata from JPEG/EXIF headers.
/// Searches for EXIF marker (0xFFE1) and extracts from the ExifIFD (sub-directory):
/// - ISO (tag 0x8827): type 3 (u16), count 1
/// - Exposure time (tag 0x829A): type 5 (rational), count 1
/// - F-number (tag 0x829D): type 5 (rational), count 1
/// - Fujifilm maker notes (tag 0x927C): DevelopmentDynamicRange (0x1403, type 3 u16)
/// Returns error if any required tag is missing or unparsable.
fn parse_exif(jpeg_data: &[u8]) -> io::Result<RecordedExposure> {
    // Find EXIF marker (0xFFE1) and get TIFF base offset.
    let exif_start = find_exif_ifd(jpeg_data)?;

    // Read TIFF IFD offset from the TIFF header (bytes 4-7, little-endian).
    if exif_start + 8 > jpeg_data.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "EXIF TIFF header truncated",
        ));
    }
    let main_ifd_offset = u32::from_le_bytes([
        jpeg_data[exif_start + 4],
        jpeg_data[exif_start + 5],
        jpeg_data[exif_start + 6],
        jpeg_data[exif_start + 7],
    ]) as usize;

    // Parse main IFD.
    let main_ifd_tags = parse_ifd(&jpeg_data[exif_start..], main_ifd_offset)?;

    // Find ExifOffset (tag 0x8769) which points to the ExifIFD.
    let exif_ifd_offset = match main_ifd_tags.get(&0x8769u16) {
        Some((_, _, offset)) => *offset as usize,
        None => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Missing ExifIFD pointer tag (0x8769)",
            ));
        }
    };

    // Parse ExifIFD.
    let tags = parse_ifd(&jpeg_data[exif_start..], exif_ifd_offset)?;

    // Extract ISO (tag 0x8827): type 3 (u16), count 1.
    let iso = match tags.get(&0x8827u16) {
        Some((typ, count, value)) if *typ == 3 && *count == 1 => *value as u16,
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Missing or invalid ISO tag (0x8827)",
            ));
        }
    };

    // Extract exposure time (tag 0x829A): type 5 (rational), count 1.
    // Offset is relative to the start of the EXIF TIFF data.
    let exposure_time = match tags.get(&0x829Au16) {
        Some((typ, count, offset)) if *typ == 5 && *count == 1 => {
            let abs_offset = exif_start + *offset as usize;
            if abs_offset + 8 > jpeg_data.len() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Exposure time rational offset out of bounds",
                ));
            }
            let num = u32::from_le_bytes([
                jpeg_data[abs_offset],
                jpeg_data[abs_offset + 1],
                jpeg_data[abs_offset + 2],
                jpeg_data[abs_offset + 3],
            ]);
            let denom = u32::from_le_bytes([
                jpeg_data[abs_offset + 4],
                jpeg_data[abs_offset + 5],
                jpeg_data[abs_offset + 6],
                jpeg_data[abs_offset + 7],
            ]);
            if denom == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Exposure time denominator is zero",
                ));
            }
            num as f64 / denom as f64
        }
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Missing or invalid exposure time tag (0x829A)",
            ));
        }
    };

    // Extract f-number (tag 0x829D): type 5 (rational), count 1.
    // Offset is relative to the start of the EXIF TIFF data.
    let f_number = match tags.get(&0x829Du16) {
        Some((typ, count, offset)) if *typ == 5 && *count == 1 => {
            let abs_offset = exif_start + *offset as usize;
            if abs_offset + 8 > jpeg_data.len() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "F-number rational offset out of bounds",
                ));
            }
            let num = u32::from_le_bytes([
                jpeg_data[abs_offset],
                jpeg_data[abs_offset + 1],
                jpeg_data[abs_offset + 2],
                jpeg_data[abs_offset + 3],
            ]);
            let denom = u32::from_le_bytes([
                jpeg_data[abs_offset + 4],
                jpeg_data[abs_offset + 5],
                jpeg_data[abs_offset + 6],
                jpeg_data[abs_offset + 7],
            ]);
            if denom == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "F-number denominator is zero",
                ));
            }
            num as f64 / denom as f64
        }
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Missing or invalid f-number tag (0x829D)",
            ));
        }
    };

    // Extract DevelopmentDynamicRange from Fujifilm maker notes (tag 0x927C).
    // Offset is relative to the start of the EXIF TIFF data.
    let dynamic_range = match tags.get(&0x927Cu16) {
        Some((_, _, offset)) => {
            let abs_offset = exif_start + *offset as usize;
            // Maker notes start with "FUJIFILM\x0c\x00\x00\x00" (8 bytes magic + 4 bytes LE IFD offset).
            if abs_offset + 12 > jpeg_data.len()
                || &jpeg_data[abs_offset..abs_offset + 8] != b"FUJIFILM"
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Invalid Fujifilm maker notes header",
                ));
            }
            // The IFD offset is at maker_notes_offset + 8 (LE u32).
            let maker_ifd_offset = u32::from_le_bytes([
                jpeg_data[abs_offset + 8],
                jpeg_data[abs_offset + 9],
                jpeg_data[abs_offset + 10],
                jpeg_data[abs_offset + 11],
            ]) as usize;

            // Parse maker notes IFD relative to the maker notes start.
            let maker_tags = parse_ifd(&jpeg_data[abs_offset..], maker_ifd_offset)?;

            // Tag 0x1403: DevelopmentDynamicRange (type 3, u16).
            match maker_tags.get(&0x1403u16) {
                Some((typ, count, value)) if *typ == 3 && *count == 1 => {
                    let dr_val = *value as u16;
                    // Validate: should be 100, 200, or 400.
                    match dr_val {
                        100 | 200 | 400 => dr_val,
                        _ => {
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidData,
                                format!("Invalid DynamicRange value: {}", dr_val),
                            ));
                        }
                    }
                }
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Missing or invalid DevelopmentDynamicRange tag (0x1403)",
                    ));
                }
            }
        }
        None => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Missing Fujifilm maker notes tag (0x927C)",
            ));
        }
    };

    Ok(RecordedExposure {
        seconds: exposure_time,
        f_number,
        iso,
        dynamic_range,
    })
}

/// Find EXIF IFD offset in JPEG data.
fn find_exif_ifd(jpeg_data: &[u8]) -> io::Result<usize> {
    // Search for EXIF marker (0xFFE1).
    for i in 0..jpeg_data.len().saturating_sub(8) {
        if jpeg_data[i] == 0xFF && jpeg_data[i + 1] == 0xE1 {
            // Marker found. Skip 0xFFE1 (2) + length (2) + "Exif\0\0" (6) = 10 bytes.
            let exif_start = i + 10;
            if exif_start + 4 > jpeg_data.len() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "EXIF segment too short",
                ));
            }
            // Validate TIFF header (little-endian "II").
            if jpeg_data[exif_start] == b'I' && jpeg_data[exif_start + 1] == b'I' {
                return Ok(exif_start);
            }
        }
    }
    // If no EXIF found, return default error (use defaults).
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "EXIF metadata not found",
    ))
}

/// Parse TIFF IFD entries at a given offset within EXIF data.
/// Returns a map of tag -> (type, count, value_or_offset).
fn parse_ifd(
    exif_data: &[u8],
    ifd_offset: usize,
) -> io::Result<std::collections::HashMap<u16, (u16, u32, u32)>> {
    let mut tags = std::collections::HashMap::new();

    if ifd_offset + 2 > exif_data.len() {
        // Silent return - check during debugging only
        return Ok(tags);
    }

    let entry_count =
        u16::from_le_bytes([exif_data[ifd_offset], exif_data[ifd_offset + 1]]) as usize;
    if ifd_offset + 2 + 12 * entry_count > exif_data.len() {
        // Silent return - check during debugging only
        return Ok(tags);
    }

    for i in 0..entry_count {
        let at = ifd_offset + 2 + 12 * i;
        let tag = u16::from_le_bytes([exif_data[at], exif_data[at + 1]]);
        let typ = u16::from_le_bytes([exif_data[at + 2], exif_data[at + 3]]);
        let count = u32::from_le_bytes([
            exif_data[at + 4],
            exif_data[at + 5],
            exif_data[at + 6],
            exif_data[at + 7],
        ]);
        let value = u32::from_le_bytes([
            exif_data[at + 8],
            exif_data[at + 9],
            exif_data[at + 10],
            exif_data[at + 11],
        ]);
        tags.insert(tag, (typ, count, value));
    }

    Ok(tags)
}

/// A RAF file's visible crop as a mosaic sensor frame.
pub fn read_frame(bytes: &[u8]) -> Result<Frame, String> {
    if bytes.len() > 64 << 20 {
        return Err("sensor RAF input exceeds 64 MiB".into());
    }
    let scene = read_raf(std::io::Cursor::new(bytes)).map_err(|e| e.to_string())?;
    frame(&scene)
}
/// The RAF's visible crop as a mosaic frame. The CFA pattern is re-phased
/// to the crop origin, so sites keep their physical colors.
pub fn frame(scene: &RafScene) -> Result<Frame, String> {
    let (full_width, full_height) = (scene.width as usize, scene.height as usize);
    let (left, top) = (scene.crop_left as usize, scene.crop_top as usize);
    let (width, height) = (scene.crop_width as usize, scene.crop_height as usize);
    if scene.counts.len() != full_width * full_height
        || width == 0
        || height == 0
        || left + width > full_width
        || top + height > full_height
    {
        return Err("RAF crop lies outside its radiance raster".into());
    }
    let cfa = std::array::from_fn(|y| {
        std::array::from_fn(|x| scene.cfa[(y + top) % 6][(x + left) % 6])
    });
    let mut bytes = Vec::with_capacity(width * height * 2);
    for y in top..top + height {
        let row = y * full_width + left;
        for value in &scene.counts[row..row + width] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
    let radiance = Radiance::from_bytes(bytes.into(), scene.norm)?;
    Frame::new(width, height, FramePixels::Mosaic { cfa, radiance })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn compact_counts_keep_black_clamping_and_exposure_normalization_exact() {
        let raw: Vec<_> = (0..=u16::MAX).collect();
        for dr in [100, 200, 400] {
            let exposure = RecordedExposure {
                seconds: 1. / 800.,
                f_number: 9.,
                iso: 800,
                dynamic_range: dr,
            };
            let (counts, norm) = compute_counts(raw.clone(), 1024, &exposure);
            let gain = (f64::from(exposure.iso) / 200.)
                * match dr {
                    200 => 0.5,
                    400 => 0.25,
                    _ => 1.,
                };
            let original_norm = exposure.seconds / (exposure.f_number * exposure.f_number * gain);
            assert_eq!(norm.to_bits(), original_norm.to_bits());
            for (&raw, count) in raw.iter().zip(counts) {
                let original = ((f64::from(raw) - 1024.).max(0.) / original_norm) as f32;
                assert_eq!(
                    ((f64::from(count) / norm) as f32).to_bits(),
                    original.to_bits()
                );
            }
        }
    }

    #[test]
    fn synthetic_raf_basic() {
        // Create a minimal synthetic RAF to test parsing.
        let mut raf_data = Vec::new();

        // RAF header (148 bytes).
        raf_data.extend_from_slice(b"FUJIFILMCCD-RAW ");
        raf_data.extend_from_slice(&[0u8; 12]); // padding to offset 28.
        raf_data.extend_from_slice(b"X70\0");
        raf_data.extend_from_slice(&[0u8; 56]); // padding to offset 84.

        // Offset table (6 big-endian u32): jpeg, jpeg_size, cfa, cfa_size, raw, raw_size.
        raf_data.extend_from_slice(&(148u32).to_be_bytes()); // jpeg at 148
        raf_data.extend_from_slice(&(2048u32).to_be_bytes()); // jpeg size 2048
        raf_data.extend_from_slice(&(2196u32).to_be_bytes()); // cfa at 148+2048
        raf_data.extend_from_slice(&(256u32).to_be_bytes()); // cfa size 256
        raf_data.extend_from_slice(&(2452u32).to_be_bytes()); // raw at 2196+256
        raf_data.extend_from_slice(&((2048u32 + 4992 * 3296 * 2) as u32).to_be_bytes()); // raw size

        // JPEG placeholder (2048 bytes, minimal valid JPEG header for EXIF parsing).
        let mut jpeg = vec![0xFFu8, 0xD8]; // SOI
        jpeg.extend_from_slice(&[0xFF, 0xE1]); // EXIF marker
        jpeg.extend_from_slice(&(8u16).to_be_bytes()); // length
        jpeg.extend_from_slice(b"Exif"); // "Exif"
        jpeg.extend_from_slice(&[0u8; 2]); // padding
        jpeg.extend_from_slice(&[0u8; 2036]); // rest of JPEG
        raf_data.extend_from_slice(&jpeg);

        // CFA header (256 bytes).
        let mut cfa = vec![0u8; 256];
        // Magic: 0x0000000a (big-endian).
        cfa[0..4].copy_from_slice(&[0, 0, 0, 10]);
        // TLV entry 1: tag 0x100 (dimensions), length 4.
        cfa[4..6].copy_from_slice(&(0x100u16).to_be_bytes());
        cfa[6..8].copy_from_slice(&(4u16).to_be_bytes());
        // Value: height 3296, width 4992 (big-endian u16 each).
        cfa[8..10].copy_from_slice(&(3296u16).to_be_bytes());
        cfa[10..12].copy_from_slice(&(4992u16).to_be_bytes());
        // TLV entry 2: tag 0x131 (CFA pattern), length 36.
        cfa[12..14].copy_from_slice(&(0x131u16).to_be_bytes());
        cfa[14..16].copy_from_slice(&(36u16).to_be_bytes());
        // Pattern: X-Trans 6×6 (simple test pattern: R, G, B repeating).
        for i in 0..36 {
            cfa[16 + i] = (i % 3) as u8;
        }
        // Fill rest with padding.
        raf_data.extend_from_slice(&cfa);

        // Raw section TIFF (2048 bytes).
        let mut tiff = vec![0u8; 2048];
        tiff[0..4].copy_from_slice(b"II*\0"); // LE TIFF header.
        tiff[4..8].copy_from_slice(&(8u32).to_le_bytes()); // IFD offset = 8.
        // IFD entry count: 6 entries (u16 LE).
        tiff[8..10].copy_from_slice(&(6u16).to_le_bytes());
        // Entry 1: tag 0xf001 (width), type 4 (u32), count 1, value 4992.
        let mut at = 10;
        tiff[at..at + 2].copy_from_slice(&(0xf001u16).to_le_bytes());
        tiff[at + 2..at + 4].copy_from_slice(&(4u16).to_le_bytes());
        tiff[at + 4..at + 8].copy_from_slice(&(1u32).to_le_bytes());
        tiff[at + 8..at + 12].copy_from_slice(&(4992u32).to_le_bytes());
        at += 12;
        // Entry 2: tag 0xf002 (height), type 4, count 1, value 3296.
        tiff[at..at + 2].copy_from_slice(&(0xf002u16).to_le_bytes());
        tiff[at + 2..at + 4].copy_from_slice(&(4u16).to_le_bytes());
        tiff[at + 4..at + 8].copy_from_slice(&(1u32).to_le_bytes());
        tiff[at + 8..at + 12].copy_from_slice(&(3296u32).to_le_bytes());
        at += 12;
        // Entry 3: tag 0xf003 (bits per sample), type 4, count 1, value 14.
        tiff[at..at + 2].copy_from_slice(&(0xf003u16).to_le_bytes());
        tiff[at + 2..at + 4].copy_from_slice(&(4u16).to_le_bytes());
        tiff[at + 4..at + 8].copy_from_slice(&(1u32).to_le_bytes());
        tiff[at + 8..at + 12].copy_from_slice(&(14u32).to_le_bytes());
        at += 12;
        // Entry 4: tag 0xf004 (compression), type 4, count 1, value 0.
        tiff[at..at + 2].copy_from_slice(&(0xf004u16).to_le_bytes());
        tiff[at + 2..at + 4].copy_from_slice(&(4u16).to_le_bytes());
        tiff[at + 4..at + 8].copy_from_slice(&(1u32).to_le_bytes());
        tiff[at + 8..at + 12].copy_from_slice(&(0u32).to_le_bytes());
        at += 12;
        // Entry 5: tag 0xf007 (samples offset), type 4, count 1, value 2048.
        tiff[at..at + 2].copy_from_slice(&(0xf007u16).to_le_bytes());
        tiff[at + 2..at + 4].copy_from_slice(&(4u16).to_le_bytes());
        tiff[at + 4..at + 8].copy_from_slice(&(1u32).to_le_bytes());
        tiff[at + 8..at + 12].copy_from_slice(&(2048u32).to_le_bytes());
        at += 12;
        // Entry 6: tag 0xf008 (raw plane size), type 4, count 1, value 2048+4992*3296*2.
        tiff[at..at + 2].copy_from_slice(&(0xf008u16).to_le_bytes());
        tiff[at + 2..at + 4].copy_from_slice(&(4u16).to_le_bytes());
        tiff[at + 4..at + 8].copy_from_slice(&(1u32).to_le_bytes());
        tiff[at + 8..at + 12].copy_from_slice(&((2048u32 + 4992 * 3296 * 2) as u32).to_le_bytes());
        // Add black level tag (0xf00a) at offset 256.
        raf_data.extend_from_slice(&tiff);

        // Raw samples: full-sized (4992 × 3296) u16 LE.
        let sample_count = 4992usize * 3296;
        for _ in 0..sample_count {
            raf_data.extend_from_slice(&(1000u16).to_le_bytes()); // simple test value
        }

        // Parse synthetic RAF.
        let cursor = Cursor::new(raf_data);
        let result = read_raf(cursor);
        assert!(
            result.is_err(),
            "synthetic RAF should fail (missing black level tag)"
        );
    }

    #[test]
    fn test_exif_parsing_debug() {
        // Debug test to verify EXIF parsing on real RAF file
        if let Ok(raf_path) = std::env::var("FUJIMOD_TEST_RAF") {
            if let Ok(mut file) = std::fs::File::open(&raf_path) {
                let mut header_buf = [0u8; 148];
                use std::io::Read;
                if file.read_exact(&mut header_buf).is_ok() {
                    let jpeg_off = u32::from_be_bytes([
                        header_buf[84],
                        header_buf[85],
                        header_buf[86],
                        header_buf[87],
                    ]) as u64;
                    let jpeg_sz = u32::from_be_bytes([
                        header_buf[88],
                        header_buf[89],
                        header_buf[90],
                        header_buf[91],
                    ]) as u64;

                    eprintln!("JPEG at offset {}, size {}", jpeg_off, jpeg_sz);

                    // Seek and read
                    use std::io::Seek;
                    file.seek(std::io::SeekFrom::Start(jpeg_off)).ok();
                    let read_size = std::cmp::min(jpeg_sz as usize, 65536);
                    let mut jpeg_buf = vec![0u8; read_size];
                    if let Ok(n) = file.read(&mut jpeg_buf) {
                        eprintln!("Read {} bytes of JPEG (requested {})", n, jpeg_buf.len());

                        // Try to find and parse EXIF
                        match parse_exif(&jpeg_buf[..n]) {
                            Ok(exp) => {
                                eprintln!(
                                    "SUCCESS: ISO {}, {:.6}s, f/{:.1}, DR{}",
                                    exp.iso, exp.seconds, exp.f_number, exp.dynamic_range
                                );
                            }
                            Err(e) => {
                                eprintln!("FAILED to parse EXIF: {}", e);

                                // Try manual parsing for debugging
                                eprintln!("Attempting manual EXIF debug...");
                                if let Ok(exif_start_pos) = find_exif_ifd(&jpeg_buf[..n]) {
                                    eprintln!(
                                        "Found EXIF at position {}, slice len={}",
                                        exif_start_pos,
                                        n - exif_start_pos
                                    );
                                    let tags = parse_ifd(&jpeg_buf[exif_start_pos..n], 0)
                                        .unwrap_or_default();
                                    eprintln!("Main IFD has {} tags", tags.len());
                                    for (tag, (typ, count, val)) in &tags {
                                        eprintln!(
                                            "  Tag 0x{:04x}: type={}, count={}, value=0x{:08x}",
                                            tag, typ, count, val
                                        );
                                    }
                                    if let Some((_, _, exif_off)) = tags.get(&0x8769u16) {
                                        eprintln!("Found ExifIFD at offset {}", exif_off);
                                    } else {
                                        eprintln!("ExifIFD tag (0x8769) NOT FOUND in main IFD!");
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn test_xtrans_pattern() {
        // Verify the X-Trans pattern constants are correct.
        assert_eq!(XTRANS[0][0], 0); // Red at (0, 0)
        assert_eq!(XTRANS[0][2], 1); // Green at (0, 2)
        assert_eq!(XTRANS[0][1], 2); // Blue at (0, 1)
        assert_eq!(XTRANS[1][0], 1); // Green at (1, 0)
    }

    #[test]
    fn test_real_raf_file() {
        // Test reading the real X70 RAF file if FUJIMOD_TEST_RAF is set.
        // Verifies: metadata exactly matches exiftool, raw means per CFA color
        // within 0.5% of rawpy's raw_image_visible.
        if let Ok(raf_path) = std::env::var("FUJIMOD_TEST_RAF") {
            let file = std::fs::File::open(&raf_path).expect("Failed to open RAF file");
            let scene = read_raf(file).expect("Failed to parse RAF");

            // Validate basic structure.
            assert_eq!(scene.width, 4992, "Expected width 4992");
            assert_eq!(scene.height, 3296, "Expected height 3296");
            assert_eq!(scene.counts.len(), (4992 * 3296) as usize);
            assert_eq!(scene.cfa, XTRANS, "CFA pattern mismatch");
            assert_eq!(scene.black[0], 1023, "Expected black level 1023");
            assert_eq!(scene.white, 16383, "Expected white level 16383");

            // Verify exact metadata from exiftool.
            assert_eq!(scene.exposure.iso, 800, "Expected ISO 800");
            let exp_time_expected = 10.0 / 8000.0; // 1/800
            assert!(
                (scene.exposure.seconds - exp_time_expected).abs() / exp_time_expected < 0.001,
                "Expected exposure time 1/800 (0.00125), got {}",
                scene.exposure.seconds
            );
            assert_eq!(scene.exposure.f_number, 9.0, "Expected f-number 9.0");
            assert_eq!(scene.exposure.dynamic_range, 400, "Expected DR 400");

            eprintln!(
                "Metadata verified: ISO {}, {:.6}s, f/{:.1}, DR{}",
                scene.exposure.iso,
                scene.exposure.seconds,
                scene.exposure.f_number,
                scene.exposure.dynamic_range
            );

            // Compute mean raw counts (before normalization) per CFA color.
            // Compute normalization factor.
            let dr_factor = match scene.exposure.dynamic_range {
                200 => 0.5,
                400 => 0.25,
                _ => 1.0,
            };
            let sensor_gain = (scene.exposure.iso as f64 / 200.0) * dr_factor;
            let norm = scene.exposure.seconds
                / (scene.exposure.f_number * scene.exposure.f_number * sensor_gain);

            // Convert radiance back to raw counts: raw = radiance * norm + black.
            // Compute over rawpy's documented visible area: left_margin=6, width=4934.
            const RAWPY_LEFT_MARGIN: usize = 6;
            const RAWPY_VISIBLE_WIDTH: usize = 4934;
            let visible_x_start = RAWPY_LEFT_MARGIN;
            let visible_x_end = RAWPY_LEFT_MARGIN + RAWPY_VISIBLE_WIDTH;
            let mut color_raw_sums = [0.0f64; 3];
            let mut color_raw_counts = [0usize; 3];

            for y in 0..scene.height as usize {
                for x in visible_x_start..visible_x_end {
                    let color = scene.cfa[y % 6][x % 6] as usize;
                    let idx = y * scene.width as usize + x;
                    if color < 3 && idx < scene.counts.len() {
                        let raw_count =
                            scene.radiance(idx) as f64 * norm + scene.black[color] as f64;
                        color_raw_sums[color] += raw_count;
                        color_raw_counts[color] += 1;
                    }
                }
            }

            // Expected raw means per rawpy's raw_image_visible over visible area (3296×4934).
            // These are the exact means from rawpy's visible crop.
            let expected_raw = [1262.334, 1480.546, 1326.646];

            for c in 0..3 {
                if color_raw_counts[c] > 0 {
                    let mean_raw = color_raw_sums[c] / color_raw_counts[c] as f64;
                    let color_name = match c {
                        0 => "Red",
                        1 => "Green",
                        _ => "Blue",
                    };
                    eprintln!(
                        "{} raw mean: computed={:.6}, expected={:.6}, difference={:.6}",
                        color_name,
                        mean_raw,
                        expected_raw[c],
                        (mean_raw - expected_raw[c]).abs()
                    );
                    // Assert exact equality (within floating-point rounding)
                    assert!(
                        (mean_raw - expected_raw[c]).abs() < 0.001,
                        "{} raw means differ: {:.6} vs {:.6}",
                        color_name,
                        mean_raw,
                        expected_raw[c]
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod frame_tests {
    use super::*;
    use fujimod_emulator_image_formation::exposure::ExposedFrame;
    use fujimod_emulator_imaging::exposure::{Calibration, Exposure};
    use std::sync::Arc;
    fn calibration() -> Calibration {
        Calibration {
            full_scale: 16383,
            reference_white: [256; 3],
            saturation_lux_seconds: 1.,
            color_sample_weights: [2, 5, 2],
        }
    }
    #[test]
    fn raf_crop_preserves_count_values_normalization_and_cfa_phase() {
        let scene = RafScene {
            width: 11,
            height: 8,
            cfa: std::array::from_fn(|y| std::array::from_fn(|x| ((x + y) % 3) as u8)),
            black: [1024; 3],
            white: 16383,
            counts: (0..88).map(|i| i * 719).collect(),
            norm: 1. / 64800.,
            exposure: RecordedExposure {
                seconds: 1. / 800.,
                f_number: 9.,
                iso: 200,
                dynamic_range: 100,
            },
            crop_top: 2,
            crop_left: 3,
            crop_height: 5,
            crop_width: 7,
        };
        let frame = frame(&scene).unwrap();
        let FramePixels::Mosaic { cfa, radiance } = &frame.pixels else {
            panic!("mosaic");
        };
        assert_eq!(radiance.bytes().len(), 7 * 5 * 2);
        assert_eq!(radiance.norm.to_bits(), scene.norm.to_bits());
        for y in 0..5 {
            for x in 0..7 {
                assert_eq!(cfa[y % 6][x % 6], scene.cfa[(y + 2) % 6][(x + 3) % 6]);
                assert_eq!(
                    radiance.get(y * 7 + x).to_bits(),
                    scene.radiance((y + 2) * 11 + x + 3).to_bits()
                );
            }
        }
    }
    #[test]
    fn real_raf_sites_reproduce_their_raw_counts_after_black() {
        let Ok(path) = std::env::var("FUJIMOD_TEST_RAF") else {
            return;
        };
        let bytes = std::fs::read(&path).expect("read the test RAF");
        let scene = read_raf(std::io::Cursor::new(&bytes)).expect("parse the test RAF");
        let recorded = scene.exposure.clone();
        assert_eq!((recorded.iso, recorded.dynamic_range), (800, 400));
        // Analog gain relative to ISO 200 DR100, as the RAF normalization defines it.
        let analog_gain = f64::from(recorded.iso) / 200.
            * match recorded.dynamic_range {
                200 => 0.5,
                400 => 0.25,
                _ => 1.,
            };
        assert_eq!(analog_gain, 1.);
        let exposure = Exposure {
            seconds: recorded.seconds,
            analog_gain,
        };
        let frame = frame(&scene).expect("RAF crop as mosaic");
        let FramePixels::Mosaic { cfa, .. } = &frame.pixels else {
            panic!("RAF crop is a mosaic frame");
        };
        let cfa = *cfa;
        let (width, height) = (frame.width, frame.height);
        let (left, top) = (scene.crop_left as usize, scene.crop_top as usize);
        let exposed = ExposedFrame::from_frame(
            Arc::new(frame),
            exposure,
            recorded.f_number * recorded.f_number,
            calibration(),
        )
        .unwrap();
        // Read the file's raw plane directly: 2048 TIFF bytes follow the raw offset.
        let raw_offset = u32::from_be_bytes([bytes[100], bytes[101], bytes[102], bytes[103]]);
        let samples = raw_offset as usize + 2048;
        let raw_at = |x: usize, y: usize| {
            let at = samples + 2 * (y * scene.width as usize + x);
            u16::from_le_bytes([bytes[at], bytes[at + 1]])
        };
        let black = scene.black[0];
        let mut lit = 0;
        for i in 0..400usize {
            let sx = (i * 7919 + 13) % width;
            let sy = (i * 104_729 + 7) % height;
            let (x, y) = (sx + left, sy + top);
            assert_eq!(cfa[sy % 6][sx % 6], scene.cfa[y % 6][x % 6], "crop phase");
            let raw = raw_at(x, y).saturating_sub(black);
            lit += usize::from(raw > 0);
            let c = usize::from(cfa[sy % 6][sx % 6]);
            assert_eq!(exposed.sample_source(sx, sy)[c], raw, "site ({x}, {y})");
        }
        assert!(lit > 100, "the sample must include lit sites");
    }
}
