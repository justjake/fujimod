//! The image wires and inputs EXR II image models share. They are the
//! resources' image part: models that read images, sensors or ISP state reach
//! it through [`images`], [`wire`] and [`image_wire!`]; sessions and every
//! other crate see it only as `ImageResources`.
use fujimod_emulator_devices::checkpoint::Checkpoint;
use fujimod_emulator_exr2_resources::{
    Edge, ImageResources, ImageSnapshot, Resources, SensorFrame, SensorFrameCodec, SensorRecord,
    SensorSnapshot, SensorState,
};
use fujimod_emulator_imaging::exr2_isp as isp;
use fujimod_emulator_imaging::scene::{
    Frame, FrameEdge, FramePixels, ImageFrames, SensorFrames, SensorFramesHandle,
    StatisticsFrames, frames::sensor_decoders,
};
use std::{
    any::Any,
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

mod checkpoint;
mod preparation;
#[cfg(test)]
mod preparation_tests;
#[cfg(test)]
mod tests;

/// A shared image wire, declared once by the plan with one concrete type.
pub enum ImageWire {
    FramePeriod(fujimod_emulator_imaging::timing::FramePeriod),
    ImageGrid(fujimod_emulator_imaging::analysis::GridHandle),
    Exposure(fujimod_emulator_imaging::exposure::ExposureHandle),
    Parameters(isp::parameters::ParameterHandle),
    MemoryOutputs(isp::memory_outputs::MemoryOutputHandle),
    Stream(isp::stream::StreamHandle),
    OutputChannels(isp::output_channels::OutputChannels),
    PhaseEdges(fujimod_emulator_imaging::scene::FramePhases),
}
impl ImageWire {
    pub fn fingerprint(&self) -> Result<serde_json::Value, String> {
        use serde_json::json;
        Ok(match self {
            Self::FramePeriod(v) => json!(*v.lock().map_err(|_| "frame clock poisoned")?),
            Self::ImageGrid(v) => json!(*v.lock().map_err(|_| "image grid poisoned")?),
            Self::Exposure(v) => json!(*v.lock().map_err(|_| "exposure poisoned")?),
            Self::Parameters(v) => json!(*v.lock().map_err(|_| "parameters poisoned")?),
            Self::MemoryOutputs(v) => v
                .lock()
                .map_err(|_| "memory outputs poisoned")?
                .fingerprint(),
            Self::Stream(v) => json!(*v.lock().map_err(|_| "stream poisoned")?),
            Self::OutputChannels(v) => v.fingerprint(),
            Self::PhaseEdges(v) => json!(*v.lock().map_err(|_| "phase edges poisoned")?),
        })
    }
    pub fn exposure() -> Self {
        Self::Exposure(Default::default())
    }
    pub fn frame_period() -> Self {
        Self::FramePeriod(Default::default())
    }
    pub fn image_grid() -> Self {
        Self::ImageGrid(Arc::new(Mutex::new([1, 1])))
    }
    pub fn parameters() -> Self {
        Self::Parameters(Arc::new(Mutex::new(Default::default())))
    }
    pub fn memory_outputs() -> Self {
        Self::MemoryOutputs(Arc::new(Mutex::new(Default::default())))
    }
    pub fn stream() -> Self {
        Self::Stream(Arc::new(Mutex::new(Default::default())))
    }
    pub fn output_channels() -> Self {
        Self::OutputChannels(Default::default())
    }
    pub fn phase_edges() -> Self {
        Self::PhaseEdges(Arc::new(Mutex::new([None; 2])))
    }
}

/// The image part: image wires and prepared image, statistics and sensor
/// inputs. Values contain bytes or physical input handles only.
#[derive(Default)]
pub struct Images {
    pub wires: BTreeMap<String, ImageWire>,
    pub images: BTreeMap<String, ImageFrames>,
    pub statistics: BTreeMap<String, StatisticsFrames>,
    pub sensors: BTreeMap<String, SensorFramesHandle>,
}

/// The board's image part.
pub fn images(resources: &Resources) -> Result<&Images, String> {
    resources
        .images()?
        .as_any()
        .downcast_ref()
        .ok_or_else(|| "board image part is not the EXR II image part".into())
}

/// The board's image part, installing an empty one first if it has none.
pub fn images_mut(resources: &mut Resources) -> Result<&mut Images, String> {
    resources
        .images
        .get_or_insert_with(|| Box::new(Images::default()))
        .as_any_mut()
        .downcast_mut()
        .ok_or_else(|| "board image part is not the EXR II image part".into())
}

/// The named image wire, or `None` for a device wire, recorded as used by node `node`.
pub fn wire<'a>(
    resources: &'a mut Resources,
    name: &str,
    node: usize,
) -> Result<Option<&'a ImageWire>, String> {
    resources.connect(name, node)?;
    Ok(images(resources)?.wires.get(name))
}

/// The named image input, recorded as used by node `node`.
pub fn image_input(resources: &mut Resources, name: &str, node: usize) -> Result<ImageFrames, String> {
    let image = images(resources)?
        .images
        .get(name)
        .cloned()
        .ok_or_else(|| format!("missing external image input {name}"))?;
    resources
        .image_users
        .entry(name.to_owned())
        .or_default()
        .insert(node);
    Ok(image)
}

/// The named image wire of the given kind, recorded as used by node `$id`.
#[macro_export]
macro_rules! image_wire {
    ($resources:expr, $id:expr, $name:expr, $kind:ident) => {
        match $crate::wire($resources, &$name, $id)? {
            Some($crate::ImageWire::$kind(value)) => value.clone(),
            _ => return Err(format!("{} requires a {} wire", $name, stringify!($kind))),
        }
    };
}

/// Install this part's preparation of run image inputs.
pub fn install_preparation() {
    fujimod_emulator_exr2_resources::install_image_preparation(preparation::prepare);
}

/// Install the codec input logs use for this part's sensor frames.
pub fn install_sensor_frame_codec() {
    fujimod_emulator_exr2_resources::install_sensor_frame_codec(SensorFrameCodec {
        encode: |frame| {
            frame
                .frame
                .clone()
                .downcast::<Frame>()
                .map_err(|_| "sensor frame is not an EXR II image frame")?
                .checkpoint()?
                .to_bytes()
        },
        decode: |bytes| {
            Ok(sensor_frame(Arc::new(Frame::from_checkpoint(
                &Checkpoint::from_bytes(bytes)?,
            )?)))
        },
    });
}

fn frame_bytes(frame: &Frame) -> usize {
    std::mem::size_of::<Frame>().saturating_add(match &frame.pixels {
        FramePixels::Rgb24(bytes) => bytes.len(),
        FramePixels::Raw16(words) => words.len() * 2,
        FramePixels::Mosaic { radiance, .. } => radiance.bytes().len(),
    })
}
fn sensor_frame(frame: Arc<Frame>) -> SensorFrame {
    SensorFrame {
        bytes: frame_bytes(&frame),
        frame,
    }
}

impl Images {
    fn image(&self, name: &str) -> Result<&ImageFrames, String> {
        self.images.get(name).ok_or_else(|| "no such image input".into())
    }
    fn sensor(&self, name: &str) -> Result<&SensorFramesHandle, String> {
        self.sensors.get(name).ok_or_else(|| "no such sensor input".into())
    }
}

impl ImageResources for Images {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    fn has_wire(&self, name: &str) -> bool {
        self.wires.contains_key(name)
    }
    fn has_image(&self, name: &str) -> bool {
        self.images.contains_key(name)
    }
    fn has_sensor(&self, name: &str) -> bool {
        self.sensors.contains_key(name)
    }
    fn wire_fingerprints(&self) -> Result<BTreeMap<String, serde_json::Value>, String> {
        self.wires
            .iter()
            .map(|(name, v)| Ok((name.clone(), v.fingerprint()?)))
            .collect()
    }
    fn image_fingerprints(&self) -> Result<BTreeMap<String, serde_json::Value>, String> {
        Ok(self
            .images
            .iter()
            .map(|(k, v)| (k.clone(), v.fingerprint()))
            .collect())
    }
    fn statistics_fingerprints(&self) -> Result<BTreeMap<String, serde_json::Value>, String> {
        Ok(self
            .statistics
            .iter()
            .map(|(k, v)| (k.clone(), v.fingerprint()))
            .collect())
    }
    fn sensor_fingerprints(&self) -> Result<BTreeMap<String, serde_json::Value>, String> {
        self.sensors
            .iter()
            .map(|(name, v)| {
                Ok((
                    name.clone(),
                    v.lock().map_err(|_| "sensor input poisoned")?.fingerprint(),
                ))
            })
            .collect()
    }

    fn prepare_rgb(&self, name: &str, width: usize, height: usize, bytes: &[u8]) -> Result<(), String> {
        self.image(name)?.prepare_rgb(width, height, bytes)
    }
    fn use_analytic(&self, name: &str) -> Result<(), String> {
        self.image(name)?.use_analytic();
        Ok(())
    }
    fn use_sensor(&self, name: &str) -> Result<(), String> {
        self.image(name)?.use_sensor();
        Ok(())
    }
    fn sensor_rgb(
        &self,
        name: &str,
        width: usize,
        height: usize,
        bytes: &[u8],
        white_luminance: f64,
    ) -> Result<(), String> {
        if !white_luminance.is_finite() || white_luminance <= 0. {
            return Err("scene white luminance must be positive finite cd/m²".into());
        }
        let input = self.sensor(name)?;
        if bytes.len() > 64 << 20 {
            return Err("sensor RGB input exceeds 64 MiB".into());
        }
        let mut frame = Frame::new(width, height, FramePixels::Rgb24(bytes.to_vec().into()))?;
        frame.white_luminance = white_luminance;
        input
            .lock()
            .map_err(|_| "sensor input is poisoned")?
            .set_frame(frame);
        Ok(())
    }
    fn sensor_raw16(&self, name: &str, width: usize, height: usize, bytes: &[u8]) -> Result<(), String> {
        let input = self.sensor(name)?;
        let expected = width
            .checked_mul(height)
            .and_then(|samples| samples.checked_mul(2))
            .ok_or("sensor RAW input size overflow")?;
        if bytes.len() > 64 << 20 || bytes.len() != expected {
            return Err("sensor RAW input must contain exactly width * height U16 samples".into());
        }
        let samples = bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        let frame = Frame::new(width, height, FramePixels::Raw16(samples))?;
        input
            .lock()
            .map_err(|_| "sensor input is poisoned")?
            .set_frame(frame);
        Ok(())
    }
    fn decode_raf(&self, bytes: &[u8]) -> Result<SensorFrame, String> {
        let read = sensor_decoders()
            .ok_or("RAF sensor input needs the image decoders")?
            .raf;
        Ok(sensor_frame(Arc::new(read(bytes)?)))
    }
    fn sensor_frame(&self, name: &str, frame: &SensorFrame) -> Result<(), String> {
        let frame = frame
            .frame
            .clone()
            .downcast::<Frame>()
            .map_err(|_| "sensor frame is not an EXR II image frame")?;
        self.sensor(name)?
            .lock()
            .map_err(|_| "sensor input is poisoned")?
            .current = frame;
        Ok(())
    }
    fn mosaic_frames(&self) -> Result<Vec<(String, SensorFrame)>, String> {
        let mut frames = vec![];
        for (name, input) in &self.sensors {
            let frame = input
                .lock()
                .map_err(|_| "sensor input is poisoned")?
                .current
                .clone();
            if matches!(frame.pixels, FramePixels::Mosaic { .. }) {
                frames.push((name.clone(), sensor_frame(frame)));
            }
        }
        Ok(frames)
    }

    fn checkpoint(&self) -> Result<ImageSnapshot, String> {
        checkpoint::checkpoint(self)
    }
    fn validate_checkpoint(&self, saved: &ImageSnapshot) -> Result<(), String> {
        checkpoint::validate(self, saved)
    }
    fn restore_checkpoint(&self, saved: &ImageSnapshot) -> Result<(), String> {
        checkpoint::restore(self, saved)
    }
    fn import_sensor(
        &self,
        record: &SensorRecord,
        frames: &mut dyn FnMut() -> Result<Checkpoint, String>,
    ) -> Result<SensorSnapshot, String> {
        let current = Arc::new(Frame::from_checkpoint(&frames()?)?);
        let latched = if record.shared {
            current.clone()
        } else {
            Arc::new(Frame::from_checkpoint(&frames()?)?)
        };
        Ok(SensorSnapshot(Arc::new(Sensor(SensorFrames {
            current,
            latched,
            view: record
                .view
                .map(|(field, rectangle)| {
                    fujimod_emulator_imaging::scene::view::View::new(field, rectangle)
                })
                .transpose()?,
            edge: record.edge.map(|edge| FrameEdge {
                index: edge.index,
                tick: edge.tick,
            }),
        }))))
    }
}

/// A sensor input's captured frames.
struct Sensor(SensorFrames);
impl SensorState for Sensor {
    fn view(&self) -> Option<([usize; 2], [usize; 4])> {
        self.0.view.map(|v| (v.field, v.rectangle))
    }
    fn edge(&self) -> Option<Edge> {
        self.0.edge.map(|edge| Edge {
            index: edge.index,
            tick: edge.tick,
        })
    }
    fn shared(&self) -> bool {
        Arc::ptr_eq(&self.0.current, &self.0.latched)
    }
    fn frames(&self) -> Result<Vec<Checkpoint>, String> {
        let mut frames = vec![self.0.current.checkpoint()?];
        if !self.shared() {
            frames.push(self.0.latched.checkpoint()?);
        }
        Ok(frames)
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}
