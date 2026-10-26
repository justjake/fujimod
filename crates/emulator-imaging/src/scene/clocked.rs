//! The declared external frame clock a clocked frame source follows.
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub period_ticks: u64,
    pub phase_source: Option<usize>,
    pub optics: Option<super::optics::Options>,
}
