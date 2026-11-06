//! Generated stock restart facts for one firmware.
//!
//! How a camera can be restarted from its debug USB link through its own
//! power-off: the stock request that cuts power with a wake deadline, how the
//! request reaches the stock sender, and what the stock power-off writes to
//! arm the power controller's alarm. The document holds addresses, sizes and
//! hashes, never firmware bytes.
use fujimod_catalog_bulk::CodeCheck;
use serde::{Deserialize, Serialize};

pub const RESTART_SCHEMA_URI: &str = "https://fujimod.com/schemas/restart.generated.schema.json";

/// Located restart for one firmware.
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

/// The stock power-off request with a wake deadline. The power-off arms the
/// power controller's alarm and cuts power; the alarm boots the camera again.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(missing_as_null)
)]
pub struct RestartMechanism {
    /// How the host reaches the sender.
    pub transport: RequestTransport,
    /// Stock routine that sends the power-off request; its first argument is
    /// the address of the request parameters.
    pub sender: u64,
    /// Request word that selects power-off with a wake.
    pub request: u32,
    /// Byte offset of the request word in the sender's parameters.
    pub request_offset: u32,
    /// Byte offset of the wake deadline in the sender's parameters.
    pub deadline_offset: u32,
    /// Bytes of sender parameters.
    pub parameter_bytes: u32,
    /// Where REC idle is observed.
    pub ui: RecIdle,
    /// What the stock power-off does with the request.
    pub power_off: PowerOff,
    /// Optional stock timer ABI for an autonomous deferred restart. This is
    /// present only when its locator qualified a callback timer on the same
    /// firmware; delivery and callback placement remain separate mechanisms.
    pub deferred_timer: Option<DeferredTimer>,
}

/// A stock timer that calls a no-argument callback after a millisecond delay.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(missing_as_null)
)]
pub struct DeferredTimer {
    /// Normalized ABI identifier.
    pub kind: String,
    /// Unit accepted by `start`'s first argument.
    pub period_unit: String,
    /// Value of the periodic selector for a one-shot callback.
    pub one_shot_periodic: u32,
    /// Whether nonzero selector values repeat the callback.
    pub periodic_nonzero: bool,
    /// Number of arguments passed to the callback.
    pub callback_arguments: u32,
    /// Stock routine that starts a timer.
    pub start: TimerRoutine,
    /// Stock routine that cancels the handle returned by `start`.
    pub cancel: TimerRoutine,
}

/// A bounded stock routine that Rust verifies before using the timer ABI.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify)
)]
pub struct TimerRoutine {
    /// Entry address before `code_alias`.
    pub address: u64,
    /// Exact routine bytes covered by the matching code check.
    pub size: u64,
}

/// How the host makes the camera call the sender.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify)
)]
pub enum RequestTransport {
    /// A borrowed diagnostic query calls the sender with the address of the
    /// query output fields, which hold the parameters.
    BorrowedQuery {
        /// Address of the pointer to the diagnostic callback table.
        callback_table: u64,
        /// Byte offset of the query callback in that table.
        callback_offset: u64,
        /// Stock callback value.
        stock_callback: u64,
        /// Output fields passed to the callback; the query tag is their address.
        output_fields: u64,
        /// Bytes in the output fields.
        output_bytes: u32,
    },
    /// The debug console's command callback is pointed at a short stub in an
    /// unused executable area. The stub passes its parameters to the sender
    /// and returns through it; the stock callback is restored after the reply.
    ConsoleCallback {
        /// Command callback slot (a pointer the console calls for `CMD`).
        slot: u64,
        /// Stock callback value the slot must hold and gets back.
        stock_callback: u64,
        /// Unused executable area for the stub; it must read as zero.
        stub_area: u64,
        /// Bytes of the stub area.
        stub_bytes: u32,
        /// Translation entries that keep the stub area executable.
        translation: Vec<WordValue>,
        /// Words showing the stock cache maintenance is ready on every core.
        ready_words: Vec<WordValue>,
        /// Stock routine that cleans the data cache on every core.
        data_cache_clean: u64,
        /// Stock routine that invalidates the instruction cache.
        instruction_cache_sync: u64,
    },
}

/// An exact memory word.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify)
)]
pub struct WordValue {
    /// Address of the word.
    pub address: u64,
    /// Bytes in the word: 4 or 8.
    pub size: u32,
    /// Value, little-endian.
    pub value: u64,
}

/// UI state that is REC with nothing pending and no menu open.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(missing_as_null)
)]
pub struct RecIdle {
    /// Current UI mode.
    pub mode_address: u64,
    /// Bytes in each UI mode cell.
    pub mode_size: u32,
    /// UI mode value for REC.
    pub rec_mode: u32,
    /// Pending UI mode, zero when no change is pending, where located.
    pub pending_mode_address: Option<u64>,
    /// Nonzero while a menu is open, where located.
    pub menu_active_address: Option<u64>,
}

/// The stock power-off reached by the request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(missing_as_null)
)]
pub struct PowerOff {
    /// Stock power-off routine.
    pub entry: u64,
    /// Its final self-branch, reached after the power cut.
    pub halt: u64,
    /// Stock routine that cuts power.
    pub power_cut: u64,
    /// Byte offset of the deadline in the request the power-off receives.
    pub request_deadline_offset: u32,
    /// Power-controller registers for the alarm.
    pub alarm: AlarmRegisters,
    /// Persistent power-controller parameter offset of the saved deadline
    /// word, where the power-off saves it.
    pub deadline_parameter: Option<u32>,
    /// Persistent parameter offset of the alarm's rollover byte.
    pub rollover_parameter: u32,
}

/// Power-controller registers the power-off writes for the wake alarm.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify)
)]
pub struct AlarmRegisters {
    /// Registers holding the seconds until the alarm, least significant first.
    pub seconds: Vec<u32>,
    /// Register that enables the alarm.
    pub enable: u32,
    /// Value written to enable it.
    pub enable_value: u32,
}

impl FirmwareRestart {
    /// Check identity, schema and the mechanism's internal consistency.
    pub fn validate(&self) -> Result<(), String> {
        fujimod_catalog_identity::validate_id(&self.id)?;
        fujimod_catalog_identity::validate_id(&self.firmware_id)?;
        let fail = |message: &str| Err(format!("restart {}: {message}", self.id));
        if self.schema_uri != RESTART_SCHEMA_URI || self.schema != 2 {
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
        let m = &self.mechanism;
        let fits = |offset: u32| offset.is_multiple_of(4) && offset + 4 <= m.parameter_bytes;
        if !fits(m.request_offset)
            || !fits(m.deadline_offset)
            || m.request_offset == m.deadline_offset
        {
            return fail("request and deadline must be distinct parameter words");
        }
        match &m.transport {
            RequestTransport::BorrowedQuery {
                callback_offset,
                stock_callback,
                output_bytes,
                ..
            } => {
                if !callback_offset.is_multiple_of(4) || *stock_callback == 0 {
                    return fail("invalid query callback slot");
                }
                if m.parameter_bytes > *output_bytes {
                    return fail("parameters exceed the query output fields");
                }
            }
            RequestTransport::ConsoleCallback {
                slot,
                stock_callback,
                stub_area,
                stub_bytes,
                translation,
                ready_words,
                ..
            } => {
                if !slot.is_multiple_of(8) || *stock_callback == 0 {
                    return fail("invalid console callback slot");
                }
                if !stub_area.is_multiple_of(16) || *stub_bytes < 16 + m.parameter_bytes {
                    return fail("stub area cannot hold the stub and its parameters");
                }
                if translation.is_empty()
                    || translation
                        .iter()
                        .chain(ready_words)
                        .any(|w| !matches!(w.size, 4 | 8))
                {
                    return fail("translation and ready words must be 4 or 8 bytes");
                }
            }
        }
        if !matches!(m.ui.mode_size, 1 | 2 | 4) {
            return fail("UI mode cells are 1, 2 or 4 bytes");
        }
        if m.power_off.alarm.seconds.is_empty() {
            return fail("alarm seconds registers are required");
        }
        if let Some(timer) = &m.deferred_timer {
            if timer.kind != "software-timer-callback-v1"
                || timer.period_unit != "milliseconds"
                || timer.callback_arguments != 0
                || timer.start.address == 0
                || timer.cancel.address == 0
                || timer.start.size == 0
                || timer.cancel.size == 0
                || timer.start.address == timer.cancel.address
            {
                return fail("invalid deferred callback timer");
            }
            let covered = |routine: &TimerRoutine| {
                self.code_checks.iter().any(|check| {
                    check.address == routine.address && check.size == routine.size
                })
            };
            if !covered(&timer.start) || !covered(&timer.cancel) {
                return fail("deferred timer routines need matching code checks");
            }
        }
        Ok(())
    }
}
