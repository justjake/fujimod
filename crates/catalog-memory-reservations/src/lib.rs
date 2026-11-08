//! Generated fixed DRAM reservations for one firmware.
//!
//! The stock firmware owns these buffers while it runs: heaps, the font
//! library, the UI language font bank, debug rings and configuration storage.
//! Each address and size comes from the stock code that initializes or
//! reserves it. Mod memory must avoid every reservation. A locator that could
//! not resolve one of them is recorded by name, so no reservation is implied.
use serde::{Deserialize, Serialize};

pub const MEMORY_RESERVATIONS_SCHEMA_URI: &str =
    "https://fujimod.com/schemas/memory-reservations.generated.schema.json";

/// Located fixed reservations of one firmware.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(missing_as_null)
)]
pub struct FirmwareMemoryReservations {
    /// Schema identifying the generated document.
    #[serde(rename = "$schema")]
    pub schema_uri: String,
    /// Document version.
    pub schema: u32,
    /// Stable document identity.
    pub id: String,
    /// Firmware whose stock code owns these reservations.
    pub firmware_id: String,
    /// DRAM window, in bytes, that every reservation lies within.
    pub dram_size: u64,
    /// Located reservations, by stock name.
    pub reservations: Vec<Reservation>,
    /// Fixed reservations whose locator failed; each is absent from `reservations`.
    pub locator_failures: Vec<LocatorFailure>,
}

/// One fixed DRAM reservation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(missing_as_null)
)]
pub struct Reservation {
    /// Stock name of the reservation, such as `language_fonts`.
    pub name: String,
    /// Physical DRAM address of the first byte.
    pub address: u64,
    /// Bytes reserved.
    pub size: u64,
}

/// A fixed reservation that its locator could not resolve.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(missing_as_null)
)]
pub struct LocatorFailure {
    /// Stock name of the reservation whose locator failed.
    pub locator: String,
    /// Why the stock code did not verify.
    pub reason: String,
}

impl FirmwareMemoryReservations {
    pub fn validate(&self) -> Result<(), String> {
        fujimod_catalog_identity::validate_id(&self.id)?;
        fujimod_catalog_identity::validate_id(&self.firmware_id)?;
        let fail = |message: &str| Err(format!("memory reservations {}: {message}", self.id));
        if self.schema_uri != MEMORY_RESERVATIONS_SCHEMA_URI || self.schema != 1 {
            return fail("unsupported schema");
        }
        if self.dram_size == 0 {
            return fail("empty DRAM window");
        }
        if self.reservations.is_empty() {
            return fail("no reservations located");
        }
        let mut names = std::collections::BTreeSet::new();
        for reservation in &self.reservations {
            if reservation.name.trim().is_empty() || !names.insert(reservation.name.as_str()) {
                return fail("empty or duplicate reservation name");
            }
            if reservation.size == 0
                || reservation
                    .address
                    .checked_add(reservation.size)
                    .is_none_or(|end| end > self.dram_size)
            {
                return fail("reservation outside the DRAM window");
            }
        }
        for failure in &self.locator_failures {
            if failure.locator.trim().is_empty() || names.contains(failure.locator.as_str()) {
                return fail("locator failure names a located reservation");
            }
        }
        Ok(())
    }
}
