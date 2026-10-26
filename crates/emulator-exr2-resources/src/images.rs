//! The image part of the resources: image wires, image and statistics
//! inputs and sensor scenes. Its implementation names the image interface
//! types; this crate, the sessions and the models that use no image type
//! reach it only through [`ImageResources`].
use fujimod_emulator_devices::checkpoint::Checkpoint;
use serde::{Deserialize, Serialize};
use std::{
    any::Any,
    collections::BTreeMap,
    sync::{Arc, OnceLock},
};

/// A sensor input's latched frame edge, as checkpoints record it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edge {
    pub index: u64,
    pub tick: u64,
}

/// A decoded sensor frame a session can install again without its source file.
/// Input logs carry it in the image part's encoding.
#[derive(Clone)]
pub struct SensorFrame {
    /// The image part's frame.
    pub frame: Arc<dyn Any + Send + Sync>,
    /// Host bytes the frame retains.
    pub bytes: usize,
}

/// How the image part encodes sensor frames in input logs.
pub struct SensorFrameCodec {
    pub encode: fn(&SensorFrame) -> Result<Vec<u8>, String>,
    pub decode: fn(&[u8]) -> Result<SensorFrame, String>,
}
static SENSOR_FRAME_CODEC: OnceLock<SensorFrameCodec> = OnceLock::new();

/// Install the image part's sensor frame codec; there is one per process.
pub fn install_sensor_frame_codec(codec: SensorFrameCodec) {
    let _ = SENSOR_FRAME_CODEC.set(codec);
}
fn sensor_frame_codec() -> Result<&'static SensorFrameCodec, String> {
    SENSOR_FRAME_CODEC
        .get()
        .ok_or_else(|| "sensor frames need the image part's codec".into())
}
impl Serialize for SensorFrame {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let codec = sensor_frame_codec().map_err(serde::ser::Error::custom)?;
        let bytes = (codec.encode)(self).map_err(serde::ser::Error::custom)?;
        serializer.serialize_bytes(&bytes)
    }
}
impl<'de> Deserialize<'de> for SensorFrame {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Bytes;
        impl<'de> serde::de::Visitor<'de> for Bytes {
            type Value = Vec<u8>;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("an encoded sensor frame")
            }
            fn visit_bytes<E: serde::de::Error>(self, bytes: &[u8]) -> Result<Vec<u8>, E> {
                Ok(bytes.to_vec())
            }
            fn visit_byte_buf<E: serde::de::Error>(self, bytes: Vec<u8>) -> Result<Vec<u8>, E> {
                Ok(bytes)
            }
        }
        let bytes = deserializer.deserialize_bytes(Bytes)?;
        let codec = sensor_frame_codec().map_err(serde::de::Error::custom)?;
        (codec.decode)(&bytes).map_err(serde::de::Error::custom)
    }
}

/// One sensor input's captured frames.
pub trait SensorState: Send + Sync {
    fn edge(&self) -> Option<Edge>;
    fn view(&self) -> Option<([usize; 2], [usize; 4])>;
    /// Whether the current and latched frames are one frame.
    fn shared(&self) -> bool;
    /// The current frame and, unless shared, the latched frame.
    fn frames(&self) -> Result<Vec<Checkpoint>, String>;
    fn as_any(&self) -> &dyn Any;
}
#[derive(Clone)]
pub struct SensorSnapshot(pub Arc<dyn SensorState>);

/// A sensor entry of an exported resource snapshot.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SensorRecord {
    pub name: String,
    pub edge: Option<Edge>,
    pub view: Option<([usize; 2], [usize; 4])>,
    pub shared: bool,
}

/// The image part's captured state.
#[derive(Clone, Default)]
pub struct ImageSnapshot {
    pub wires: BTreeMap<String, Checkpoint>,
    pub images: BTreeMap<String, Checkpoint>,
    pub statistics: BTreeMap<String, Checkpoint>,
    pub sensors: BTreeMap<String, SensorSnapshot>,
}

/// The image wires and inputs of a board.
pub trait ImageResources: Send + Sync {
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;

    fn has_wire(&self, name: &str) -> bool;
    fn has_image(&self, name: &str) -> bool;
    fn has_sensor(&self, name: &str) -> bool;
    fn wire_fingerprints(&self) -> Result<BTreeMap<String, serde_json::Value>, String>;
    fn image_fingerprints(&self) -> Result<BTreeMap<String, serde_json::Value>, String>;
    fn statistics_fingerprints(&self) -> Result<BTreeMap<String, serde_json::Value>, String>;
    fn sensor_fingerprints(&self) -> Result<BTreeMap<String, serde_json::Value>, String>;

    /// Present caller RGB pixels on an image input.
    fn prepare_rgb(&self, name: &str, width: usize, height: usize, bytes: &[u8])
    -> Result<(), String>;
    /// Select an image input's analytic scene.
    fn use_analytic(&self, name: &str) -> Result<(), String>;
    /// Select an image input's sensor scene.
    fn use_sensor(&self, name: &str) -> Result<(), String>;
    /// Replace a sensor input with caller RGB pixels at the given scene white.
    fn sensor_rgb(
        &self,
        name: &str,
        width: usize,
        height: usize,
        bytes: &[u8],
        white_luminance: f64,
    ) -> Result<(), String>;
    /// Replace a sensor input with a little-endian U16 RAW raster.
    fn sensor_raw16(&self, name: &str, width: usize, height: usize, bytes: &[u8])
    -> Result<(), String>;
    /// Decode a RAF's visible crop as a sensor frame.
    fn decode_raf(&self, bytes: &[u8]) -> Result<SensorFrame, String>;
    /// Install a decoded frame on a sensor input.
    fn sensor_frame(&self, name: &str, frame: &SensorFrame) -> Result<(), String>;
    /// The current frames of sensor inputs that hold a decoded RAF mosaic.
    fn mosaic_frames(&self) -> Result<Vec<(String, SensorFrame)>, String>;

    fn checkpoint(&self) -> Result<ImageSnapshot, String>;
    /// Check a snapshot against this part's names and kinds without changing it.
    fn validate_checkpoint(&self, saved: &ImageSnapshot) -> Result<(), String>;
    fn restore_checkpoint(&self, saved: &ImageSnapshot) -> Result<(), String>;
    /// Decode one exported sensor entry, taking its frames in order.
    fn import_sensor(
        &self,
        record: &SensorRecord,
        frames: &mut dyn FnMut() -> Result<Checkpoint, String>,
    ) -> Result<SensorSnapshot, String>;
}

/// A run's declared image, statistics and sensor input documents.
#[derive(Default)]
pub struct ImageSpec {
    pub images: BTreeMap<String, serde_json::Value>,
    pub statistics: BTreeMap<String, serde_json::Value>,
    pub sensors: BTreeMap<String, serde_json::Value>,
}
impl ImageSpec {
    pub fn is_empty(&self) -> bool {
        self.images.is_empty() && self.statistics.is_empty() && self.sensors.is_empty()
    }
}

/// Builds the image part from a run's image inputs and scene assets.
pub type ImagePreparation =
    fn(ImageSpec, &BTreeMap<String, Vec<u8>>) -> Result<Box<dyn ImageResources>, String>;
static IMAGE_PREPARATION: OnceLock<ImagePreparation> = OnceLock::new();

/// Install the image part's preparation; there is one per process.
pub fn install_image_preparation(preparation: ImagePreparation) {
    let _ = IMAGE_PREPARATION.set(preparation);
}

/// Build the image part of a run with the installed preparation.
pub fn prepare_images(
    spec: ImageSpec,
    assets: &BTreeMap<String, Vec<u8>>,
) -> Result<Box<dyn ImageResources>, String> {
    let prepare = IMAGE_PREPARATION
        .get()
        .ok_or("image inputs need the image part's preparation")?;
    prepare(spec, assets)
}
