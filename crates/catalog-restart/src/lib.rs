//! Generated stock restart facts for one firmware.
//!
//! How a camera can be restarted from its debug USB link through its own
//! power-off: the stock request that cuts power with a wake deadline, how the
//! request reaches the stock sender, and what the stock power-off writes to
//! arm the power controller's alarm. An X-Processor 4 body instead publishes
//! the message request its core hands to the processor that acts on it; that
//! image holds no power-off, so the document carries none, and states that no
//! camera run has qualified the path. The document holds addresses, sizes and
//! hashes, never firmware bytes.
use fujimod_catalog_bulk::CodeCheck;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const RESTART_SCHEMA_URI: &str = "https://fujimod.com/schemas/restart.generated.schema.json";

/// Document version for a request the stock power-off sends, so the wake alarm
/// it arms brings the camera back after the cut.
pub const WAKE_DEADLINE_SCHEMA: u32 = 2;

/// Document version for a request that leaves the core for another processor,
/// with no power-off the image can prove.
pub const XP4_WAKE_DEADLINE_SCHEMA: u32 = 3;

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
    /// Document version: [`WAKE_DEADLINE_SCHEMA`] or
    /// [`XP4_WAKE_DEADLINE_SCHEMA`].
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
    /// Which mechanism this is. Absent from the wake-deadline documents
    /// schema 2 started with; `xp4-wake-deadline` in schema 3.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<RestartKind>,
    /// How the host reaches the sender, where the firmware takes a request.
    /// A firmware whose power-off arms a fixed-interval alarm instead carries
    /// none of the request fields, and `power_off.fixed_interval` says so.
    pub transport: Option<RequestTransport>,
    /// Stock routine that sends the power-off request; its first argument is
    /// the address of the request parameters.
    pub sender: Option<u64>,
    /// Request word that selects power-off with a wake.
    pub request: Option<u32>,
    /// Byte offset of the request word in the sender's parameters.
    pub request_offset: Option<u32>,
    /// Byte offset of the wake deadline in the sender's parameters.
    pub deadline_offset: Option<u32>,
    /// Bytes of sender parameters.
    pub parameter_bytes: Option<u32>,
    /// Where the wake deadline travels once the request leaves the core.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline: Option<PayloadDeadline>,
    /// Queue that carries the built request to the processor that acts on it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue: Option<MessageQueue>,
    /// Request-word rewrite the sender applies before sending.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_conversion: Option<RequestConversion>,
    /// Debug-configuration keys that steer the request path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configuration_override: Option<ConfigurationOverride>,
    /// Stock clock the deadline is compared against.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clock: Option<ClockIo>,
    /// Persistent storage for the sender's parameter record. An X-Processor 4
    /// document publishes none, and `validate` refuses one until a locator
    /// proves where the record is kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameter_storage: Option<ParameterStorage>,
    /// Whether a camera run confirmed the mechanism's runtime behavior. False
    /// states that the request was located but the path never ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_qualified: Option<bool>,
    /// Where REC idle is observed.
    pub ui: RecIdle,
    /// What the stock power-off does with the request. Schema 3 publishes none:
    /// that image holds no power-off to describe.
    pub power_off: Option<PowerOff>,
    /// Optional stock timer ABI for an autonomous deferred restart. This is
    /// present only when its locator qualified a callback timer on the same
    /// firmware; delivery and callback placement remain separate mechanisms.
    pub deferred_timer: Option<DeferredTimer>,
    /// Freestanding restart before the operating system, when located.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub early_boot: Option<EarlyBootMechanism>,
}

/// The mechanism a restart document describes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify)
)]
pub enum RestartKind {
    /// Message 3 on queue 2 for the processor that acts on it, with the wake
    /// deadline as absolute epoch seconds in the queued payload.
    Xp4WakeDeadline,
}

/// Where the wake deadline travels once the request leaves the core.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify)
)]
pub struct PayloadDeadline {
    /// Byte offset of the deadline in the queued payload.
    pub payload_offset: u32,
    /// Unit of the deadline word.
    pub unit: DeadlineUnit,
}

/// The unit a queued deadline is expressed in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify)
)]
pub enum DeadlineUnit {
    /// Absolute seconds since the epoch the stock clock counts.
    EpochSeconds,
}

/// The message queue a request leaves the core on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify)
)]
pub struct MessageQueue {
    /// Queue identifier the request is sent on.
    pub id: u32,
    /// Stock routine that builds the queue message.
    pub sender: u64,
    /// Stock routine that enqueues the built message.
    pub enqueue: u64,
    /// Bytes in one payload.
    pub payload_bytes: u32,
    /// Sender parameters carried in the payload, keyed by their payload byte
    /// offset and mapped to their parameter byte offset.
    #[cfg_attr(feature = "schema", schemars(with = "BTreeMap<String, u32>"))]
    pub parameters: BTreeMap<u32, u32>,
    /// Payload bytes the sender leaves uninitialized.
    pub uninitialized_payload_offsets: Vec<u32>,
}

/// The request-word rewrite a sender applies before it builds its message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify)
)]
pub struct RequestConversion {
    /// Request word the host passes.
    pub source: u32,
    /// Request word the sender sends instead.
    pub target: u32,
}

/// Debug-configuration keys that steer the request path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify)
)]
pub struct ConfigurationOverride {
    /// Configuration key that enables the path.
    pub enable_key: u32,
    /// Configuration keys the path reads as its delays.
    pub delay_keys: Vec<u32>,
}

/// The stock clock the wake deadline is compared against.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify)
)]
pub struct ClockIo {
    /// Stock routine that reads the counter.
    pub reader: u64,
    /// Stock routine that writes the counter.
    pub writer: u64,
    /// Stock routine that reads one counter register.
    pub register_reader: u64,
    /// Stock routine that writes one counter register.
    pub register_writer: u64,
    /// Serial transaction both register helpers target.
    pub transaction: u64,
    /// Byte both register helpers read as their bank.
    pub bank_pointer: u64,
    /// Counter registers.
    pub registers: ClockRegisters,
    /// Register selectors read, in order, to take one counter reading.
    pub read_order: Vec<u32>,
    /// Value the writer stores in the control register to start the counter.
    pub start_value: u32,
    /// Value the writer stores in the control register to stop it.
    pub stop_value: u32,
}

/// The counter registers a writer frames and a reader selects.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify)
)]
pub struct ClockRegisters {
    /// Control register the start and stop values are written to.
    pub control: u32,
    /// Low halfword of the counter.
    pub low: u32,
    /// High halfword of the counter.
    pub high: u32,
}

/// Persistent storage for the parameter record a sender takes. No document
/// publishes one yet: an X-Processor 4 image's query output is smaller than the
/// record, and no locator has placed the copy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify)
)]
pub struct ParameterStorage {
    /// Where the record is kept.
    pub address: u64,
    /// Bytes kept.
    pub bytes: u32,
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
        /// Stock routine that decodes the query tag, where the image locates
        /// the decoder separately from the query (X-Processor 4).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tag_decoder: Option<u64>,
        /// Stock routine that forwards a decoded tag to the callback table,
        /// where the image locates that routine (X-Processor 4).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        forward: Option<u64>,
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
    pub request_deadline_offset: Option<u32>,
    /// Power-controller registers for the alarm.
    pub alarm: AlarmRegisters,
    /// Persistent power-controller parameter offset of the saved deadline
    /// word, where the power-off saves it.
    pub deadline_parameter: Option<u32>,
    /// Persistent parameter offset of the alarm's rollover byte.
    pub rollover_parameter: Option<u32>,
    /// The interval the power-off arms instead of a request deadline.
    pub fixed_interval: Option<AlarmInterval>,
    /// Wake header bit the alarm boot tests before a normal boot, where the
    /// power-off tests one.
    pub boot_flag: Option<BootFlag>,
}

/// A byte the alarm boot tests for a normal boot. Without the bit, the wake
/// takes a boot that never reaches USB. The restart sets the bit before the
/// request; the power-off publishes it and the firmware clears it after the
/// wake.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify)
)]
pub struct BootFlag {
    /// Address of the byte.
    pub address: u64,
    /// The single bit the restart ORs into the byte before the request.
    pub mask: u8,
}

/// Constant alarm values a power-off writes instead of taking a deadline.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(missing_as_null)
)]
pub struct AlarmInterval {
    /// Values written to the alarm's seconds registers, least significant first.
    pub seconds: Vec<u32>,
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
        if self.schema_uri != RESTART_SCHEMA_URI {
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
        if !matches!(self.mechanism.ui.mode_size, 1 | 2 | 4) {
            return fail("UI mode cells are 1, 2 or 4 bytes");
        }
        match self.schema {
            WAKE_DEADLINE_SCHEMA => self.validate_wake_deadline(),
            XP4_WAKE_DEADLINE_SCHEMA => self.validate_xp4_wake_deadline(),
            _ => fail("unsupported schema"),
        }
    }

    /// A request the stock power-off sends, so the wake alarm it arms returns.
    fn validate_wake_deadline(&self) -> Result<(), String> {
        let fail = |message: &str| Err(format!("restart {}: {message}", self.id));
        let m = &self.mechanism;
        if m.kind.is_some()
            || m.deadline.is_some()
            || m.queue.is_some()
            || m.request_conversion.is_some()
            || m.configuration_override.is_some()
            || m.clock.is_some()
            || m.parameter_storage.is_some()
            || m.runtime_qualified.is_some()
        {
            return fail("a wake-deadline restart carries no message-queue facts");
        }
        let Some(power_off) = &m.power_off else {
            return fail("a wake-deadline restart publishes its stock power-off");
        };
        let request = (
            m.sender,
            m.request,
            m.request_offset,
            m.deadline_offset,
            m.parameter_bytes,
        );
        match (&m.transport, power_off.fixed_interval.as_ref()) {
            (Some(_), Some(_)) => {
                return fail("a fixed-interval wake takes no request");
            }
            (None, None) => {
                return fail(
                    "a restart needs either a request transport or a fixed-interval wake",
                );
            }
            (None, Some(interval)) => {
                if request != (None, None, None, None, None)
                    || power_off.request_deadline_offset.is_some()
                    || power_off.deadline_parameter.is_some()
                    || power_off.rollover_parameter.is_some()
                {
                    return fail("a fixed-interval wake carries no request fields");
                }
                if interval.seconds.len() != power_off.alarm.seconds.len()
                    || interval.seconds.iter().all(|seconds| *seconds == 0)
                {
                    return fail("a fixed-interval wake needs a nonzero alarm interval");
                }
            }
            (Some(transport), None) => {
                let (Some(_), Some(_), Some(request_offset), Some(deadline_offset), Some(bytes)) =
                    request
                else {
                    return fail("a request restart needs every request field");
                };
                if power_off.request_deadline_offset.is_none()
                    || power_off.rollover_parameter.is_none()
                {
                    return fail("a request restart saves the deadline and its rollover byte");
                }
                if let Err(reason) = validate_transport(
                    transport,
                    bytes,
                    request_offset,
                    deadline_offset,
                    true,
                ) {
                    return fail(reason);
                }
            }
        }
        if power_off.alarm.seconds.is_empty() {
            return fail("alarm seconds registers are required");
        }
        if let Some(flag) = &power_off.boot_flag {
            if m.transport.is_none() {
                return fail("a boot flag is set only before a request");
            }
            if !flag.mask.is_power_of_two() {
                return fail("a boot flag is a single bit");
            }
        }
        self.validate_optional_mechanisms()
    }

    /// A message request for the processor that acts on it, with no power-off
    /// the image can prove. The request word, the queue layout and the clock
    /// must agree with each other, and the document states its qualification.
    fn validate_xp4_wake_deadline(&self) -> Result<(), String> {
        let fail = |message: &str| Err(format!("restart {}: {message}", self.id));
        let m = &self.mechanism;
        if m.kind != Some(RestartKind::Xp4WakeDeadline) {
            return fail("an X-Processor 4 restart names its mechanism");
        }
        if m.power_off.is_some() {
            return fail("this image publishes no power-off the request reaches");
        }
        if m.parameter_storage.is_some() {
            return fail("this document publishes no parameter storage");
        }
        if m.runtime_qualified.is_none() {
            return fail("an X-Processor 4 restart states its runtime qualification");
        }
        if m.deferred_timer.is_some() || m.early_boot.is_some() {
            return fail("this restart publishes no callback timer or early boot");
        }
        let (
            Some(transport),
            Some(request),
            Some(request_offset),
            Some(deadline_offset),
            Some(bytes),
        ) = (
            m.transport.as_ref(),
            m.request,
            m.request_offset,
            m.deadline_offset,
            m.parameter_bytes,
        ) else {
            return fail("a message request needs every request field");
        };
        let (Some(deadline), Some(queue), Some(conversion), Some(configuration), Some(clock)) = (
            m.deadline.as_ref(),
            m.queue.as_ref(),
            m.request_conversion.as_ref(),
            m.configuration_override.as_ref(),
            m.clock.as_ref(),
        ) else {
            return fail(
                "a message request needs its deadline, queue, conversion, configuration and clock",
            );
        };
        match transport {
            RequestTransport::BorrowedQuery {
                tag_decoder,
                forward,
                ..
            } => {
                if tag_decoder.is_none() || forward.is_none() {
                    return fail("the message request publishes its tag decoder and forwarder");
                }
            }
            RequestTransport::ConsoleCallback { .. } => {
                return fail("a message request leaves through the diagnostic query");
            }
        }
        // The X-Processor 4 query output is smaller than the sender's record;
        // the document says so by publishing no parameter storage.
        if let Err(reason) =
            validate_transport(transport, bytes, request_offset, deadline_offset, false)
        {
            return fail(reason);
        }
        if request == 0 || queue.sender == 0 || queue.enqueue == 0 {
            return fail("a message request needs its request word, sender and enqueue");
        }
        if conversion.source == 0
            || conversion.target == 0
            || conversion.source == conversion.target
        {
            return fail("the sender rewrites one request word into another");
        }
        if conversion.target != request {
            return fail("the request word is the word the sender sends");
        }
        if queue.payload_bytes == 0
            || queue.parameters.is_empty()
            || queue
                .parameters
                .values()
                .all(|offset| *offset != request_offset)
            || queue.parameters.get(&deadline.payload_offset) != Some(&deadline_offset)
        {
            return fail("the queue layout carries the request and deadline parameters");
        }
        for (&payload_offset, &parameter_offset) in &queue.parameters {
            if u64::from(payload_offset) + 4 > u64::from(queue.payload_bytes)
                || u64::from(parameter_offset) + 4 > u64::from(bytes)
            {
                return fail("a queued parameter starts past the payload or the parameters");
            }
        }
        if queue
            .uninitialized_payload_offsets
            .iter()
            .any(|offset| *offset >= queue.payload_bytes || queue.parameters.contains_key(offset))
        {
            return fail("an uninitialized payload byte holds no parameter");
        }
        if u64::from(deadline.payload_offset) + 4 > u64::from(queue.payload_bytes) {
            return fail("the deadline starts past the payload");
        }
        if configuration.delay_keys.is_empty()
            || configuration.delay_keys.contains(&configuration.enable_key)
        {
            return fail("the configuration override needs its enable and delay keys");
        }
        if clock.reader == 0
            || clock.writer == 0
            || clock.register_reader == 0
            || clock.register_writer == 0
            || clock.transaction == 0
            || clock.bank_pointer == 0
            || clock.registers.control == 0
            || clock.registers.low == 0
            || clock.registers.high == 0
            || clock.registers.low == clock.registers.high
            || clock.read_order.is_empty()
            || clock.read_order.iter().any(|register| {
                *register != clock.registers.low && *register != clock.registers.high
            })
            || clock.start_value == clock.stop_value
        {
            return fail("the clock publishes its routines, registers and read order");
        }
        Ok(())
    }

    /// Early boot and deferred timer facts a wake-deadline mechanism may carry.
    fn validate_optional_mechanisms(&self) -> Result<(), String> {
        let fail = |message: &str| Err(format!("restart {}: {message}", self.id));
        let m = &self.mechanism;
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
                    != m.power_off.as_ref().and_then(|p| p.alarm.seconds.first().copied())
                || Some(constants.exr2_restart_alarm_high)
                    != m.power_off.as_ref().and_then(|p| p.alarm.seconds.get(1).copied())
                || m.power_off.as_ref().is_none_or(|p| {
                    constants.exr2_restart_alarm_enable != p.alarm.enable
                })
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

/// The request transport's own consistency, once the request fields are present.
fn validate_transport(
    transport: &RequestTransport,
    parameter_bytes: u32,
    request_offset: u32,
    deadline_offset: u32,
    output_holds_parameters: bool,
) -> Result<(), &'static str> {
    let fits = |offset: u32| offset.is_multiple_of(4) && offset + 4 <= parameter_bytes;
    if !fits(request_offset) || !fits(deadline_offset) || request_offset == deadline_offset {
        return Err("request and deadline must be distinct parameter words");
    }
    match transport {
        RequestTransport::BorrowedQuery {
            callback_offset,
            stock_callback,
            output_bytes,
            ..
        } => {
            if !callback_offset.is_multiple_of(4) || *stock_callback == 0 {
                return Err("invalid query callback slot");
            }
            if output_holds_parameters && parameter_bytes > *output_bytes {
                return Err("parameters exceed the query output fields");
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
                return Err("invalid console callback slot");
            }
            if !stub_area.is_multiple_of(16) || *stub_bytes < 16 + parameter_bytes {
                return Err("stub area cannot hold the stub and its parameters");
            }
            if translation.is_empty()
                || translation
                    .iter()
                    .chain(ready_words)
                    .any(|word| !matches!(word.size, 4 | 8))
            {
                return Err("translation and ready words must be 4 or 8 bytes");
            }
        }
    }
    Ok(())
}
