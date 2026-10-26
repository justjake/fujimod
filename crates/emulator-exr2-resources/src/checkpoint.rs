//! The graph owns shared wire identities. Restore their values in place.
mod portable;
use super::*;
use fujimod_emulator_devices::checkpoint::Checkpoint;

#[derive(Clone)]
pub struct ResourceSnapshot {
    wires: BTreeMap<String, Checkpoint>,
    images: ImageSnapshot,
    media: Checkpoint,
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
            + self.media.bytes()
    }
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
            media: Checkpoint::encode("resource-media", &(&self.inputs.cards, &self.inputs.files))?,
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
        let (cards, files): (
            BTreeMap<String, Card>,
            BTreeMap<String, Vec<(String, Vec<u8>)>>,
        ) = saved.media.decode("resource-media")?;
        same_keys(&self.inputs.cards, &cards)?;
        same_keys(&self.inputs.files, &files)?;
        for card in cards.values() {
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
        let (cards, files) = saved.media.decode("resource-media")?;
        self.inputs.cards = cards;
        self.inputs.files = files;
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
