//! In-process checkpoint and restore of the device table.
//!
//! A device that implements the exact `Device::checkpoint` path is captured
//! through it. A device without one is captured through its observational
//! `save()` / `restore()` pair. That fallback is interim and labelled: the
//! maintainers' trait documentation says the diagnostic state is not a
//! complete checkpoint, so `DevicesCheckpoint::observational` names every
//! device that used it and a caller reports the checkpoint as not exact.
use super::Devices;
use fujimod_emulator_devices::Record;
use fujimod_emulator_devices::checkpoint::Checkpoint;
use fujimod_emulator_devices::graphics::scanout::Frame;
use std::collections::{BTreeMap, BTreeSet};

const NO_EXACT: &str = "has no exact checkpoint implementation";

#[derive(Clone)]
enum State {
    Exact(Checkpoint),
    /// Interim: diagnostic JSON, not a complete typed state.
    Observational(serde_json::Value),
}

#[derive(Clone)]
pub struct DevicesCheckpoint {
    names: Vec<Option<&'static str>>,
    states: Vec<Option<State>>,
    events: BTreeSet<(u64, usize, u64)>,
    log: Vec<Record>,
    prior_records: usize,
    frames: BTreeMap<usize, Frame>,
    outputs: BTreeMap<usize, serde_json::Value>,
}

impl DevicesCheckpoint {
    /// Devices captured through the interim observational JSON, in table order.
    pub fn observational(&self) -> Vec<&'static str> {
        self.names
            .iter()
            .zip(&self.states)
            .filter_map(|(name, state)| match (name, state) {
                (Some(name), Some(State::Observational(_))) => Some(*name),
                _ => None,
            })
            .collect()
    }
    /// Number of devices captured through the exact path.
    pub fn exact(&self) -> usize {
        self.states
            .iter()
            .filter(|state| matches!(state, Some(State::Exact(_))))
            .count()
    }
}

impl Devices {
    /// Capture every device and the scheduler state. Refuses a device that
    /// waits for a host result: that request lives outside the table.
    pub fn checkpoint(&self) -> Result<DevicesCheckpoint, String> {
        let mut names = Vec::with_capacity(self.models.len());
        let mut states = Vec::with_capacity(self.models.len());
        for model in &self.models {
            let Some(model) = model else {
                names.push(None);
                states.push(None);
                continue;
            };
            if model.has_host_request() {
                return Err(format!(
                    "{} waits for a host result; checkpoint after it completes",
                    model.name()
                ));
            }
            names.push(Some(model.name()));
            states.push(Some(match model.checkpoint() {
                Ok(exact) => State::Exact(exact),
                Err(error) if error.ends_with(NO_EXACT) => {
                    State::Observational(model.save())
                }
                Err(error) => return Err(error),
            }));
        }
        Ok(DevicesCheckpoint {
            names,
            states,
            events: self.events.clone(),
            log: self.log.clone(),
            prior_records: self.prior_records,
            frames: self.frames.clone(),
            outputs: self.outputs.clone(),
        })
    }

    /// Check a checkpoint against this table without changing it.
    pub fn validate_checkpoint(&self, saved: &DevicesCheckpoint) -> Result<(), String> {
        if saved.names.len() != self.models.len() {
            return Err("Device table differs from the checkpoint".into());
        }
        for (index, model) in self.models.iter().enumerate() {
            match (model, &saved.names[index], &saved.states[index]) {
                (None, None, None) => {}
                (Some(model), Some(name), Some(state)) if model.name() == *name => {
                    if let State::Exact(exact) = state {
                        model.validate_checkpoint(exact)?;
                    }
                }
                _ => return Err(format!("Device {index} differs from the checkpoint")),
            }
        }
        Ok(())
    }

    /// Restore every device in place. Shared wiring (`Arc` handles) is kept;
    /// nothing is replayed through a register. The caller has stopped all
    /// processors and validated first; an error here leaves the table partly
    /// restored.
    pub fn restore_checkpoint(&mut self, saved: &DevicesCheckpoint) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        for (model, state) in self.models.iter_mut().zip(&saved.states) {
            let (Some(model), Some(state)) = (model, state) else {
                continue;
            };
            match state {
                State::Exact(exact) => model.restore_checkpoint(exact)?,
                State::Observational(value) => model.restore(value)?,
            }
        }
        self.events = saved.events.clone();
        self.log = saved.log.clone();
        self.prior_records = saved.prior_records;
        self.frames = saved.frames.clone();
        self.outputs = saved.outputs.clone();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::{Cx, Device};
    use std::sync::{Arc, Mutex};

    /// A device with a JSON-only state.
    struct Counter {
        value: u64,
    }
    impl Device for Counter {
        fn name(&self) -> &'static str {
            "counter"
        }
        fn read(&mut self, _cx: &mut Cx<'_>, _address: u64, _size: u8) -> u64 {
            self.value
        }
        fn write(&mut self, _cx: &mut Cx<'_>, _address: u64, _size: u8, value: u64) {
            self.value = value;
        }
        fn save(&self) -> serde_json::Value {
            serde_json::json!({ "value": self.value })
        }
        fn restore(&mut self, state: &serde_json::Value) -> Result<(), String> {
            self.value = state["value"].as_u64().ok_or("counter state")?;
            Ok(())
        }
    }

    /// A device with an exact checkpoint.
    struct Exact {
        value: Arc<Mutex<u32>>,
    }
    impl Device for Exact {
        fn name(&self) -> &'static str {
            "exact"
        }
        fn read(&mut self, _cx: &mut Cx<'_>, _address: u64, _size: u8) -> u64 {
            u64::from(*self.value.lock().unwrap())
        }
        fn write(&mut self, _cx: &mut Cx<'_>, _address: u64, _size: u8, value: u64) {
            *self.value.lock().unwrap() = value as u32;
        }
        fn checkpoint(&self) -> Result<Checkpoint, String> {
            Checkpoint::encode("test-exact", &*self.value.lock().unwrap())
        }
        fn validate_checkpoint(&self, state: &Checkpoint) -> Result<(), String> {
            state.decode::<u32>("test-exact").map(|_| ())
        }
        fn restore_checkpoint(&mut self, state: &Checkpoint) -> Result<(), String> {
            *self.value.lock().unwrap() = state.decode::<u32>("test-exact")?;
            Ok(())
        }
    }

    fn table(shared: &Arc<Mutex<u32>>) -> Devices {
        let mut devices = Devices::empty();
        devices.models.push(Some(Box::new(Counter { value: 7 })));
        devices.models.push(Some(Box::new(Exact { value: shared.clone() })));
        devices.models.push(None);
        devices
    }

    #[test]
    fn exact_and_observational_devices_round_trip_and_are_labelled() {
        let shared = Arc::new(Mutex::new(5));
        let mut devices = table(&shared);
        devices.events.insert((10, 0, 1));
        let saved = devices.checkpoint().unwrap();
        assert_eq!(saved.observational(), vec!["counter"]);
        assert_eq!(saved.exact(), 1);
        // Change everything the checkpoint holds.
        *shared.lock().unwrap() = 99;
        devices.events.clear();
        devices.events.insert((20, 1, 2));
        devices.models[0] = Some(Box::new(Counter { value: 1 }));
        devices.restore_checkpoint(&saved).unwrap();
        assert_eq!(*shared.lock().unwrap(), 5);
        assert_eq!(
            devices.events.iter().copied().collect::<Vec<_>>(),
            vec![(10, 0, 1)]
        );
        assert_eq!(devices.snapshots()["counter"], r#"{"value":7}"#);
        // Restoring twice from one checkpoint gives the same state.
        *shared.lock().unwrap() = 6;
        devices.restore_checkpoint(&saved).unwrap();
        assert_eq!(*shared.lock().unwrap(), 5);
    }

    #[test]
    fn a_checkpoint_does_not_restore_into_a_different_table() {
        let shared = Arc::new(Mutex::new(5));
        let devices = table(&shared);
        let saved = devices.checkpoint().unwrap();
        let mut other = Devices::empty();
        other.models.push(Some(Box::new(Counter { value: 0 })));
        assert!(other.restore_checkpoint(&saved).is_err());
        let mut swapped = Devices::empty();
        swapped.models.push(Some(Box::new(Exact { value: shared.clone() })));
        swapped.models.push(Some(Box::new(Counter { value: 0 })));
        swapped.models.push(None);
        assert!(swapped.restore_checkpoint(&saved).is_err());
    }
}
