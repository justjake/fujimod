//! Data-line faults declared on an SD card transfer and the faults that occurred.
use serde::{Deserialize, Serialize};

/// Direction of a multi-block data transfer on the card's data lines.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[derive(tsify::Tsify)]
#[tsify(hashmap_as_object, missing_as_null)]
pub enum DataDirection {
    Read,
    Write,
}
/// An external data CRC failure on the card's data lines. It never selects a
/// filesystem result: the host reports SDHCI data CRC error status and the
/// guest's own recovery runs.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[derive(tsify::Tsify)]
#[tsify(hashmap_as_object, missing_as_null)]
pub struct DataFault {
    pub direction: DataDirection,
    /// Sector whose transfer fails; `None` selects each transfer's first sector.
    pub sector: Option<u32>,
    /// Sectors after `sector` that still transfer before the failure.
    pub after_sectors: u32,
    /// Matching multi-block transfers that fail, firmware retries included.
    pub occurrences: u32,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[derive(tsify::Tsify)]
#[tsify(hashmap_as_object, missing_as_null)]
pub struct DataFaultEvent {
    pub command: u8,
    pub sector: u32,
    pub transferred: usize,
}
