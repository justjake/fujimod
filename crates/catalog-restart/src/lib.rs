//! Generated stock restart facts for one firmware.
//!
//! How a camera can be restarted from its debug USB link with stock code
//! only: the routines that must match, how a borrowed diagnostic query reaches
//! the stock sender, and what the stock power-off writes. The document holds
//! addresses, sizes and hashes, never firmware bytes.
use fujimod_catalog_bulk::CodeCheck;
use serde::{Deserialize, Serialize};

pub const RESTART_SCHEMA_URI: &str = "https://fujimod.com/schemas/restart.generated.schema.json";

/// Located restart mechanism for one firmware.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct FirmwareRestart {
    /// Schema identifying the generated document.
    #[serde(rename = "$schema")]
    pub schema_uri: String,
    /// Document version.
    pub schema: u32,
    /// Stable document identity.
    pub id: String,
    /// Firmware whose stock code provides this mechanism.
    pub firmware_id: String,
    /// Product name the camera reports.
    pub product: String,
    /// Internal version the camera reports.
    pub internal_version: String,
    /// Uncached RAM alias: debug reads of code, the callback table pointer,
    /// the callback slot and UI state use `address + code_alias`.
    pub code_alias: u64,
    /// Stock code that must match before any write.
    pub code_checks: Vec<CodeCheck>,
    /// How the restart is requested.
    pub mechanism: RestartMechanism,
}

/// A restart path through stock code.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify)
)]
pub enum RestartMechanism {
    /// The stock power-off request carries a wake deadline. A borrowed
    /// diagnostic query calls the sender with the request in the query output
    /// fields; the power-off arms the power controller's alarm and cuts power,
    /// and the alarm boots the camera again.
    WakeDeadline {
        /// The borrowed diagnostic query.
        query: BorrowedQuery,
        /// Stock routine that sends the power-off request; called with the
        /// address of the query output fields as its first argument.
        sender: u64,
        /// Request word that selects power-off.
        request: u32,
        /// Byte offset of the request word in the sender's parameters.
        request_offset: u32,
        /// Byte offset of the wake deadline in the sender's parameters.
        deadline_offset: u32,
        /// Where REC idle is observed.
        ui: RecIdle,
        /// What the stock power-off does with the request.
        power_off: PowerOff,
    },
    /// A stock routine cuts power with no wake; the camera stays off.
    PowerCut {
        /// Stock power-cut routine.
        power_cut: u64,
        /// GPIO pin the routine releases to drop the power latch.
        latch_pin: u32,
    },
}

/// A diagnostic query whose callback slot is borrowed for one call.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify)
)]
pub struct BorrowedQuery {
    /// Address of the pointer to the diagnostic callback table.
    pub callback_table: u64,
    /// Byte offset of the query callback in that table.
    pub callback_offset: u64,
    /// Stock callback value.
    pub stock_callback: u64,
    /// Output fields passed to the callback; the query tag is their address.
    pub output_fields: u64,
    /// Bytes in the output fields.
    pub output_bytes: u32,
}

/// UI state that is REC with nothing pending and no menu open.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify)
)]
pub struct RecIdle {
    /// Current UI mode.
    pub mode_address: u64,
    /// Bytes in each UI mode cell.
    pub mode_size: u32,
    /// UI mode value for REC.
    pub rec_mode: u32,
    /// Pending UI mode; zero when no change is pending.
    pub pending_mode_address: u64,
    /// Nonzero while a menu is open.
    pub menu_active_address: u64,
}

/// The stock power-off reached by the request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify)
)]
pub struct PowerOff {
    /// Stock power-off routine.
    pub entry: u64,
    /// Its final self-branch, reached after the power cut.
    pub halt: u64,
    /// Byte offset of the deadline in the request the power-off receives.
    pub request_deadline_offset: u32,
    /// Power-controller registers for the alarm.
    pub alarm: AlarmRegisters,
    /// Persistent power-controller parameter offset of the saved deadline word.
    pub deadline_parameter: u32,
    /// Persistent parameter offset of the alarm's high (rollover) byte.
    pub rollover_parameter: u32,
}

/// Power-controller registers the power-off writes for the wake alarm.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify)
)]
pub struct AlarmRegisters {
    /// Low byte of the seconds until the alarm.
    pub low: u32,
    /// High byte of the seconds until the alarm.
    pub high: u32,
    /// Alarm enable.
    pub enable: u32,
}

impl FirmwareRestart {
    /// Check identity, schema and the mechanism's internal consistency.
    pub fn validate(&self) -> Result<(), String> {
        fujimod_catalog_identity::validate_id(&self.id)?;
        fujimod_catalog_identity::validate_id(&self.firmware_id)?;
        let fail = |message: &str| Err(format!("restart {}: {message}", self.id));
        if self.schema_uri != RESTART_SCHEMA_URI || self.schema != 1 {
            return fail("unsupported schema");
        }
        if self.product.trim().is_empty() || self.internal_version.trim().is_empty() {
            return fail("empty camera identity");
        }
        if self.code_checks.is_empty()
            || self
                .code_checks
                .iter()
                .any(|check| check.size == 0 || check.sha256.len() != 64)
        {
            return fail("stock code checks are required");
        }
        if let RestartMechanism::WakeDeadline {
            query,
            request_offset,
            deadline_offset,
            ui,
            ..
        } = &self.mechanism
        {
            let fits = |offset: u32| offset.is_multiple_of(4) && offset + 4 <= query.output_bytes;
            if !fits(*request_offset)
                || !fits(*deadline_offset)
                || request_offset == deadline_offset
            {
                return fail(
                    "request and deadline must be distinct words in the query output fields",
                );
            }
            if !query.callback_offset.is_multiple_of(4) || query.stock_callback == 0 {
                return fail("invalid query callback slot");
            }
            if !matches!(ui.mode_size, 1 | 2 | 4) {
                return fail("UI mode cells are 1, 2 or 4 bytes");
            }
        }
        Ok(())
    }
}
