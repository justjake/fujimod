//! A resource archive includes external pixels and media. Shared wires are
//! rebound by the receiving graph's existing checkpoint validator/restorer.
use super::*;
use serde::{Deserialize, Serialize};

const KIND: &str = "session-resources-v1";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Metadata {
    wires: Vec<String>,
    images: Vec<String>,
    statistics: Vec<String>,
    nands: Vec<String>,
    sensors: Vec<SensorRecord>,
}

impl ResourceSnapshot {
    /// Encode captured values only, including both current and latched sensor
    /// inputs. The live graph is never inspected or advanced by this export.
    pub fn export(&self) -> Result<Checkpoint, String> {
        let wires: BTreeMap<&String, &Checkpoint> =
            self.wires.iter().chain(&self.images.wires).collect();
        let metadata = Metadata {
            wires: wires.keys().map(|name| (*name).clone()).collect(),
            images: self.images.images.keys().cloned().collect(),
            statistics: self.images.statistics.keys().cloned().collect(),
            nands: self.nands.keys().cloned().collect(),
            sensors: self
                .images
                .sensors
                .iter()
                .map(|(name, frames)| SensorRecord {
                    name: name.clone(),
                    edge: frames.0.edge(),
                    view: frames.0.view(),
                    shared: frames.0.shared(),
                })
                .collect(),
        };
        let mut children: Vec<_> = wires
            .values()
            .copied()
            .chain(self.images.images.values())
            .chain(self.images.statistics.values())
            .chain(self.nands.values())
            .chain(std::iter::once(&self.media))
            .cloned()
            .collect();
        for frames in self.images.sensors.values() {
            children.extend(frames.0.frames()?);
        }
        Ok(Checkpoint::encode(KIND, &metadata)?.with_children(children))
    }
}

impl Resources {
    /// Decode detached data for this graph. Call `validate_checkpoint` before
    /// installing any decoded resource values.
    pub fn import_checkpoint(&self, encoded: &Checkpoint) -> Result<ResourceSnapshot, String> {
        let metadata: Metadata = encoded.decode(KIND)?;
        if !encoded.blobs().is_empty() {
            return Err("resource snapshot has unexpected blobs".into());
        }
        let ordered = |names: Vec<&str>| names.windows(2).all(|v| v[0] < v[1]);
        if [
            &metadata.wires,
            &metadata.images,
            &metadata.statistics,
            &metadata.nands,
        ]
        .iter()
        .any(|names| !ordered(names.iter().map(String::as_str).collect()))
            || !ordered(metadata.sensors.iter().map(|s| s.name.as_str()).collect())
        {
            return Err("resource snapshot names must be unique and ordered".into());
        }
        let mut children = encoded.children().iter();
        let mut next = || {
            children
                .next()
                .cloned()
                .ok_or_else(|| "resource snapshot is missing a child".to_owned())
        };
        let mut group = |names: Vec<String>| -> Result<BTreeMap<String, Checkpoint>, String> {
            names
                .into_iter()
                .map(|name| Ok((name, next()?)))
                .collect()
        };
        let all_wires = group(metadata.wires)?;
        let images = group(metadata.images)?;
        let statistics = group(metadata.statistics)?;
        let nands = group(metadata.nands)?;
        let media = next()?;
        let (wires, image_wires) = all_wires
            .into_iter()
            .partition(|(name, _)| !self.images.as_ref().is_some_and(|part| part.has_wire(name)));
        let mut sensors = BTreeMap::new();
        for record in &metadata.sensors {
            if record.edge.is_some_and(|edge| edge.index == 0) {
                return Err("resource snapshot sensor edge index is zero".into());
            }
            let part = self
                .images
                .as_ref()
                .ok_or("resource snapshot has sensors the graph does not declare")?;
            sensors.insert(record.name.clone(), part.import_sensor(record, &mut next)?);
        }
        if children.next().is_some() {
            return Err("resource snapshot has extra children".into());
        }
        Ok(ResourceSnapshot {
            wires,
            images: ImageSnapshot {
                wires: image_wires,
                images,
                statistics,
                sensors,
            },
            media,
            nands,
        })
    }
}
