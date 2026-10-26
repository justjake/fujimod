//! The graph owns shared image wire and input identities; restore their
//! values in place.
use super::*;

impl ImageWire {
    pub(crate) fn checkpoint(&self) -> Result<Checkpoint, String> {
        Ok(match self {
            Self::FramePeriod(v) => Checkpoint::encode(
                "sensor-frame-period",
                &*v.lock().map_err(|_| "frame period poisoned")?,
            )?,
            Self::ImageGrid(v) => Checkpoint::encode(
                "sensor-image-grid",
                &*v.lock().map_err(|_| "image grid poisoned")?,
            )?,
            Self::Exposure(v) => Checkpoint::encode(
                "physical-exposure",
                &*v.lock().map_err(|_| "exposure poisoned")?,
            )?,
            Self::Parameters(v) => {
                Checkpoint::encode("parameters", &*v.lock().map_err(|_| "parameters poisoned")?)?
            }
            Self::MemoryOutputs(v) => Checkpoint::encode(
                "memory-outputs",
                &*v.lock().map_err(|_| "memory outputs poisoned")?,
            )?,
            Self::Stream(v) => {
                Checkpoint::encode("stream", &*v.lock().map_err(|_| "stream poisoned")?)?
            }
            Self::PhaseEdges(v) => Checkpoint::encode(
                "phase-edges",
                &*v.lock().map_err(|_| "phase edges poisoned")?,
            )?,
            Self::OutputChannels(v) => v.checkpoint()?,
        })
    }
    pub(crate) fn validate_checkpoint(&self, saved: &Checkpoint) -> Result<(), String> {
        match self {
            Self::FramePeriod(_) => {
                let _: u64 = saved.decode("sensor-frame-period")?;
            }
            Self::ImageGrid(_) => {
                let grid: [usize; 2] = saved.decode("sensor-image-grid")?;
                if grid.iter().any(|n| !(1..=16384).contains(n)) {
                    return Err("invalid sensor image grid checkpoint".into());
                }
            }
            Self::Exposure(_) => {
                let value: fujimod_emulator_imaging::exposure::Exposure =
                    saved.decode("physical-exposure")?;
                if !value.seconds.is_finite()
                    || value.seconds < 0.
                    || !value.analog_gain.is_finite()
                    || value.analog_gain < 0.
                    || value.seconds > 0. && value.analog_gain == 0.
                {
                    return Err("invalid physical exposure checkpoint".into());
                }
            }
            Self::Parameters(_) => {
                let _: isp::parameters::Parameters = saved.decode("parameters")?;
            }
            Self::MemoryOutputs(_) => {
                let _: isp::memory_outputs::MemoryOutputs = saved.decode("memory-outputs")?;
            }
            Self::Stream(_) => {
                let _: isp::stream::Stream = saved.decode("stream")?;
            }
            Self::PhaseEdges(_) => {
                let _: [Option<FrameEdge>; 2] = saved.decode("phase-edges")?;
            }
            Self::OutputChannels(v) => v.validate_checkpoint(saved)?,
        }
        Ok(())
    }
    pub(crate) fn restore_checkpoint(&self, saved: &Checkpoint) -> Result<(), String> {
        match self {
            Self::FramePeriod(v) => {
                *v.lock().map_err(|_| "frame period poisoned")? =
                    saved.decode("sensor-frame-period")?;
            }
            Self::ImageGrid(v) => {
                *v.lock().map_err(|_| "image grid poisoned")? =
                    saved.decode("sensor-image-grid")?;
            }
            Self::Exposure(v) => {
                *v.lock().map_err(|_| "exposure poisoned")? = saved.decode("physical-exposure")?
            }
            Self::Parameters(v) => {
                *v.lock().map_err(|_| "parameters poisoned")? = saved.decode("parameters")?
            }
            Self::MemoryOutputs(v) => {
                *v.lock().map_err(|_| "memory outputs poisoned")? =
                    saved.decode("memory-outputs")?
            }
            Self::Stream(v) => {
                *v.lock().map_err(|_| "stream poisoned")? = saved.decode("stream")?
            }
            Self::PhaseEdges(v) => {
                *v.lock().map_err(|_| "phase edges poisoned")? = saved.decode("phase-edges")?
            }
            Self::OutputChannels(v) => v.restore_checkpoint(saved)?,
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

pub(crate) fn checkpoint(part: &Images) -> Result<ImageSnapshot, String> {
    Ok(ImageSnapshot {
        wires: part
            .wires
            .iter()
            .map(|(k, v)| Ok((k.clone(), v.checkpoint()?)))
            .collect::<Result<_, String>>()?,
        images: part
            .images
            .iter()
            .map(|(k, v)| Ok((k.clone(), v.checkpoint()?)))
            .collect::<Result<_, String>>()?,
        statistics: part
            .statistics
            .iter()
            .map(|(k, v)| Ok((k.clone(), v.checkpoint()?)))
            .collect::<Result<_, String>>()?,
        sensors: part
            .sensors
            .iter()
            .map(|(k, v)| {
                Ok((
                    k.clone(),
                    SensorSnapshot(Arc::new(Sensor(
                        v.lock().map_err(|_| "sensor frames poisoned")?.clone(),
                    ))),
                ))
            })
            .collect::<Result<_, String>>()?,
    })
}

pub(crate) fn saved_sensor(saved: &SensorSnapshot) -> Result<&SensorFrames, String> {
    saved
        .0
        .as_any()
        .downcast_ref::<Sensor>()
        .map(|sensor| &sensor.0)
        .ok_or_else(|| "sensor snapshot is not an EXR II sensor".into())
}

pub(crate) fn validate(part: &Images, saved: &ImageSnapshot) -> Result<(), String> {
    let sensor = sensor_decoders();
    same_keys(&part.wires, &saved.wires)?;
    same_keys(&part.images, &saved.images)?;
    same_keys(&part.statistics, &saved.statistics)?;
    same_keys(&part.sensors, &saved.sensors)?;
    for (k, v) in &part.wires {
        v.validate_checkpoint(&saved.wires[k])?;
    }
    for (k, v) in &part.images {
        v.validate_checkpoint(&saved.images[k], sensor)?;
    }
    for (k, v) in &part.statistics {
        v.validate_checkpoint(&saved.statistics[k])?;
    }
    for v in saved.sensors.values() {
        saved_sensor(v)?;
    }
    Ok(())
}

pub(crate) fn restore(part: &Images, saved: &ImageSnapshot) -> Result<(), String> {
    let sensor = sensor_decoders();
    for (k, v) in &part.wires {
        v.restore_checkpoint(&saved.wires[k])?;
    }
    for (k, v) in &part.images {
        v.restore_checkpoint(&saved.images[k], sensor)?;
    }
    for (k, v) in &part.statistics {
        v.restore_checkpoint(&saved.statistics[k])?;
    }
    for (k, v) in &part.sensors {
        *v.lock().map_err(|_| "sensor frames poisoned")? = saved_sensor(&saved.sensors[k])?.clone();
    }
    Ok(())
}
