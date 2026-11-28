//! The camera description every front door opens a camera from, and the timed
//! inputs a command-line run applies.
//!
//! A host names a catalog body, optionally one of its firmwares, and the
//! session's own choices: settings, storage cards, scene, date, engine and how
//! the camera starts. The same description opens a camera from the MCP server,
//! the Python binding and the command line, whose `--actions` list holds the
//! inputs a run applies at guest times. Host paths in either are relative to
//! the directory of the file they were read from.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Schema URI of a [`CameraDescription`].
pub const CAMERA_SCHEMA_URI: &str =
    "https://fujimod.com/schemas/emulator-camera.generated.schema.json";
/// Schema URI of an [`Actions`] list.
pub const ACTIONS_SCHEMA_URI: &str =
    "https://fujimod.com/schemas/emulator-actions.generated.schema.json";

/// One camera to open.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct CameraDescription {
    /// Catalog body identity, for example `xh2s`.
    pub body: String,
    /// The body's firmware: a version such as `7.30`, a firmware identity
    /// such as `xh2s-730`, or `none` for an empty flash. Omitted, the newest
    /// firmware the catalog has for the body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub firmware: Option<String>,
    /// Named camera settings the firmware's settings record writes before the
    /// first instruction, such as `{"language": "en", "setup": "done"}`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub settings: BTreeMap<String, String>,
    /// The card in the first SD slot. Omitted, one empty SD card is inserted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sd_card: Option<StorageCard>,
    /// The card in the second SD slot, on a body that has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sd_card_2: Option<StorageCard>,
    /// The card in the CFexpress Type B slot, on a body that has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cfexpress_card: Option<StorageCard>,
    /// The camera clock at power-on, `YYYY-MM-DDTHH:MM:SS` local time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    /// Host path of a scene document the sensor sees, in place of the run's
    /// declared scene.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scene: Option<String>,
    /// Scheduler engine: `unicorn` or `hypervisor`. Omitted, the run's engine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<String>,
    /// CPU execution policy. Omitted, `exact`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_policy: Option<ExecutionPolicyName>,
    /// How the camera starts with FujiMod. Omitted, `stock`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boot: Option<BootPreset>,
    /// Hold a power-on combination from the body's controls, such as `sd`,
    /// `firmware_update` or `service`, from power-on. A `stock` boot only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hold: Option<String>,
    /// Start at the native loader with a RAM image at address zero, in place
    /// of the boot ROM. A `stock` boot only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loader_image: Option<LoaderImage>,
    /// Host path of a saved image of the internal flash's stage-1 blocks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flash_image: Option<String>,
    /// Host path of a saved committed internal-flash image, restored before
    /// the first instruction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nand_image: Option<String>,
    /// The firmware's update and boot ROM as host files, in place of the
    /// firmware library's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub firmware_files: Option<FirmwareFiles>,
    /// Host path of the application startup plan an X-Processor 4 or
    /// X-Processor Pro firmware starts from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub application_plan: Option<String>,
    /// External face detector results in place of the passive face bank:
    /// `empty`, `box64x20` or `box172x120`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub face_result_profile: Option<crate::RunFaceResultProfile>,
    /// Catalog overlays of the firmware's run configuration.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overlays: Vec<String>,
    /// Run to REC before returning. Omitted, true; false returns before the
    /// first instruction with the power-on holds applied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_to_rec: Option<bool>,
    /// Restore a saved boot with the same inputs, and save this one. Omitted,
    /// true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boot_cache: Option<bool>,
    /// Host path of a checkpoint file to restore instead of booting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<String>,
}

/// A CPU execution policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum ExecutionPolicyName {
    /// Deterministic: runs compare instruction for instruction.
    Exact,
    /// Faster where the result cannot differ.
    Interactive,
}

/// How a camera starts with FujiMod.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "kebab-case")]
pub enum BootPreset {
    /// The firmware installed in internal flash.
    Stock,
    /// FujiMod loaded from the SD card by the boot ROM; the card is
    /// `sd_card`, or the current release card when it names no contents.
    SdCard,
    /// FujiMod started from the SD card's autostart installer in internal
    /// flash.
    Autostart,
}

/// A card in one of the body's storage slots.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct StorageCard {
    /// Whether the card is in the slot at power-on. Omitted, true.
    #[serde(default = "yes")]
    pub present: bool,
    /// Whether the card's write-protect switch is set. Omitted, false.
    #[serde(default)]
    pub write_protected: bool,
    /// Files on a freshly formatted card, by their full path on the card.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<BTreeMap<String, CardFile>>,
    /// Host path of a folder whose tree becomes the card's files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directory: Option<String>,
    /// Host path of a raw card image, read without being written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// FAT geometry for a freshly formatted card, in place of the run's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<CardLayout>,
    /// Guest ticks the modeled host charges for each data transfer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transfer_ticks: Option<u64>,
    /// The modeled SD host's initialization fault, as the device model names
    /// it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initialization_fault: Option<serde_json::Value>,
}

impl Default for StorageCard {
    fn default() -> Self {
        Self {
            present: true,
            write_protected: false,
            files: None,
            directory: None,
            image: None,
            layout: None,
            transfer_ticks: None,
            initialization_fault: None,
        }
    }
}

fn yes() -> bool {
    true
}

/// One file on a card: a host file, or its bytes in base64.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(untagged)]
pub enum CardFile {
    /// A host file.
    Source {
        /// Host path of the file.
        source: String,
    },
    /// Bytes the host already holds.
    Bytes {
        /// The file's bytes, base64-encoded.
        bytes: String,
    },
}

/// A declared FAT geometry for a freshly formatted card.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct CardLayout {
    /// Sectors on the blank card.
    pub sectors: u32,
    /// One to eleven ASCII bytes.
    pub label: String,
    /// First sector of the FAT partition.
    pub partition_start: u32,
    /// Sectors per FAT cluster: a power of two from 1 to 128.
    pub sectors_per_cluster: u8,
}

/// A RAM image the native loader starts from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct LoaderImage {
    /// Host path of the image.
    pub path: String,
    /// SHA-256 of the image, which the open checks.
    pub sha256: String,
}

/// The firmware's two files as host paths.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct FirmwareFiles {
    /// The stock update file.
    pub update: String,
    /// The boot ROM capture, for a firmware whose start runs it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boot_rom: Option<String>,
}

/// The inputs a run applies, in any order; each names its guest time.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(transparent)]
pub struct Actions(pub Vec<Action>);

/// One input at a guest time.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct Action {
    /// Guest milliseconds after the run starts.
    pub at_ms: u64,
    /// What happens then.
    #[serde(flatten)]
    pub input: ActionInput,
}

/// An input a run applies, named as the MCP server's input tools name it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ActionInput {
    /// Press a key and release it after `hold_ms`.
    Press(Press),
    /// Press a key and keep it down until a `release`.
    Hold(Control),
    /// Release a held key.
    Release(Control),
    /// Move a dial, switch or lever to a named position.
    Set(ControlPosition),
    /// Drive a GPIO input pin.
    Gpio(GpioLevel),
    /// Write a 32-bit word to physical memory.
    Poke(MemoryWord),
    /// Move a declared input word of the power peer.
    PowerInput(PowerWord),
    /// Connect an HDMI sink to the body's HDMI connector.
    Hdmi(HdmiSink),
}

/// A key press and its hold.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct Press {
    /// The key's catalog name, such as `MENU_OK`.
    pub control: String,
    /// Guest milliseconds the key stays down. Omitted, 150.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hold_ms: Option<u64>,
}

/// A key by its catalog name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct Control {
    /// The key's catalog name, such as `MENU_OK`.
    pub control: String,
}

/// A dial, switch or lever and its position.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ControlPosition {
    pub control: String,
    pub position: String,
}

/// A GPIO input level.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct GpioLevel {
    pub port: u8,
    pub pin: u8,
    pub high: bool,
}

/// A word written to physical memory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MemoryWord {
    /// Four-byte aligned physical address.
    pub address: u64,
    pub value: u32,
}

/// A declared input word of the power peer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct PowerWord {
    pub register: u8,
    pub value: u16,
}

/// An HDMI sink on the body's connector.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct HdmiSink {
    /// Whether the sink is connected; only connecting is modeled.
    pub connected: bool,
}
