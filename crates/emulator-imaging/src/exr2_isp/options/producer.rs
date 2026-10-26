//! Plan options of the EXR II statistics producer model in `fujimod-emulator-exr2-isp`.
use serde::{Deserialize, Serialize};
use crate::exr2_isp::dma;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub clock: dma::Clock,
    pub pixels_per_tick: Option<u64>,
    pub ack_ticks: u64,
    pub stop_ticks: u64,
    pub histogram: bool,
    /// Preserve complete external statistics spans, including histogram data.
    #[serde(default)]
    pub external_statistics: bool,
    pub contrast: Option<crate::exr2_isp::contrast::Arithmetic>,
    pub metering: Option<super::metering::Arithmetic>,
    #[serde(default)]
    pub diagnostic_reads: bool,
}
