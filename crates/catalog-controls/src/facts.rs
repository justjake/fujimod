//! Physical controls and generated firmware input/observation facts.

use serde::{Deserialize, Serialize};

/// A published manual and the pages used to identify controls.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct ManualReference {
    /// Public manual URL.
    pub url: String,
    /// Manual title.
    pub title: String,
    /// Referenced page numbers.
    pub pages: Vec<u32>,
}

/// Firmware interpretation of a control outside the key scanner.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum ControlInterface {
    /// Power switch sampled through a physical GPIO and serial status field.
    PowerSwitch {
        /// Body connection carrying the switch input.
        connection_id: String,
        /// Firmware address retaining sampled GPIO state.
        saved_inputs: u64,
        /// Firmware serial endpoint selector.
        endpoint: u32,
        /// Firmware-selected status register.
        register: u32,
        /// Mask of status bits interpreted by the firmware.
        mask: u32,
        /// Masked status value interpreted as on.
        on_value: u32,
        /// Masked status value interpreted as off.
        off_value: u32,
    },
    /// Touch reports read through a prefixed control-bus endpoint.
    PrefixedTouch {
        /// Body connection carrying control-bus traffic.
        connection_id: String,
        /// Firmware control-bus transaction mode.
        mode: u32,
        /// First report register read by the firmware.
        register: u32,
        /// Report byte count consumed by the firmware.
        report_bytes: u32,
        /// Maximum simultaneous contacts decoded by the firmware.
        contacts: u32,
        /// Number of encoded coordinate bits.
        coordinate_bits: u32,
        /// Firmware logical coordinate width.
        width: u32,
        /// Firmware logical coordinate height.
        height: u32,
        /// Configuration-byte offsets for logical coordinate correction.
        offset_tags: [u32; 2],
        /// Firmware interrupt line selected for reports.
        irq: u32,
        /// Firmware-selected interrupt priority.
        priority: u32,
        /// Firmware-selected interrupt mode.
        irq_mode: u32,
        /// Firmware memory retaining the latest report decoded by the touch task.
        decoded: DecodedTouch,
    },
}

/// Layout of the touch task's latest decoded report in firmware memory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct DecodedTouch {
    /// Firmware address of the decoded report.
    pub address: u64,
    /// Byte offset of the decoded contact count.
    pub count: u32,
    /// Byte offsets of each decoded contact, in report order.
    pub contacts: Vec<DecodedTouchContact>,
}

/// Byte offsets of one decoded contact within the report.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct DecodedTouchContact {
    /// Byte that is nonzero while the contact is down.
    pub active: u32,
    /// Little-endian 16-bit logical x coordinate.
    pub x: u32,
    /// Little-endian 16-bit logical y coordinate.
    pub y: u32,
}

/// Firmware scanner predicates used for digital keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum GpioScanner {
    /// Direct GPIO comparison using the declared expected logic level.
    Direct,
    /// Combined scanner predicate with its firmware-selected source and bounds.
    Combined {
        /// Scanner source selector.
        source: u32,
        /// Lower comparison operand.
        lower: u16,
        /// Upper comparison operand.
        upper: u16,
    },
}

/// Closed firmware scanner predicate discriminator from the scanner row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum ScannerInputKind {
    /// Direct analog level predicate.
    #[serde(rename = "kind_0")]
    Kind0,
    /// Calibrated analog interval predicate.
    #[serde(rename = "kind_1")]
    Kind1,
    /// Direct GPIO predicate.
    #[serde(rename = "kind_2")]
    Kind2,
    /// Callback or specialized digital predicate.
    #[serde(rename = "kind_3")]
    Kind3,
    /// Combined-source predicate.
    #[serde(rename = "kind_4")]
    Kind4,
    /// Callback-based scanner predicate.
    #[serde(rename = "kind_5")]
    Kind5,
}
impl ScannerInputKind {
    /// Numeric value stored in the firmware scanner row.
    pub const fn raw(self) -> u8 {
        match self {
            Self::Kind0 => 0,
            Self::Kind1 => 1,
            Self::Kind2 => 2,
            Self::Kind3 => 3,
            Self::Kind4 => 4,
            Self::Kind5 => 5,
        }
    }
}

/// Checked identity and predicate operands stored in one scanner record.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct ScannerRecordWitness {
    /// Physical key identity stored at byte offset zero.
    pub physical_key: u32,
    /// Closed predicate kind stored in the scanner record.
    pub input_kind: ScannerInputKind,
    /// Source selector stored at byte offset twelve.
    pub source: u32,
    /// Lower comparison operand or calibration-table offset, a 32-bit record word.
    pub lower: u32,
    /// Upper comparison operand or calibration-table offset, a 32-bit record word.
    pub upper: u32,
}

/// A firmware scanner state reader.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct ScannerReader {
    /// Stable-state field offset within a scanner record.
    pub stable_offset: u32,
    /// Width of the stable-state field in bytes.
    pub stable_width: u8,
    /// Stored value interpreted as a pressed key.
    pub pressed_value: u8,
    /// Stored value interpreted as a released key.
    pub released_value: u8,
    /// Reader routine identity.
    pub routine: AddressIdentity,
}

/// A firmware table used to scan physical input predicates.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct ScannerTable {
    /// Table base address.
    pub base: u64,
    /// Number of scanner records.
    pub count: u32,
    /// Scanner record size in bytes.
    pub stride: u32,
    /// Routine that looks up scanner entries.
    pub lookup: u64,
    /// SHA-256 of the scanner table.
    pub sha256: String,
}

/// Native firmware event names and the identity of their source table.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct NativeKeyNames {
    /// Native event names and their numeric identities.
    pub rows: Vec<NativeKeyName>,
    /// Routine used to read the table.
    pub reader: u64,
    /// Table byte length.
    pub size: u64,
    /// SHA-256 of the table.
    pub sha256: String,
}

/// One named native firmware event.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct NativeKeyName {
    /// Numeric event identity.
    pub event: u32,
    /// Native event name.
    pub name: String,
    /// Address of the native name string.
    pub string_address: u64,
}

/// An identified firmware address range without its contents.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct AddressIdentity {
    /// Start address.
    pub address: u64,
    /// Byte length.
    pub size: u64,
    /// SHA-256 of the range.
    pub sha256: String,
}

/// A configuration block addressed directly or through a firmware pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum ConfigurationLocation {
    /// Firmware address containing a configuration-block pointer.
    Pointer {
        /// Pointer address.
        address: u64,
    },
    /// Fixed configuration-block base address.
    Direct {
        /// Block address.
        address: u64,
    },
}

/// Firmware UI mode numbers beyond the REC and setup modes used by boot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct AdditionalUiModes {
    /// Playback UI mode.
    pub play: u32,
    /// Playback setup UI mode.
    pub setup_play: u32,
}

/// Firmware key identities used to navigate menus.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct MenuKeys {
    /// Accept or open-menu key identity.
    pub menu: String,
    /// Back key identity.
    pub back: String,
    /// Up key identity.
    pub up: String,
    /// Down key identity.
    pub down: String,
}

impl AddressIdentity {
    /// Validate the digest and nonempty extent of a firmware range identity.
    pub fn validate(&self) -> Result<(), String> {
        if self.size == 0 || self.address.checked_add(self.size).is_none() {
            return Err("firmware range identity has an empty or overflowing extent".into());
        }
        fujimod_catalog_identity::validate_content_sha256(&self.sha256.clone())
    }
}
