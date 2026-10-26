//! Plan options of the EXR II output banks model in `fujimod-emulator-exr2-isp`.
use serde::{Deserialize};
use crate::exr2_isp::dma;

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GeometryPolicy {
    SkipIncomplete,
    RefuseIncomplete,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub stop_ticks: u64,
    pub clock: dma::Clock,
    pub geometry: GeometryPolicy,
    #[serde(default)]
    pub diagnostic_reads: bool,
}
