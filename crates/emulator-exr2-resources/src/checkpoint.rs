//! The graph owns shared wire identities. Restore their values in place.
mod portable;
use super::*;
use fujimod_emulator_devices::checkpoint::Checkpoint;
use fujimod_emulator_devices::parts::sd_card::SECTOR;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Typed external media captured by a checkpoint. Card sectors and file
/// contents are shared by handle, so a checkpoint copies no media bytes.
#[derive(Clone, PartialEq)]
pub(crate) struct MediaSnapshot {
    cards: BTreeMap<String, Card>,
    files: CardFiles,
}
impl MediaSnapshot {
    fn capture(inputs: &Inputs) -> Self {
        Self {
            cards: inputs.cards.clone(),
            files: inputs.files.clone(),
        }
    }

    /// Conservative encoded size for retention budgets; the media itself is
    /// held typed and never encoded by an in-process checkpoint.
    fn encoded_bytes(&self) -> usize {
        self.cards
            .values()
            .map(|card| card.dirty_sectors() * (SECTOR + 8))
            .sum::<usize>()
            + self
                .files
                .values()
                .flat_map(|entries| entries.iter().map(|(path, data)| path.len() + data.len() + 8))
                .sum::<usize>()
    }

    fn diff(&self, other: &Self) -> Option<String> {
        for (name, left) in &self.cards {
            let Some(right) = other.cards.get(name) else {
                return Some(format!("cards.{name}: only in left state"));
            };
            if left != right {
                return Some(format!("cards.{name}: state differs"));
            }
        }
        if let Some(name) = other
            .cards
            .keys()
            .find(|name| !self.cards.contains_key(*name))
        {
            return Some(format!("cards.{name}: only in right state"));
        }
        for (name, left) in &self.files {
            let Some(right) = other.files.get(name) else {
                return Some(format!("files.{name}: only in left state"));
            };
            if left != right {
                return Some(format!("files.{name}: state differs"));
            }
        }
        if let Some(name) = other
            .files
            .keys()
            .find(|name| !self.files.contains_key(*name))
        {
            return Some(format!("files.{name}: only in right state"));
        }
        None
    }
}

impl Serialize for MediaSnapshot {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeTuple;
        let mut tuple = serializer.serialize_tuple(2)?;
        tuple.serialize_element(&self.cards)?;
        tuple.serialize_element(&Files(&self.files))?;
        tuple.end()
    }
}

impl<'de> Deserialize<'de> for MediaSnapshot {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (cards, files) = <(BTreeMap<String, Card>, OwnedFiles)>::deserialize(deserializer)?;
        Ok(Self {
            cards,
            files: files.0,
        })
    }
}

struct Files<'a>(&'a CardFiles);
struct OwnedFiles(CardFiles);

impl Serialize for Files<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::{SerializeMap, SerializeSeq, SerializeTuple};
        struct Bytes<'a>(&'a [u8]);
        impl Serialize for Bytes<'_> {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_bytes(self.0)
            }
        }
        struct Entry<'a>(&'a str, &'a Arc<[u8]>);
        impl Serialize for Entry<'_> {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                let mut tuple = serializer.serialize_tuple(2)?;
                tuple.serialize_element(self.0)?;
                tuple.serialize_element(&Bytes(self.1))?;
                tuple.end()
            }
        }
        struct Entries<'a>(&'a [(String, Arc<[u8]>)]);
        impl Serialize for Entries<'_> {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                let mut sequence = serializer.serialize_seq(Some(self.0.len()))?;
                for (path, data) in self.0 {
                    sequence.serialize_element(&Entry(path, data))?;
                }
                sequence.end()
            }
        }
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (name, entries) in self.0 {
            map.serialize_entry(name, &Entries(entries))?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for OwnedFiles {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Entry(
            String,
            #[serde(with = "fujimod_emulator_devices::checkpoint::shared_bytes")] Arc<[u8]>,
        );
        let files = BTreeMap::<String, Vec<Entry>>::deserialize(deserializer)?;
        Ok(Self(
            files
                .into_iter()
                .map(|(name, entries)| {
                    (
                        name,
                        entries
                            .into_iter()
                            .map(|Entry(path, data)| (path, data))
                            .collect(),
                    )
                })
                .collect(),
        ))
    }
}

#[derive(Clone, PartialEq)]
pub struct ResourceSnapshot {
    wires: BTreeMap<String, Checkpoint>,
    images: ImageSnapshot,
    media: MediaSnapshot,
    nands: BTreeMap<String, Checkpoint>,
}
impl ResourceSnapshot {
    /// Encoded mutable resource state; shared sensor rasters remain external inputs.
    pub fn encoded_bytes(&self) -> usize {
        self.wires
            .values()
            .chain(self.images.wires.values())
            .chain(self.images.images.values())
            .chain(self.images.statistics.values())
            .chain(self.nands.values())
            .map(Checkpoint::bytes)
            .sum::<usize>()
            + self.media.encoded_bytes()
    }

    /// First structural difference between two resource snapshots.
    pub fn diff(&self, other: &Self) -> Option<String> {
        for (name, left) in &self.wires {
            let Some(right) = other.wires.get(name) else {
                return Some(format!("wires.{name}: only in left state"));
            };
            if let Some(message) = left.diff(right) {
                return Some(format!("wires.{name}/{message}"));
            }
        }
        if let Some(name) = other
            .wires
            .keys()
            .find(|name| !self.wires.contains_key(*name))
        {
            return Some(format!("wires.{name}: only in right state"));
        }
        if let Some(message) = diff_map("images.wires", &self.images.wires, &other.images.wires) {
            return Some(message);
        }
        if let Some(message) = diff_map("images.images", &self.images.images, &other.images.images)
        {
            return Some(message);
        }
        if let Some(message) = diff_map(
            "images.statistics",
            &self.images.statistics,
            &other.images.statistics,
        ) {
            return Some(message);
        }
        for (name, left) in &self.images.sensors {
            let Some(right) = other.images.sensors.get(name) else {
                return Some(format!("images.sensors.{name}: only in left state"));
            };
            if left != right {
                return Some(format!("images.sensors.{name}: state differs"));
            }
        }
        if let Some(name) = other
            .images
            .sensors
            .keys()
            .find(|name| !self.images.sensors.contains_key(*name))
        {
            return Some(format!("images.sensors.{name}: only in right state"));
        }
        if let Some(message) = self.media.diff(&other.media) {
            return Some(format!("media/{message}"));
        }
        diff_map("nands", &self.nands, &other.nands)
    }

    /// Canonical typed encoding for clients that compare states as bytes.
    pub fn to_checkpoint(&self) -> Result<Checkpoint, String> {
        let mut children = Vec::new();
        for (name, state) in &self.wires {
            children.push(Checkpoint::encode("wire", &name)?.with_children(vec![state.clone()]));
        }
        for (name, state) in &self.images.wires {
            children
                .push(Checkpoint::encode("image-wire", &name)?.with_children(vec![state.clone()]));
        }
        for (name, state) in &self.images.images {
            children.push(Checkpoint::encode("image", &name)?.with_children(vec![state.clone()]));
        }
        for (name, state) in &self.images.statistics {
            children
                .push(Checkpoint::encode("statistics", &name)?.with_children(vec![state.clone()]));
        }
        for (name, state) in &self.images.sensors {
            let frames = state.0.frames()?;
            children.push(
                Checkpoint::encode(
                    "sensor",
                    &SensorRecord {
                        name: name.clone(),
                        edge: state.0.edge(),
                        view: state.0.view(),
                        shared: state.0.shared(),
                    },
                )?
                .with_children(frames),
            );
        }
        children.push(Checkpoint::encode("resource-media", &self.media)?);
        for (name, state) in &self.nands {
            children.push(Checkpoint::encode("nand", &name)?.with_children(vec![state.clone()]));
        }
        Ok(Checkpoint::encode("resources", &())?.with_children(children))
    }

    /// Readable direct-state JSON; binary content stays in the typed checkpoint.
    pub fn to_json(&self) -> Result<serde_json::Value, String> {
        self.checkpoint_json()
    }

    fn checkpoint_json(&self) -> Result<serde_json::Value, String> {
        use serde_json::json;
        let cards = &self.media.cards;
        let files = &self.media.files;
        let images = self.images.to_json()?;
        let mut wires = self
            .wires
            .iter()
            .map(|(name, state)| Ok((name.clone(), wire_json(state)?)))
            .collect::<Result<BTreeMap<_, _>, String>>()?;
        if let Some(image_wires) = images["wires"].as_object() {
            wires.extend(
                image_wires
                    .iter()
                    .map(|(name, value)| (name.clone(), value.clone())),
            );
        }
        Ok(json!({
            "wires":wires,
            "images":images["images"].clone(),
            "statistics":images["statistics"].clone(),
            "sensors":images["sensors"].clone(),
            "cards":cards.iter().map(|(name, card)| (name.clone(), card.state())).collect::<BTreeMap<_, _>>(),
            "nands":self.nands.iter().map(|(name, state)| {
                Ok((name.clone(), fujimod_emulator_devices::parts::nand::Nand::from_checkpoint(state)?.state()))
            }).collect::<Result<BTreeMap<_, _>, String>>()?,
            "files":files.iter().map(|(name, entries)| {
                (name.clone(), entries.iter().map(|(path, data)| json!({
                    "path":path,
                    "contents":fujimod_emulator_devices::state::bytes(data),
                })).collect::<Vec<_>>())
            }).collect::<BTreeMap<_, _>>(),
        }))
    }
}

/// Readable value carried by one resource wire's typed checkpoint.
fn wire_json(saved: &Checkpoint) -> Result<serde_json::Value, String> {
    use serde_json::json;
    Ok(match saved.kind() {
        "physical-focus" => json!(saved.decode::<f64>("physical-focus")?),
        "physical-aperture" => json!(saved.decode::<f64>("physical-aperture")?),
        "software-pulse" => json!(saved.decode::<bool>("software-pulse")?),
        "logic-level" => json!(saved.decode::<bool>("logic-level")?),
        "optical-position" => json!(saved.decode::<i64>("optical-position")?),
        "image-irq-routes" => irq_routes::IrqRoutes::from_checkpoint(saved)?.fingerprint(),
        "bridge-registers" => json!(saved.decode::<Vec<u8>>("bridge-registers")?),
        _ => saved.state(),
    })
}

fn diff_map(
    path: &str,
    left: &BTreeMap<String, Checkpoint>,
    right: &BTreeMap<String, Checkpoint>,
) -> Option<String> {
    for (name, value) in left {
        let Some(other) = right.get(name) else {
            return Some(format!("{path}.{name}: only in left state"));
        };
        if let Some(message) = value.diff(other) {
            return Some(format!("{path}.{name}/{message}"));
        }
    }
    if let Some(name) = right.keys().find(|name| !left.contains_key(*name)) {
        return Some(format!("{path}.{name}: only in right state"));
    }
    None
}
impl Wire {
    fn checkpoint(&self) -> Result<Checkpoint, String> {
        Ok(match self {
            Self::Focus(v) => {
                Checkpoint::encode("physical-focus", &*v.lock().map_err(|_| "focus poisoned")?)?
            }
            Self::Aperture(v) => Checkpoint::encode(
                "physical-aperture",
                &*v.lock().map_err(|_| "aperture poisoned")?,
            )?,
            Self::SoftwarePulse(v) => Checkpoint::encode("software-pulse", &v.asserted())?,
            Self::LogicLevel(v) => Checkpoint::encode("logic-level", &v.high())?,
            Self::OpticalPosition(v) => Checkpoint::encode("optical-position", &v.value())?,
            Self::IrqRoutes(v) => v.checkpoint()?,
            Self::BridgeRegisters(v) => Checkpoint::encode(
                "bridge-registers",
                &(0..4)
                    .map(|i| v.read_register(i))
                    .collect::<Result<Vec<_>, _>>()?,
            )?,
        })
    }
    fn validate_checkpoint(&self, saved: &Checkpoint) -> Result<(), String> {
        match self {
            Self::Focus(_) => {
                let position: f64 = saved.decode("physical-focus")?;
                if !position.is_finite() {
                    return Err("invalid physical focus checkpoint".into());
                }
            }
            Self::Aperture(_) => {
                let value: f64 = saved.decode("physical-aperture")?;
                if !value.is_finite() || value <= 0. {
                    return Err("invalid physical aperture checkpoint".into());
                }
            }
            Self::SoftwarePulse(_) => {
                let _: bool = saved.decode("software-pulse")?;
            }
            Self::LogicLevel(_) => {
                let _: bool = saved.decode("logic-level")?;
            }
            Self::OpticalPosition(_) => {
                let _: i64 = saved.decode("optical-position")?;
            }
            Self::IrqRoutes(v) => v.validate_checkpoint(saved)?,
            Self::BridgeRegisters(_) => {
                let bytes: Vec<u8> = saved.decode("bridge-registers")?;
                if bytes.len() != 4 {
                    return Err("bridge snapshot extent differs".into());
                }
            }
        }
        Ok(())
    }
    fn restore_checkpoint(&self, saved: &Checkpoint) -> Result<(), String> {
        match self {
            Self::Focus(v) => {
                *v.lock().map_err(|_| "focus poisoned")? = saved.decode("physical-focus")?;
            }
            Self::Aperture(v) => {
                *v.lock().map_err(|_| "aperture poisoned")? = saved.decode("physical-aperture")?
            }
            Self::SoftwarePulse(v) => v.restore_level(saved.decode("software-pulse")?),
            Self::LogicLevel(v) => v.drive(saved.decode("logic-level")?),
            Self::OpticalPosition(v) => v.publish(saved.decode("optical-position")?),
            Self::IrqRoutes(v) => v.restore_checkpoint(saved)?,
            Self::BridgeRegisters(v) => {
                for (i, b) in saved
                    .decode::<Vec<u8>>("bridge-registers")?
                    .into_iter()
                    .enumerate()
                {
                    v.write_register(i as u8, b)?;
                }
            }
        }
        Ok(())
    }
}
fn same_keys<A, B>(a: &BTreeMap<String, A>, b: &BTreeMap<String, B>) -> Result<(), String> {
    if a.keys().ne(b.keys()) {
        Err("snapshot graph resource names differ".into())
    } else {
        Ok(())
    }
}

impl Resources {
    pub fn checkpoint(&self) -> Result<ResourceSnapshot, String> {
        Ok(ResourceSnapshot {
            wires: self
                .wires
                .iter()
                .map(|(k, v)| Ok((k.clone(), v.checkpoint()?)))
                .collect::<Result<_, String>>()?,
            images: match &self.images {
                Some(part) => part.checkpoint()?,
                None => ImageSnapshot::default(),
            },
            media: MediaSnapshot::capture(&self.inputs),
            nands: self
                .inputs
                .nands
                .iter()
                .map(|(k, v)| Ok((k.clone(), v.checkpoint()?)))
                .collect::<Result<_, String>>()?,
        })
    }
    pub fn validate_checkpoint(&self, saved: &ResourceSnapshot) -> Result<(), String> {
        same_keys(&self.wires, &saved.wires)?;
        same_keys(&self.inputs.nands, &saved.nands)?;
        match &self.images {
            Some(part) => part.validate_checkpoint(&saved.images)?,
            None => {
                let images = &saved.images;
                if !images.wires.is_empty()
                    || !images.images.is_empty()
                    || !images.statistics.is_empty()
                    || !images.sensors.is_empty()
                {
                    return Err("snapshot graph resource names differ".into());
                }
            }
        }
        for (k, v) in &self.wires {
            v.validate_checkpoint(&saved.wires[k])?;
        }
        same_keys(&self.inputs.cards, &saved.media.cards)?;
        same_keys(&self.inputs.files, &saved.media.files)?;
        for card in saved.media.cards.values() {
            card.validate()?;
        }
        for v in saved.nands.values() {
            fujimod_emulator_devices::parts::nand::Nand::from_checkpoint(v)?;
        }
        Ok(())
    }
    pub fn restore_checkpoint(&mut self, saved: &ResourceSnapshot) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        for (k, v) in &self.wires {
            v.restore_checkpoint(&saved.wires[k])?;
        }
        if let Some(part) = &self.images {
            part.restore_checkpoint(&saved.images)?;
        }
        self.inputs.cards = saved.media.cards.clone();
        self.inputs.files = saved.media.files.clone();
        self.inputs.nands = saved
            .nands
            .iter()
            .map(|(k, v)| {
                Ok((
                    k.clone(),
                    fujimod_emulator_devices::parts::nand::Nand::from_checkpoint(v)?,
                ))
            })
            .collect::<Result<_, String>>()?;
        Ok(())
    }
}

#[cfg(test)]
mod optical_wire_tests {
    use super::*;
    #[test]
    fn optical_position_restores_the_existing_shared_wire() {
        let position = fujimod_emulator_devices::parts::optics::Position::default();
        let reader = position.clone();
        position.publish(-62);
        let wire = Wire::OpticalPosition(position.clone());
        let saved = wire.checkpoint().unwrap();
        assert_eq!(reader.value(), -62);
        position.publish(7);
        wire.validate_checkpoint(&saved).unwrap();
        wire.restore_checkpoint(&saved).unwrap();
        assert_eq!(reader.value(), -62);
        assert_eq!(wire.fingerprint().unwrap(), serde_json::json!(-62));
    }
}

#[cfg(test)]
mod media_tests {
    use super::*;
    use fujimod_emulator_devices::parts::sd_card::Options;

    fn card() -> Card {
        let mut card = Card::new(Options {
            sectors: 0x400000,
            max_dirty_sectors: 4,
        })
        .unwrap();
        card.write_bytes(0, &[0x11; SECTOR]).unwrap();
        card.write_bytes(SECTOR as u64 * 7, &[0x22; SECTOR])
            .unwrap();
        card
    }

    #[test]
    fn checkpoint_shares_sectors_and_restore_recovers_the_saved_sector() {
        let mut resources = Resources::default();
        resources.inputs.cards.insert("sd".into(), card());
        let saved = resources.checkpoint().unwrap();
        let live = &resources.inputs.cards["sd"];
        assert_eq!(live.sector(0).unwrap(), &[0x11; SECTOR]);
        assert!(std::ptr::eq(
            live.sector(0).unwrap().as_ptr(),
            saved.media.cards["sd"].sector(0).unwrap().as_ptr()
        ));
        let revision = live.revision();

        resources
            .inputs
            .cards
            .get_mut("sd")
            .unwrap()
            .write_bytes(0, &[0x33; SECTOR])
            .unwrap();
        assert_eq!(
            resources.inputs.cards["sd"].sector(0).unwrap(),
            &[0x33; SECTOR]
        );
        assert_eq!(saved.media.cards["sd"].sector(0).unwrap(), &[0x11; SECTOR]);

        resources.restore_checkpoint(&saved).unwrap();
        let restored = &resources.inputs.cards["sd"];
        assert_eq!(restored.sector(0).unwrap(), &[0x11; SECTOR]);
        assert_eq!(restored.sector(7).unwrap(), &[0x22; SECTOR]);
        assert_eq!(restored.revision(), revision);
        assert_eq!(saved.media.cards["sd"].sector(0).unwrap(), &[0x11; SECTOR]);
    }

    #[test]
    fn portable_media_round_trip_restores_shared_bytes() {
        let mut resources = Resources::default();
        resources.inputs.cards.insert("sd".into(), card());
        resources.inputs.files.insert(
            "host".into(),
            vec![("DCIM/A.JPG".into(), Arc::from(&[9, 8, 7][..]))],
        );
        let saved = resources.checkpoint().unwrap();
        let encoded = saved.export().unwrap();
        let imported = resources.import_checkpoint(&encoded).unwrap();
        assert!(imported == saved);

        let mut target = Resources::default();
        target.inputs.cards.insert("sd".into(), card());
        target.inputs.files.insert(
            "host".into(),
            vec![("DCIM/A.JPG".into(), Arc::from(&[1][..]))],
        );
        target.restore_checkpoint(&imported).unwrap();
        assert_eq!(
            target.inputs.cards["sd"].sector(0).unwrap(),
            &[0x11; SECTOR]
        );
        assert_eq!(&*target.inputs.files["host"][0].1, &[9, 8, 7]);
    }
}

#[cfg(test)]
mod aperture_wire_tests {
    use super::*;
    #[test]
    fn a_bad_iris_is_refused_before_mutation() {
        let mut resources = Resources::default();
        let aperture = Arc::new(Mutex::new(16.));
        resources
            .wires
            .insert("iris".into(), Wire::Aperture(aperture.clone()));
        let saved = resources.checkpoint().unwrap();
        *aperture.lock().unwrap() = 64.;
        let changed = resources.fingerprint().unwrap();
        let mut corrupt = saved.clone();
        corrupt.wires.insert(
            "iris".into(),
            Checkpoint::encode("physical-aperture", &0.0f64).unwrap(),
        );
        assert!(resources.restore_checkpoint(&corrupt).is_err());
        assert_eq!(resources.fingerprint().unwrap(), changed);
        resources.restore_checkpoint(&saved).unwrap();
        assert_eq!(*aperture.lock().unwrap(), 16.);
    }
}

#[cfg(test)]
mod readable_snapshot_tests {
    use super::*;

    #[test]
    fn readable_wire_values_are_derived_from_the_typed_checkpoint() {
        let mut resources = Resources::default();
        resources
            .wires
            .insert("usb".into(), Wire::logic_level(false));
        resources.wires.insert("dial".into(), {
            let position = optics::Position::default();
            position.publish(-4);
            Wire::OpticalPosition(position)
        });
        let saved = resources.checkpoint().unwrap();
        let state = saved.to_json().unwrap();
        assert_eq!(state["wires"]["usb"], serde_json::json!(false));
        assert_eq!(state["wires"]["dial"], serde_json::json!(-4));
    }
}
