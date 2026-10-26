//! The declared optical blur law that reads the lens focus-axis position.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub target_position: i64,
    pub pulses_per_radius: u32,
    pub maximum_radius: u32,
}
