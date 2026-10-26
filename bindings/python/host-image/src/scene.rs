//! Python bindings for the declared virtual scene (`fujimod_virtual_scene`) and
//! its parameterized analytic statistics.

use fujimod_virtual_scene::{
    Crop, MAX_DIMENSION, MAX_OUTPUT_BYTES, Scene, SceneError, dimension, histogram, output,
    output_pairs, output_rgb, rgb, validate_crop, ycbcr,
};
use pyo3::exceptions::{PyMemoryError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyInt};

fn scene_error(error: SceneError) -> PyErr {
    match error {
        SceneError::Invalid(message) => PyValueError::new_err(message),
        SceneError::Allocation => PyMemoryError::new_err("virtual scene allocation failed"),
    }
}

fn integer(value: &Bound<'_, PyAny>, name: &'static str) -> PyResult<u64> {
    if !value.is_exact_instance_of::<PyInt>() {
        return Err(PyValueError::new_err(format!(
            "{name} must be an actual Python int, not bool or a coercible value"
        )));
    }
    value
        .extract::<u64>()
        .map_err(|_| PyValueError::new_err(format!("{name} must be in 0..2^64-1")))
}

/// Immutable host RGB24 raster mapped over a device's declared source grid.
/// A frame owns its pixels, so later webcam buffers cannot alter an exposure.
#[pyclass(frozen, module = "fujimod._native.emulator")]
pub(crate) struct HostImage {
    width: u64,
    height: u64,
    pixels: Vec<u8>,
}

#[pymethods]
impl HostImage {
    #[new]
    #[pyo3(signature = (width: "int", height: "int", rgb))]
    fn new(
        width: &Bound<'_, PyAny>,
        height: &Bound<'_, PyAny>,
        rgb: &Bound<'_, PyBytes>,
    ) -> PyResult<Self> {
        let width = integer(width, "width")?;
        let height = integer(height, "height")?;
        dimension(width).map_err(scene_error)?;
        dimension(height).map_err(scene_error)?;
        let length = width * height * 3;
        if length > MAX_OUTPUT_BYTES || rgb.as_bytes().len() as u64 != length {
            return Err(PyValueError::new_err(
                "RGB24 requires exactly width*height*3 bytes, at most 64MiB",
            ));
        }
        let mut pixels = Vec::new();
        pixels
            .try_reserve_exact(length as usize)
            .map_err(|_| SceneError::Allocation)
            .map_err(scene_error)?;
        pixels.extend_from_slice(rgb.as_bytes());
        Ok(Self {
            width,
            height,
            pixels,
        })
    }

    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature = (width: "int", height: "int", stride: "int | None", source_width: "int", source_height: "int", crop_x: "int", crop_y: "int", crop_width: "int", crop_height: "int"))]
    fn yycbcr<'py>(
        &self,
        py: Python<'py>,
        width: &Bound<'py, PyAny>,
        height: &Bound<'py, PyAny>,
        stride: &Bound<'py, PyAny>,
        source_width: &Bound<'py, PyAny>,
        source_height: &Bound<'py, PyAny>,
        crop_x: &Bound<'py, PyAny>,
        crop_y: &Bound<'py, PyAny>,
        crop_width: &Bound<'py, PyAny>,
        crop_height: &Bound<'py, PyAny>,
    ) -> PyResult<Bound<'py, PyBytes>> {
        let scene = Scene {
            width: integer(source_width, "source_width")?,
            height: integer(source_height, "source_height")?,
            frame: 0,
        };
        let crop = Crop {
            x: integer(crop_x, "crop_x")?,
            y: integer(crop_y, "crop_y")?,
            width: integer(crop_width, "crop_width")?,
            height: integer(crop_height, "crop_height")?,
        };
        let width = integer(width, "width")?;
        let height = integer(height, "height")?;
        let stride = if stride.is_none() {
            None
        } else {
            Some(integer(stride, "stride")?)
        };
        let bytes = py
            .detach(|| {
                output_rgb(scene, crop, width, height, stride, |x, y| {
                    let sx = ((2 * x + 1) * self.width / (2 * scene.width)).min(self.width - 1);
                    let sy = ((2 * y + 1) * self.height / (2 * scene.height)).min(self.height - 1);
                    let offset = ((sy * self.width + sx) * 3) as usize;
                    [
                        self.pixels[offset],
                        self.pixels[offset + 1],
                        self.pixels[offset + 2],
                    ]
                })
            })
            .map_err(scene_error)?;
        Ok(PyBytes::new(py, &bytes))
    }

    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature = (film, width: "int", height: "int", stride: "int | None", source_width: "int", source_height: "int", crop_x: "int", crop_y: "int", crop_width: "int", crop_height: "int"))]
    fn film_yycbcr<'py>(
        &self,
        py: Python<'py>,
        film: &crate::film::FilmStage,
        width: &Bound<'py, PyAny>,
        height: &Bound<'py, PyAny>,
        stride: &Bound<'py, PyAny>,
        source_width: &Bound<'py, PyAny>,
        source_height: &Bound<'py, PyAny>,
        crop_x: &Bound<'py, PyAny>,
        crop_y: &Bound<'py, PyAny>,
        crop_width: &Bound<'py, PyAny>,
        crop_height: &Bound<'py, PyAny>,
    ) -> PyResult<Bound<'py, PyBytes>> {
        let scene = Scene {
            width: integer(source_width, "source_width")?,
            height: integer(source_height, "source_height")?,
            frame: 0,
        };
        let crop = Crop {
            x: integer(crop_x, "crop_x")?,
            y: integer(crop_y, "crop_y")?,
            width: integer(crop_width, "crop_width")?,
            height: integer(crop_height, "crop_height")?,
        };
        let width = integer(width, "width")?;
        let height = integer(height, "height")?;
        let stride = if stride.is_none() {
            None
        } else {
            Some(integer(stride, "stride")?)
        };
        validate_crop(scene, crop).map_err(scene_error)?;
        dimension(width).map_err(scene_error)?;
        dimension(height).map_err(scene_error)?;
        let sample = |x: u64, y: u64| {
            let sx = ((2 * x + 1) * self.width / (2 * scene.width)).min(self.width - 1);
            let sy = ((2 * y + 1) * self.height / (2 * scene.height)).min(self.height - 1);
            let offset = ((sy * self.width + sx) * 3) as usize;
            [
                self.pixels[offset],
                self.pixels[offset + 1],
                self.pixels[offset + 2],
            ]
        };
        let bytes = py
            .detach(|| {
                // Pointwise color commutes with nearest source-grid sampling. A
                // phone webcam raster is smaller than its two display outputs:
                // develop each input pixel once instead of repeating soft math
                // for every enlarged output pixel. Large photos sample directly.
                let developed: Option<Vec<[u8; 3]>> = (self.width * self.height < width * height)
                    .then(|| {
                        self.pixels
                            .chunks_exact(3)
                            .map(|p| film.color([p[0], p[1], p[2]]))
                            .collect()
                    });
                let color = |x: u64, y: u64| {
                    if let Some(ref raster) = developed {
                        let sx = ((2 * x + 1) * self.width / (2 * scene.width)).min(self.width - 1);
                        let sy =
                            ((2 * y + 1) * self.height / (2 * scene.height)).min(self.height - 1);
                        raster[(sy * self.width + sx) as usize]
                    } else {
                        film.color(sample(x, y))
                    }
                };
                output_pairs(scene, crop, width, height, stride, |x0, x1, y| {
                    crate::film::FilmStage::pack(color(x0, y), color(x1, y))
                })
            })
            .map_err(scene_error)?;
        Ok(PyBytes::new(py, &bytes))
    }
}

/// Return packed bytes only. Stride may be None for a tightly packed raster;
/// every other argument is an explicit, nonnegative, actual Python integer.
#[pyfunction]
#[allow(clippy::too_many_arguments)]
#[pyo3(signature = (width: "int", height: "int", stride: "int | None", frame: "int", source_width: "int", source_height: "int", crop_x: "int", crop_y: "int", crop_width: "int", crop_height: "int"))]
pub fn virtual_scene_yycbcr<'py>(
    py: Python<'py>,
    width: &Bound<'py, PyAny>,
    height: &Bound<'py, PyAny>,
    stride: &Bound<'py, PyAny>,
    frame: &Bound<'py, PyAny>,
    source_width: &Bound<'py, PyAny>,
    source_height: &Bound<'py, PyAny>,
    crop_x: &Bound<'py, PyAny>,
    crop_y: &Bound<'py, PyAny>,
    crop_width: &Bound<'py, PyAny>,
    crop_height: &Bound<'py, PyAny>,
) -> PyResult<Bound<'py, PyBytes>> {
    let scene = Scene {
        width: integer(source_width, "source_width")?,
        height: integer(source_height, "source_height")?,
        frame: integer(frame, "frame")?,
    };
    let crop = Crop {
        x: integer(crop_x, "crop_x")?,
        y: integer(crop_y, "crop_y")?,
        width: integer(crop_width, "crop_width")?,
        height: integer(crop_height, "crop_height")?,
    };
    let stride = if stride.is_none() {
        None
    } else {
        Some(integer(stride, "stride")?)
    };
    let width = integer(width, "width")?;
    let height = integer(height, "height")?;
    let bytes = py
        .detach(move || output(scene, crop, width, height, stride))
        .map_err(scene_error)?;
    Ok(PyBytes::new(py, &bytes))
}

/// Return256 little-endian U32 bins under the declared half-horizontal law.
#[pyfunction]
#[pyo3(signature = (source_width: "int", source_height: "int", crop_x: "int", crop_y: "int", crop_width: "int", crop_height: "int", frame: "int"))]
pub fn virtual_scene_histogram<'py>(
    py: Python<'py>,
    source_width: &Bound<'py, PyAny>,
    source_height: &Bound<'py, PyAny>,
    crop_x: &Bound<'py, PyAny>,
    crop_y: &Bound<'py, PyAny>,
    crop_width: &Bound<'py, PyAny>,
    crop_height: &Bound<'py, PyAny>,
    frame: &Bound<'py, PyAny>,
) -> PyResult<Bound<'py, PyBytes>> {
    let scene = Scene {
        width: integer(source_width, "source_width")?,
        height: integer(source_height, "source_height")?,
        frame: integer(frame, "frame")?,
    };
    let crop = Crop {
        x: integer(crop_x, "crop_x")?,
        y: integer(crop_y, "crop_y")?,
        width: integer(crop_width, "crop_width")?,
        height: integer(crop_height, "crop_height")?,
    };
    let bytes = py
        .detach(move || histogram(scene, crop))
        .map_err(scene_error)?;
    Ok(PyBytes::new(py, &bytes))
}

pub(crate) const STATISTICS_PROFILE: &str = "declared-analytic-statistics-quadrature-v1";

struct SceneFrameNumber(u64);

#[derive(Clone, Copy, serde::Deserialize)]
#[serde(transparent)]
struct CellCount(usize);

#[derive(Clone, Copy, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SamplingGrid {
    cells: Crop,
    columns: CellCount,
    rows: CellCount,
    gap_x: u64,
    gap_y: u64,
}

impl SamplingGrid {
    fn cell(self, column: usize, row: usize) -> Crop {
        Crop {
            x: self.cells.x + column as u64 * (self.cells.width + self.gap_x),
            y: self.cells.y + row as u64 * (self.cells.height + self.gap_y),
            ..self.cells
        }
    }

    fn extent(self) -> Crop {
        Crop {
            width: self.columns.0 as u64 * self.cells.width
                + (self.columns.0 - 1) as u64 * self.gap_x,
            height: self.rows.0 as u64 * self.cells.height + (self.rows.0 - 1) as u64 * self.gap_y,
            ..self.cells
        }
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SamplingProgram {
    color: SamplingGrid,
    color_regions: [Crop; 10],
    contrast: SamplingGrid,
    contrast_regions: [Crop; 10],
    phase: SamplingGrid,
    totals: Crop,
    columns: SamplingGrid,
    rows: SamplingGrid,
    histogram: Option<Crop>,
}

impl SamplingProgram {
    fn scene(&self, frame: SceneFrameNumber) -> Result<Scene, SceneError> {
        if self.columns.rows.0 != 1 || self.rows.columns.0 != 1 {
            return Err(SceneError::Invalid(
                "axis sampling grids require one cell on the other axis",
            ));
        }
        for grid in [
            self.color,
            self.contrast,
            self.phase,
            self.columns,
            self.rows,
        ] {
            if !(1..=16).contains(&grid.columns.0)
                || !(1..=16).contains(&grid.rows.0)
                || (grid.columns.0 > 1 && grid.gap_x > MAX_DIMENSION)
                || (grid.rows.0 > 1 && grid.gap_y > MAX_DIMENSION)
            {
                return Err(SceneError::Invalid(
                    "statistics grids require 1..16 cells per axis and bounded gaps",
                ));
            }
            validate_region(grid.cells)?;
        }
        let extents = [
            self.color.extent(),
            self.contrast.extent(),
            self.phase.extent(),
            self.totals,
            self.columns.extent(),
            self.rows.extent(),
        ];
        let mut width = 1;
        let mut height = 1;
        for region in extents
            .iter()
            .chain(&self.color_regions)
            .chain(&self.contrast_regions)
            .chain(self.histogram.iter())
        {
            validate_region(*region)?;
            width = width.max(region.x + region.width);
            height = height.max(region.y + region.height);
        }
        dimension(width)?;
        dimension(height)?;
        Ok(Scene {
            width,
            height,
            frame: frame.0,
        })
    }
}

fn validate_region(region: Crop) -> Result<(), SceneError> {
    dimension(region.width)?;
    dimension(region.height)?;
    if region.x > MAX_DIMENSION || region.y > MAX_DIMENSION {
        return Err(SceneError::Invalid(
            "statistics region origin exceeds the scene bound",
        ));
    }
    Ok(())
}

struct RegionMeasurements {
    colors: [u64; 3],
    luma: u64,
    contrast: [u64; 2],
    mean: u16,
    population: u64,
}

// This numerical law is unmeasured; phase splitting and row/column field meanings remain unrecovered.
fn measure(scene: Scene, region: Crop) -> RegionMeasurements {
    let columns = region.width.min(8);
    let rows = region.height.min(8);
    let samples = columns * rows;
    let population = region.width * region.height;
    let mut colors = [0u64; 3];
    let mut luma = 0u64;
    let mut contrast = [0u64; 2];
    let mut row_luma = [0i64; 8];
    for row in 0..rows {
        let mut previous = 0i64;
        for column in 0..columns {
            let x = region.x + ((2 * column + 1) * region.width / (2 * columns));
            let y = region.y + ((2 * row + 1) * region.height / (2 * rows));
            let color = rgb(scene, x, y);
            let value = i64::from(ycbcr(color)[0]);
            for (sum, component) in colors.iter_mut().zip(color) {
                *sum += u64::from(component) * 64;
            }
            luma += value as u64 * 64;
            if column != 0 {
                contrast[0] += (value - previous).unsigned_abs().pow(2);
            }
            if row != 0 {
                contrast[1] += (value - row_luma[column as usize]).unsigned_abs().pow(2);
            }
            previous = value;
            row_luma[column as usize] = value;
        }
    }
    RegionMeasurements {
        colors: std::array::from_fn(|channel| {
            let numerator = [2, 5, 2][channel];
            (population * numerator / 9) * colors[channel] / samples
        }),
        luma: population * luma / samples,
        mean: (luma / samples) as u16,
        contrast: contrast.map(|energy| energy * population / samples),
        population,
    }
}

fn u32_word(value: u64) -> u32 {
    value.min(u64::from(u32::MAX)) as u32
}

fn words32<'py>(py: Python<'py>, words: &[u32]) -> PyResult<Bound<'py, PyBytes>> {
    PyBytes::new_with(py, words.len() * 4, |output| {
        for (bytes, word) in output.chunks_exact_mut(4).zip(words) {
            bytes.copy_from_slice(&word.to_le_bytes());
        }
        Ok(())
    })
}

fn words64<'py>(py: Python<'py>, words: &[u64]) -> PyResult<Bound<'py, PyBytes>> {
    PyBytes::new_with(py, words.len() * 8, |output| {
        for (bytes, word) in output.chunks_exact_mut(8).zip(words) {
            bytes.copy_from_slice(&word.to_le_bytes());
        }
        Ok(())
    })
}

type StatisticsBytes<'py> = (
    Bound<'py, PyBytes>,
    Bound<'py, PyBytes>,
    Bound<'py, PyBytes>,
    Bound<'py, PyBytes>,
    Bound<'py, PyBytes>,
    Bound<'py, PyBytes>,
    Bound<'py, PyBytes>,
    Bound<'py, PyBytes>,
    Bound<'py, PyBytes>,
    Option<Bound<'py, PyBytes>>,
);

#[pyfunction]
#[pyo3(signature = (frame: "int", configuration))]
pub(crate) fn virtual_scene_statistics<'py>(
    py: Python<'py>,
    frame: &Bound<'py, PyAny>,
    configuration: &str,
) -> PyResult<StatisticsBytes<'py>> {
    let program: SamplingProgram = serde_json::from_str(configuration)
        .map_err(|error| PyValueError::new_err(error.to_string()))?;
    let frame = integer(frame, "frame")?;
    if frame == 0 {
        return Err(PyValueError::new_err("statistics frame must be positive"));
    }
    let scene = program
        .scene(SceneFrameNumber(frame))
        .map_err(scene_error)?;
    let mut contrast = [[0u32; 256]; 2];
    let mut colors = [[0u32; 256]; 4];
    let mut phase = [0u32; 256];
    let mut scalar = [0u32; 256];
    for row in 0..program.color.rows.0 {
        for column in 0..program.color.columns.0 {
            let measurement = measure(scene, program.color.cell(column, row));
            for channel in 0..4 {
                colors[channel][row * 16 + column] =
                    u32_word(measurement.colors[[0, 1, 2, 1][channel]]);
            }
        }
    }
    for row in 0..program.contrast.rows.0 {
        for column in 0..program.contrast.columns.0 {
            let measurement = measure(scene, program.contrast.cell(column, row));
            for channel in 0..2 {
                contrast[channel][row * 16 + column] = u32_word(measurement.contrast[channel]);
            }
        }
    }
    for row in 0..program.phase.rows.0 {
        for column in 0..program.phase.columns.0 {
            let measurement = measure(scene, program.phase.cell(column, row));
            let mean = u32::from(measurement.mean);
            phase[row * 16 + column] = (mean << 16) | mean;
            scalar[row * 16 + column] = mean;
        }
    }
    let bands = program.contrast_regions.map(|region| {
        let measurement = measure(scene, region);
        u64::from(u32_word(measurement.contrast[0] + measurement.contrast[1]))
    });
    let color_regions = program.color_regions.map(|region| {
        let measurement = measure(scene, region);
        [
            measurement.colors[0],
            measurement.colors[1],
            measurement.colors[2],
            measurement.colors[1],
        ]
    });
    let total = measure(scene, program.totals);
    let totals = [
        u32_word(total.luma),
        u32_word(total.luma),
        u32_word(total.luma),
        u32_word(total.luma),
        u32_word(total.population),
        u32_word(total.population),
        u32_word(total.population),
        u32_word(total.population),
    ];
    let mut columns = [[0u32; 16]; 8];
    let mut rows = [[0u32; 16]; 8];
    for column in 0..program.columns.columns.0 {
        let measurement = measure(scene, program.columns.cell(column, 0));
        for channel in 0..8 {
            columns[channel][column] = u32_word(if channel < 4 {
                measurement.luma
            } else {
                measurement.population
            });
        }
    }
    for row in 0..program.rows.rows.0 {
        let measurement = measure(scene, program.rows.cell(0, row));
        for channel in 0..8 {
            rows[channel][row] = u32_word(if channel < 4 {
                measurement.luma
            } else {
                measurement.population
            });
        }
    }
    let histogram = if let Some(region) = program.histogram {
        let mut bins = [0u32; 256];
        let columns = region.width.min(8);
        let rows = region.height.min(8);
        let samples = columns * rows;
        let population = region.width * region.height;
        for row in 0..rows {
            for column in 0..columns {
                let x = region.x + ((2 * column + 1) * region.width / (2 * columns));
                let y = region.y + ((2 * row + 1) * region.height / (2 * rows));
                let index = row * columns + column;
                let weight = population / samples + u64::from(index < population % samples);
                bins[ycbcr(rgb(scene, x, y))[0] as usize] += weight as u32;
            }
        }
        Some(words32(py, &bins)?)
    } else {
        None
    };
    Ok((
        words32(py, contrast.as_flattened())?,
        words64(py, &bands)?,
        words32(py, colors.as_flattened())?,
        words64(py, color_regions.as_flattened())?,
        words32(py, &phase)?,
        words32(py, &scalar)?,
        words32(py, &totals)?,
        words32(py, columns.as_flattened())?,
        words32(py, rows.as_flattened())?,
        histogram,
    ))
}
