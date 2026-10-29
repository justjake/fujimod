//! Shared coverage rasterization over floating-point edge coordinates.

use super::GpuContext;
use std::mem::size_of;
use wgpu::util::DeviceExt;

const MAX_EDGE_GRID_COORDINATE: f64 = 16_383.0;
const MAX_FLOAT_EDGE_COORDINATE: f32 = 16_777_216.0;
// This fixed-point rasterizer keeps signed edge cross products within i32.
const MAX_BOUNDS: u32 = 2048;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FillRule {
    EvenOdd,
    NonZero,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EdgeCoordinates {
    /// Coordinates already represent a raster on the eighth-pixel grid.
    EighthPixel,
    /// Preserve fractional pixel coordinates for fill classification.
    FloatPixel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleGrid {
    OneByOne,
    TwoByTwo,
    FourByFour,
}

impl SampleGrid {
    pub const fn side(self) -> u32 {
        match self {
            Self::OneByOne => 1,
            Self::TwoByTwo => 2,
            Self::FourByFour => 4,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CoverageEdge {
    pub start: [f32; 2],
    pub end: [f32; 2],
    start_exact: [f64; 2],
    end_exact: [f64; 2],
}

impl CoverageEdge {
    pub const fn new(start: [f32; 2], end: [f32; 2]) -> Self {
        Self {
            start,
            end,
            start_exact: [start[0] as f64, start[1] as f64],
            end_exact: [end[0] as f64, end[1] as f64],
        }
    }

    /// Preserve f64 edge inputs for rebasing before GPU fill math.
    pub fn from_f64(start: [f64; 2], end: [f64; 2]) -> Self {
        Self {
            start: start.map(|value| value as f32),
            end: end.map(|value| value as f32),
            start_exact: start,
            end_exact: end,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct CoverageRequest<'a> {
    /// Pixel bounds as `[left, right, top, bottom)`, with right and bottom exclusive.
    pub bounds: [u32; 4],
    /// Line segments in pixel coordinates.
    pub edges: &'a [CoverageEdge],
    pub edge_coordinates: EdgeCoordinates,
    pub fill_rule: FillRule,
    pub sample_grid: SampleGrid,
    /// Optional stroke radius in pixels, also classified on the eighth-pixel grid.
    pub stroke_radius: Option<f32>,
}

pub struct CoverageKernel {
    pipeline: wgpu::ComputePipeline,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CoverageRaster {
    pub offset: u32,
    pub length: usize,
}

/// Coverage counts written by a GPU dispatch for direct use by later GPU passes.
pub struct CoverageBatch {
    output: Option<wgpu::Buffer>,
    rasters: Vec<CoverageRaster>,
    total_count: usize,
}

impl CoverageBatch {
    pub fn buffer(&self) -> Option<&wgpu::Buffer> {
        self.output.as_ref()
    }

    pub fn rasters(&self) -> &[CoverageRaster] {
        &self.rasters
    }

    pub fn total_count(&self) -> usize {
        self.total_count
    }
}

impl CoverageKernel {
    pub fn new(context: &GpuContext) -> Self {
        let device = context.device();
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("GPU coverage"),
            source: wgpu::ShaderSource::Wgsl(include_str!("coverage.wgsl").into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("GPU coverage"),
            layout: None,
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        Self { pipeline }
    }

    /// Encode row-major coverage counts for later GPU passes in the same command list.
    pub fn encode_batch(
        &self,
        context: &GpuContext,
        encoder: &mut wgpu::CommandEncoder,
        requests: &[CoverageRequest<'_>],
    ) -> Result<CoverageBatch, String> {
        if requests.is_empty() {
            return Ok(CoverageBatch {
                output: None,
                rasters: Vec::new(),
                total_count: 0,
            });
        }

        let mut jobs = Vec::<u32>::with_capacity(requests.len() * 12);
        let mut edges = Vec::<[f32; 8]>::new();
        let mut rasters = Vec::with_capacity(requests.len());
        let mut output_count = 0usize;
        let mut max_width = 0usize;
        let mut max_height = 0usize;

        for request in requests {
            let [x0, x1, y0, y1] = request.bounds;
            if x0 > x1 || y0 > y1 || x1 > MAX_BOUNDS || y1 > MAX_BOUNDS {
                return Err("coverage input has invalid bounds".into());
            }
            for edge in request.edges {
                match request.edge_coordinates {
                    EdgeCoordinates::EighthPixel => {
                        for coordinate in edge.start.into_iter().chain(edge.end) {
                            validate_grid_coordinate(coordinate)?;
                        }
                    }
                    EdgeCoordinates::FloatPixel => {
                        let origins = [x0, y0, x0, y0];
                        for (coordinate, origin) in edge
                            .start_exact
                            .into_iter()
                            .chain(edge.end_exact)
                            .zip(origins)
                        {
                            validate_float_coordinate(coordinate - f64::from(origin))?;
                        }
                    }
                }
            }
            if request.edge_coordinates == EdgeCoordinates::FloatPixel
                && request.stroke_radius.is_some()
            {
                return Err("float-pixel coverage does not support stroke radii".into());
            }
            let stroke_radius = request
                .stroke_radius
                .map(|radius| quantized_grid_coordinate(radius))
                .transpose()?;

            let width = (x1 - x0) as usize;
            let height = (y1 - y0) as usize;
            let output_len = width
                .checked_mul(height)
                .ok_or("coverage output extent overflow")?;
            let edge_offset: u32 = edges
                .len()
                .try_into()
                .map_err(|_| "coverage edge offset overflow")?;
            let edge_count: u32 = request
                .edges
                .len()
                .try_into()
                .map_err(|_| "coverage edge count overflow")?;
            let coverage_offset: u32 = output_count
                .try_into()
                .map_err(|_| "coverage output offset overflow")?;
            let mut flags = u32::from(stroke_radius.is_some());
            flags |= u32::from(request.fill_rule == FillRule::NonZero) << 1;
            flags |= u32::from(request.edge_coordinates == EdgeCoordinates::FloatPixel) << 2;
            jobs.extend([
                x0,
                y0,
                width as u32,
                height as u32,
                request.sample_grid.side(),
                edge_offset,
                edge_count,
                flags,
                stroke_radius.unwrap_or(0) as u32,
                coverage_offset,
                0,
                0,
            ]);
            edges.extend(
                request
                    .edges
                    .iter()
                    .map(|edge| match request.edge_coordinates {
                        EdgeCoordinates::EighthPixel => [
                            edge.start[0],
                            edge.start[1],
                            edge.end[0],
                            edge.end[1],
                            0.0,
                            0.0,
                            0.0,
                            0.0,
                        ],
                        EdgeCoordinates::FloatPixel => {
                            let [x0, _, y0, _] = request.bounds;
                            let start_x =
                                split_float_coordinate(edge.start_exact[0] - f64::from(x0));
                            let start_y =
                                split_float_coordinate(edge.start_exact[1] - f64::from(y0));
                            let end_x = split_float_coordinate(edge.end_exact[0] - f64::from(x0));
                            let end_y = split_float_coordinate(edge.end_exact[1] - f64::from(y0));
                            [
                                start_x.0, start_y.0, end_x.0, end_y.0, start_x.1, start_y.1,
                                end_x.1, end_y.1,
                            ]
                        }
                    }),
            );
            output_count = output_count
                .checked_add(output_len)
                .ok_or("coverage batch output overflow")?;
            rasters.push(CoverageRaster {
                offset: coverage_offset,
                length: output_len,
            });
            max_width = max_width.max(width);
            max_height = max_height.max(height);
        }

        if output_count == 0 {
            return Ok(CoverageBatch {
                output: None,
                rasters,
                total_count: 0,
            });
        }
        if edges.is_empty() {
            edges.push([0.0; 8]);
        }

        let edge_bytes = edges
            .len()
            .checked_mul(size_of::<[f32; 8]>())
            .ok_or("coverage edge buffer size overflow")?;
        let job_bytes = jobs
            .len()
            .checked_mul(size_of::<u32>())
            .ok_or("coverage job buffer size overflow")?;
        let output_bytes = output_count
            .checked_mul(size_of::<u32>())
            .ok_or("coverage output buffer size overflow")?;
        let limits = context.device().limits();
        if edge_bytes > limits.max_storage_buffer_binding_size as usize
            || job_bytes > limits.max_storage_buffer_binding_size as usize
            || output_bytes > limits.max_storage_buffer_binding_size as usize
            || output_bytes as u64 > limits.max_buffer_size
            || requests.len() > limits.max_compute_workgroups_per_dimension as usize
            || max_width.div_ceil(8) > limits.max_compute_workgroups_per_dimension as usize
            || max_height.div_ceil(8) > limits.max_compute_workgroups_per_dimension as usize
        {
            return Err("coverage batch exceeds host GPU limits".into());
        }

        let edge_bytes_data: Vec<u8> = edges
            .iter()
            .flat_map(|edge| edge.iter().flat_map(|value| value.to_le_bytes()))
            .collect();
        let job_bytes_data: Vec<u8> = jobs.into_iter().flat_map(u32::to_le_bytes).collect();
        let device = context.device();
        let jobs_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("coverage jobs"),
            contents: &job_bytes_data,
            usage: wgpu::BufferUsages::STORAGE,
        });
        let edges_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("coverage edges"),
            contents: &edge_bytes_data,
            usage: wgpu::BufferUsages::STORAGE,
        });
        let output = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("coverage output"),
            size: output_bytes as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("coverage bindings"),
            layout: &self.pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: jobs_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: edges_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: output.as_entire_binding(),
                },
            ],
        });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("coverage raster"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bindings, &[]);
            pass.dispatch_workgroups(
                max_width.div_ceil(8) as u32,
                max_height.div_ceil(8) as u32,
                requests.len() as u32,
            );
        }
        Ok(CoverageBatch {
            output: Some(output),
            rasters,
            total_count: output_count,
        })
    }

    /// Rasterize a batch and return the sample counts to the CPU.
    pub fn coverage_batch(
        &self,
        context: &GpuContext,
        requests: &[CoverageRequest<'_>],
    ) -> Result<Vec<Vec<u32>>, String> {
        if requests.is_empty() {
            return Ok(Vec::new());
        }
        let device = context.device();
        let mut encoder = device.create_command_encoder(&Default::default());
        let batch = self.encode_batch(context, &mut encoder, requests)?;
        if batch.total_count == 0 {
            return Ok(batch.rasters.iter().map(|_| Vec::new()).collect());
        }
        let output = batch
            .output
            .as_ref()
            .ok_or("coverage output buffer is missing")?;
        let output_bytes = batch
            .total_count
            .checked_mul(size_of::<u32>())
            .ok_or("coverage readback size overflow")?;
        let download = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("coverage readback"),
            size: output_bytes as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        encoder.copy_buffer_to_buffer(output, 0, &download, 0, output_bytes as u64);
        context.submit(encoder.finish());

        let (sender, receiver) = std::sync::mpsc::channel();
        download
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = sender.send(result);
            });
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|error| format!("GPU coverage poll: {error}"))?;
        receiver
            .recv()
            .map_err(|error| format!("GPU coverage readback: {error}"))?
            .map_err(|error| format!("GPU coverage map: {error}"))?;
        let bytes = download
            .slice(..)
            .get_mapped_range()
            .map_err(|error| format!("GPU coverage mapped range: {error}"))?;
        let values: Vec<u32> = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|chunk| u32::from_le_bytes(*chunk))
            .collect();
        drop(bytes);
        download.unmap();

        let result = batch
            .rasters
            .iter()
            .map(|raster| {
                let start = raster.offset as usize;
                values[start..start + raster.length].to_vec()
            })
            .collect();
        Ok(result)
    }
}

fn validate_grid_coordinate(value: f32) -> Result<(), String> {
    let _ = quantized_grid_coordinate(value)?;
    Ok(())
}

fn validate_float_coordinate(value: f64) -> Result<(), String> {
    if !value.is_finite() {
        return Err("coverage edge coordinate is not finite".into());
    }
    if value.abs() > f64::from(MAX_FLOAT_EDGE_COORDINATE) {
        return Err("coverage edge coordinate exceeds the float-pixel range".into());
    }
    Ok(())
}

fn split_float_coordinate(value: f64) -> (f32, f32) {
    let high = value as f32;
    let low = (value - f64::from(high)) as f32;
    (high, low)
}

fn quantized_grid_coordinate(value: f32) -> Result<i32, String> {
    if !value.is_finite() {
        return Err("coverage edge coordinate is not finite".into());
    }
    let coordinate = (f64::from(value) * 8.0).round();
    if coordinate.abs() > MAX_EDGE_GRID_COORDINATE {
        return Err("coverage edge coordinate exceeds the fixed-point range".into());
    }
    Ok(coordinate as i32)
}

#[cfg(test)]
mod tests {
    use super::{
        CoverageEdge, CoverageKernel, CoverageRequest, EdgeCoordinates, FillRule, SampleGrid,
    };

    #[test]
    fn float_pixel_edges_preserve_fractional_fill_positions() {
        let context = match crate::context() {
            Ok(context) => context,
            Err(error) if error.starts_with("host GPU adapter:") => {
                eprintln!(
                    "skipping GPU coverage test because no host GPU adapter is available: {error}"
                );
                return;
            }
            Err(error) => panic!("host GPU coverage setup failed: {error}"),
        };
        let kernel = CoverageKernel::new(context);
        let edges = [
            CoverageEdge::new([0.53, 0.1], [1.49, 0.1]),
            CoverageEdge::new([1.49, 0.1], [1.49, 0.9]),
            CoverageEdge::new([1.49, 0.9], [0.53, 0.9]),
            CoverageEdge::new([0.53, 0.9], [0.53, 0.1]),
        ];
        let request = |edge_coordinates| CoverageRequest {
            bounds: [0, 2, 0, 1],
            edges: &edges,
            edge_coordinates,
            fill_rule: FillRule::EvenOdd,
            sample_grid: SampleGrid::OneByOne,
            stroke_radius: None,
        };

        let fractional = kernel
            .coverage_batch(context, &[request(EdgeCoordinates::FloatPixel)])
            .expect("float-edge GPU coverage succeeds");
        let eighth_pixel = kernel
            .coverage_batch(context, &[request(EdgeCoordinates::EighthPixel)])
            .expect("eighth-pixel GPU coverage succeeds");
        assert_eq!(fractional, vec![vec![0, 0]]);
        assert_eq!(eighth_pixel, vec![vec![1, 0]]);
    }

    #[test]
    fn float_pixel_edges_preserve_f64_residuals_below_f32_precision() {
        let context = match crate::context() {
            Ok(context) => context,
            Err(error) if error.starts_with("host GPU adapter:") => {
                eprintln!(
                    "skipping GPU coverage test because no host GPU adapter is available: {error}"
                );
                return;
            }
            Err(error) => panic!("host GPU coverage setup failed: {error}"),
        };
        let kernel = CoverageKernel::new(context);
        let edges = [
            CoverageEdge::from_f64([0.40000001, 0.0], [1.49, 0.0]),
            CoverageEdge::from_f64([1.49, 0.0], [1.49, 1.0]),
            CoverageEdge::from_f64([1.49, 1.0], [0.60000001, 1.0]),
            CoverageEdge::from_f64([0.60000001, 1.0], [0.40000001, 0.0]),
        ];
        let request = |edge_coordinates| CoverageRequest {
            bounds: [0, 2, 0, 1],
            edges: &edges,
            edge_coordinates,
            fill_rule: FillRule::EvenOdd,
            sample_grid: SampleGrid::OneByOne,
            stroke_radius: None,
        };

        let fractional = kernel
            .coverage_batch(context, &[request(EdgeCoordinates::FloatPixel)])
            .expect("float-edge GPU coverage succeeds");
        let eighth_pixel = kernel
            .coverage_batch(context, &[request(EdgeCoordinates::EighthPixel)])
            .expect("eighth-pixel GPU coverage succeeds");
        assert_eq!(fractional, vec![vec![0, 0]]);
        assert_eq!(eighth_pixel, vec![vec![1, 0]]);
    }

    #[test]
    fn float_pixel_edges_keep_sloped_sample_side_near_intersections() {
        let context = match crate::context() {
            Ok(context) => context,
            Err(error) if error.starts_with("host GPU adapter:") => {
                eprintln!(
                    "skipping GPU coverage test because no host GPU adapter is available: {error}"
                );
                return;
            }
            Err(error) => panic!("host GPU coverage setup failed: {error}"),
        };
        let kernel = CoverageKernel::new(context);
        let edges = [
            CoverageEdge::from_f64(
                [538.6606754515077, 442.2918976665267],
                [547.5942692015077, 465.0907257915267],
            ),
            CoverageEdge::from_f64(
                [546.8959651734923, 465.3643523334733],
                [537.9623714234923, 442.5655242084733],
            ),
        ];
        let actual = kernel
            .coverage_batch(
                context,
                &[CoverageRequest {
                    bounds: [544, 545, 456, 457],
                    edges: &edges,
                    edge_coordinates: EdgeCoordinates::FloatPixel,
                    fill_rule: FillRule::NonZero,
                    sample_grid: SampleGrid::FourByFour,
                    stroke_radius: None,
                }],
            )
            .expect("GPU coverage succeeds near the sloped edge intersection");
        assert_eq!(actual, vec![vec![3]]);
    }
}
