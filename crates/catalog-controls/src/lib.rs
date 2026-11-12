//! Physical control declarations and firmware-located input identities.
//!
//! A binding names one vocabulary explicitly. Numeric event identities establish
//! the checked relationship; spelling is never used to translate vocabularies.

pub mod facts;
pub mod ladders;

pub use crate::ladders::{ControlLadder, LadderRung, LadderWire};

use crate::facts::{
    ControlInterface, DecodedTouch, GpioScanner, ManualReference, NativeKeyNames, ScannerReader,
    ScannerInputKind, ScannerRecordWitness, ScannerTable,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const PHYSICAL_CONTROLS_SCHEMA_URI: &str =
    "https://fujimod.com/schemas/physical-controls.schema.json";
pub const CONTROL_IDS_SCHEMA_URI: &str = "https://fujimod.com/schemas/control-ids.schema.json";

/// One explicitly located command in the firmware's debug transport vocabulary.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct DebugKey {
    /// Exact command token accepted by that transport.
    pub name: String,
    /// UI event identity delivered by the command.
    pub event: u32,
}

/// Hand-maintained physical controls for one body.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct PhysicalControls {
    /// Schema identifying this document's closed Rust type.
    #[serde(rename = "$schema")]
    pub schema_uri: String,
    /// Document format version.
    pub schema: u32,
    /// Stable document identity referenced by the body.
    pub id: String,
    /// Body owning the documented controls.
    pub body_id: String,
    /// Published manual identifying the controls.
    pub manual: ManualReference,
    /// Analog sample resolution used by this body's contacts.
    pub analog_levels: Option<u16>,
    /// Confirmed inputs held while powering on.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[cfg_attr(
        all(target_arch = "wasm32", not(target_os = "emscripten")),
        tsify(type = "Partial<Record<BootMode, BootCombination>>")
    )]
    pub boot: BTreeMap<BootMode, BootCombination>,
    /// Controls keyed by their canonical physical identities.
    pub controls: BTreeMap<String, PhysicalControl>,
    /// Momentary controls read as bands of one ADC channel or power-controller
    /// register; a control without an established band has no rung.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[cfg_attr(
        all(target_arch = "wasm32", not(target_os = "emscripten")),
        tsify(optional)
    )]
    pub ladders: Vec<ControlLadder>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify)
)]
#[serde(rename_all = "snake_case")]
pub enum BootMode {
    Sd,
    FirmwareUpdate,
    Service,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
#[serde(deny_unknown_fields)]
pub struct BootCombination {
    /// Required USB cable state, when specified.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(
        all(target_arch = "wasm32", not(target_os = "emscripten")),
        tsify(optional)
    )]
    pub usb: Option<bool>,
    pub hold: Vec<BootHold>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
#[serde(deny_unknown_fields)]
pub struct BootHold {
    /// Control action identity, including a pad direction or dial .press action.
    pub id: String,
    /// Shutter stage, when required.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(
        all(target_arch = "wasm32", not(target_os = "emscripten")),
        tsify(optional)
    )]
    pub stage: Option<String>,
}

/// A control's documented appearance and declared physical actions.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct PhysicalControl {
    /// Manual name of the control.
    pub name: String,
    /// Surface on which the control is located.
    pub location: ControlSurface,
    /// Manual page identifying the control, when available.
    #[serde(default)]
    pub manual_page: Option<u32>,
    /// Text printed on the control, when present.
    #[serde(default)]
    pub face: Option<String>,
    /// Meaning of a printed icon, when present.
    #[serde(default)]
    pub icon: Option<ControlIcon>,
    /// Short uppercase caption for an unmarked control (at most 10 characters).
    #[serde(default)]
    pub short_name: Option<String>,
    /// Body connections carrying the control's electrical signals.
    pub connection_ids: Vec<String>,
    /// Whether the control's function can be assigned in the camera.
    pub assignable: bool,
    /// Whether the manual documents a press action, independently of its binding.
    pub pushable: bool,
    /// Documented positions or stages, including controls with no located inputs.
    pub positions: Vec<ControlPosition>,
    /// Documented initial function, when available.
    #[serde(default)]
    pub default_function: Option<String>,
    /// Physical role required by a declared cross-body operation.
    #[serde(default)]
    pub role: Option<ControlRole>,
    /// Whether this control has no electronic action.
    pub mechanical: bool,
    /// Actions defined by this control's mechanism; absent for mechanical controls.
    #[serde(default)]
    pub behavior: Option<ControlBehavior>,
}

/// Printed symbols on a camera control.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum ControlIcon {
    Playback,
    Delete,
    Record,
    Flash,
}

/// Physical roles that an operation may require from a body.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum ControlRole {
    /// Relative command dial used to change the selected value.
    CommandControl,
}

/// Body surfaces used to locate a physical control in a manual.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum ControlSurface {
    /// Upper body surface.
    Top,
    /// Rear body surface.
    Rear,
    /// Front surface, including controls on the lens.
    Front,
    /// Lower body surface.
    Bottom,
    /// Controls mounted on the lens barrel.
    Lens,
    /// Left side of the body.
    Left,
    /// Right side of the body.
    Right,
}

/// A debug key, native input name, or exact located scanner-row identity.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum InputBinding {
    /// Command name accepted by the firmware's debug transport.
    DebugKey {
        /// Exact command token, without conversion from a native event name.
        name: String,
    },
    /// Scanned native input which need not have any debug transport command.
    NativeInput {
        /// Exact name in the firmware's located native input vocabulary.
        name: String,
    },
    /// Stable identity of a located scanner row, even when it has no command name.
    ScannerRow {
        /// Generated scanner row identity, such as `row-69`.
        row: String,
    },
}

/// The actions offered by a physical control's mechanism.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum ControlBehavior {
    /// A momentary button.
    Button {
        /// Input asserted by a press and deasserted by release.
        press: Option<InputBinding>,
    },
    /// A shutter release with named stages.
    Shutter {
        /// Inputs asserted at each named shutter stage.
        stages: BTreeMap<String, Option<Vec<InputBinding>>>,
    },
    /// A directional pad with individually addressable direction identities.
    Pad {
        /// Inputs asserted by the four directions.
        #[cfg_attr(
            all(target_arch = "wasm32", not(target_os = "emscripten")),
            tsify(type = "Partial<Record<PadDirection, InputBinding | null>>")
        )]
        directions: BTreeMap<PadDirection, Option<InputBinding>>,
        /// Center press input, when present.
        #[serde(default)]
        press: Option<InputBinding>,
    },
    /// A spring-centered control with separate left and right contacts.
    Rocker {
        decrease: Option<InputBinding>,
        increase: Option<InputBinding>,
        #[serde(default)]
        press: Option<InputBinding>,
    },
    /// A dial with positions or relative steps.
    Dial {
        /// Documented positions or relative step directions.
        motion: ControlMotion,
        /// Press input, when the dial is pushable.
        #[serde(default)]
        press: Option<InputBinding>,
        /// Whether the mechanism rotates without an end stop.
        endless: Option<bool>,
    },
    /// A ring with positions or relative steps.
    Ring {
        /// Documented positions or relative step directions.
        motion: ControlMotion,
        /// Press input, when the ring is pushable.
        #[serde(default)]
        press: Option<InputBinding>,
        /// Whether the mechanism rotates without an end stop.
        endless: Option<bool>,
    },
    /// A switch with named positions.
    Switch {
        /// Inputs selecting each documented position.
        positions: Vec<BoundPosition>,
    },
    /// A lever with named positions.
    Lever {
        /// Inputs selecting each documented position.
        positions: Vec<BoundPosition>,
        /// Whether releasing the lever returns it to an unasserted rest position.
        momentary: Option<bool>,
    },
    /// A touch surface whose protocol is located in the firmware input document.
    Touch,
}

/// A rotary mechanism's mutually exclusive position or step declarations.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum ControlMotion {
    /// Finite, named mechanical detents.
    Positions {
        /// Input and asserted level for each detent.
        positions: Vec<BoundPosition>,
    },
    /// Relative movement in either direction.
    Steps {
        /// Input generated by one decreasing step.
        decrease: Option<InputBinding>,
        /// Input generated by one increasing step.
        increase: Option<InputBinding>,
    },
}

/// A position's printed identity and explicit input selection.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct BoundPosition {
    /// Exact position identity accepted by the physical panel.
    pub id: String,
    /// Explicit firmware input binding.
    pub key: Option<InputBinding>,
    /// Whether this position asserts that input.
    pub asserted: Option<bool>,
}

/// A documented control position, independent of any firmware binding.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct ControlPosition {
    /// Unique manual position identity.
    pub id: String,
    /// Printed label, when present.
    pub label: Option<String>,
    /// Meaning of an unlabeled position or icon.
    pub description: Option<String>,
}

/// Directions addressable as `<pad-id>.<direction>`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum PadDirection {
    /// Upper direction.
    Up,
    /// Lower direction.
    Down,
    /// Left direction.
    Left,
    /// Right direction.
    Right,
}

impl PadDirection {
    /// Canonical final component of a direction's control identity.
    pub const fn id(self) -> &'static str {
        match self {
            Self::Up => "up",
            Self::Down => "down",
            Self::Left => "left",
            Self::Right => "right",
        }
    }
}

/// Generated firmware input facts, separate from the body's declarations.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct FirmwareControlIds {
    /// Schema identifying the generated document's closed Rust type.
    #[serde(rename = "$schema")]
    pub schema_uri: String,
    /// Document format version.
    pub schema: u32,
    /// Stable document identity referenced by the firmware.
    pub id: String,
    /// Firmware from which these inputs were located.
    pub firmware_id: String,
    /// Scanner table identity and extent.
    pub scanner: ScannerTable,
    /// Calibration and stable-state readers located in that firmware.
    pub input_readers: InputReaders,
    /// Native names and their source table identity, when the table was located.
    #[serde(default)]
    pub native_key_names: Option<NativeKeyNames>,
    /// Physical-key to UI-event table of a processor whose scanner rows hold
    /// physical keys; absent when the rows hold UI events directly.
    #[serde(default)]
    pub event_translation: Option<EventTranslation>,
    /// Exact debug command vocabulary, independent of the native names.
    pub debug_keys: Vec<DebugKey>,
    /// Every located scanner or protocol input, including unclaimed connections.
    pub inputs: Vec<LocatedInput>,
    /// Scanner rows whose physical key has no event-translation row, so the
    /// firmware delivers no UI event for them.
    pub undelivered_rows: Vec<LocatedScannerRecord>,
    /// Protocols outside the key scanner, keyed by physical body connection ID.
    pub interfaces: BTreeMap<String, ControlInterface>,
}

/// A located table translating scanner physical keys to UI events.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct EventTranslation {
    /// Table base address.
    pub address: u64,
    /// Table row size in bytes.
    pub stride: u32,
    /// SHA-256 of the rows and their terminator.
    pub sha256: String,
    /// Translated rows in table order.
    pub rows: Vec<TranslatedKey>,
}

/// One physical key and the UI event the firmware delivers for it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct TranslatedKey {
    /// Physical key stored in scanner rows.
    pub physical_key: u32,
    /// UI event delivered for that key.
    pub event: u32,
}

/// Input routines located in one firmware; a routine the locators did not find is absent.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct InputReaders {
    /// Routine reading 16-bit control calibration values.
    #[serde(default)]
    pub calibration: Option<u64>,
    /// Routine reading stable key scanner state.
    #[serde(default)]
    pub scanner: Option<ScannerReader>,
}

/// One located input's identity, ownership, and checked electrical predicates.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct LocatedInput {
    /// Stable identity of the located input record.
    pub id: String,
    /// UI/native event identity established by the locator.
    pub event: u32,
    /// Exact debug command token for this input, absent for native-only inputs.
    #[serde(default)]
    pub debug_key: Option<String>,
    /// Physical control, connection detector, or unclaimed firmware input.
    pub target: InputTarget,
    /// Scanner row and its source witness, absent for inputs outside that table.
    #[serde(default)]
    pub scanner: Option<LocatedScannerRecord>,
    /// Located electrical interpretation, absent when the hardware is not modeled.
    #[serde(default)]
    pub electrical: Option<ElectricalInput>,
}

/// One scanner row's identity and located predicate operands.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct LocatedScannerRecord {
    /// Zero-based index within the identified scanner table.
    pub index: u32,
    /// Event identity, predicate kind, source selector, and bounds stored in the row.
    pub witness: ScannerRecordWitness,
}

/// Configuration-byte offsets used by a calibrated analog phase decoder.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct AdcEncoderCalibration {
    /// Little-endian u16 center offsets, one for each channel.
    pub centers: [u16; 2],
    /// Little-endian signed u16 center adjustment offset.
    pub offset: u16,
    /// Byte holding the Schmitt half-band width.
    pub hysteresis: u16,
    /// Byte holding accepted edges per native direction event.
    pub edges_per_step: u16,
    /// Byte selecting normal or reversed native direction.
    pub direction: u16,
}

/// Located electrical meaning, separate from scanner table operands.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum ElectricalInput {
    /// A callback whose Boolean result is the conjunction of several ADC bands.
    AdcConjunction {
        /// Every term must hold for the callback to return one.
        terms: Vec<AdcTerm>,
        /// Located callback evaluating all terms.
        callback: u64,
        /// Callback result interpreted as an active input.
        expected_result: u32,
    },
    /// Analog input interpreted using located sample bands or calibration offsets.
    Adc {
        /// Body connection carrying this analog input.
        connection_id: String,
        /// Accepted analog intervals and the source of their bounds.
        intervals: Vec<AdcInterval>,
        /// Firmware callback evaluating the input, when present.
        #[serde(default)]
        callback: Option<u64>,
        /// Callback result interpreted as active, when present.
        #[serde(default)]
        expected_result: Option<u32>,
    },
    /// Digital contact interpreted by a located scanner predicate.
    Gpio {
        /// Body connection carrying this digital input.
        connection_id: String,
        /// Logic level interpreted as active.
        expected_level: bool,
        /// Firmware interpretation of the contact.
        scanner: GpioScanner,
    },
    /// Conjunction of the ordered digital contacts in one body connection.
    GpioContacts {
        /// Body connection carrying all of the contacts, in predicate order.
        connection_id: String,
        /// Required logic level of each contact.
        expected_levels: Vec<bool>,
        /// Firmware callback evaluating the conjunction.
        callback: u64,
    },
    /// Physical encoder interpreted by a located quadrature decoder.
    GpioQuadrature {
        /// Body connection carrying the encoder phases.
        connection_id: String,
        /// Decoder phase mapping.
        phase_map: [u8; 4],
        /// Decoder transition values.
        transitions: [u8; 16],
        /// Decoder reset counters and phase state.
        reset: [u32; 4],
        /// Decoder-local direction event, distinct from the UI event.
        decoder_event: u32,
        /// Located decoder guard address.
        guard: u64,
        /// Located decoder activation threshold.
        threshold: u32,
    },
    /// Physical encoder whose decoder emits each accepted transition without a latch.
    GpioTransition {
        /// Ordered body connection carrying the two encoder phases.
        connection_id: String,
        /// Decoder phase mapping.
        phase_map: [u8; 4],
        /// Decoder transition values indexed by new phase, then old phase.
        transitions: [u8; 16],
        /// Decoder-local direction event, distinct from the UI event.
        decoder_event: u32,
        /// Located decoder guard address.
        guard: u64,
        /// Located decoder activation threshold.
        threshold: u32,
    },
    /// Calibrated analog phase decoder with a native edge accumulator.
    AdcTransition {
        /// Ordered body connection carrying the two analog channels.
        connection_id: String,
        /// Decoder phase mapping.
        phase_map: [u8; 4],
        /// Decoder transition values indexed by new phase, then old phase.
        transitions: [u8; 16],
        /// Decoder-local direction event, distinct from the UI event.
        decoder_event: u32,
        /// Configuration offsets for thresholds, edge count, and direction.
        calibration_offsets: AdcEncoderCalibration,
    },
    /// Physical encoder whose firmware counts whole phase cycles, one event per cycle.
    GpioFixedCycle {
        /// Body connection carrying the encoder phases.
        connection_id: String,
        /// Sign of the physical phase cycle that produces this input's event.
        cycle_sign: i8,
    },
    /// Contact reported as bits of a serial status register and tested by a scanner callback.
    SerialStatus {
        /// Body connection carrying the status bits.
        connection_id: String,
        /// Firmware serial endpoint selector.
        endpoint: u32,
        /// Status register read by the firmware.
        register: u32,
        /// Status bits tested by the firmware.
        mask: u32,
        /// Masked status value while the input is active.
        active_value: u32,
    },
}

/// One ADC connection and the band required by a compound callback.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct AdcTerm {
    /// Body connection carrying this sample.
    pub connection_id: String,
    /// Unconditional interval tested on this sample.
    pub interval: AdcInterval,
}

impl ElectricalInput {
    fn validate_conjunction(&self) -> Result<(), String> {
        if let Self::AdcConjunction {
            terms,
            callback,
            expected_result,
        } = self
        {
            let mut seen = BTreeSet::new();
            if !(2..=12).contains(&terms.len())
                || *callback == 0
                || callback % 4 != 0
                || *expected_result > 1
                || terms.iter().any(|term| {
                    term.connection_id.is_empty()
                        || !seen.insert(&term.connection_id)
                        || !term.interval.bounds_valid()
                        || term.interval.condition().is_some()
                        || !term.interval.exclusions().is_empty()
                })
            {
                return Err("invalid ADC conjunction".into());
            }
        }
        Ok(())
    }
}

/// Bounds of one accepted analog interval.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum AdcInterval {
    /// Sample values stored directly in the firmware's input predicate.
    Literal {
        /// Inclusive lower sample value.
        lower: u16,
        /// Inclusive upper sample value.
        upper: u16,
        /// Scanner state the firmware requires before the interval counts, when present.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        when: Option<AdcCondition>,
        /// Unconditional intervals whose samples the firmware excludes from this one.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        unless: Vec<AdcInterval>,
    },
    /// Byte offsets of little-endian u16 bounds in the loaded configuration.
    ConfigurationU16 {
        /// Byte offset of the inclusive lower sample value.
        lower: u32,
        /// Byte offset of the inclusive upper sample value.
        upper: u32,
        /// Scanner state the firmware requires before the interval counts, when present.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        when: Option<AdcCondition>,
        /// Unconditional intervals whose samples the firmware excludes from this one.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        unless: Vec<AdcInterval>,
    },
}

impl AdcInterval {
    /// Scanner state required before the interval counts.
    pub fn condition(&self) -> Option<&AdcCondition> {
        match self {
            Self::Literal { when, .. } | Self::ConfigurationU16 { when, .. } => when.as_ref(),
        }
    }

    /// Intervals excluded from this one.
    pub fn exclusions(&self) -> &[AdcInterval] {
        match self {
            Self::Literal { unless, .. } | Self::ConfigurationU16 { unless, .. } => unless,
        }
    }

    fn bounds_valid(&self) -> bool {
        match self {
            Self::Literal { lower, upper, .. } => lower <= upper,
            Self::ConfigurationU16 { lower, upper, .. } => *lower != u32::MAX && *upper != u32::MAX,
        }
    }
}

/// Scanner state a callback requires before an analog interval counts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum AdcCondition {
    /// No listed scanner row holds its stable pressed state.
    RowsInactive {
        /// Scanner row indices of the same table, excluding the input's own row.
        rows: Vec<u32>,
    },
    /// The input's own scanner row already holds its stable pressed state.
    SelfActive,
}

/// Ownership of a generated input record without a control alias map.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum InputTarget {
    /// Input claimed by a canonical physical control identity.
    Control {
        /// Exact control identity, including a pad direction where applicable.
        control_id: String,
    },
    /// Card, cable, door, or other connection detector.
    Connection {
        /// Exact connection identity declared by the body.
        connection_id: String,
    },
    /// Located firmware input which no documented physical control claims.
    Unclaimed,
}

/// A checked reference to one exact physical control or pad direction.
#[derive(Clone, Copy, Debug)]
pub struct ControlRef<'a> {
    /// Canonical control metadata.
    pub control: &'a PhysicalControl,
    /// Addressed pad direction, absent for a top-level control identity.
    pub direction: Option<PadDirection>,
}

/// An operation requested using the same identity as the installed physical panel.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ControlOperation {
    /// Assert a momentary input.
    Press,
    /// Release a momentary input or all shutter stages.
    Release,
    /// Select a named shutter stage.
    Stage {
        /// Exact stage identity.
        id: String,
    },
    /// Select a named detent or switch position.
    Position {
        /// Exact position identity.
        id: String,
    },
    /// Take a signed number of relative steps.
    Step {
        /// Negative decreases; positive increases; zero is refused.
        delta: i32,
    },
    /// Put a finger down on a touch surface at firmware logical coordinates.
    TouchStart {
        /// Logical x coordinate, less than the located width.
        x: u32,
        /// Logical y coordinate, less than the located height.
        y: u32,
    },
    /// Move the finger already down on a touch surface.
    TouchMove {
        /// Logical x coordinate, less than the located width.
        x: u32,
        /// Logical y coordinate, less than the located height.
        y: u32,
    },
    /// Lift the finger from a touch surface.
    TouchEnd,
}

/// A touch control's located firmware protocol.
#[derive(Clone, Copy, Debug)]
pub struct TouchSurface<'a> {
    /// Body connection carrying the touch reports.
    pub connection_id: &'a str,
    /// Firmware logical coordinate width.
    pub width: u32,
    /// Firmware logical coordinate height.
    pub height: u32,
    /// Firmware memory retaining the latest decoded report.
    pub decoded: &'a DecodedTouch,
}

/// A declared physical action resolved against one firmware's located inputs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ResolvedControlAction {
    /// Exact identity supplied to the checked physical panel.
    pub control_id: String,
    /// Requested physical operation.
    pub operation: ControlOperation,
    /// Number of times to apply the selected movement.
    pub repetitions: u32,
    /// Asserted level for the selected input bindings.
    pub asserted: bool,
    /// Located inputs named by the declaration; no event names are inferred.
    pub inputs: Vec<LocatedInput>,
}

impl PhysicalControls {
    /// Decode a stored physical-controls document with its source in errors.
    pub fn decode_document(document: &serde_json::Value, source: &str) -> Result<Self, String> {
        serde_json::from_value(document.clone()).map_err(|error| format!("{source}: {error}"))
    }

    /// Whether any control declares an input binding.
    pub fn has_bindings(&self) -> bool {
        !self.bindings().is_empty()
    }
    fn bindings(&self) -> Vec<(String, &InputBinding)> {
        let mut bindings = Vec::new();
        for (id, control) in &self.controls {
            let Some(behavior) = &control.behavior else {
                continue;
            };
            match behavior {
                ControlBehavior::Pad { directions, press } => {
                    for (direction, binding) in directions {
                        if let Some(binding) = binding {
                            bindings.push((format!("{id}.{}", direction.id()), binding));
                        }
                    }
                    if let Some(binding) = press {
                        bindings.push((id.clone(), binding));
                    }
                }
                ControlBehavior::Rocker {
                    decrease,
                    increase,
                    press,
                } => {
                    for binding in [decrease, increase, press].into_iter().flatten() {
                        bindings.push((id.clone(), binding));
                    }
                }
                ControlBehavior::Button { press } => {
                    if let Some(press) = press {
                        bindings.push((id.clone(), press));
                    }
                }
                ControlBehavior::Shutter { stages } => {
                    for binding in stages.values().flatten().flatten() {
                        bindings.push((id.clone(), binding));
                    }
                }
                ControlBehavior::Dial { motion, press, .. }
                | ControlBehavior::Ring { motion, press, .. } => {
                    if let Some(binding) = press {
                        bindings.push((id.clone(), binding));
                    }
                    match motion {
                        ControlMotion::Positions { positions } => {
                            for position in positions {
                                if let Some(key) = &position.key {
                                    bindings.push((id.clone(), key));
                                }
                            }
                        }
                        ControlMotion::Steps { decrease, increase } => {
                            if let Some(key) = decrease {
                                bindings.push((id.clone(), key));
                            }
                            if let Some(key) = increase {
                                bindings.push((id.clone(), key));
                            }
                        }
                    }
                }
                ControlBehavior::Switch { positions }
                | ControlBehavior::Lever { positions, .. } => {
                    for position in positions {
                        if let Some(key) = &position.key {
                            bindings.push((id.clone(), key));
                        }
                    }
                }
                ControlBehavior::Touch => {}
            }
        }
        bindings
    }

    /// The located touch protocol of a touch control.
    pub fn touch<'a>(
        &self,
        firmware: &'a FirmwareControlIds,
        id: &str,
    ) -> Result<TouchSurface<'a>, String> {
        let control = self.control(id)?.control;
        if !matches!(control.behavior, Some(ControlBehavior::Touch)) {
            return Err(format!("physical control {id} is not a touch surface"));
        }
        let mut located = control.connection_ids.iter().filter_map(|connection| {
            match firmware.interfaces.get(connection) {
                Some(ControlInterface::PrefixedTouch {
                    connection_id,
                    width,
                    height,
                    decoded,
                    ..
                }) => Some(TouchSurface {
                    connection_id,
                    width: *width,
                    height: *height,
                    decoded,
                }),
                _ => None,
            }
        });
        match (located.next(), located.next()) {
            (Some(surface), None) => Ok(surface),
            (None, _) => Err(format!("touch control {id} has no located touch protocol")),
            _ => Err(format!(
                "touch control {id} has several located touch protocols"
            )),
        }
    }

    /// Look up a canonical identity, including a declared pad direction.
    pub fn control(&self, id: &str) -> Result<ControlRef<'_>, String> {
        if let Some(control) = self.controls.get(id) {
            return Ok(ControlRef {
                control,
                direction: None,
            });
        }
        if let Some((parent, member)) = id.split_once('.')
            && let Some(control) = self.controls.get(parent)
            && let Some(ControlBehavior::Pad { directions, .. }) = &control.behavior
            && let Some(direction) = directions.keys().find(|direction| direction.id() == member)
        {
            return Ok(ControlRef {
                control,
                direction: Some(*direction),
            });
        }
        Err(format!("unknown physical control identity {id}"))
    }

    /// Validate identity, mechanism declarations, and unique physical roles.
    pub fn validate(&self) -> Result<(), String> {
        if self.schema != 1 || self.schema_uri != PHYSICAL_CONTROLS_SCHEMA_URI {
            return Err("physical controls schema differs".into());
        }
        fujimod_catalog_identity::validate_id(&self.id)?;
        fujimod_catalog_identity::validate_id(&self.body_id)?;
        if self.analog_levels.is_some_and(|levels| levels < 2)
            || self.controls.is_empty()
            || self.manual.url.is_empty()
            || self.manual.title.is_empty()
            || self.manual.pages.is_empty()
        {
            return Err(
                "physical controls require a manual, controls, and a valid known analog resolution"
                    .into(),
            );
        }
        if self.boot.contains_key(&BootMode::Sd)
            && !self.boot.contains_key(&BootMode::FirmwareUpdate)
        {
            return Err("SD boot requires a firmware update combination".into());
        }
        let mut roles = BTreeSet::new();
        for (mode, combination) in &self.boot {
            if combination.hold.is_empty() {
                return Err(format!("boot mode {mode:?} has no held controls"));
            }
            let mut held = BTreeSet::new();
            for input in &combination.hold {
                if !held.insert(&input.id) {
                    return Err(format!("boot mode {mode:?} repeats {}", input.id));
                }
                let id = input.id.strip_suffix(".press").unwrap_or(&input.id);
                let selected = self.control(id)?;
                let control = selected.control;
                if input.id.ends_with(".press")
                    && !matches!(
                        control.behavior,
                        Some(
                            ControlBehavior::Dial { .. }
                                | ControlBehavior::Ring { .. }
                                | ControlBehavior::Rocker { .. }
                        )
                    )
                {
                    return Err(format!("{} has no center press", input.id));
                }
                if let Some(stage) = &input.stage {
                    if !matches!(&control.behavior, Some(ControlBehavior::Shutter { stages }) if stages.contains_key(&format!("{stage}_press")))
                    {
                        return Err(format!("{} has no shutter stage {stage}", input.id));
                    }
                } else if !control.pushable && selected.direction.is_none() {
                    return Err(format!("{} cannot be held", input.id));
                }
            }
        }
        for (id, control) in &self.controls {
            if id.is_empty()
                || !id
                    .bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
            {
                return Err(format!("invalid physical control identity {id}"));
            }
            if control.name.trim().is_empty()
                || control.manual_page == Some(0)
                || (control.mechanical && control.behavior.is_some())
            {
                return Err(format!("physical control {id} has an invalid declaration"));
            }
            if control.face.as_ref().is_some_and(|face| {
                face.split('/')
                    .any(|line| line.trim().is_empty() || line.trim() != line)
            }) || control.short_name.as_ref().is_some_and(|name| {
                name.is_empty()
                    || name.len() > 10
                    || name.trim() != name
                    || !name
                        .bytes()
                        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b' ')
            }) {
                return Err(format!("physical control {id} has invalid markings"));
            }
            if !control.mechanical
                && !matches!(control.behavior, Some(ControlBehavior::Touch))
                && control.face.is_none()
                && control.icon.is_none()
                && control.short_name.is_none()
            {
                return Err(format!(
                    "physical control {id} needs a marking or short name"
                ));
            }
            let mut positions = BTreeSet::new();
            if control
                .positions
                .iter()
                .any(|position| position.id.is_empty() || !positions.insert(&position.id))
            {
                return Err(format!(
                    "physical control {id} has invalid manual positions"
                ));
            }
            if let Some(
                ControlBehavior::Switch { positions: bound }
                | ControlBehavior::Lever {
                    positions: bound, ..
                }
                | ControlBehavior::Dial {
                    motion: ControlMotion::Positions { positions: bound },
                    ..
                }
                | ControlBehavior::Ring {
                    motion: ControlMotion::Positions { positions: bound },
                    ..
                },
            ) = &control.behavior
                && bound
                    .iter()
                    .any(|position| !positions.contains(&position.id))
            {
                return Err(format!(
                    "physical control {id} binds an undocumented position"
                ));
            }
            if let Some(role) = control.role
                && !roles.insert(role)
            {
                return Err(format!("physical role {role:?} is assigned more than once"));
            }
            let mut connection_ids = BTreeSet::new();
            if control
                .connection_ids
                .iter()
                .any(|id| !connection_ids.insert(id))
            {
                return Err(format!("physical control {id} repeats a body connection"));
            }
            if let Some(behavior) = &control.behavior {
                behavior.validate()?;
            }
        }
        self.validate_ladders()
    }

    /// Resolve a declared action against checked firmware input facts.
    pub fn action(
        &self,
        firmware: &FirmwareControlIds,
        id: &str,
        operation: ControlOperation,
    ) -> Result<ResolvedControlAction, String> {
        let selected = self.control(id)?;
        let behavior = selected
            .control
            .behavior
            .as_ref()
            .ok_or_else(|| format!("physical control {id} has no bound actions"))?;
        if let ControlBehavior::Touch = behavior {
            let surface = self.touch(firmware, id)?;
            let asserted = match operation {
                ControlOperation::TouchStart { x, y } | ControlOperation::TouchMove { x, y } => {
                    if x >= surface.width || y >= surface.height {
                        return Err(format!(
                            "touch ({x}, {y}) is outside {id}'s {}x{} logical surface",
                            surface.width, surface.height
                        ));
                    }
                    true
                }
                ControlOperation::TouchEnd => false,
                _ => return Err(format!("touch control {id} accepts only touch operations")),
            };
            return Ok(ResolvedControlAction {
                control_id: id.into(),
                operation,
                repetitions: 1,
                asserted,
                inputs: vec![],
            });
        }
        let (bindings, asserted, repetitions) =
            behavior.bindings(selected.direction, &operation)?;
        if bindings.is_empty() {
            return Err(format!("physical control {id} has no located inputs"));
        }
        let inputs = bindings
            .into_iter()
            .map(|binding| firmware.input(id, binding).cloned())
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ResolvedControlAction {
            control_id: id.into(),
            operation,
            repetitions,
            asserted,
            inputs,
        })
    }
}

impl ControlBehavior {
    fn validate(&self) -> Result<(), String> {
        match self {
            Self::Shutter { stages } => {
                if stages.is_empty()
                    || stages.iter().any(|(id, inputs)| {
                        id.is_empty() || inputs.as_ref().is_some_and(Vec::is_empty)
                    })
                {
                    return Err("shutter requires named stages with explicit inputs".into());
                }
            }
            Self::Pad { directions, .. } => {
                if directions.len() != 4 {
                    return Err("pad requires all four declared directions".into());
                }
            }
            Self::Switch { positions }
            | Self::Lever { positions, .. }
            | Self::Dial {
                motion: ControlMotion::Positions { positions },
                ..
            }
            | Self::Ring {
                motion: ControlMotion::Positions { positions },
                ..
            } => {
                let mut ids = BTreeSet::new();
                if positions
                    .iter()
                    .any(|p| p.id.is_empty() || !ids.insert(&p.id))
                {
                    return Err("control positions must have distinct nonempty identities".into());
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn bindings<'a>(
        &'a self,
        direction: Option<PadDirection>,
        operation: &ControlOperation,
    ) -> Result<(Vec<&'a InputBinding>, bool, u32), String> {
        let press = match self {
            Self::Button { press } => press.as_ref(),
            Self::Pad { directions, press } => match direction {
                Some(direction) => directions.get(&direction).and_then(Option::as_ref),
                None => press.as_ref(),
            },
            Self::Dial { press, .. } | Self::Ring { press, .. } | Self::Rocker { press, .. } => {
                press.as_ref()
            }
            _ => None,
        };
        match operation {
            ControlOperation::Press | ControlOperation::Release if press.is_some() => Ok((
                vec![press.expect("checked press")],
                matches!(operation, ControlOperation::Press),
                1,
            )),
            ControlOperation::Release => match self {
                Self::Shutter { stages } => {
                    let inputs: BTreeSet<_> = stages.values().flatten().flatten().collect();
                    Ok((inputs.into_iter().collect(), false, 1))
                }
                Self::Lever {
                    positions,
                    momentary: Some(true),
                } => Ok((
                    positions
                        .iter()
                        .filter_map(|position| position.key.as_ref())
                        .collect(),
                    false,
                    1,
                )),
                _ => Err("control has no momentary release action".into()),
            },
            ControlOperation::Stage { id } => match self {
                Self::Shutter { stages } => stages
                    .get(id)
                    .and_then(Option::as_ref)
                    .map(|inputs| (inputs.iter().collect(), true, 1))
                    .ok_or_else(|| format!("unknown shutter stage {id}")),
                _ => Err("control is not a staged shutter".into()),
            },
            ControlOperation::Position { id } => {
                let positions = match self {
                    Self::Switch { positions }
                    | Self::Lever { positions, .. }
                    | Self::Dial {
                        motion: ControlMotion::Positions { positions },
                        ..
                    }
                    | Self::Ring {
                        motion: ControlMotion::Positions { positions },
                        ..
                    } => positions,
                    _ => return Err("control has no named positions".into()),
                };
                let position = positions
                    .iter()
                    .find(|p| p.id == *id)
                    .ok_or_else(|| format!("unknown control position {id}"))?;
                Ok((
                    vec![position.key.as_ref().ok_or("control position is unbound")?],
                    position.asserted.ok_or("position assertion is unlocated")?,
                    1,
                ))
            }
            ControlOperation::Step { delta } if *delta != 0 => match self {
                Self::Dial {
                    motion: ControlMotion::Steps { decrease, increase },
                    ..
                }
                | Self::Ring {
                    motion: ControlMotion::Steps { decrease, increase },
                    ..
                }
                | Self::Rocker {
                    decrease, increase, ..
                } => Ok((
                    vec![
                        (if *delta < 0 { decrease } else { increase })
                            .as_ref()
                            .ok_or("relative movement is unbound")?,
                    ],
                    true,
                    delta.unsigned_abs(),
                )),
                _ => Err("control has no relative step action".into()),
            },
            ControlOperation::Step { .. } => Err("control step must be nonzero".into()),
            _ => Err("control has no press action".into()),
        }
    }
}

impl FirmwareControlIds {
    /// Decode stored firmware control IDs with their source in errors.
    pub fn decode_document(document: &serde_json::Value, source: &str) -> Result<Self, String> {
        serde_json::from_value(document.clone()).map_err(|error| format!("{source}: {error}"))
    }

    /// Validate every scanner row, vocabulary reference, and physical input claim.
    pub fn validate(
        &self,
        physical: &PhysicalControls,
        connections: &BTreeSet<&str>,
    ) -> Result<(), String> {
        physical.validate()?;
        if self.schema != 1 || self.schema_uri != CONTROL_IDS_SCHEMA_URI {
            return Err("firmware control IDs schema differs".into());
        }
        fujimod_catalog_identity::validate_id(&self.id)?;
        fujimod_catalog_identity::validate_id(&self.firmware_id)?;
        for (id, control) in &physical.controls {
            if control
                .connection_ids
                .iter()
                .any(|connection| !connections.contains(connection.as_str()))
            {
                return Err(format!(
                    "physical control {id} names an unknown body connection"
                ));
            }
        }
        let mut debug_names = BTreeSet::new();
        for key in &self.debug_keys {
            if key.name.is_empty() || !debug_names.insert(&key.name) {
                return Err("firmware debug command names are empty or duplicated".into());
            }
        }
        let mut native_names = BTreeSet::new();
        for key in self.native_key_names.iter().flat_map(|names| &names.rows) {
            if key.name.is_empty() || !native_names.insert(&key.name) {
                return Err("firmware native input names are empty or duplicated".into());
            }
        }
        if let Some(reader) = &self.input_readers.scanner {
            reader.routine.validate()?;
            if reader.stable_width != 1
                || reader.stable_offset >= self.scanner.stride
                || reader.pressed_value == reader.released_value
            {
                return Err("firmware stable-state reader is invalid".into());
            }
        }
        fujimod_catalog_identity::validate_sha256(&self.scanner.sha256)?;
        if let Some(table) = &self.event_translation {
            fujimod_catalog_identity::validate_sha256(&table.sha256)?;
            let keys: BTreeSet<_> = table.rows.iter().map(|row| row.physical_key).collect();
            if table.rows.is_empty() || keys.len() != table.rows.len() || keys.contains(&0) {
                return Err("firmware event translation rows are empty or duplicated".into());
            }
        }
        if let Some(names) = &self.native_key_names {
            fujimod_catalog_identity::validate_sha256(&names.sha256)?;
        }
        if self.scanner.count == 0
            || self.scanner.stride < 20
            || self
                .scanner
                .base
                .checked_add(u64::from(self.scanner.count) * u64::from(self.scanner.stride))
                .is_none()
        {
            return Err("firmware input table is invalid".into());
        }
        let mut ids = BTreeSet::new();
        let mut rows = BTreeSet::new();
        for input in &self.inputs {
            fujimod_catalog_identity::validate_id(&input.id)?;
            if !ids.insert(&input.id) {
                return Err(format!("duplicate located input {}", input.id));
            }
            if let Some(key) = &input.debug_key
                && !self
                    .debug_keys
                    .iter()
                    .any(|candidate| candidate.name == *key && candidate.event == input.event)
            {
                return Err(format!(
                    "input {} names an undeclared debug key or event",
                    input.id
                ));
            }
            match &input.target {
                InputTarget::Control { control_id } => {
                    if physical.control(control_id)?.control.mechanical {
                        return Err(format!(
                            "mechanical control {control_id} claims an electronic input"
                        ));
                    }
                }
                InputTarget::Connection { connection_id } => {
                    if !connections.contains(connection_id.as_str()) {
                        return Err(format!(
                            "input {} names an unknown connection {connection_id}",
                            input.id
                        ));
                    }
                }
                InputTarget::Unclaimed => {}
            }
            if let Some(scanner) = &input.scanner
                && (scanner.index >= self.scanner.count
                    || !rows.insert(scanner.index)
                    || self.ui_event(scanner.witness.physical_key) != Some(input.event))
            {
                return Err(format!(
                    "input {} has an invalid or repeated scanner row",
                    input.id
                ));
            }
            if let Some(electrical) = &input.electrical {
                let connection_ids: Vec<&String> = match electrical {
                    ElectricalInput::Adc { connection_id, .. }
                    | ElectricalInput::Gpio { connection_id, .. }
                    | ElectricalInput::GpioContacts { connection_id, .. }
                    | ElectricalInput::GpioTransition { connection_id, .. }
                    | ElectricalInput::AdcTransition { connection_id, .. }
                    | ElectricalInput::GpioQuadrature { connection_id, .. } => vec![connection_id],
                    ElectricalInput::GpioFixedCycle { connection_id, .. }
                    | ElectricalInput::SerialStatus { connection_id, .. } => vec![connection_id],
                    ElectricalInput::AdcConjunction { terms, .. } => {
                        terms.iter().map(|term| &term.connection_id).collect()
                    }
                };
                electrical.validate_conjunction()?;
                for connection_id in connection_ids {
                    if !connections.contains(connection_id.as_str()) {
                        return Err(format!(
                            "input {} has unknown electrical connection {connection_id}",
                            input.id
                        ));
                    }
                    if let InputTarget::Control { control_id } = &input.target
                        && !physical
                            .control(control_id)?
                            .control
                            .connection_ids
                            .contains(connection_id)
                    {
                        return Err(format!(
                            "input {} uses a connection not assigned to {control_id}",
                            input.id
                        ));
                    }
                }
                if let ElectricalInput::GpioFixedCycle { cycle_sign, .. } = electrical
                    && !matches!(cycle_sign, -1 | 1)
                {
                    return Err(format!(
                        "input {} has an invalid encoder cycle sign",
                        input.id
                    ));
                }
                if let ElectricalInput::GpioTransition {
                    phase_map, transitions, decoder_event, guard, threshold, ..
                } = electrical {
                    let mut phases = *phase_map;
                    phases.sort_unstable();
                    if phases != [0, 1, 2, 3]
                        || transitions.iter().any(|event| *event > 2)
                        || !matches!(*decoder_event, 1 | 2)
                        || !transitions.contains(&(*decoder_event as u8))
                        || *guard == 0
                        || *threshold != 1
                        || input.scanner.is_some()
                    {
                        return Err(format!("input {} has an invalid transition encoder", input.id));
                    }
                }
                if let ElectricalInput::AdcTransition {
                    phase_map,
                    transitions,
                    decoder_event,
                    calibration_offsets,
                    ..
                } = electrical
                    && (*phase_map != [0, 1, 2, 3]
                        || transitions.iter().any(|event| *event > 3)
                        || !matches!(*decoder_event, 1 | 2)
                        || calibration_offsets.centers.contains(&u16::MAX)
                        || calibration_offsets.offset == u16::MAX
                        || input.scanner.is_some())
                {
                    return Err(format!("input {} has an invalid analog encoder", input.id));
                }
                if let ElectricalInput::GpioContacts { expected_levels, callback, .. } = electrical
                    && (!(2..=8).contains(&expected_levels.len())
                        || *callback == 0
                        || input.scanner.as_ref().is_none_or(|scanner|
                            scanner.witness.input_kind != ScannerInputKind::Kind5
                                || u64::from(scanner.witness.source) != *callback
                                || scanner.witness.upper != 1))
                {
                    return Err(format!("input {} has an invalid GPIO conjunction", input.id));
                }
                if let ElectricalInput::Adc {
                    intervals,
                    callback,
                    expected_result,
                    ..
                } = electrical
                    && (intervals.is_empty()
                        || callback.is_some() != expected_result.is_some()
                        || intervals.iter().any(|interval| {
                            !interval.bounds_valid()
                                || interval.exclusions().iter().any(|excluded| {
                                    !excluded.bounds_valid()
                                        || excluded.condition().is_some()
                                        || !excluded.exclusions().is_empty()
                                })
                        }))
                {
                    return Err(format!(
                        "input {} has an invalid ADC interval or callback",
                        input.id
                    ));
                }
                if let ElectricalInput::Adc {
                    intervals,
                    callback,
                    ..
                } = electrical
                {
                    let own = input.scanner.as_ref().map(|scanner| scanner.index);
                    for condition in intervals.iter().filter_map(AdcInterval::condition) {
                        let valid = callback.is_some()
                            && own.is_some()
                            && match condition {
                                AdcCondition::SelfActive => true,
                                AdcCondition::RowsInactive { rows } => {
                                    let mut seen = BTreeSet::new();
                                    !rows.is_empty()
                                        && rows.iter().all(|row| {
                                            *row < self.scanner.count
                                                && Some(*row) != own
                                                && seen.insert(*row)
                                        })
                                }
                            };
                        if !valid {
                            return Err(format!(
                                "input {} has an invalid ADC scanner condition",
                                input.id
                            ));
                        }
                    }
                }
                if let ElectricalInput::SerialStatus {
                    register,
                    mask,
                    active_value,
                    ..
                } = electrical
                    && (*register > 0x7f || *mask == 0 || *mask > 0xff || active_value & !mask != 0)
                {
                    return Err(format!(
                        "input {} has an invalid serial status register or mask",
                        input.id
                    ));
                }
                if matches!(
                    electrical,
                    ElectricalInput::Adc { .. }
                        | ElectricalInput::AdcConjunction { .. }
                        | ElectricalInput::Gpio { .. }
                        | ElectricalInput::GpioContacts { .. }
                        | ElectricalInput::SerialStatus { .. }
                ) && input.scanner.is_none()
                {
                    return Err(format!(
                        "input {} lacks its electrical scanner witness",
                        input.id
                    ));
                }
            }
        }
        for record in &self.undelivered_rows {
            if record.index >= self.scanner.count
                || !rows.insert(record.index)
                || self.ui_event(record.witness.physical_key).is_some()
            {
                return Err(format!(
                    "scanner row {} is not an undelivered row",
                    record.index
                ));
            }
        }
        if rows.len() != self.scanner.count as usize {
            return Err("generated input document does not retain every scanner row".into());
        }
        for (control_id, binding) in physical.bindings() {
            self.input(&control_id, binding)?;
        }
        for (connection_id, interface) in &self.interfaces {
            if !connections.contains(connection_id.as_str()) {
                return Err(format!(
                    "firmware input interface names unknown connection {connection_id}"
                ));
            }
            if let ControlInterface::PrefixedTouch {
                contacts,
                width,
                height,
                decoded,
                ..
            } = interface
                && (decoded.address == 0
                    || decoded.contacts.len() != *contacts as usize
                    || *width == 0
                    || *height == 0)
            {
                return Err(format!(
                    "touch interface {connection_id} has an invalid decoded report layout"
                ));
            }
        }
        Ok(())
    }

    /// UI event delivered for a scanner row's physical key.
    pub fn ui_event(&self, physical_key: u32) -> Option<u32> {
        match &self.event_translation {
            None => Some(physical_key),
            Some(table) => table
                .rows
                .iter()
                .find(|row| row.physical_key == physical_key)
                .map(|row| row.event),
        }
    }

    /// Resolve one explicit vocabulary name and require exact physical ownership.
    pub fn input(&self, control_id: &str, binding: &InputBinding) -> Result<&LocatedInput, String> {
        let event = match binding {
            InputBinding::DebugKey { name } => {
                let mut matches = self.debug_keys.iter().filter(|key| key.name == *name);
                let event = matches
                    .next()
                    .ok_or_else(|| format!("unknown firmware debug key {name}"))?
                    .event;
                if matches.next().is_some() {
                    return Err(format!("duplicate firmware debug key {name}"));
                }
                event
            }
            InputBinding::NativeInput { name } => {
                let mut matches = self
                    .native_key_names
                    .iter()
                    .flat_map(|names| &names.rows)
                    .filter(|key| key.name == *name);
                let event = matches
                    .next()
                    .ok_or_else(|| format!("unknown firmware native input {name}"))?
                    .event;
                if matches.next().is_some() {
                    return Err(format!("duplicate firmware native input {name}"));
                }
                event
            }
            InputBinding::ScannerRow { row } => {
                let mut matches = self.inputs.iter().filter(|input| input.id == *row);
                let input = matches
                    .next()
                    .ok_or_else(|| format!("unknown firmware scanner row {row}"))?;
                if matches.next().is_some() {
                    return Err(format!("duplicate firmware scanner row {row}"));
                }
                let row_index = row
                    .strip_prefix("row-")
                    .and_then(|value| value.parse::<u32>().ok());
                if row_index.is_none_or(|index| {
                    input
                        .scanner
                        .as_ref()
                        .is_none_or(|scanner| scanner.index != index)
                }) {
                    return Err(format!(
                        "firmware input {row} is not the declared scanner row"
                    ));
                }
                input.event
            }
        };
        let mut matches = self.inputs.iter().filter(|input| input.event == event
            && matches!(&input.target, InputTarget::Control { control_id: owner } if owner == control_id));
        let input = matches.next().ok_or_else(|| {
            format!("firmware input {event} is not claimed by control {control_id}")
        })?;
        if matches.next().is_some() {
            return Err(format!(
                "firmware control {control_id} has ambiguous input {event}"
            ));
        }
        if let InputBinding::DebugKey { name } = binding
            && input.debug_key.as_ref() != Some(name)
        {
            return Err(format!(
                "firmware input {event} does not declare debug key {name}"
            ));
        }
        if matches!(binding, InputBinding::NativeInput { .. }) && input.debug_key.is_some() {
            return Err(format!(
                "native-only input {} unexpectedly declares a debug transport key",
                input.id
            ));
        }
        if let InputBinding::ScannerRow { row } = binding
            && (input.id != *row || input.scanner.is_none())
        {
            return Err(format!("firmware input {row} is not the declared scanner row"));
        }
        Ok(input)
    }
}

impl ResolvedControlAction {
    /// Return declared debug commands, refusing native-only input before transport.
    pub fn debug_keys(&self) -> Result<Vec<&str>, String> {
        self.inputs
            .iter()
            .map(|input| {
                input.debug_key.as_deref().ok_or_else(|| {
                    format!(
                        "control {} has no debug transport key for input {}",
                        self.control_id, input.id
                    )
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    #[test]
    fn unbound_directions_never_use_the_center_binding_and_known_stages_release() {
        let binding = InputBinding::DebugKey {
            name: "CENTER".into(),
        };
        let pad = ControlBehavior::Pad {
            directions: BTreeMap::from([
                (PadDirection::Up, None),
                (PadDirection::Down, None),
                (PadDirection::Left, None),
                (PadDirection::Right, None),
            ]),
            press: Some(binding.clone()),
        };
        assert!(
            pad.bindings(Some(PadDirection::Up), &ControlOperation::Press)
                .is_err()
        );
        assert_eq!(
            pad.bindings(None, &ControlOperation::Press).unwrap().0,
            vec![&binding]
        );
        let shutter = ControlBehavior::Shutter {
            stages: BTreeMap::from([
                ("half_press".into(), Some(vec![binding.clone()])),
                ("full_press".into(), None),
            ]),
        };
        let (released, asserted, _) = shutter.bindings(None, &ControlOperation::Release).unwrap();
        assert_eq!(released, vec![&binding]);
        assert!(!asserted);
        assert!(
            shutter
                .bindings(
                    None,
                    &ControlOperation::Stage {
                        id: "full_press".into()
                    }
                )
                .is_err()
        );
    }

    #[test]
    fn rocker_steps_and_center_use_independent_contacts() {
        let binding = |name: &str| InputBinding::DebugKey { name: name.into() };
        let rocker = ControlBehavior::Rocker {
            decrease: Some(binding("LEFT")),
            increase: Some(binding("RIGHT")),
            press: Some(binding("PUSH")),
        };
        for (delta, name) in [(-3, "LEFT"), (2, "RIGHT")] {
            let (inputs, asserted, count) = rocker
                .bindings(None, &ControlOperation::Step { delta })
                .unwrap();
            assert_eq!(inputs, vec![&binding(name)]);
            assert!(asserted);
            assert_eq!(count, delta.unsigned_abs());
        }
        for (operation, expected) in [
            (ControlOperation::Press, true),
            (ControlOperation::Release, false),
        ] {
            let (inputs, asserted, count) = rocker.bindings(None, &operation).unwrap();
            assert_eq!(inputs, vec![&binding("PUSH")]);
            assert_eq!(asserted, expected);
            assert_eq!(count, 1);
        }
        assert!(
            rocker
                .bindings(None, &ControlOperation::Step { delta: 0 })
                .is_err()
        );
        let unbound = ControlBehavior::Rocker {
            decrease: None,
            increase: None,
            press: None,
        };
        assert!(
            unbound
                .bindings(None, &ControlOperation::Step { delta: -1 })
                .is_err()
        );
        assert!(unbound.bindings(None, &ControlOperation::Press).is_err());
    }

    #[test]
    fn adc_intervals_require_an_explicit_bound_source() {
        for kind in ["literal", "configuration_u16"] {
            let value = json!({"kind":kind,"lower":100,"upper":200});
            let interval: AdcInterval = serde_json::from_value(value.clone()).unwrap();
            assert_eq!(serde_json::to_value(interval).unwrap(), value);
        }
        assert!(serde_json::from_value::<AdcInterval>(json!({"lower":100,"upper":200})).is_err());
        assert!(
            serde_json::from_value::<AdcInterval>(
                json!({"kind":"literal","lower":0,"upper":65536})
            )
            .is_err()
        );
    }

    #[test]
    fn compound_adc_preserves_wide_configuration_offsets() {
        let value = json!({"kind":"adc_conjunction","callback":4096,"expected_result":1,
            "terms":[{"connection_id":"vertical","interval":{"kind":"configuration_u16","lower":72506,"upper":72504}},
                     {"connection_id":"press","interval":{"kind":"literal","lower":100,"upper":200}}]});
        let input: ElectricalInput = serde_json::from_value(value.clone()).unwrap();
        input.validate_conjunction().unwrap();
        assert_eq!(serde_json::to_value(input).unwrap(), value);
        for changed in [
            json!({"kind":"adc_conjunction","callback":4096,"expected_result":2,"terms":value["terms"]}),
            json!({"kind":"adc_conjunction","callback":4096,"expected_result":1,"terms":[value["terms"][0].clone()]}),
            json!({"kind":"adc_conjunction","callback":4096,"expected_result":1,"terms":[value["terms"][0].clone(),value["terms"][0].clone()]}),
        ] {
            let input: ElectricalInput = serde_json::from_value(changed).unwrap();
            assert!(input.validate_conjunction().is_err());
        }
        let overflow: AdcInterval = serde_json::from_value(
            json!({"kind":"configuration_u16","lower":4294967295u32,"upper":1}),
        )
        .unwrap();
        assert!(!overflow.bounds_valid());
    }

    #[test]
    fn compound_adc_checks_every_connection_and_scanner_witness() {
        let (mut physical, mut firmware) = fixture();
        let compound = json!({"kind":"adc_conjunction","callback":4096,"expected_result":1,
            "terms":[{"connection_id":"contact","interval":{"kind":"literal","lower":1,"upper":2}},
                     {"connection_id":"second","interval":{"kind":"configuration_u16","lower":72506,"upper":72504}}]});
        firmware
            .inputs
            .iter_mut()
            .find(|i| i.id == "input-movie")
            .unwrap()
            .electrical = Some(serde_json::from_value(compound).unwrap());
        assert!(
            firmware
                .validate(&physical, &BTreeSet::from(["contact"]))
                .is_err()
        );
        assert!(
            firmware
                .validate(&physical, &BTreeSet::from(["contact", "second"]))
                .is_err()
        );
        physical
            .controls
            .get_mut("movie")
            .unwrap()
            .connection_ids
            .push("second".into());
        firmware
            .validate(&physical, &BTreeSet::from(["contact", "second"]))
            .unwrap();
        firmware
            .inputs
            .iter_mut()
            .find(|i| i.id == "input-movie")
            .unwrap()
            .scanner = None;
        assert!(
            firmware
                .validate(&physical, &BTreeSet::from(["contact", "second"]))
                .is_err()
        );
    }

    #[test]
    fn adc_interval_conditions_round_trip_and_stay_optional() {
        let value = json!({"kind":"literal","lower":0,"upper":65535,
            "when":{"kind":"self_active"},
            "unless":[{"kind":"configuration_u16","lower":14186,"upper":14184}]});
        let interval: AdcInterval = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(interval.condition(), Some(&AdcCondition::SelfActive));
        assert_eq!(interval.exclusions().len(), 1);
        assert_eq!(serde_json::to_value(interval).unwrap(), value);
        let rows = json!({"kind":"configuration_u16","lower":1,"upper":2,
            "when":{"kind":"rows_inactive","rows":[3,4]}});
        let interval: AdcInterval = serde_json::from_value(rows).unwrap();
        assert_eq!(
            interval.condition(),
            Some(&AdcCondition::RowsInactive { rows: vec![3, 4] })
        );
        assert!(
            serde_json::from_value::<AdcInterval>(
                json!({"kind":"literal","lower":1,"upper":2,"when":{"kind":"held"}})
            )
            .is_err()
        );
    }

    #[test]
    fn adc_conditions_name_other_rows_of_a_callback_predicate() {
        let check = |electrical: Value| {
            let (physical, mut firmware) = fixture();
            firmware.scanner.count = 3;
            for index in [1, 2] {
                let input = firmware
                    .inputs
                    .iter_mut()
                    .find(|input| input.event == index)
                    .unwrap();
                input.scanner = Some(serde_json::from_value(json!({"index":index,
                    "witness":{"physical_key":index,"input_kind":"kind_2","source":11,"lower":0,"upper":0}}))
                    .unwrap());
            }
            let movie = firmware
                .inputs
                .iter_mut()
                .find(|input| input.id == "input-movie")
                .unwrap();
            movie.electrical = Some(serde_json::from_value(electrical).unwrap());
            firmware.validate(&physical, &BTreeSet::from(["contact"]))
        };
        let adc = |intervals: Value, callback: Value| {
            json!({"kind":"adc","connection_id":"contact","intervals":intervals,
                "callback":callback,"expected_result":if callback.is_null() {Value::Null} else {json!(1)}})
        };
        let press = |rows: Value| {
            json!({"kind":"configuration_u16","lower":10,"upper":12,
                "when":{"kind":"rows_inactive","rows":rows}})
        };
        let hold = |unless: Value| json!({"kind":"literal","lower":0,"upper":65535,"when":{"kind":"self_active"},"unless":unless});
        let release =
            json!([{"kind":"configuration_u16","lower":20,"upper":22}, press(json!([1]))]);
        check(adc(
            json!([press(json!([1, 2])), hold(json!([]))]),
            json!(4096),
        ))
        .unwrap();
        assert!(
            check(adc(
                json!([hold(
                    json!([{"kind":"configuration_u16","lower":20,"upper":22}])
                )]),
                json!(4096)
            ))
            .is_ok()
        );
        for invalid in [
            adc(json!([press(json!([0]))]), json!(4096)),
            adc(json!([press(json!([3]))]), json!(4096)),
            adc(json!([press(json!([1, 1]))]), json!(4096)),
            adc(json!([press(json!([]))]), json!(4096)),
            adc(json!([press(json!([1]))]), Value::Null),
            adc(json!([hold(release)]), json!(4096)),
        ] {
            assert!(check(invalid).unwrap_err().contains("invalid ADC"));
        }
    }

    #[test]
    fn spring_return_lever_releases_all_held_directions() {
        let position = |id: &str| BoundPosition {
            id: id.into(),
            key: Some(InputBinding::DebugKey { name: id.into() }),
            asserted: Some(true),
        };
        let lever = ControlBehavior::Lever {
            positions: vec![position("wide"), position("tele")],
            momentary: Some(true),
        };
        let (held, asserted, _) = lever
            .bindings(None, &ControlOperation::Position { id: "wide".into() })
            .unwrap();
        assert!(asserted);
        assert_eq!(
            held,
            [&InputBinding::DebugKey {
                name: "wide".into()
            }]
        );
        let (released, asserted, _) = lever.bindings(None, &ControlOperation::Release).unwrap();
        assert!(!asserted);
        assert_eq!(released.len(), 2);
        let fixed = ControlBehavior::Lever {
            positions: vec![position("AUTO")],
            momentary: Some(false),
        };
        assert!(fixed.bindings(None, &ControlOperation::Release).is_err());
    }

    fn control(behavior: Value) -> Value {
        json!({"name":"Documented control", "location":"rear", "manual_page":3,
            "face":"KEY","icon":null,"connection_ids":["contact"],"assignable":false,"pushable":true,"positions":[],
            "default_function":null,"role":null,"mechanical":false,"behavior":behavior})
    }

    fn fixture() -> (PhysicalControls, FirmwareControlIds) {
        let debug = |name| json!({"kind":"debug_key","name":name});
        let native = |name| json!({"kind":"native_input","name":name});
        let physical = serde_json::from_value(json!({
            "$schema":PHYSICAL_CONTROLS_SCHEMA_URI,"schema":1,"id":"body-controls","body_id":"body",
            "manual":{"url":"https://example.com/manual","title":"Camera manual","pages":[3]},
            "analog_levels":4096,"controls":{
                "menu_ok":control(json!({"kind":"button","press":debug("EXE_M")})),
                "movie":control(json!({"kind":"button","press":native("KEY_FUNCTION3")})),
                "selector":control(json!({"kind":"pad","directions":{
                    "up":debug("UP"),"down":debug("DOWN"),"left":debug("LEFT"),"right":debug("RIGHT")},"press":null})),
                "command_control":control(json!({"kind":"dial","motion":{"kind":"steps",
                    "decrease":debug("CMD_LEFT"),"increase":debug("CMD_RIGHT")},"press":null,"endless":true}))
            }
        })).unwrap();
        let names = [
            "EXE_M",
            "UP",
            "DOWN",
            "LEFT",
            "RIGHT",
            "CMD_LEFT",
            "CMD_RIGHT",
        ];
        let mut debug_keys = Vec::new();
        let mut inputs = Vec::new();
        for (index, name) in names.iter().enumerate() {
            let event = index as u32 + 1;
            debug_keys.push(json!({"name":name,"event":event}));
            let id = match *name {
                "EXE_M" => "menu_ok",
                "UP" => "selector.up",
                "DOWN" => "selector.down",
                "LEFT" => "selector.left",
                "RIGHT" => "selector.right",
                _ => "command_control",
            };
            inputs.push(
                json!({"id":format!("input-{event}"),"event":event,"debug_key":name,
                "target":{"kind":"control","control_id":id},"scanner":null,"electrical":null}),
            );
        }
        inputs.push(json!({"id":"input-movie","event":61,"debug_key":null,
            "target":{"kind":"control","control_id":"movie"},"electrical":null,
            "scanner":{"index":0,"witness":{"physical_key":61,"input_kind":"kind_2","source":11,"lower":0,"upper":0}}}));
        let firmware = serde_json::from_value(json!({
            "$schema":CONTROL_IDS_SCHEMA_URI,"schema":1,"id":"firmware-control-ids","firmware_id":"firmware",
            "scanner":{"base":4096,"count":1,"stride":28,"lookup":8192,"sha256":"0".repeat(64)},
            "input_readers":{"calibration":12288,"scanner":{"stable_offset":21,"stable_width":1,"pressed_value":1,
                "released_value":0,"routine":{"address":8192,"size":4,"sha256":"0".repeat(64)}}},
            "native_key_names":{"rows":[{"event":61,"name":"KEY_FUNCTION3","string_address":16384},
                {"event":1,"name":"KEY_OK","string_address":16400}],"reader":16380,"size":32,"sha256":"0".repeat(64)},
            "debug_keys":debug_keys,"inputs":inputs,"undelivered_rows":[],"interfaces":{}
        })).unwrap();
        (physical, firmware)
    }

    #[test]
    fn exact_pad_member_identity_resolves_without_aliases() {
        let (physical, firmware) = fixture();
        firmware
            .validate(&physical, &BTreeSet::from(["contact"]))
            .unwrap();
        assert_eq!(
            physical.control("selector.up").unwrap().direction,
            Some(PadDirection::Up)
        );
        let action = physical
            .action(&firmware, "selector.up", ControlOperation::Press)
            .unwrap();
        assert_eq!(action.control_id, "selector.up");
        assert_eq!(action.debug_keys().unwrap(), ["UP"]);
        assert!(physical.control("selector.UP").is_err());
        assert!(physical.control("selector_function_buttons.up").is_err());
        assert!(
            physical
                .action(&firmware, "selector", ControlOperation::Press)
                .is_err()
        );
    }

    #[test]
    fn touch_surface_resolves_bounded_touch_operations_only() {
        let (mut physical, mut firmware) = fixture();
        let mut surface = control(json!({"kind":"touch"}));
        surface["connection_ids"] = json!(["panel"]);
        physical.controls.insert(
            "touchscreen".into(),
            serde_json::from_value(surface).unwrap(),
        );
        assert!(
            physical
                .action(&firmware, "touchscreen", ControlOperation::TouchEnd)
                .unwrap_err()
                .contains("no located touch protocol")
        );
        firmware.interfaces.insert(
            "panel".into(),
            serde_json::from_value(
                json!({"kind":"prefixed_touch","connection_id":"panel","mode":1,
                "register":0,"report_bytes":11,"contacts":2,"coordinate_bits":12,"width":720,
                "height":480,"offset_tags":[1,2],"irq":15,"priority":10,"irq_mode":2,
                "decoded":{"address":4096,"count":0,"contacts":[{"active":8,"x":10,"y":12},
                    {"active":20,"x":22,"y":24}]}}),
            )
            .unwrap(),
        );
        firmware
            .validate(&physical, &BTreeSet::from(["contact", "panel"]))
            .unwrap();
        assert_eq!(
            physical
                .touch(&firmware, "touchscreen")
                .unwrap()
                .decoded
                .address,
            4096
        );
        let start = physical
            .action(
                &firmware,
                "touchscreen",
                ControlOperation::TouchStart { x: 719, y: 479 },
            )
            .unwrap();
        assert!(start.asserted && start.inputs.is_empty());
        assert!(
            !physical
                .action(&firmware, "touchscreen", ControlOperation::TouchEnd)
                .unwrap()
                .asserted
        );
        for refused in [
            ControlOperation::TouchMove { x: 720, y: 0 },
            ControlOperation::TouchStart { x: 0, y: 480 },
            ControlOperation::Press,
            ControlOperation::Release,
        ] {
            assert!(physical.action(&firmware, "touchscreen", refused).is_err());
        }
        assert!(
            physical
                .action(
                    &firmware,
                    "menu_ok",
                    ControlOperation::TouchStart { x: 0, y: 0 }
                )
                .is_err()
        );
        assert!(physical.touch(&firmware, "menu_ok").is_err());
    }

    #[test]
    fn native_only_action_remains_physical_and_refuses_debug_transport() {
        let (physical, mut firmware) = fixture();
        firmware.inputs.last_mut().unwrap().id = "row-0".into();
        let action = physical
            .action(&firmware, "movie", ControlOperation::Press)
            .unwrap();
        assert!(action.asserted);
        assert_eq!(action.inputs[0].event, 61);
        assert_eq!(action.inputs[0].debug_key, None);
        assert!(
            action
                .debug_keys()
                .unwrap_err()
                .contains("no debug transport key")
        );
        assert!(
            !physical
                .action(&firmware, "movie", ControlOperation::Release)
                .unwrap()
                .asserted
        );
        assert!(
            firmware
                .input(
                    "movie",
                    &InputBinding::DebugKey {
                        name: "FUNCTION3".into()
                    }
                )
                .is_err()
        );
        assert_eq!(
            firmware
                .input(
                    "movie",
                    &InputBinding::ScannerRow {
                        row: "row-0".into()
                    }
                )
                .unwrap()
                .event,
            61
        );
        assert!(
            firmware
                .input(
                    "menu_ok",
                    &InputBinding::ScannerRow {
                        row: "row-0".into()
                    }
                )
                .is_err()
        );
        assert!(
            firmware
                .input(
                    "movie",
                    &InputBinding::ScannerRow {
                        row: "row-1".into()
                    }
                )
                .is_err()
        );
        firmware.inputs.last_mut().unwrap().id = "row-1".into();
        assert!(
            firmware
                .input(
                    "movie",
                    &InputBinding::ScannerRow {
                        row: "row-1".into()
                    }
                )
                .is_err()
        );
        assert!(
            firmware
                .input(
                    "menu_ok",
                    &InputBinding::DebugKey {
                        name: "KEY_OK".into()
                    }
                )
                .is_err()
        );
        assert!(
            firmware
                .input(
                    "menu_ok",
                    &InputBinding::NativeInput {
                        name: "EXE_M".into()
                    }
                )
                .is_err()
        );
    }

    #[test]
    fn debug_preflight_refuses_a_later_native_only_input() {
        let (mut physical, mut firmware) = fixture();
        physical.controls.remove("menu_ok");
        let mut shutter = physical.controls.remove("movie").unwrap();
        shutter.behavior = Some(ControlBehavior::Shutter {
            stages: BTreeMap::from([(
                "full".into(),
                Some(vec![
                    InputBinding::DebugKey {
                        name: "EXE_M".into(),
                    },
                    InputBinding::NativeInput {
                        name: "KEY_FUNCTION3".into(),
                    },
                ]),
            )]),
        });
        physical.controls.insert("shutter".into(), shutter);
        for input in &mut firmware.inputs {
            if input.event == 1 || input.event == 61 {
                input.target = InputTarget::Control {
                    control_id: "shutter".into(),
                };
            }
        }
        firmware
            .validate(&physical, &BTreeSet::from(["contact"]))
            .unwrap();
        let action = physical
            .action(
                &firmware,
                "shutter",
                ControlOperation::Stage { id: "full".into() },
            )
            .unwrap();
        assert_eq!(action.inputs.len(), 2);
        assert_eq!(action.inputs[0].debug_key.as_deref(), Some("EXE_M"));
        assert!(action.debug_keys().is_err());
    }

    #[test]
    fn signed_step_selects_declared_binding_and_count() {
        let (physical, firmware) = fixture();
        let action = physical
            .action(
                &firmware,
                "command_control",
                ControlOperation::Step { delta: -3 },
            )
            .unwrap();
        assert_eq!(action.repetitions, 3);
        assert_eq!(action.debug_keys().unwrap(), ["CMD_LEFT"]);
        assert_eq!(
            physical
                .action(
                    &firmware,
                    "command_control",
                    ControlOperation::Step { delta: 2 }
                )
                .unwrap()
                .debug_keys()
                .unwrap(),
            ["CMD_RIGHT"]
        );
        assert!(
            physical
                .action(
                    &firmware,
                    "command_control",
                    ControlOperation::Step { delta: 0 }
                )
                .is_err()
        );
    }

    #[test]
    fn input_claims_and_all_scanner_rows_are_checked() {
        let (physical, mut firmware) = fixture();
        firmware.inputs.last_mut().unwrap().target = InputTarget::Unclaimed;
        assert!(
            firmware
                .validate(&physical, &BTreeSet::from(["contact"]))
                .unwrap_err()
                .contains("not claimed")
        );
        let (_, mut firmware) = fixture();
        firmware.scanner.count = 2;
        assert!(
            firmware
                .validate(&physical, &BTreeSet::from(["contact"]))
                .unwrap_err()
                .contains("every scanner row")
        );
        let (_, mut firmware) = fixture();
        firmware.inputs.last_mut().unwrap().debug_key = Some("FUNCTION3".into());
        assert!(
            firmware
                .validate(&physical, &BTreeSet::from(["contact"]))
                .unwrap_err()
                .contains("undeclared debug key")
        );
    }

    #[test]
    fn untranslated_scanner_rows_are_recorded_as_undelivered() {
        let (physical, mut firmware) = fixture();
        firmware.scanner.count = 2;
        firmware.event_translation = Some(EventTranslation {
            address: 0x3000,
            stride: 6,
            sha256: "0".repeat(64),
            rows: vec![TranslatedKey {
                physical_key: 61,
                event: 61,
            }],
        });
        let witness = |physical_key| ScannerRecordWitness {
            physical_key,
            ..firmware
                .inputs
                .last()
                .unwrap()
                .scanner
                .clone()
                .unwrap()
                .witness
        };
        firmware.undelivered_rows = vec![LocatedScannerRecord {
            index: 1,
            witness: witness(5),
        }];
        firmware
            .validate(&physical, &BTreeSet::from(["contact"]))
            .unwrap();
        let mut translated = firmware.clone();
        translated.undelivered_rows[0].witness = witness(61);
        assert!(
            translated
                .validate(&physical, &BTreeSet::from(["contact"]))
                .unwrap_err()
                .contains("not an undelivered row")
        );
        firmware.undelivered_rows[0].index = 0;
        assert!(
            firmware
                .validate(&physical, &BTreeSet::from(["contact"]))
                .unwrap_err()
                .contains("not an undelivered row")
        );
    }

    #[test]
    fn electronic_controls_may_be_unbound_but_mechanical_controls_have_no_actions() {
        let (mut physical, _) = fixture();
        physical.controls.get_mut("menu_ok").unwrap().behavior = None;
        physical.validate().unwrap();
        assert!(
            physical
                .action(&fixture().1, "menu_ok", ControlOperation::Press)
                .is_err()
        );
        let control = physical.controls.get_mut("movie").unwrap();
        control.mechanical = true;
        assert!(physical.validate().is_err());
    }

    #[test]
    fn roles_are_unique_and_documents_refuse_unknown_fields() {
        let (mut physical, _) = fixture();
        physical.controls.get_mut("menu_ok").unwrap().role = Some(ControlRole::CommandControl);
        physical.controls.get_mut("command_control").unwrap().role =
            Some(ControlRole::CommandControl);
        assert!(
            physical
                .validate()
                .unwrap_err()
                .contains("assigned more than once")
        );
        let mut value = serde_json::to_value(fixture().0).unwrap();
        value["controls"]["menu_ok"]["alias"] = json!("ok");
        assert!(serde_json::from_value::<PhysicalControls>(value).is_err());
        assert!(
            serde_json::from_value::<InputBinding>(
                json!({"kind":"debug_key","name":"EXE_M","native":"KEY_OK"})
            )
            .is_err()
        );
    }
}
