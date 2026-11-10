//! GPU-resident surfaces with exact guest-memory transfers and byte-write masks.
//!
//! Surface texels retain the packed guest pixel bits in an `r32uint` texture.
//! The format stays in the cache key so later draw pipelines can decode the
//! same color and depth formats without losing guest bytes during upload or
//! readback. A second texture exposes the resolved per-byte write mask; its
//! atomic accumulator lets multiple draws contribute disjoint byte writes.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard, OnceLock},
};
#[cfg(not(target_arch = "wasm32"))]
use std::{
    future::Future,
    task::{Context, Poll, Wake},
};

use fujimod_perf::{DeviceWorkCounter, add_work, work_timer};
use wgpu::util::DeviceExt;

const TILE_SIDE: u32 = 8;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SurfaceFormat {
    Rgba8,
    Rgb565,
    Rgb5A1,
    Rgba4,
    D16,
    D24,
    D24S8,
}

impl SurfaceFormat {
    pub const fn bytes_per_pixel(self) -> u32 {
        match self {
            Self::Rgba8 | Self::D24S8 => 4,
            Self::D24 => 3,
            Self::Rgb565 | Self::Rgb5A1 | Self::Rgba4 | Self::D16 => 2,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SurfaceExtent {
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SurfaceLayout {
    Linear,
    Morton8x8,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SurfaceKey {
    pub guest_address: u64,
    pub format: SurfaceFormat,
    pub extent: SurfaceExtent,
    pub layout: SurfaceLayout,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PixelWriteMask {
    pub x: u32,
    pub y: u32,
    /// Bit `n` marks guest byte lane `n` within this pixel as written.
    pub byte_mask: u32,
}

pub struct SurfaceReadback {
    /// Packed guest bytes in the layout selected by the surface key.
    pub bytes: Vec<u8>,
    /// Sorted, contiguous byte ranges written since the last upload or clear.
    pub written_ranges: Vec<(u64, usize)>,
}

/// GPU surface readbacks encoded into a command buffer and mapped after it is
/// submitted. Mapping a batch polls once for all of its surfaces.
pub struct PendingSurfaceReadbacks {
    surfaces: Vec<PendingSurfaceReadback>,
}

struct PendingSurfaceReadback {
    surface: Arc<GpuSurface>,
    slot: usize,
}

struct SurfaceReadbackBuffers {
    pixel_result: Arc<wgpu::Buffer>,
    mask_result: Arc<wgpu::Buffer>,
    download: Arc<wgpu::Buffer>,
    pixel_bytes: usize,
    busy: bool,
}

pub struct GpuSurface {
    key: SurfaceKey,
    texture: wgpu::Texture,
    write_mask_texture: wgpu::Texture,
    write_mask_accumulator: wgpu::Buffer,
    pixel_count: usize,
    byte_len: usize,
    guest_shadow: Mutex<Vec<u8>>,
    readback_buffers: Mutex<Vec<Option<SurfaceReadbackBuffers>>>,
}

impl Drop for PendingSurfaceReadback {
    fn drop(&mut self) {
        if let Ok(mut slots) = self.surface.readback_buffers.lock() {
            if self.slot == 0 {
                if let Some(Some(slot)) = slots.get_mut(self.slot) {
                    slot.busy = false;
                }
            } else if let Some(slot) = slots.get_mut(self.slot) {
                *slot = None;
            }
        }
    }
}

impl GpuSurface {
    pub fn key(&self) -> SurfaceKey {
        self.key
    }

    /// Packed guest pixel values, one `r32uint` texel per linear image pixel.
    pub fn texture(&self) -> &wgpu::Texture {
        &self.texture
    }

    /// Resolved per-pixel byte masks. Bit `n` marks byte lane `n` as written.
    /// Call [`GpuContext::readback`] to resolve pending atomic writes first.
    pub fn write_mask_texture(&self) -> &wgpu::Texture {
        &self.write_mask_texture
    }

    /// Atomic byte-mask accumulator indexed by `y * width + x`.
    ///
    /// Draw shaders can bind this as `array<atomic<u32>>` and use
    /// `atomicOr(&mask[y * width + x], byte_mask)` to preserve writes across
    /// overlapping draws. The cache resolves it into `write_mask_texture`
    /// before readback.
    pub fn write_mask_accumulator(&self) -> &wgpu::Buffer {
        &self.write_mask_accumulator
    }

    pub fn byte_len(&self) -> usize {
        self.byte_len
    }
}

/// Process-wide host GPU context and persistent guest surface cache.
pub struct GpuContext {
    gpu: Gpu,
    surfaces: Mutex<HashMap<SurfaceKey, Arc<GpuSurface>>>,
    submission_lock: Mutex<()>,
}

impl GpuContext {
    async fn new() -> Result<Self, String> {
        Ok(Self {
            gpu: Gpu::new().await?,
            surfaces: Mutex::new(HashMap::new()),
            submission_lock: Mutex::new(()),
        })
    }

    /// Serialize a multi-pass submission that updates resident targets.
    pub fn acquire(&self) -> Result<MutexGuard<'_, ()>, String> {
        self.submission_lock
            .lock()
            .map_err(|_| "GPU submission lock poisoned".into())
    }

    /// Get or create a resident surface, synchronizing it when guest memory changed.
    ///
    /// Passing `None` preserves an existing surface or initializes a new one to zero.
    pub fn get_or_create_target(
        &self,
        key: SurfaceKey,
        guest_bytes: Option<&[u8]>,
    ) -> Result<Arc<GpuSurface>, String> {
        let (pixel_count, byte_len) = validate_key(key, &self.gpu)?;
        if guest_bytes.is_some_and(|bytes| bytes.len() != byte_len) {
            return Err(format!(
                "host GPU surface upload has {} bytes, expected {byte_len}",
                guest_bytes.map_or(0, <[u8]>::len)
            ));
        }
        let mut surfaces = self
            .surfaces
            .lock()
            .map_err(|_| "host GPU surface cache lock poisoned")?;
        if let Some(surface) = surfaces.get(&key).cloned() {
            if let Some(bytes) = guest_bytes {
                let mut shadow = surface
                    .guest_shadow
                    .lock()
                    .map_err(|_| "host GPU surface shadow lock poisoned")?;
                if shadow.as_slice() != bytes {
                    self.gpu.upload(&surface, bytes)?;
                    shadow.clear();
                    shadow.extend_from_slice(bytes);
                }
            }
            return Ok(surface);
        }

        surfaces.retain(|existing, _| existing.guest_address != key.guest_address);
        let surface = Arc::new(create_surface(key, pixel_count, byte_len, &self.gpu));
        let initial_bytes = guest_bytes
            .map(<[u8]>::to_vec)
            .unwrap_or_else(|| vec![0; byte_len]);
        self.gpu.upload(&surface, &initial_bytes)?;
        *surface
            .guest_shadow
            .lock()
            .map_err(|_| "host GPU surface shadow lock poisoned")? = initial_bytes;
        surfaces.insert(key, Arc::clone(&surface));
        Ok(surface)
    }

    /// Get or create a resident surface and record any guest upload into the
    /// caller's command encoder. Guest bytes are queued with `write_buffer`
    /// before the encoder is submitted.
    pub fn get_or_create_target_encoded(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        key: SurfaceKey,
        guest_bytes: Option<&[u8]>,
    ) -> Result<Arc<GpuSurface>, String> {
        let (pixel_count, byte_len) = validate_key(key, &self.gpu)?;
        if guest_bytes.is_some_and(|bytes| bytes.len() != byte_len) {
            return Err(format!(
                "host GPU surface upload has {} bytes, expected {byte_len}",
                guest_bytes.map_or(0, <[u8]>::len)
            ));
        }
        let mut surfaces = self
            .surfaces
            .lock()
            .map_err(|_| "host GPU surface cache lock poisoned")?;
        if let Some(surface) = surfaces.get(&key).cloned() {
            if let Some(bytes) = guest_bytes {
                let shadow = surface
                    .guest_shadow
                    .lock()
                    .map_err(|_| "host GPU surface shadow lock poisoned")?;
                if shadow.as_slice() != bytes {
                    self.gpu.encode_upload(encoder, &surface, bytes)?;
                }
            } else if surface
                .guest_shadow
                .lock()
                .map_err(|_| "host GPU surface shadow lock poisoned")?
                .is_empty()
            {
                self.gpu
                    .encode_upload(encoder, &surface, &vec![0; byte_len])?;
            }
            return Ok(surface);
        }

        surfaces.retain(|existing, _| existing.guest_address != key.guest_address);
        let surface = Arc::new(create_surface(key, pixel_count, byte_len, &self.gpu));
        let initial_bytes = guest_bytes
            .map(<[u8]>::to_vec)
            .unwrap_or_else(|| vec![0; byte_len]);
        self.gpu.encode_upload(encoder, &surface, &initial_bytes)?;
        surfaces.insert(key, Arc::clone(&surface));
        Ok(surface)
    }

    pub fn get(&self, key: SurfaceKey) -> Result<Option<Arc<GpuSurface>>, String> {
        self.surfaces
            .lock()
            .map(|surfaces| surfaces.get(&key).cloned())
            .map_err(|_| "host GPU surface cache lock poisoned".into())
    }

    pub fn len(&self) -> Result<usize, String> {
        self.surfaces
            .lock()
            .map(|surfaces| surfaces.len())
            .map_err(|_| "host GPU surface cache lock poisoned".into())
    }

    pub fn is_empty(&self) -> Result<bool, String> {
        self.surfaces
            .lock()
            .map(|surfaces| surfaces.is_empty())
            .map_err(|_| "host GPU surface cache lock poisoned".into())
    }

    pub fn evict(&self, key: SurfaceKey) -> Result<bool, String> {
        self.surfaces
            .lock()
            .map(|mut surfaces| surfaces.remove(&key).is_some())
            .map_err(|_| "host GPU surface cache lock poisoned".into())
    }

    /// Atomically add byte writes for any pixels. Duplicate coordinates are
    /// ORed, so disjoint channel or depth/stencil writes remain exact.
    pub fn record_write_masks(
        &self,
        key: SurfaceKey,
        writes: &[PixelWriteMask],
    ) -> Result<(), String> {
        if writes.is_empty() {
            return Ok(());
        }
        let surface = self
            .surfaces
            .lock()
            .map_err(|_| "host GPU surface cache lock poisoned")?
            .get(&key)
            .cloned()
            .ok_or("host GPU surface is not resident")?;
        let bytes_per_pixel = key.format.bytes_per_pixel();
        let valid_mask = (1 << bytes_per_pixel) - 1;
        let record_bytes = writes
            .len()
            .checked_mul(12)
            .ok_or("host GPU write-mask record size overflow")?;
        let limits = self.gpu.device.limits();
        if writes.len() > u32::MAX as usize
            || record_bytes > limits.max_storage_buffer_binding_size as usize
            || (writes.len() as u32).div_ceil(64) > limits.max_compute_workgroups_per_dimension
        {
            return Err("host GPU write-mask batch exceeds host GPU limits".into());
        }
        let mut records = Vec::with_capacity(record_bytes);
        for write in writes {
            if write.x >= key.extent.width || write.y >= key.extent.height {
                return Err("host GPU write-mask coordinate is outside the surface".into());
            }
            if write.byte_mask & !valid_mask != 0 {
                return Err("host GPU write mask exceeds the surface pixel format".into());
            }
            records.extend_from_slice(&write.x.to_le_bytes());
            records.extend_from_slice(&write.y.to_le_bytes());
            records.extend_from_slice(&write.byte_mask.to_le_bytes());
        }
        self.gpu
            .record_write_masks(&surface, &records, writes.len())
    }

    /// Clear the byte-write tracker without changing the resident pixels.
    pub fn clear_write_masks(&self, key: SurfaceKey) -> Result<(), String> {
        let surface = self
            .surfaces
            .lock()
            .map_err(|_| "host GPU surface cache lock poisoned")?
            .get(&key)
            .cloned()
            .ok_or("host GPU surface is not resident")?;
        self.gpu.clear_write_masks(&surface)
    }

    /// Record a write-mask clear into a caller-owned command encoder.
    pub fn encode_clear_write_masks(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        key: SurfaceKey,
    ) -> Result<(), String> {
        let surface = self
            .surfaces
            .lock()
            .map_err(|_| "host GPU surface cache lock poisoned")?
            .get(&key)
            .cloned()
            .ok_or("host GPU surface is not resident")?;
        self.gpu.encode_clear_write_masks(encoder, &surface);
        Ok(())
    }

    /// Convert resident pixels back to the keyed guest layout and exact byte ranges.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn readback(&self, key: SurfaceKey) -> Result<SurfaceReadback, String> {
        let surface = self
            .surfaces
            .lock()
            .map_err(|_| "host GPU surface cache lock poisoned")?
            .get(&key)
            .cloned()
            .ok_or("host GPU surface is not resident")?;
        self.gpu.readback(surface)
    }

    /// Convert a resident surface back to guest bytes without blocking WebGPU.
    pub async fn readback_async(&self, key: SurfaceKey) -> Result<SurfaceReadback, String> {
        let surface = self
            .surfaces
            .lock()
            .map_err(|_| "host GPU surface cache lock poisoned")?
            .get(&key)
            .cloned()
            .ok_or("host GPU surface is not resident")?;
        let mut encoder = self.gpu.device.create_command_encoder(&Default::default());
        let pending = self.gpu.encode_readback(&mut encoder, surface)?;
        self.gpu.queue.submit([encoder.finish()]);
        let (_, readback) = self
            .gpu
            .finish_readbacks_async(vec![pending])
            .await?
            .pop()
            .ok_or("host GPU readback returned no result")?;
        Ok(readback)
    }

    /// Encode readbacks for several surfaces into the caller's command list.
    pub fn encode_readbacks(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        keys: &[SurfaceKey],
    ) -> Result<PendingSurfaceReadbacks, String> {
        let surfaces = self
            .surfaces
            .lock()
            .map_err(|_| "host GPU surface cache lock poisoned")?;
        let mut pending = Vec::with_capacity(keys.len());
        for &key in keys {
            let surface = surfaces
                .get(&key)
                .cloned()
                .ok_or("host GPU surface is not resident")?;
            pending.push(self.gpu.encode_readback(encoder, surface)?);
        }
        Ok(PendingSurfaceReadbacks { surfaces: pending })
    }

    /// Map a submitted batch of surface readbacks with a single device poll.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn finish_readbacks(
        &self,
        pending: PendingSurfaceReadbacks,
    ) -> Result<Vec<(SurfaceKey, SurfaceReadback)>, String> {
        let readbacks = self.gpu.finish_readbacks(pending.surfaces)?;
        Ok(readbacks
            .into_iter()
            .map(|(surface, readback)| (surface.key, readback))
            .collect())
    }

    /// Finish a submitted batch without blocking the browser event loop.
    pub async fn finish_readbacks_async(
        &self,
        pending: PendingSurfaceReadbacks,
    ) -> Result<Vec<(SurfaceKey, SurfaceReadback)>, String> {
        let readbacks = self.gpu.finish_readbacks_async(pending.surfaces).await?;
        Ok(readbacks
            .into_iter()
            .map(|(surface, readback)| (surface.key, readback))
            .collect())
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.gpu.device
    }

    pub fn queue(&self) -> &wgpu::Queue {
        &self.gpu.queue
    }

    pub fn submit(&self, command_buffer: wgpu::CommandBuffer) -> wgpu::SubmissionIndex {
        self.gpu.queue.submit([command_buffer])
    }

    pub fn adapter_name(&self) -> &str {
        &self.gpu.name
    }

    pub fn adapter_device_type(&self) -> wgpu::DeviceType {
        self.gpu.device_type
    }

    /// Whether native `R32Uint` surfaces support in-place storage access.
    pub fn supports_read_write_r32uint_storage(&self) -> bool {
        self.gpu.read_write_r32uint_storage
    }

    pub fn accumulate_draw_mask(&self, key: SurfaceKey) -> Result<(), String> {
        let surface = self
            .surfaces
            .lock()
            .map_err(|_| "host GPU surface cache lock poisoned")?
            .get(&key)
            .cloned()
            .ok_or("host GPU surface is not resident")?;
        self.gpu.accumulate_draw_mask(&surface)
    }

    /// Record rendered byte-mask accumulation into a caller-owned encoder.
    pub fn encode_accumulate_draw_mask(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        key: SurfaceKey,
    ) -> Result<(), String> {
        let surface = self
            .surfaces
            .lock()
            .map_err(|_| "host GPU surface cache lock poisoned")?
            .get(&key)
            .cloned()
            .ok_or("host GPU surface is not resident")?;
        self.gpu.encode_accumulate_draw_mask(encoder, &surface);
        Ok(())
    }
}

static GPU_CONTEXT: OnceLock<Result<GpuContext, String>> = OnceLock::new();

pub fn context() -> Result<&'static GpuContext, String> {
    #[cfg(not(target_arch = "wasm32"))]
    let context = GPU_CONTEXT.get_or_init(|| block_on(GpuContext::new()));
    #[cfg(target_arch = "wasm32")]
    let context = GPU_CONTEXT
        .get()
        .ok_or_else(|| "GPU context is not initialized; await context_async first".to_owned())?;
    match context {
        Ok(context) => Ok(context),
        Err(error) => Err(error.clone()),
    }
}

/// Initialize the shared GPU device without blocking the browser event loop.
pub async fn context_async() -> Result<&'static GpuContext, String> {
    if let Some(context) = GPU_CONTEXT.get() {
        return match context {
            Ok(context) => Ok(context),
            Err(error) => Err(error.clone()),
        };
    }

    let new_context = GpuContext::new().await?;
    let _ = GPU_CONTEXT.set(Ok(new_context));
    context()
}

struct Gpu {
    name: String,
    device_type: wgpu::DeviceType,
    device: wgpu::Device,
    queue: wgpu::Queue,
    read_write_r32uint_storage: bool,
    upload_pipeline: wgpu::ComputePipeline,
    clear_mask_pipeline: wgpu::ComputePipeline,
    record_mask_pipeline: wgpu::ComputePipeline,
    accumulate_draw_mask_pipeline: wgpu::ComputePipeline,
    readback_pipeline: wgpu::ComputePipeline,
}

impl Gpu {
    async fn new() -> Result<Self, String> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: None,
                ..Default::default()
            })
            .await
            .map_err(|error| format!("host GPU adapter: {error}"))?;
        let info = adapter.get_info();
        let read_write_r32uint_storage = adapter
            .features()
            .contains(wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES)
            && adapter
                .get_texture_format_features(wgpu::TextureFormat::R32Uint)
                .flags
                .contains(wgpu::TextureFormatFeatureFlags::STORAGE_READ_WRITE);
        let required_features = if read_write_r32uint_storage {
            wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES
        } else {
            wgpu::Features::empty()
        };
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                required_features,
                ..Default::default()
            })
            .await
            .map_err(|error| format!("host GPU device: {error}"))?;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("host GPU resident surfaces"),
            source: wgpu::ShaderSource::Wgsl(include_str!("surfaces.wgsl").into()),
        });
        let pipeline = |label, entry_point| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(label),
                layout: None,
                module: &shader,
                entry_point: Some(entry_point),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let upload_pipeline = pipeline("host GPU surface upload", "upload_morton");
        let clear_mask_pipeline = pipeline("host GPU write-mask clear", "clear_write_mask");
        let record_mask_pipeline = pipeline("host GPU write-mask record", "record_write_masks");
        let accumulate_draw_mask_pipeline = pipeline(
            "host GPU rendered write-mask record",
            "accumulate_draw_mask",
        );
        let readback_pipeline = pipeline("host GPU surface readback", "readback_morton");
        Ok(Self {
            name: format!("{:?}: {} ({:?})", info.backend, info.name, info.device_type),
            device_type: info.device_type,
            device,
            queue,
            read_write_r32uint_storage,
            upload_pipeline,
            clear_mask_pipeline,
            record_mask_pipeline,
            accumulate_draw_mask_pipeline,
            readback_pipeline,
        })
    }

    fn upload(&self, surface: &GpuSurface, guest_bytes: &[u8]) -> Result<(), String> {
        let mut encoder = self.device.create_command_encoder(&Default::default());
        self.encode_upload(&mut encoder, surface, guest_bytes)?;
        self.queue.submit([encoder.finish()]);
        Ok(())
    }

    fn encode_upload(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        surface: &GpuSurface,
        guest_bytes: &[u8],
    ) -> Result<(), String> {
        let padded_len = guest_bytes
            .len()
            .max(4)
            .checked_add(3)
            .ok_or("host GPU surface upload size overflow")?
            & !3;
        let mut padded_bytes = guest_bytes.to_vec();
        padded_bytes.resize(padded_len, 0);
        let guest = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("host GPU surface guest bytes"),
            size: padded_len as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue.write_buffer(&guest, 0, &padded_bytes);
        let params = self.params(surface, 0);
        let upload_bindings = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("host GPU surface upload bindings"),
            layout: &self.upload_pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: guest.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(
                        &surface.texture.create_view(&Default::default()),
                    ),
                },
            ],
        });
        let clear_bindings = self.clear_mask_bindings(surface, &params);
        dispatch_2d(
            encoder,
            &self.upload_pipeline,
            &upload_bindings,
            surface.key.extent,
            "host GPU surface upload",
        );
        dispatch_2d(
            encoder,
            &self.clear_mask_pipeline,
            &clear_bindings,
            surface.key.extent,
            "host GPU write-mask clear",
        );
        Ok(())
    }

    fn record_write_masks(
        &self,
        surface: &GpuSurface,
        records: &[u8],
        count: usize,
    ) -> Result<(), String> {
        let record_buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("host GPU pixel byte-write masks"),
                contents: records,
                usage: wgpu::BufferUsages::STORAGE,
            });
        let params = self.params(surface, count as u32);
        let bindings = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("host GPU write-mask record bindings"),
            layout: &self.record_mask_pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: record_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: surface.write_mask_accumulator.as_entire_binding(),
                },
            ],
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&self.record_mask_pipeline);
            pass.set_bind_group(0, &bindings, &[]);
            pass.dispatch_workgroups((count as u32).div_ceil(64), 1, 1);
        }
        self.queue.submit([encoder.finish()]);
        Ok(())
    }

    fn clear_write_masks(&self, surface: &GpuSurface) -> Result<(), String> {
        let params = self.params(surface, 0);
        let bindings = self.clear_mask_bindings(surface, &params);
        let mut encoder = self.device.create_command_encoder(&Default::default());
        self.encode_clear_write_masks_with_bindings(&mut encoder, surface.key.extent, &bindings);
        self.queue.submit([encoder.finish()]);
        Ok(())
    }

    fn encode_clear_write_masks(&self, encoder: &mut wgpu::CommandEncoder, surface: &GpuSurface) {
        let params = self.params(surface, 0);
        let bindings = self.clear_mask_bindings(surface, &params);
        self.encode_clear_write_masks_with_bindings(encoder, surface.key.extent, &bindings);
    }

    fn encode_clear_write_masks_with_bindings(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        extent: SurfaceExtent,
        bindings: &wgpu::BindGroup,
    ) {
        dispatch_2d(
            encoder,
            &self.clear_mask_pipeline,
            bindings,
            extent,
            "host GPU write-mask clear",
        );
    }

    fn accumulate_draw_mask(&self, surface: &GpuSurface) -> Result<(), String> {
        let mut encoder = self.device.create_command_encoder(&Default::default());
        self.encode_accumulate_draw_mask(&mut encoder, surface);
        self.queue.submit([encoder.finish()]);
        Ok(())
    }

    fn encode_accumulate_draw_mask(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        surface: &GpuSurface,
    ) {
        let params = self.params(surface, 0);
        let bindings = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("host GPU rendered write-mask bindings"),
            layout: &self.accumulate_draw_mask_pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 12,
                    resource: wgpu::BindingResource::TextureView(
                        &surface.write_mask_texture.create_view(&Default::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 13,
                    resource: surface.write_mask_accumulator.as_entire_binding(),
                },
            ],
        });
        dispatch_2d(
            encoder,
            &self.accumulate_draw_mask_pipeline,
            &bindings,
            surface.key.extent,
            "host GPU rendered write-mask accumulation",
        );
    }

    fn encode_readback(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        surface: Arc<GpuSurface>,
    ) -> Result<PendingSurfaceReadback, String> {
        let pixel_bytes = surface
            .pixel_count
            .checked_mul(4)
            .ok_or("host GPU surface readback size overflow")?;
        let tiles_x = surface.key.extent.width.div_ceil(TILE_SIDE) as usize;
        let tiles_y = surface.key.extent.height.div_ceil(TILE_SIDE) as usize;
        let mask_bytes = tiles_x
            .checked_mul(tiles_y)
            .and_then(|tiles| tiles.checked_mul(8 * 4))
            .ok_or("host GPU surface mask readback size overflow")?;
        let download_size = pixel_bytes
            .checked_add(mask_bytes)
            .ok_or("host GPU surface readback size overflow")?;
        let mut slots = surface
            .readback_buffers
            .lock()
            .map_err(|_| "host GPU surface readback buffer lock poisoned")?;
        let slot_index = if slots
            .first()
            .is_none_or(|slot| slot.as_ref().is_none_or(|slot| !slot.busy))
        {
            0
        } else if let Some(slot) = slots.iter().skip(1).position(Option::is_none) {
            slot + 1
        } else {
            slots.push(None);
            slots.len() - 1
        };
        if slots.get(slot_index).is_none_or(Option::is_none) {
            let buffers = SurfaceReadbackBuffers {
                pixel_result: Arc::new(self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("host GPU surface pixel readback"),
                    size: pixel_bytes as u64,
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                    mapped_at_creation: false,
                })),
                mask_result: Arc::new(self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("host GPU surface mask readback"),
                    size: mask_bytes as u64,
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                    mapped_at_creation: false,
                })),
                download: Arc::new(self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("host GPU surface readback staging"),
                    size: download_size as u64,
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })),
                pixel_bytes,
                busy: false,
            };
            if slot_index == slots.len() {
                slots.push(Some(buffers));
            } else {
                slots[slot_index] = Some(buffers);
            }
        }
        let slot = slots[slot_index]
            .as_mut()
            .ok_or("host GPU surface readback slot disappeared")?;
        slot.busy = true;
        let pixel_result = Arc::clone(&slot.pixel_result);
        let mask_result = Arc::clone(&slot.mask_result);
        let download = Arc::clone(&slot.download);
        drop(slots);
        let params = self.params(&surface, 0);
        let readback_bindings = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("host GPU surface readback bindings"),
            layout: &self.readback_pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 9,
                    resource: wgpu::BindingResource::TextureView(
                        &surface.texture.create_view(&Default::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: surface.write_mask_accumulator.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: wgpu::BindingResource::TextureView(
                        &surface.write_mask_texture.create_view(&Default::default()),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 11,
                    resource: pixel_result.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 14,
                    resource: mask_result.as_entire_binding(),
                },
            ],
        });
        dispatch_2d(
            encoder,
            &self.readback_pipeline,
            &readback_bindings,
            surface.key.extent,
            "host GPU surface readback",
        );
        encoder.copy_buffer_to_buffer(&pixel_result, 0, &download, 0, pixel_bytes as u64);
        encoder.copy_buffer_to_buffer(
            &mask_result,
            0,
            &download,
            pixel_bytes as u64,
            mask_bytes as u64,
        );
        Ok(PendingSurfaceReadback {
            surface,
            slot: slot_index,
        })
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn finish_readbacks(
        &self,
        pending: Vec<PendingSurfaceReadback>,
    ) -> Result<Vec<(Arc<GpuSurface>, SurfaceReadback)>, String> {
        let receivers = pending
            .iter()
            .map(|readback| {
                let download = readback
                    .surface
                    .readback_buffers
                    .lock()
                    .map_err(|_| "host GPU surface readback buffer lock poisoned")?
                    .get(readback.slot)
                    .and_then(Option::as_ref)
                    .ok_or("host GPU surface readback slot disappeared")?
                    .download
                    .clone();
                let (sender, receiver) = std::sync::mpsc::channel();
                download
                    .slice(..)
                    .map_async(wgpu::MapMode::Read, move |status| {
                        let _ = sender.send(status);
                    });
                Ok(receiver)
            })
            .collect::<Result<Vec<_>, String>>()?;
        if !pending.is_empty() {
            add_work(DeviceWorkCounter::GpuReadbackBatches, 1);
            let _wait = work_timer(DeviceWorkCounter::GpuReadbackWaitNs);
            self.device
                .poll(wgpu::PollType::wait_indefinitely())
                .map_err(|error| format!("host GPU polling: {error}"))?;
        }
        for receiver in receivers {
            receiver
                .recv()
                .map_err(|error| error.to_string())?
                .map_err(|error| error.to_string())?;
        }

        self.decode_readbacks(pending)
    }

    async fn finish_readbacks_async(
        &self,
        pending: Vec<PendingSurfaceReadback>,
    ) -> Result<Vec<(Arc<GpuSurface>, SurfaceReadback)>, String> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.finish_readbacks(pending)
        }
        #[cfg(target_arch = "wasm32")]
        {
            if !pending.is_empty() {
                add_work(DeviceWorkCounter::GpuReadbackBatches, 1);
            }
            let _wait = work_timer(DeviceWorkCounter::GpuReadbackWaitNs);
            for readback in &pending {
                let download = readback
                    .surface
                    .readback_buffers
                    .lock()
                    .map_err(|_| "host GPU surface readback buffer lock poisoned")?
                    .get(readback.slot)
                    .and_then(Option::as_ref)
                    .ok_or("host GPU surface readback slot disappeared")?
                    .download
                    .clone();
                wait_for_mapping(&download).await?;
            }
            drop(_wait);
            self.decode_readbacks(pending)
        }
    }

    fn decode_readbacks(
        &self,
        pending: Vec<PendingSurfaceReadback>,
    ) -> Result<Vec<(Arc<GpuSurface>, SurfaceReadback)>, String> {
        let mut readbacks = Vec::with_capacity(pending.len());
        for pending in pending {
            let key = pending.surface.key;
            let (download, pixel_bytes) = {
                let slots = pending
                    .surface
                    .readback_buffers
                    .lock()
                    .map_err(|_| "host GPU surface readback buffer lock poisoned")?;
                let slot = slots
                    .get(pending.slot)
                    .and_then(Option::as_ref)
                    .ok_or("host GPU surface readback slot disappeared")?;
                (Arc::clone(&slot.download), slot.pixel_bytes)
            };
            let mapped = download
                .slice(..)
                .get_mapped_range()
                .map_err(|error| error.to_string())?;
            let mut shadow = pending
                .surface
                .guest_shadow
                .lock()
                .map_err(|_| "host GPU surface shadow lock poisoned")?;
            let written_ranges = match key.format.bytes_per_pixel() {
                2 => decode_pixels::<2>(
                    key,
                    &mapped[..pixel_bytes],
                    &mapped[pixel_bytes..],
                    &mut shadow,
                ),
                3 => decode_pixels::<3>(
                    key,
                    &mapped[..pixel_bytes],
                    &mapped[pixel_bytes..],
                    &mut shadow,
                ),
                4 => decode_pixels::<4>(
                    key,
                    &mapped[..pixel_bytes],
                    &mapped[pixel_bytes..],
                    &mut shadow,
                ),
                _ => unreachable!("checked surface format"),
            };
            let readback = SurfaceReadback {
                bytes: shadow.clone(),
                written_ranges,
            };
            drop(shadow);
            add_work(DeviceWorkCounter::GpuReadbackSurfaces, 1);
            add_work(
                DeviceWorkCounter::GpuReadbackPixels,
                u64::from(key.extent.width) * u64::from(key.extent.height),
            );
            add_work(DeviceWorkCounter::GpuReadbackBytes, mapped.len() as u64);
            drop(mapped);
            download.unmap();
            readbacks.push((Arc::clone(&pending.surface), readback));
        }
        Ok(readbacks)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn readback(&self, surface: Arc<GpuSurface>) -> Result<SurfaceReadback, String> {
        let mut encoder = self.device.create_command_encoder(&Default::default());
        let pending = self.encode_readback(&mut encoder, surface)?;
        self.queue.submit([encoder.finish()]);
        self.finish_readbacks(vec![pending])?
            .pop()
            .map(|(_, readback)| readback)
            .ok_or_else(|| "host GPU readback returned no result".into())
    }

    fn params(&self, surface: &GpuSurface, write_count: u32) -> wgpu::Buffer {
        let words = [
            surface.key.extent.width,
            surface.key.extent.height,
            surface.key.extent.width,
            surface.key.extent.height,
            surface.key.extent.width.div_ceil(TILE_SIDE),
            surface.key.format.bytes_per_pixel(),
            write_count,
            u32::from(surface.key.layout == SurfaceLayout::Morton8x8),
        ];
        let contents: Vec<u8> = words.into_iter().flat_map(u32::to_le_bytes).collect();
        self.device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("host GPU surface parameters"),
                contents: &contents,
                usage: wgpu::BufferUsages::UNIFORM,
            })
    }

    fn clear_mask_bindings(&self, surface: &GpuSurface, params: &wgpu::Buffer) -> wgpu::BindGroup {
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("host GPU write-mask clear bindings"),
            layout: &self.clear_mask_pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: surface.write_mask_accumulator.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(
                        &surface.write_mask_texture.create_view(&Default::default()),
                    ),
                },
            ],
        })
    }
}

// Pixel words are in guest order; packed masks are grouped by image tiles.
fn decode_pixels<const BYTES: usize>(
    key: SurfaceKey,
    pixels: &[u8],
    masks: &[u8],
    bytes: &mut Vec<u8>,
) -> Vec<(u64, usize)> {
    let width = key.extent.width as usize;
    let height = key.extent.height as usize;
    let pixel_count = width * height;
    bytes.resize(pixel_count * BYTES, 0);
    let mut written_ranges = Vec::new();
    let mut open = None;
    let full = (1u32 << BYTES) - 1;
    match key.layout {
        SurfaceLayout::Linear => {
            let tiles_per_row = width.div_ceil(TILE_SIDE as usize);
            for y in 0..height {
                let tile_row = (y / TILE_SIDE as usize) * tiles_per_row;
                let mask_group = y % TILE_SIDE as usize;
                let mut x = 0;
                while x < width {
                    let tile_column = x / TILE_SIDE as usize;
                    let run = (width - x).min(TILE_SIDE as usize - x % TILE_SIDE as usize);
                    let pixel_start = y * width + x;
                    let mask_index = (tile_row + tile_column) * TILE_SIDE as usize + mask_group;
                    let mask_offset = mask_index * 4;
                    let packed_masks =
                        u32::from_le_bytes(masks[mask_offset..mask_offset + 4].try_into().unwrap());
                    decode_pixel_run::<BYTES>(pixels, bytes, pixel_start, run);
                    decode_write_mask_run::<BYTES>(
                        key.guest_address,
                        pixel_start,
                        run,
                        packed_masks,
                        full,
                        &mut open,
                        &mut written_ranges,
                    );
                    x += run;
                }
            }
        }
        SurfaceLayout::Morton8x8 => {
            let tiles_per_row = width.div_ceil(TILE_SIDE as usize);
            let tile_rows = height.div_ceil(TILE_SIDE as usize);
            for tile in 0..tiles_per_row * tile_rows {
                for row in 0..TILE_SIDE as usize {
                    let pixel_start = tile * 64 + row * TILE_SIDE as usize;
                    let mask_index = tile * TILE_SIDE as usize + row;
                    let mask_offset = mask_index * 4;
                    let packed_masks =
                        u32::from_le_bytes(masks[mask_offset..mask_offset + 4].try_into().unwrap());
                    decode_pixel_run::<BYTES>(pixels, bytes, pixel_start, TILE_SIDE as usize);
                    decode_write_mask_run::<BYTES>(
                        key.guest_address,
                        pixel_start,
                        TILE_SIDE as usize,
                        packed_masks,
                        full,
                        &mut open,
                        &mut written_ranges,
                    );
                }
            }
        }
    }
    if let Some(start) = open {
        written_ranges.push((key.guest_address + start as u64, bytes.len() - start));
    }
    written_ranges
}

fn decode_pixel_run<const BYTES: usize>(
    pixels: &[u8],
    bytes: &mut [u8],
    pixel_start: usize,
    pixel_count: usize,
) {
    let pixel_end = pixel_start + pixel_count;
    let source = &pixels[pixel_start * 4..pixel_end * 4];
    let output = &mut bytes[pixel_start * BYTES..pixel_end * BYTES];
    if BYTES == 4 {
        output.copy_from_slice(source);
    } else {
        for (pixel_word, pixel) in source
            .as_chunks::<4>()
            .0
            .iter()
            .zip(output.as_chunks_mut::<BYTES>().0)
        {
            pixel.copy_from_slice(&pixel_word[..BYTES]);
        }
    }
}

fn decode_write_mask_run<const BYTES: usize>(
    guest_address: u64,
    pixel_start: usize,
    pixel_count: usize,
    packed_masks: u32,
    full: u32,
    open: &mut Option<usize>,
    written_ranges: &mut Vec<(u64, usize)>,
) {
    let start = pixel_start * BYTES;
    let active_nibbles = if pixel_count == 8 {
        u32::MAX
    } else {
        (1u32 << (pixel_count * 4)) - 1
    };
    let full_nibbles = full * 0x1111_1111;
    let masks = packed_masks & active_nibbles;
    if masks == 0 {
        if let Some(range_start) = open.take() {
            written_ranges.push((guest_address + range_start as u64, start - range_start));
        }
        return;
    }
    if masks == (full_nibbles & active_nibbles) {
        open.get_or_insert(start);
        return;
    }
    for pixel in 0..pixel_count {
        let mask = (packed_masks >> (pixel * 4)) & full;
        let pixel_offset = (pixel_start + pixel) * BYTES;
        if mask == full {
            open.get_or_insert(pixel_offset);
        } else if mask == 0 {
            if let Some(range_start) = open.take() {
                written_ranges.push((
                    guest_address + range_start as u64,
                    pixel_offset - range_start,
                ));
            }
        } else {
            for lane in 0..BYTES {
                let at = pixel_offset + lane;
                if mask & (1 << lane) != 0 {
                    open.get_or_insert(at);
                } else if let Some(range_start) = open.take() {
                    written_ranges.push((guest_address + range_start as u64, at - range_start));
                }
            }
        }
    }
}

fn create_surface(key: SurfaceKey, pixel_count: usize, byte_len: usize, gpu: &Gpu) -> GpuSurface {
    let texture_size = wgpu::Extent3d {
        width: key.extent.width,
        height: key.extent.height,
        depth_or_array_layers: 1,
    };
    let create_texture = |label| {
        gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: texture_size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R32Uint,
            usage: wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
    };
    let accumulator = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("host GPU atomic byte-write masks"),
        size: (pixel_count * 4) as u64,
        usage: wgpu::BufferUsages::STORAGE,
        mapped_at_creation: false,
    });
    GpuSurface {
        key,
        texture: create_texture("host GPU resident packed pixels"),
        write_mask_texture: create_texture("host GPU resolved byte-write masks"),
        write_mask_accumulator: accumulator,
        pixel_count,
        byte_len,
        guest_shadow: Mutex::new(Vec::new()),
        readback_buffers: Mutex::new(Vec::new()),
    }
}

fn validate_key(key: SurfaceKey, gpu: &Gpu) -> Result<(usize, usize), String> {
    let extent = key.extent;
    if extent.width == 0 || extent.height == 0 {
        return Err("host GPU surface extent must be nonzero".into());
    }
    if key.layout == SurfaceLayout::Morton8x8
        && (!extent.width.is_multiple_of(TILE_SIDE) || !extent.height.is_multiple_of(TILE_SIDE))
    {
        return Err("host GPU Morton surface extent must be a multiple of 8 pixels".into());
    }
    if extent.width > gpu.device.limits().max_texture_dimension_2d
        || extent.height > gpu.device.limits().max_texture_dimension_2d
    {
        return Err("host GPU surface exceeds the host GPU texture limit".into());
    }
    let pixel_count = (extent.width as usize)
        .checked_mul(extent.height as usize)
        .ok_or("host GPU surface pixel count overflow")?;
    let byte_len = pixel_count
        .checked_mul(key.format.bytes_per_pixel() as usize)
        .ok_or("host GPU surface byte count overflow")?;
    key.guest_address
        .checked_add(byte_len as u64)
        .ok_or("host GPU surface guest address overflow")?;
    let storage_limit = gpu.device.limits().max_storage_buffer_binding_size;
    if byte_len as u64 > storage_limit
        || (pixel_count as u64) * 8 > storage_limit
        || (pixel_count as u64) * 4 > storage_limit
    {
        return Err("host GPU surface exceeds the host GPU storage-buffer limit".into());
    }
    Ok((pixel_count, byte_len))
}

fn dispatch_2d(
    encoder: &mut wgpu::CommandEncoder,
    pipeline: &wgpu::ComputePipeline,
    bindings: &wgpu::BindGroup,
    extent: SurfaceExtent,
    label: &'static str,
) {
    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
        label: Some(label),
        timestamp_writes: None,
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, bindings, &[]);
    pass.dispatch_workgroups(extent.width.div_ceil(8), extent.height.div_ceil(8), 1);
}

#[cfg(test)]
fn byte_ranges(address: u64, written: &[bool]) -> Vec<(u64, usize)> {
    let mut ranges = Vec::new();
    let mut offset = 0;
    while offset < written.len() {
        if !written[offset] {
            offset += 1;
            continue;
        }
        let start = offset;
        while offset < written.len() && written[offset] {
            offset += 1;
        }
        ranges.push((address + start as u64, offset - start));
    }
    ranges
}

#[cfg(target_arch = "wasm32")]
#[derive(Default)]
struct MappingState {
    result: Option<Result<(), String>>,
    waker: Option<std::task::Waker>,
}

#[cfg(target_arch = "wasm32")]
async fn wait_for_mapping(buffer: &wgpu::Buffer) -> Result<(), String> {
    let state = Arc::new(Mutex::new(MappingState::default()));
    let callback_state = Arc::clone(&state);
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            let wake = callback_state.lock().ok().and_then(|mut state| {
                state.result = Some(result.map_err(|error| error.to_string()));
                state.waker.take()
            });
            if let Some(waker) = wake {
                waker.wake();
            }
        });
    std::future::poll_fn(move |context| {
        let Ok(mut state) = state.lock() else {
            return std::task::Poll::Ready(Err("host GPU mapping state lock poisoned".to_owned()));
        };
        if let Some(result) = state.result.take() {
            std::task::Poll::Ready(result)
        } else {
            state.waker = Some(context.waker().clone());
            std::task::Poll::Pending
        }
    })
    .await
}

#[cfg(not(target_arch = "wasm32"))]
struct ThreadWake(std::thread::Thread);

#[cfg(not(target_arch = "wasm32"))]
impl Wake for ThreadWake {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn block_on<F: Future>(future: F) -> F::Output {
    let waker = Arc::new(ThreadWake(std::thread::current())).into();
    let mut context = Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(value) => return value,
            Poll::Pending => std::thread::park(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXTENT: SurfaceExtent = SurfaceExtent {
        width: 16,
        height: 8,
    };

    fn decode_pixels_reference<const BYTES: usize>(
        key: SurfaceKey,
        pixels: &[u8],
        masks: &[u8],
    ) -> SurfaceReadback {
        let pixel_count = key.extent.width as usize * key.extent.height as usize;
        let mut bytes = vec![0; pixel_count * BYTES];
        let mut written_ranges = Vec::new();
        let mut open = None;
        let full = (1u32 << BYTES) - 1;
        let tiles_per_row = key.extent.width.div_ceil(TILE_SIDE) as usize;
        for (index, (pixel_word, pixel)) in pixels
            .as_chunks::<4>()
            .0
            .iter()
            .zip(bytes.as_chunks_mut::<BYTES>().0)
            .enumerate()
        {
            let pixel_word = u32::from_le_bytes(*pixel_word);
            pixel.copy_from_slice(&pixel_word.to_le_bytes()[..BYTES]);
            let (mask_word, lane) = match key.layout {
                SurfaceLayout::Linear => {
                    let width = key.extent.width as usize;
                    let x = index % width;
                    let y = index / width;
                    let tile = (y / TILE_SIDE as usize) * tiles_per_row + x / TILE_SIDE as usize;
                    let group = y % TILE_SIDE as usize;
                    let lane = x % TILE_SIDE as usize;
                    (tile * TILE_SIDE as usize + group, lane)
                }
                SurfaceLayout::Morton8x8 => {
                    let tile = index / 64;
                    let in_tile = index % 64;
                    (tile * 8 + in_tile / 8, in_tile % 8)
                }
            };
            let byte_offset = mask_word * 4;
            let packed_masks =
                u32::from_le_bytes(masks[byte_offset..byte_offset + 4].try_into().unwrap());
            let mask = (packed_masks >> (lane * 4)) & full;
            let offset = index * BYTES;
            if mask == full {
                open.get_or_insert(offset);
            } else if mask == 0 {
                if let Some(start) = open.take() {
                    written_ranges.push((key.guest_address + start as u64, offset - start));
                }
            } else {
                for lane in 0..BYTES {
                    let at = offset + lane;
                    if mask & (1 << lane) != 0 {
                        open.get_or_insert(at);
                    } else if let Some(start) = open.take() {
                        written_ranges.push((key.guest_address + start as u64, at - start));
                    }
                }
            }
        }
        if let Some(start) = open {
            written_ranges.push((key.guest_address + start as u64, bytes.len() - start));
        }
        SurfaceReadback {
            bytes,
            written_ranges,
        }
    }

    #[test]
    fn pixel_decode_matches_reference_and_reuses_output_for_masked_and_unmasked_surfaces() {
        fn check<const BYTES: usize>(
            layout: SurfaceLayout,
            extent: SurfaceExtent,
            mask_pattern: usize,
        ) {
            let key = SurfaceKey {
                guest_address: 0x1234,
                format: SurfaceFormat::Rgba8,
                extent,
                layout,
            };
            let tiles_per_row = key.extent.width.div_ceil(TILE_SIDE) as usize;
            let tiles_per_column = key.extent.height.div_ceil(TILE_SIDE) as usize;
            let mut pixels = Vec::new();
            let mut packed_masks = vec![0u32; tiles_per_row * tiles_per_column * 8];
            let mut expected_bytes = Vec::new();
            let mut flags = Vec::new();
            for index in 0..key.extent.width as usize * key.extent.height as usize {
                let pixel = (index as u32).wrapping_mul(0x1234567).to_le_bytes();
                let mask = match mask_pattern {
                    0 => 0,
                    1 => 15,
                    _ => match index % 64 {
                        0..=15 => 0,
                        16..=31 => 15,
                        _ => index as u32 & 15,
                    },
                };
                pixels.extend_from_slice(&pixel);
                expected_bytes.extend_from_slice(&pixel[..BYTES]);
                flags.extend((0..BYTES).map(|lane| mask & (1 << lane) != 0));
                let (tile, group, lane) = match layout {
                    SurfaceLayout::Linear => {
                        let width = key.extent.width as usize;
                        let x = index % width;
                        let y = index / width;
                        let tile = (y / 8) * tiles_per_row + x / 8;
                        (tile, y % 8, x % 8)
                    }
                    SurfaceLayout::Morton8x8 => {
                        let in_tile = index % 64;
                        let tile = index / 64;
                        (tile, in_tile / 8, in_tile % 8)
                    }
                };
                packed_masks[tile * 8 + group] |= (mask & 15) << (lane * 4);
            }
            let masks: Vec<u8> = packed_masks
                .into_iter()
                .flat_map(u32::to_le_bytes)
                .collect();
            let expected = decode_pixels_reference::<BYTES>(key, &pixels, &masks);
            let mut output = Vec::new();
            let written_ranges = decode_pixels::<BYTES>(key, &pixels, &masks, &mut output);
            let first_output = output.clone();
            let allocation = output.as_ptr();
            let reused_ranges = decode_pixels::<BYTES>(key, &pixels, &masks, &mut output);
            assert_eq!(output.as_ptr(), allocation);
            assert_eq!(output, expected_bytes);
            assert_eq!(output, expected.bytes);
            assert_eq!(first_output, expected.bytes);
            assert_eq!(reused_ranges, written_ranges);
            assert_eq!(written_ranges, expected.written_ranges);
            assert_eq!(written_ranges, byte_ranges(key.guest_address, &flags));
        }
        for (layout, extents) in [
            (
                SurfaceLayout::Linear,
                vec![
                    SurfaceExtent {
                        width: 13,
                        height: 10,
                    },
                    SurfaceExtent {
                        width: 16,
                        height: 64,
                    },
                ],
            ),
            (
                SurfaceLayout::Morton8x8,
                vec![SurfaceExtent {
                    width: 16,
                    height: 64,
                }],
            ),
        ] {
            for extent in extents {
                for mask_pattern in 0..3 {
                    check::<2>(layout, extent, mask_pattern);
                    check::<3>(layout, extent, mask_pattern);
                    check::<4>(layout, extent, mask_pattern);
                }
            }
        }
    }

    fn context_or_skip() -> Option<&'static GpuContext> {
        match context() {
            Ok(context) => Some(context),
            Err(error) if error.starts_with("host GPU adapter:") => {
                eprintln!(
                    "skipping host GPU surface test because no host GPU adapter is available: {error}"
                );
                None
            }
            Err(error) => panic!("host GPU surface setup failed: {error}"),
        }
    }

    #[test]
    fn guest_upload_and_readback_are_exact_for_every_surface_format() {
        let Some(context) = context_or_skip() else {
            return;
        };
        let formats = [
            SurfaceFormat::Rgba8,
            SurfaceFormat::Rgb565,
            SurfaceFormat::Rgb5A1,
            SurfaceFormat::Rgba4,
            SurfaceFormat::D16,
            SurfaceFormat::D24,
            SurfaceFormat::D24S8,
        ];
        for (index, format) in formats.into_iter().enumerate() {
            let key = SurfaceKey {
                guest_address: 0x100000 + index as u64 * 0x1000,
                format,
                extent: EXTENT,
                layout: SurfaceLayout::Morton8x8,
            };
            let byte_len =
                EXTENT.width as usize * EXTENT.height as usize * format.bytes_per_pixel() as usize;
            let guest_bytes: Vec<u8> = (0..byte_len)
                .map(|offset| (offset.wrapping_mul(37).wrapping_add(index * 11)) as u8)
                .collect();
            context
                .get_or_create_target(key, Some(&guest_bytes))
                .unwrap();
            let readback = context.readback(key).unwrap();
            assert_eq!(readback.bytes, guest_bytes, "{format:?}");
            assert!(readback.written_ranges.is_empty(), "{format:?}");
        }
    }

    #[test]
    fn write_mask_reports_exact_morton_guest_byte_ranges() {
        let Some(context) = context_or_skip() else {
            return;
        };
        let format = SurfaceFormat::D24S8;
        let key = SurfaceKey {
            guest_address: 0x124000,
            format,
            extent: EXTENT,
            layout: SurfaceLayout::Morton8x8,
        };
        let guest_bytes = vec![0xA5; EXTENT.width as usize * EXTENT.height as usize * 4];
        context
            .get_or_create_target(key, Some(&guest_bytes))
            .unwrap();
        context
            .record_write_masks(
                key,
                &[
                    PixelWriteMask {
                        x: 0,
                        y: 0,
                        byte_mask: 0b1111,
                    },
                    PixelWriteMask {
                        x: 1,
                        y: 0,
                        byte_mask: 0b0101,
                    },
                    PixelWriteMask {
                        x: 0,
                        y: 1,
                        byte_mask: 0b1111,
                    },
                ],
            )
            .unwrap();
        let readback = context.readback(key).unwrap();
        assert_eq!(readback.bytes, guest_bytes);
        assert_eq!(
            readback.written_ranges,
            vec![
                (key.guest_address, 5),
                (key.guest_address + 6, 1),
                (key.guest_address + 8, 4)
            ]
        );
    }

    #[test]
    fn encoded_surface_upload_and_batched_readback_preserve_write_masks() {
        let Some(context) = context_or_skip() else {
            return;
        };
        let keys = [0x134000, 0x135000].map(|guest_address| SurfaceKey {
            guest_address,
            format: SurfaceFormat::Rgba8,
            extent: EXTENT,
            layout: SurfaceLayout::Morton8x8,
        });
        let guest_bytes = keys.map(|key| {
            (0..key.extent.width as usize * key.extent.height as usize * 4)
                .map(|offset| (offset.wrapping_mul(17) + key.guest_address as usize) as u8)
                .collect::<Vec<_>>()
        });

        let mut encoder = context
            .device()
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        for (key, bytes) in keys.iter().copied().zip(&guest_bytes) {
            context
                .get_or_create_target_encoded(&mut encoder, key, Some(bytes))
                .unwrap();
            context.encode_clear_write_masks(&mut encoder, key).unwrap();
        }
        let pending = context.encode_readbacks(&mut encoder, &keys).unwrap();
        context.submit(encoder.finish());
        let initial = context.finish_readbacks(pending).unwrap();
        for ((key, bytes), (actual_key, readback)) in keys.iter().zip(&guest_bytes).zip(initial) {
            assert_eq!(actual_key, *key);
            assert_eq!(readback.bytes, *bytes);
            assert!(readback.written_ranges.is_empty());
        }

        let abandoned_key = SurfaceKey {
            guest_address: 0x136000,
            ..keys[0]
        };
        let mut abandoned = context
            .device()
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        context
            .get_or_create_target_encoded(&mut abandoned, abandoned_key, Some(&guest_bytes[0]))
            .unwrap();
        drop(abandoned);
        let mut encoder = context
            .device()
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        context
            .get_or_create_target_encoded(&mut encoder, abandoned_key, Some(&guest_bytes[0]))
            .unwrap();
        let pending = context
            .encode_readbacks(&mut encoder, &[abandoned_key])
            .unwrap();
        context.submit(encoder.finish());
        let retried = context.finish_readbacks(pending).unwrap();
        assert_eq!(retried[0].1.bytes, guest_bytes[0]);

        context
            .record_write_masks(
                keys[0],
                &[
                    PixelWriteMask {
                        x: 0,
                        y: 0,
                        byte_mask: 0b0101,
                    },
                    PixelWriteMask {
                        x: 1,
                        y: 0,
                        byte_mask: 0b1010,
                    },
                ],
            )
            .unwrap();
        let mut encoder = context
            .device()
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        let pending = context.encode_readbacks(&mut encoder, &keys).unwrap();
        context.submit(encoder.finish());
        let updated = context.finish_readbacks(pending).unwrap();
        assert_eq!(updated[0].1.bytes, guest_bytes[0]);
        assert_eq!(
            updated[0].1.written_ranges,
            vec![
                (keys[0].guest_address, 1),
                (keys[0].guest_address + 2, 1),
                (keys[0].guest_address + 5, 1),
                (keys[0].guest_address + 7, 1),
            ]
        );
        assert_eq!(updated[1].1.bytes, guest_bytes[1]);
        assert!(updated[1].1.written_ranges.is_empty());
    }

    #[test]
    fn resident_surface_reuses_unchanged_guest_bytes_and_uploads_changes() {
        let Some(context) = context_or_skip() else {
            return;
        };
        let key = SurfaceKey {
            guest_address: 0x128000,
            format: SurfaceFormat::Rgba8,
            extent: EXTENT,
            layout: SurfaceLayout::Morton8x8,
        };
        let original = vec![0x35; EXTENT.width as usize * EXTENT.height as usize * 4];
        let first = context.get_or_create_target(key, Some(&original)).unwrap();
        let same = context.get_or_create_target(key, Some(&original)).unwrap();
        assert!(Arc::ptr_eq(&first, &same));

        let mut changed = original.clone();
        changed[17] ^= 0xff;
        let resident = context.get_or_create_target(key, Some(&changed)).unwrap();
        assert!(Arc::ptr_eq(&first, &resident));
        assert_eq!(context.readback(key).unwrap().bytes, changed);
        assert_eq!(context.readback(key).unwrap().bytes, changed);
        assert_eq!(
            first
                .readback_buffers
                .lock()
                .unwrap()
                .iter()
                .filter(|slot| slot.is_some())
                .count(),
            1
        );
    }

    #[test]
    fn async_surface_readback_preserves_guest_bytes() {
        let Some(context) = context_or_skip() else {
            return;
        };
        let key = SurfaceKey {
            guest_address: 0x12a000,
            format: SurfaceFormat::Rgba8,
            extent: EXTENT,
            layout: SurfaceLayout::Morton8x8,
        };
        let guest_bytes: Vec<u8> = (0..EXTENT.width as usize * EXTENT.height as usize * 4)
            .map(|offset| (offset.wrapping_mul(23) + 9) as u8)
            .collect();
        context
            .get_or_create_target(key, Some(&guest_bytes))
            .unwrap();
        assert_eq!(
            block_on(context.readback_async(key)).unwrap().bytes,
            guest_bytes
        );
    }

    #[test]
    fn linear_surface_round_trip_preserves_row_major_bytes() {
        let Some(context) = context_or_skip() else {
            return;
        };
        let extent = SurfaceExtent {
            width: 10,
            height: 7,
        };
        let key = SurfaceKey {
            guest_address: 0x12c000,
            format: SurfaceFormat::Rgba8,
            extent,
            layout: SurfaceLayout::Linear,
        };
        let guest_bytes: Vec<u8> = (0..extent.width as usize * extent.height as usize * 4)
            .map(|offset| (offset.wrapping_mul(29) + 7) as u8)
            .collect();
        context
            .get_or_create_target(key, Some(&guest_bytes))
            .unwrap();
        assert_eq!(context.readback(key).unwrap().bytes, guest_bytes);
    }

    #[test]
    fn submitted_gpu_work_updates_resident_bytes_and_write_ranges() {
        let Some(context) = context_or_skip() else {
            return;
        };
        let extent = SurfaceExtent {
            width: 10,
            height: 7,
        };
        let key = SurfaceKey {
            guest_address: 0x130000,
            format: SurfaceFormat::Rgba8,
            extent,
            layout: SurfaceLayout::Linear,
        };
        let surface = context
            .get_or_create_target(
                key,
                Some(&vec![0; extent.width as usize * extent.height as usize * 4]),
            )
            .unwrap();
        context.clear_write_masks(key).unwrap();
        let shader = context
            .device()
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("resident target write test"),
                source: wgpu::ShaderSource::Wgsl(
                    r#"
                    @group(0) @binding(0) var surface: texture_storage_2d<r32uint, write>;
                    @group(0) @binding(1) var<storage, read_write> masks: array<atomic<u32>>;
                    @compute @workgroup_size(1)
                    fn main() {
                        textureStore(surface, vec2<i32>(3, 4), vec4<u32>(0x44332211u, 0u, 0u, 0u));
                        atomicOr(&masks[43u], 5u);
                    }
                    "#
                    .into(),
                ),
            });
        let pipeline = context
            .device()
            .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("resident target write test"),
                layout: None,
                module: &shader,
                entry_point: Some("main"),
                compilation_options: Default::default(),
                cache: None,
            });
        let bindings = context
            .device()
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("resident target write test"),
                layout: &pipeline.get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(
                            &surface.texture().create_view(&Default::default()),
                        ),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: surface.write_mask_accumulator().as_entire_binding(),
                    },
                ],
            });
        let mut encoder = context.device().create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bindings, &[]);
            pass.dispatch_workgroups(1, 1, 1);
        }
        context.submit(encoder.finish());

        let readback = context.readback(key).unwrap();
        let pixel_offset = (4 * extent.width as usize + 3) * 4;
        assert_eq!(
            &readback.bytes[pixel_offset..pixel_offset + 4],
            &[0x11, 0x22, 0x33, 0x44]
        );
        assert_eq!(
            readback.written_ranges,
            vec![
                (key.guest_address + pixel_offset as u64, 1),
                (key.guest_address + pixel_offset as u64 + 2, 1)
            ]
        );
    }
}
