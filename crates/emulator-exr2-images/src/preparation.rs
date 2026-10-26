//! A run's declared image, statistics and sensor inputs, built from scene assets.
use super::*;
use fujimod_emulator_exr2_resources::ImageSpec;
use fujimod_emulator_imaging::{analysis::ImageLayout, scene::StatisticsSpan};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RgbSource {
    width: usize,
    height: usize,
    asset: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Image {
    width: usize,
    height: usize,
    stride: usize,
    asset: String,
    rgb_source: Option<RgbSource>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Span {
    destination: usize,
    offset: u32,
    asset: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Pixels {
    Rgb24,
    Raw16,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Sensor {
    width: usize,
    height: usize,
    pixels: Pixels,
    asset: String,
}
fn decode<T: serde::de::DeserializeOwned>(value: serde_json::Value) -> Result<T, String> {
    serde_json::from_value(value).map_err(|e| format!("scene input: {e}"))
}

/// Build the image part of a run's resources.
pub(crate) fn prepare(
    spec: ImageSpec,
    assets: &BTreeMap<String, Vec<u8>>,
) -> Result<Box<dyn ImageResources>, String> {
    let asset = |name: &str| {
        assets
            .get(name)
            .ok_or_else(|| format!("missing scene asset {name}"))
    };
    let mut images = Images::default();
    for (name, image) in spec.images {
        let image: Image = decode(image)?;
        let layout = ImageLayout {
            width: image.width,
            height: image.height,
            stride: image.stride,
        };
        let bytes = asset(&image.asset)?;
        if bytes.len() != layout.bytes()? {
            return Err("image asset extent does not match its declared raster".into());
        }
        let frames = ImageFrames::from_pixels(layout, bytes.clone())?;
        if let Some(source) = image.rgb_source {
            frames.prepare_rgb(source.width, source.height, asset(&source.asset)?)?;
        }
        images.images.insert(name, frames);
    }
    for (name, spans) in spec.statistics {
        let spans: Vec<Span> = decode(spans)?;
        if spans.len() > 256 {
            return Err("statistics input has too many spans".into());
        }
        let mut total = 0usize;
        let spans = spans
            .into_iter()
            .map(|s| {
                let bytes = asset(&s.asset)?;
                total = total.saturating_add(bytes.len());
                if total > 64 << 20 {
                    return Err("statistics input exceeds 64 MiB".into());
                }
                Ok(StatisticsSpan {
                    destination: s.destination,
                    offset: s.offset,
                    bytes: bytes.clone(),
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        images
            .statistics
            .insert(name, StatisticsFrames::new(spans)?);
    }
    for (name, sensor) in spec.sensors {
        let sensor: Sensor = decode(sensor)?;
        let bytes = asset(&sensor.asset)?;
        let pixels = match sensor.pixels {
            Pixels::Rgb24 => FramePixels::Rgb24(bytes.clone().into()),
            Pixels::Raw16 => {
                if bytes.len() % 2 != 0 {
                    return Err("RAW16 scene asset has an incomplete sample".into());
                }
                FramePixels::Raw16(
                    bytes
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|b| u16::from_le_bytes(*b))
                        .collect(),
                )
            }
        };
        images.sensors.insert(
            name,
            Arc::new(Mutex::new(SensorFrames::new(Frame::new(
                sensor.width,
                sensor.height,
                pixels,
            )?))),
        );
    }
    Ok(Box::new(images))
}
