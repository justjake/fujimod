//! SMAPH's adapter to the shared GPU coverage rasterizer.

use super::scanline;
use fujimod_emulator_gpu::{
    CoverageEdge, CoverageKernel, CoverageRequest, EdgeCoordinates, FillRule, SampleGrid,
};
use std::sync::OnceLock;

#[cfg(test)]
pub type Point = [f64; 2];

struct Gpu {
    #[cfg(test)]
    name: String,
    kernel: CoverageKernel,
}

impl Gpu {
    fn new(context: &fujimod_emulator_gpu::GpuContext) -> Self {
        Self {
            #[cfg(test)]
            name: context.adapter_name().to_owned(),
            kernel: CoverageKernel::new(context),
        }
    }

    fn coverage_batch(
        &self,
        context: &fujimod_emulator_gpu::GpuContext,
        inputs: &[scanline::CoverageInput],
        output_lengths: &[usize],
    ) -> Result<Vec<Vec<u32>>, String> {
        let mut active_indices = Vec::new();
        let mut edge_sets = Vec::new();
        for (index, input) in inputs.iter().enumerate() {
            if input.samples == 0 || output_lengths[index] == 0 {
                continue;
            }
            active_indices.push(index);
            edge_sets.push(
                input
                    .edges
                    .iter()
                    .map(|edge| {
                        CoverageEdge::new(
                            [edge[0] as f32 / 8.0, edge[1] as f32 / 8.0],
                            [edge[2] as f32 / 8.0, edge[3] as f32 / 8.0],
                        )
                    })
                    .collect::<Vec<_>>(),
            );
        }
        if active_indices.is_empty() {
            return Ok(output_lengths
                .iter()
                .map(|length| vec![0; *length])
                .collect());
        }

        let requests: Vec<_> = active_indices
            .iter()
            .zip(&edge_sets)
            .map(|(&index, edges)| {
                let input = &inputs[index];
                let sample_grid = match input.samples {
                    1 => SampleGrid::OneByOne,
                    2 => SampleGrid::TwoByTwo,
                    4 => SampleGrid::FourByFour,
                    _ => unreachable!("SMAPH coverage inputs are validated before dispatch"),
                };
                let [x0, x1, y0, y1] = input.bounds;
                CoverageRequest {
                    bounds: [x0 as u32, x1 as u32, y0 as u32, y1 as u32],
                    edges,
                    edge_coordinates: EdgeCoordinates::EighthPixel,
                    fill_rule: if input.nonzero {
                        FillRule::NonZero
                    } else {
                        FillRule::EvenOdd
                    },
                    sample_grid,
                    stroke_radius: input.stroke_radius.map(|radius| radius as f32 / 8.0),
                }
            })
            .collect();
        let rasterized = self
            .kernel
            .coverage_batch(context, &requests)
            .map_err(|error| format!("SMAPH coverage {error}"))?;
        let mut outputs: Vec<_> = output_lengths
            .iter()
            .map(|length| vec![0; *length])
            .collect();
        for (index, coverage) in active_indices.into_iter().zip(rasterized) {
            outputs[index] = coverage;
        }
        Ok(outputs)
    }
}

static GPU: OnceLock<Result<Option<Gpu>, String>> = OnceLock::new();

fn shared_gpu() -> Result<Option<&'static Gpu>, String> {
    let context = match fujimod_emulator_gpu::context() {
        Ok(context) => context,
        Err(error) if error.starts_with("host GPU adapter:") => return Ok(None),
        Err(error) => return Err(error),
    };
    match GPU.get_or_init(|| Ok(Some(Gpu::new(context)))) {
        Ok(Some(gpu)) => Ok(Some(gpu)),
        Ok(None) => Ok(None),
        Err(error) => Err(error.clone()),
    }
}

fn validate_input(input: &scanline::CoverageInput) -> Result<usize, String> {
    let [x0, x1, y0, y1] = input.bounds;
    if x0 > x1 || y0 > y1 || x1 > 2048 || y1 > 2048 {
        return Err("SMAPH coverage batch has invalid bounds".into());
    }
    if !matches!(input.samples, 0 | 1 | 2 | 4) {
        return Err("SMAPH coverage sample count must be 1, 2, or 4".into());
    }
    input.output_len()
}

/// Compute coverage for a complete pixel bounding box in one GPU dispatch.
///
/// Edges are quantized by SMAPH to eighth-pixel coordinates before they are
/// converted to the shared rasterizer's floating-point pixel coordinates.
/// Results are row-major; stroke edges follow SMAPH's open contour windows.
#[cfg(test)]
pub fn coverage(
    contours: &[Vec<Point>],
    x0: usize,
    x1: usize,
    y0: usize,
    y1: usize,
    samples: u32,
    stroke: Option<f64>,
    nonzero: bool,
) -> Result<Vec<u32>, String> {
    let input = scanline::CoverageInput::new(contours, [x0, x1, y0, y1], samples, stroke, nonzero);
    coverage_batch(std::slice::from_ref(&input))
        .map(|mut outputs| outputs.pop().unwrap_or_default())
}

/// Rasterize every path in a stream with one dispatch and one edge buffer.
pub(super) fn coverage_batch(inputs: &[scanline::CoverageInput]) -> Result<Vec<Vec<u32>>, String> {
    let output_lengths: Vec<_> = inputs
        .iter()
        .map(validate_input)
        .collect::<Result<_, _>>()?;
    if inputs
        .iter()
        .zip(&output_lengths)
        .all(|(input, output_len)| input.samples == 0 || *output_len == 0)
    {
        return Ok(output_lengths
            .iter()
            .map(|length| vec![0; *length])
            .collect());
    }
    let gpu = shared_gpu()?.ok_or_else(|| "SMAPH coverage has no wgpu adapter".to_owned())?;
    gpu.coverage_batch(fujimod_emulator_gpu::context()?, inputs, &output_lengths)
}

#[cfg(test)]
mod tests {
    use super::{coverage, scanline, shared_gpu};

    type Point = [f64; 2];

    fn require_gpu() -> bool {
        match shared_gpu() {
            Ok(Some(gpu)) => {
                println!("SMAPH GPU coverage adapter: {}", gpu.name);
                true
            }
            Ok(None) => {
                println!("SKIP SMAPH GPU coverage: no wgpu adapter is available");
                false
            }
            Err(error) => panic!("SMAPH GPU adapter initialization failed: {error}"),
        }
    }

    fn compare(contours: &[Vec<Point>]) {
        let x0 = 0;
        let x1 = 16;
        let y0 = 0;
        let y1 = 16;
        for samples in [1, 2, 4] {
            for stroke in [None, Some(0.0), Some(0.5), Some(2.0)] {
                for nonzero in [false, true] {
                    let actual = coverage(contours, x0, x1, y0, y1, samples, stroke, nonzero)
                        .unwrap_or_else(|error| panic!("GPU coverage failed: {error}"));
                    let expected: Vec<u32> = (y0..y1)
                        .flat_map(|y| {
                            scanline::coverage(contours, x0, x1, y, samples, stroke, nonzero)
                        })
                        .collect();
                    assert_eq!(
                        actual, expected,
                        "samples={samples} stroke={stroke:?} nonzero={nonzero} contours={contours:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn authored_smaph_rectangle_matches_cpu_coverage() {
        if !require_gpu() {
            return;
        }
        compare(&[vec![[0.0, 0.0], [16.0, 0.0], [16.0, 16.0], [0.0, 16.0]]]);
    }

    #[test]
    fn scanline_contours_match_cpu_coverage() {
        if !require_gpu() {
            return;
        }
        compare(&[
            vec![[1.5, 1.5], [11.5, 1.5], [11.5, 9.5], [1.5, 9.5]],
            vec![[3.5, 3.5], [8.5, 3.5], [8.5, 7.5], [3.5, 7.5]],
        ]);
        compare(&[
            vec![[2.0, 2.0], [10.0, 10.0], [2.0, 10.0], [10.0, 2.0]],
            vec![[5.0, 5.0], [5.0, 5.0]],
            vec![[0.0, 4.0], [13.0, 4.0]],
        ]);
        compare(&[
            vec![[6.125, 0.125], [6.125, 11.125]],
            vec![[0.125, 6.125], [13.125, 6.125]],
            vec![[6.0, 6.0]],
        ]);
    }

    #[test]
    fn randomized_contours_match_cpu_coverage() {
        if !require_gpu() {
            return;
        }
        let mut seed = 123456789u32;
        for _ in 0..24 {
            let mut contours = Vec::new();
            for _ in 0..3 {
                let mut points = Vec::new();
                for _ in 0..8 {
                    let mut point = [0.0; 2];
                    for coordinate in &mut point {
                        seed ^= seed << 13;
                        seed ^= seed >> 17;
                        seed ^= seed << 5;
                        *coordinate = (seed % 128) as f64 / 8.0 - 2.0;
                    }
                    points.push(point);
                }
                contours.push(points);
            }
            compare(&contours);
        }
    }
}
