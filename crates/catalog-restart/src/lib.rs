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
    /// Freestanding restart before the operating system, when located.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub early_boot: Option<EarlyBootMechanism>,
}

/// A processor-specific freestanding power-controller transport.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify)
)]
pub enum EarlyBootMechanism {
    /// Polled EXR II SPI, persistent packet, wake alarm and power-hold release.
    Exr2PolledAlarm {
        /// SPI channel of the power controller's serial endpoint.
        spi_channel: u32,
        /// Exact C constants derived from the endpoint table and power path.
        constants: EarlyBootConstants,
        /// The stock endpoint-table reader and power cut establishing the transport.
        code_checks: Vec<CodeCheck>,
    },
}

/// Exact register and command values for this firmware's early restart.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify)
)]
pub struct EarlyBootConstants {
    /// Alarm enable.
    pub exr2_restart_alarm_enable: u32,
    /// Alarm high.
    pub exr2_restart_alarm_high: u32,
    /// Alarm low.
    pub exr2_restart_alarm_low: u32,
    /// Gpio banks.
    pub exr2_restart_gpio_banks: u32,
    /// Gpio base.
    pub exr2_restart_gpio_base: u32,
    /// Gpio config.
    pub exr2_restart_gpio_config: u32,
    /// Gpio inactive mask.
    pub exr2_restart_gpio_inactive_mask: u32,
    /// Gpio mode1.
    pub exr2_restart_gpio_mode1: u32,
    /// Gpio mode2.
    pub exr2_restart_gpio_mode2: u32,
    /// Gpio mode3.
    pub exr2_restart_gpio_mode3: u32,
    /// Gpio mode4.
    pub exr2_restart_gpio_mode4: u32,
    /// Gpio output.
    pub exr2_restart_gpio_output: u32,
    /// Gpio stride.
    pub exr2_restart_gpio_stride: u32,
    /// Packet bytes.
    pub exr2_restart_packet_bytes: u32,
    /// Packet register.
    pub exr2_restart_packet_register: u32,
    /// Power command.
    pub exr2_restart_power_command: u32,
    /// Power hold.
    pub exr2_restart_power_hold: u32,
    /// Power hold mask.
    pub exr2_restart_power_hold_mask: u32,
    /// Power off value.
    pub exr2_restart_power_off_value: u32,
    /// Spi base.
    pub exr2_restart_spi_base: u32,
    /// Spi busy.
    pub exr2_restart_spi_busy: u32,
    /// Spi control.
    pub exr2_restart_spi_control: u32,
    /// Spi count mask.
    pub exr2_restart_spi_count_mask: u32,
    /// Spi enable.
    pub exr2_restart_spi_enable: u32,
    /// Spi fifo control.
    pub exr2_restart_spi_fifo_control: u32,
    /// Spi fifo reset.
    pub exr2_restart_spi_fifo_reset: u32,
    /// Spi format.
    pub exr2_restart_spi_format: u32,
    /// Spi rx.
    pub exr2_restart_spi_rx: u32,
    /// Spi rx count.
    pub exr2_restart_spi_rx_count: u32,
    /// Spi rx ready.
    pub exr2_restart_spi_rx_ready: u32,
    /// Spi status.
    pub exr2_restart_spi_status: u32,
    /// Spi status clear.
    pub exr2_restart_spi_status_clear: u32,
    /// Spi tx.
    pub exr2_restart_spi_tx: u32,
    /// Spi tx count.
    pub exr2_restart_spi_tx_count: u32,
    /// Write.
    pub exr2_restart_write: u32,
    /// Gpio pin.
    pub restart_gpio_pin: u32,
    /// Panel gpio bank.
    pub restart_panel_gpio_bank: u32,
    /// Power gpio bank.
    pub restart_power_gpio_bank: u32,
    /// Spi format value.
    pub restart_spi_format_value: u32,
    /// Spi idle control.
    pub restart_spi_idle_control: u32,
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
        if let Some(EarlyBootMechanism::Exr2PolledAlarm {
            spi_channel,
            constants,
            code_checks,
        }) = &m.early_boot
            && (*spi_channel > 3
                || code_checks.len() != 2
                || code_checks
                    .iter()
                    .any(|check| check.size == 0 || check.sha256.len() != 64)
                || constants.exr2_restart_spi_base != 0xfff84000 + spi_channel * 0x40
                || constants.exr2_restart_packet_bytes != 16
                || Some(constants.exr2_restart_alarm_low)
                    != m.power_off.alarm.seconds.first().copied()
                || Some(constants.exr2_restart_alarm_high)
                    != m.power_off.alarm.seconds.get(1).copied()
                || constants.exr2_restart_alarm_enable != m.power_off.alarm.enable
                || constants.restart_gpio_pin >= 16
                || constants.restart_power_gpio_bank >= 16
                || constants.restart_panel_gpio_bank >= 16)
        {
            return fail("invalid early-boot restart mechanism");
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
