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
    /// Producer register window (0x1000 bytes). The X70 places it at FFF18000; EXR Processor Pro (X-Pro1) at
    /// FFF22000 with the same register layout.
    #[serde(default = "default_base")]
    pub base: u64,
    /// Parameter/LUT bank window (0x3400 bytes): FFF28000 on the X70, FFF34000 on the X-Pro1.
    #[serde(default = "default_parameter_base")]
    pub parameter_base: u64,
    /// Interrupt source raised when a transfer completes: 0x2E on the X70, 0x27 on the X-Pro1.
    #[serde(default = "default_irq")]
    pub irq: u32,
    /// Register layout. `exr2` is the X70's. `exr-pro` (X-Pro1 3.82, from the values its driver writes):
    /// +0x600 holds an enable in bit 0 and the mode in bits 5:4 (driver 0x7fb2ec/0x7fb234; the X70 keeps the mode in
    /// bits 1:0 and the enable at +0x418); the global mode word also uses bit 24 (alongside bit 28); producer +0x10,
    /// +0x310, +0x314 and parameter-bank +0x08, +0x48, +0x4c have other field layouts and are stored without being
    /// interpreted, so contrast and metering arithmetic (which read the parameter bank) are refused.
    #[serde(default)]
    pub mode_layout: ModeLayout,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ModeLayout {
    #[default]
    Exr2,
    ExrPro,
}

fn default_base() -> u64 {
    0xfff18000
}

fn default_parameter_base() -> u64 {
    0xfff28000
}

fn default_irq() -> u32 {
    0x2e
}
