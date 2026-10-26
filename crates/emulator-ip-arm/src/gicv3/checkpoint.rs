//! Decode register state into a detached controller and reconstruct pin caches.
use super::*;
impl Serialize for Gic {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.state.serialize(serializer)
    }
}
impl<'de> Deserialize<'de> for Gic {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let state = State::deserialize(deserializer)?;
        let mut controller = Self::new(state.affinities.clone(), state.shared.len())
            .map_err(serde::de::Error::custom)?;
        controller
            .restore_state(state)
            .map_err(serde::de::Error::custom)?;
        Ok(controller)
    }
}
