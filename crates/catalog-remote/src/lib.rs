//! Firmware layouts and build declarations consumed by camera remote control.
//!
//! Addresses describe live memory. Menu rows, captions, glyphs and cursor values
//! are read from the device; a build manifest supplies declaration identities.
use fujimod_catalog_controls::facts::{AddressIdentity, ConfigurationLocation};
use fujimod_catalog_controls::{FirmwareControlIds, PhysicalControls};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const FIRMWARE_FACTS_SCHEMA_URI: &str =
    "https://fujimod.com/schemas/remote-firmware.generated.schema.json";
pub const MENU_MANIFEST_SCHEMA_URI: &str =
    "https://fujimod.com/schemas/build-menu.generated.schema.json";

/// All inputs required to resolve remote references.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct Facts {
    /// Physical control identities.
    pub physical_controls: PhysicalControls,
    /// Located inputs of the selected firmware.
    pub control_ids: FirmwareControlIds,
    /// Selected firmware layouts and inputs.
    pub firmware: FirmwareFacts,
    /// Installed build declarations, absent for stock firmware.
    pub menu_manifest: Option<BuildMenuManifest>,
    /// Optional task receive boundary required by native input consumers.
    #[serde(default)]
    pub input_tasks: Option<InputTasks>,
    /// Controls accepted in declared states while REC parameter work remains active.
    #[serde(default)]
    pub parameter_independent_inputs: Vec<ParameterIndependentInput>,
}

impl Facts {
    /// Build validated remote inputs from documents in a checked catalog selection.
    pub fn from_selection(
        selection: &Selection<'_>,
        menu_manifest: Option<BuildMenuManifest>,
    ) -> Result<Self, String> {
        let firmware: FirmwareFacts = decode_document(
            selection
                .remote_facts
                .ok_or("selected firmware has no remote facts")?,
            "firmware remote facts",
        )?;
        let physical_controls = PhysicalControls::decode_document(
            selection
                .physical_controls
                .ok_or("selected body has no physical controls")?,
            "physical controls",
        )?;
        let control_ids = FirmwareControlIds::decode_document(
            selection
                .control_ids
                .ok_or("selected firmware has no control IDs")?,
            "firmware control IDs",
        )?;
        if firmware.firmware_id != selection.firmware_id
            || firmware.body_id != selection.body_id
            || selection.firmware_sha256 != Some(firmware.firmware_sha256.as_str())
        {
            return Err("remote facts differ from selected firmware/body".into());
        }
        let facts = Self {
            physical_controls,
            control_ids,
            firmware,
            menu_manifest,
            input_tasks: None,
            parameter_independent_inputs: Vec::new(),
        };
        facts.physical_controls.validate()?;
        facts
            .control_ids
            .validate(&facts.physical_controls, &selection.connections)
            .map_err(|error| format!("control IDs {}: {error}", facts.control_ids.id))?;
        facts.validate()?;
        Ok(facts)
    }
}

/// The document and identity fields needed to build remote facts from a resolved selection.
pub struct Selection<'a> {
    pub firmware_id: &'a str,
    pub body_id: &'a str,
    pub firmware_sha256: Option<&'a str>,
    pub remote_facts: Option<&'a serde_json::Value>,
    pub physical_controls: Option<&'a serde_json::Value>,
    pub control_ids: Option<&'a serde_json::Value>,
    pub connections: BTreeSet<&'a str>,
}

fn decode_document<T: serde::de::DeserializeOwned>(
    value: &serde_json::Value,
    label: &str,
) -> Result<T, String> {
    serde_json::from_value(value.clone()).map_err(|error| format!("{label}: {error}"))
}

/// Task-table layout used to observe input consumers without caching live pointers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct InputTasks {
    pub headers: Vec<u64>,
    pub entries: Vec<u32>,
    pub address_mask: u64,
    pub address_limit: u64,
    pub max_count: u32,
    pub stride: u64,
    pub descriptor_offset: u64,
    pub entry_offset: u64,
    pub descriptor_size: u64,
    pub state_offset: u64,
    pub receive_state: u8,
    /// Additional mode-qualified busy registers required at the input boundary.
    pub busy: Vec<ModeBusy>,
}

/// A control with a state-qualified input path independent of REC parameter work.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ParameterIndependentInput {
    pub control: String,
    /// Every condition must match before the parameter checks are inapplicable.
    pub conditions: Vec<InputStateCondition>,
}

/// A bounded integer observation and its permitted values.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct InputStateCondition {
    pub address: u64,
    pub width: usize,
    pub values: Vec<u64>,
}

/// A busy register whose nonzero value withholds native input in one UI mode.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ModeBusy {
    pub mode: u32,
    pub address: u64,
    pub width: usize,
}

/// Firmware-specific remote inputs, without snapshots of live tables.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct FirmwareFacts {
    /// Schema identity.
    #[serde(rename = "$schema")]
    pub schema_uri: String,
    /// Format version; currently 1.
    pub schema: u32,
    /// Firmware identity.
    pub firmware_id: String,
    /// Update content digest.
    pub firmware_sha256: String,
    /// Physical body identity.
    pub body_id: String,
    /// Alias applied to physical RAM reads.
    pub uncached_alias: u64,
    /// Firmware content checks.
    pub identity_checks: Vec<AddressIdentity>,
    /// Settings block location.
    pub configuration: ConfigurationLocation,
    /// UI mode and readiness addresses.
    pub state: FirmwareObservation,
    /// Live menu tables and cursor layout.
    pub menus: MenuTables,
    /// Scanout memory layout.
    pub scanout: Scanout,
    /// Live text and glyph resource layout.
    pub resources: Resources,
}

/// A name accepted by the stock debug key command.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct DebugKey {
    /// Native name.
    pub name: String,
    /// Native event number.
    pub event: u32,
}

/// Live menu roots, row descriptors and navigation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct MenuTables {
    /// Address of the root pointer table.
    pub roots_address: u64,
    /// Number of root pointers.
    pub root_count: u32,
    /// Named root indices.
    pub roles: BTreeMap<String, u32>,
    /// Root indices mapped to caption resource IDs.
    pub root_titles: BTreeMap<u32, u32>,
    /// Declared root entry strategies.
    pub root_routes: BTreeMap<u32, RootRoute>,
    /// Root indices mapped to first tab keys.
    pub page_tabs: BTreeMap<u32, u32>,
    /// Tab navigation bounds.
    pub navigation: MenuNavigation,
    /// Node and row layout.
    pub layout: MenuLayout,
    /// Cursor structure base.
    pub cursor_address: u64,
    /// Cursor and row status layout.
    pub cursor: CursorLayout,
    /// Quick menu geometry and menu ID.
    pub q_grid: QuickGrid,
}

/// A root menu entry strategy.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct RootRoute {
    /// Navigation role used to open the root.
    pub entry: String,
    /// Optional tab key.
    #[serde(default)]
    pub tab: Option<u32>,
}

/// Native menu tab bounds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct MenuNavigation {
    /// Shooting first tab.
    pub shooting_first_tab: u32,
    /// Shooting last tab.
    pub shooting_last_tab: u32,
    /// Setup first tab.
    pub setup_first_tab: u32,
}

/// Offsets within a native menu descriptor and row.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct MenuLayout {
    /// Node byte size.
    pub size: u32,
    /// Node word offsets.
    pub fields: MenuFields,
    /// Row size.
    pub row_size: u32,
    /// Row key.
    pub row_key: u32,
    /// Row child.
    pub row_child: u32,
    /// Caption words.
    pub caption_words: u32,
    /// Sentinel.
    pub sentinel: u32,
    /// Max kind.
    pub max_kind: u32,
    /// Nowrap flags.
    pub nowrap_flags: u32,
}

/// Native menu descriptor word offsets.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct MenuFields {
    /// Menu id.
    pub menu_id: u32,
    /// Type code.
    pub type_code: u32,
    /// Setting id.
    pub setting_id: u32,
    /// Rows per page.
    pub rows_per_page: u32,
    /// Flags word.
    pub flags_word: u32,
    /// Parent pointer.
    pub parent_pointer: u32,
    /// Item array.
    pub item_array: u32,
    /// Handlers pointer.
    pub handlers_pointer: u32,
}

/// Live menu cursor and status-array offsets.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct CursorLayout {
    /// Cursor byte size.
    pub size: u32,
    /// Cursor word offsets.
    pub fields: CursorFields,
    /// Status offset.
    pub status_offset: u32,
    /// Status stride.
    pub status_stride: u32,
    /// Status width.
    pub status_width: u32,
}

/// Live cursor word offsets.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct CursorFields {
    /// Pointer.
    pub pointer: u32,
    /// Cursor.
    pub cursor: u32,
    /// Subcursor.
    pub subcursor: u32,
    /// Visible count.
    pub visible_count: u32,
    /// Value cursor.
    pub value_cursor: u32,
    /// Raw count.
    pub raw_count: u32,
    /// Filtered count.
    pub filtered_count: u32,
}

/// Quick menu dimensions.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct QuickGrid {
    /// Rows.
    pub rows: u32,
    /// Columns.
    pub columns: u32,
    /// Menu code.
    pub menu_code: u32,
}

/// A located framebuffer and its live selection pointer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct Scanout {
    /// Address.
    pub address: u64,
    /// Width.
    pub width: u32,
    /// Height.
    pub height: u32,
    /// Pixel format.
    pub pixel_format: PixelFormat,
    /// Stride bytes.
    pub stride_bytes: u32,
    /// Instance.
    pub instance: u32,
    /// Layer.
    pub layer: u32,
    /// Channel.
    pub channel: u32,
    /// Pointer address.
    pub pointer_address: u64,
    /// Address mask.
    pub address_mask: u64,
}

/// Implemented scanout encodings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub enum PixelFormat {
    Bgra8888,
}

/// Native text and bitmap resource table layouts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct Resources {
    /// Language.
    pub language: ConfigurationLocation,
    /// Text record size.
    pub text_record_size: u32,
    /// Text variants offset.
    pub text_variants_offset: u32,
    /// Text metrics offset.
    pub text_metrics_offset: u32,
    /// Text pointer offset.
    pub text_pointer_offset: u32,
    /// Text unit bytes.
    pub text_unit_bytes: u32,
    /// Text max units.
    pub text_max_units: u32,
    /// Private text first.
    pub private_text_first: u32,
    /// Private text last.
    pub private_text_last: u32,
    /// Private text bias.
    pub private_text_bias: u32,
    /// Glyph first.
    pub glyph_first: u32,
    /// Glyph last.
    pub glyph_last: u32,
    /// Supplementary address.
    pub supplementary_address: u64,
    /// Supplementary first.
    pub supplementary_first: u32,
    /// Last caption.
    pub last_caption: u32,
    /// Supplementary stride.
    pub supplementary_stride: u32,
    /// Supplementary max units.
    pub supplementary_max_units: u32,
    /// Bitmaps.
    pub bitmaps: BitmapResources,
    /// Registry layout locating the icon font in live memory.
    pub glyphs: GlyphResources,
}

/// Registry layout locating the native icon font resource.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct GlyphResources {
    /// Resource registry base address.
    pub registry_address: u64,
    /// Registry record size in bytes.
    pub record_size: u32,
    /// Icon font record index.
    pub icon_index: u32,
    /// Byte offset of the font data pointer.
    pub data_address_offset: u32,
    /// Byte offset of the font byte length.
    pub data_size_offset: u32,
}

/// Native bitmap descriptor table layout.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct BitmapResources {
    /// Table.
    pub table: u64,
    /// Stride.
    pub stride: u32,
    /// Final table.
    pub final_table: u64,
    /// Final first index.
    pub final_first_index: u32,
    /// Last index.
    pub last_index: u32,
    /// Last record.
    pub last_record: u64,
    /// Layout.
    pub layout: BitmapLayout,
}

/// Bitmap record fields, with explicit widths.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct BitmapLayout {
    /// Data bytes.
    pub data_bytes: u32,
    /// Data offset.
    pub data_offset: u32,
    /// Height bytes.
    pub height_bytes: u32,
    /// Height offset.
    pub height_offset: u32,
    /// Kind bytes.
    pub kind_bytes: u32,
    /// Kind offset.
    pub kind_offset: u32,
    /// Width bytes.
    pub width_bytes: u32,
    /// Width offset.
    pub width_offset: u32,
}

/// Build-owned menu declarations; never a snapshot of the live menu tree.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct BuildMenuManifest {
    /// Schema identity.
    #[serde(rename = "$schema")]
    pub schema_uri: String,
    /// Format version; currently 1.
    pub schema: u32,
    /// Firmware identity.
    pub firmware_id: String,
    /// Firmware content digest.
    pub firmware_sha256: String,
    /// Digest of the compiler input used to extract declarations.
    pub elf_sha256: String,
    /// Installed region and changed-section identities.
    pub artifacts: Vec<BuildArtifact>,
    /// Declared menu, native and inline owners.
    pub items: Vec<MenuItem>,
    /// Linked symbol addresses and extents.
    pub symbols: BTreeMap<String, LinkedSymbol>,
    /// Build-file spans for immutable declaration/artwork reads.
    pub memory: Vec<BuildMemory>,
    /// Compiler-produced owner layout constants.
    pub owner_constants: BTreeMap<String, u32>,
    /// Compiler-derived coverage and observation metadata.
    pub walkthrough: WalkthroughMetadata,
}

/// A build output bound to this manifest.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct BuildArtifact {
    /// Relative build file.
    pub file: String,
    /// Byte length.
    pub size: u64,
    /// Content digest.
    pub sha256: String,
}

/// A linked address range backed by an installed artifact.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct BuildMemory {
    /// Address.
    pub address: u64,
    /// Size.
    pub size: u64,
    /// File.
    pub file: String,
    /// Offset.
    pub offset: u64,
}

/// Linked symbol identity.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct LinkedSymbol {
    /// Linked address.
    pub address: u64,
    /// Declared byte size.
    pub size: u64,
}

/// Surfaces on which an owner is available.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Surface {
    Menu,
    Q,
    Fn,
}

/// Stable option identity and native key.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MenuOption {
    /// Stable name supplied by the declaration.
    pub name: String,
    /// Native choice key or normalized option index.
    pub key: u32,
    /// Persisted value when declared.
    pub value: Option<u32>,
}

/// One compiled UI owner with placement and stable option identities.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct MenuItem {
    /// Name.
    pub name: String,
    /// Owning_mod.
    #[serde(rename = "mod")]
    pub owning_mod: String,
    /// Identity.
    pub identity: String,
    /// Address.
    pub address: u64,
    /// Binding.
    pub binding: u64,
    /// Parent.
    pub parent: u64,
    /// Parent item.
    pub parent_item: u64,
    /// Label.
    pub label: u64,
    /// Page.
    pub page: u64,
    /// Open page.
    pub open_page: u64,
    /// Event.
    pub event: u64,
    /// Input.
    pub input: u64,
    /// Activate.
    pub activate: u64,
    /// Fn action.
    pub fn_action: u64,
    /// Native row key.
    pub key: u32,
    /// Whether Q assignment is declared.
    pub q_assignable: u32,
    /// Declared label.
    pub label_text: String,
    /// Declared surfaces.
    pub surfaces: Vec<Surface>,
    /// Declared choice capacity.
    pub choice_capacity: u32,
    /// Selectable choice count.
    pub choice_count: u32,
    /// Stable named choices; open catalogs may have unnamed entries.
    pub options: Vec<MenuOption>,
    /// Stock node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stock_node: Option<u64>,
    /// Stock parent setting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stock_parent_setting: Option<u32>,
    /// Native.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native: Option<NativeSetting>,
    /// Inline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inline: Option<InlineChoices>,
}

/// Compiler-declared stock choice observation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct NativeSetting {
    /// Owner.
    pub owner: u32,
    /// Canonical.
    pub canonical: u32,
    /// Node.
    pub node: u32,
    /// Setting.
    pub setting: u32,
    /// Children.
    pub children: u32,
    /// Static count.
    pub static_count: u32,
    /// Dynamic count.
    pub dynamic_count: u32,
    /// Dynamic key first.
    pub dynamic_key_first: u32,
    /// Dynamic value first.
    pub dynamic_value_first: u32,
    /// Value address.
    pub value_address: u32,
    /// Surface.
    pub surface: u32,
    /// Parent.
    pub parent: u32,
    /// Key.
    pub key: u32,
    /// Static values.
    pub static_values: u32,
    /// Capacity.
    pub capacity: u32,
    /// name.
    pub name: String,
    /// owning_mod.
    #[serde(rename = "mod")]
    pub owning_mod: String,
    /// identity.
    pub identity: String,
    /// surface_name.
    pub surface_name: String,
    /// keys.
    pub keys: Vec<u32>,
    /// values.
    pub values: Vec<u32>,
}

/// Compiler-declared canvas choice observation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct InlineChoices {
    /// Owner.
    pub owner: u32,
    /// Host.
    pub host: u32,
    /// Screen.
    pub screen: u32,
    /// Row.
    pub row: u32,
    /// Count.
    pub count: u32,
    /// Value address.
    pub value_address: u32,
    /// Callback.
    pub callback: u32,
    /// Values.
    pub values: u32,
    /// Table count.
    pub table_count: u32,
    /// Stride.
    pub stride: u32,
    /// Last value.
    pub last_value: u32,
    /// name.
    pub name: String,
    /// owning_mod.
    #[serde(rename = "mod")]
    pub owning_mod: String,
    /// identity.
    pub identity: String,
    /// Selectable values.
    pub accepted_values: Vec<u32>,
}

/// Build-time validated observation layouts and coverage.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct WalkthroughMetadata {
    /// Menu layout.
    pub menu: Option<DeclarationLayout>,
    /// Qfn layout.
    pub qfn: Option<QfnLayout>,
    /// Binding layout.
    pub binding: Option<BindingLayout>,
    /// Canvas layout.
    pub canvas: Option<CanvasLayout>,
    /// Shared choice storage identities.
    pub choice_sets: BTreeMap<String, String>,
    /// Exact binding picker enrollment.
    pub binding_choices: BTreeMap<u64, BindingChoices>,
    /// Enrolled host screen identities.
    pub canvas_hosts: BTreeMap<u64, CanvasHost>,
    /// Required surface observations.
    pub requirements: Vec<(Surface, String)>,
    /// Required selections.
    pub option_requirements: Vec<(String, u32)>,
    /// Required screen observations.
    pub screen_requirements: Vec<(String, String, u32)>,
}

/// Compiler-produced DeclarationLayout offsets.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct DeclarationLayout {
    /// Magic.
    pub magic: u32,
    /// Version.
    pub version: u32,
    /// Item size.
    pub item_size: u32,
    /// Parent.
    pub parent: u32,
    /// Parent item.
    pub parent_item: u32,
    /// Label.
    pub label: u32,
    /// Key.
    pub key: u32,
    /// Page.
    pub page: u32,
    /// Open page.
    pub open_page: u32,
    /// Event.
    pub event: u32,
    /// Input.
    pub input: u32,
    /// Activate.
    pub activate: u32,
    /// Fn action.
    pub fn_action: u32,
    /// Q assignable.
    pub q_assignable: u32,
    /// Node size.
    pub node_size: u32,
    /// Cursor active.
    pub cursor_active: u32,
    /// Binding id.
    pub binding_id: u32,
}

/// Compiler-produced QfnLayout offsets.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct QfnLayout {
    /// Magic.
    pub magic: u32,
    /// Version.
    pub version: u32,
    /// Binding size.
    pub binding_size: u32,
    /// Owner.
    pub owner: u32,
    /// Choices.
    pub choices: u32,
    /// Fn action.
    pub fn_action: u32,
    /// Setting.
    pub setting: u32,
    /// Setting size.
    pub setting_size: u32,
    /// Setting context.
    pub setting_context: u32,
    /// Setting apply.
    pub setting_apply: u32,
    /// Used.
    pub used: u32,
    /// Bindings.
    pub bindings: u32,
    /// Staged.
    pub staged: u32,
    /// Active.
    pub active: u32,
    /// Session.
    pub session: u32,
    /// Trigger phase.
    pub trigger_phase: u32,
    /// Capacity.
    pub capacity: u32,
    /// Assignments.
    pub assignments: u32,
    /// Stock choice.
    pub stock_choice: u32,
}

/// Compiler-produced BindingLayout offsets.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct BindingLayout {
    /// Magic.
    pub magic: u32,
    /// Version.
    pub version: u32,
    /// Binding size.
    pub binding_size: u32,
    /// Apply.
    pub apply: u32,
    /// Capacity.
    pub capacity: u32,
}

/// Compiler-produced CanvasLayout offsets.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct CanvasLayout {
    /// Magic.
    pub magic: u32,
    /// Version.
    pub version: u32,
    /// Host size.
    pub host_size: u32,
    /// Screens.
    pub screens: u32,
    /// Screen count.
    pub screen_count: u32,
    /// Screen size.
    pub screen_size: u32,
    /// Id.
    pub id: u32,
    /// Live host.
    pub live_host: u32,
    /// Live screen.
    pub live_screen: u32,
    /// Live busy.
    pub live_busy: u32,
}

/// Exact binding picker owners.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct BindingChoices {
    /// Selectable count.
    pub count: u32,
    /// Native prefix owners.
    pub native_owners: Vec<u64>,
    /// Curated mod owners.
    pub mod_owners: Vec<u64>,
}

/// Declared canvas screen identities.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct CanvasHost {
    /// Owning menu identity.
    pub identity: String,
    /// Declared screen IDs.
    pub screens: BTreeSet<u32>,
}

impl Facts {
    /// Validate identities before exposing any remote operations.
    pub fn validate(&self) -> Result<(), String> {
        self.firmware.validate()?;
        for input in &self.parameter_independent_inputs {
            self.physical_controls.control(&input.control)?;
            if input.conditions.is_empty()
                || input.conditions.iter().any(|state| {
                    state.address == 0
                        || !matches!(state.width, 1 | 2 | 4 | 8)
                        || state.values.is_empty()
                        || state
                            .values
                            .iter()
                            .any(|value| state.width < 8 && *value >= 1u64 << (state.width * 8))
                })
            {
                return Err("Invalid parameter-independent input conditions".into());
            }
        }
        if let Some(tasks) = &self.input_tasks {
            let distinct: BTreeSet<_> = tasks.entries.iter().collect();
            if tasks.headers.is_empty()
                || tasks.entries.is_empty()
                || distinct.len() != tasks.entries.len()
                || tasks.max_count == 0
                || tasks.max_count > 1024
                || tasks.stride < 4
                || tasks
                    .descriptor_offset
                    .checked_add(4)
                    .is_none_or(|end| end > tasks.stride)
                || tasks.state_offset >= tasks.stride
                || tasks.entry_offset > 4096
                || tasks
                    .entry_offset
                    .checked_add(4)
                    .is_none_or(|end| end > tasks.descriptor_size)
                || tasks.address_limit == 0
                || tasks.address_mask == 0
                || tasks
                    .busy
                    .iter()
                    .any(|gate| gate.address == 0 || !matches!(gate.width, 1 | 2 | 4 | 8))
            {
                return Err("Invalid input task observation layout".into());
            }
        }
        if let Some(manifest) = &self.menu_manifest {
            manifest.validate()?;
            if manifest.firmware_id != self.firmware.firmware_id
                || manifest.firmware_sha256 != self.firmware.firmware_sha256
            {
                return Err("menu manifest belongs to a different firmware".into());
            }
        }
        Ok(())
    }
}

impl FirmwareFacts {
    /// Check supported layouts and bounded address extents.
    pub fn validate(&self) -> Result<(), String> {
        if self.schema != 1 || self.schema_uri != FIRMWARE_FACTS_SCHEMA_URI {
            return Err("unsupported remote firmware schema".into());
        }
        fujimod_catalog_identity::validate_id(&self.firmware_id)?;
        fujimod_catalog_identity::validate_id(&self.body_id)?;
        fujimod_catalog_identity::validate_content_sha256(&self.firmware_sha256.clone())?;
        if self.identity_checks.is_empty() {
            return Err("remote facts require firmware identity checks".into());
        }
        for identity in &self.identity_checks {
            identity.validate()?;
        }
        let m = &self.menus;
        if m.roots_address == 0
            || m.root_count == 0
            || m.layout.size < 32
            || m.layout.row_size < 8
            || m.cursor.size < 32
            || m.cursor_address == 0
            || m.q_grid.rows == 0
            || m.q_grid.columns == 0
            || m.roles.values().any(|index| *index >= m.root_count)
        {
            return Err("invalid remote menu layout or roots".into());
        }
        let node = &m.layout.fields;
        let cursor = &m.cursor.fields;
        for (width, offsets) in [
            (
                m.layout.size,
                vec![
                    node.menu_id,
                    node.type_code,
                    node.setting_id,
                    node.rows_per_page,
                    node.flags_word,
                    node.parent_pointer,
                    node.item_array,
                    node.handlers_pointer,
                ],
            ),
            (
                m.cursor.size,
                vec![
                    cursor.pointer,
                    cursor.cursor,
                    cursor.subcursor,
                    cursor.visible_count,
                    cursor.value_cursor,
                    cursor.raw_count,
                    cursor.filtered_count,
                ],
            ),
            (
                m.layout.row_size,
                vec![m.layout.row_key, m.layout.row_child],
            ),
        ] {
            let unique: BTreeSet<_> = offsets.iter().copied().collect();
            if width < 4
                || unique.len() != offsets.len()
                || offsets
                    .iter()
                    .any(|offset| offset % 4 != 0 || *offset > width - 4)
            {
                return Err("remote menu field offset exceeds its record or overlaps".into());
            }
        }
        if m.cursor.status_width != 4
            || m.cursor.status_stride < m.cursor.status_width
            || m.cursor.status_offset < m.cursor.size
        {
            return Err("invalid remote cursor status layout".into());
        }
        let r = &self.resources;
        let glyphs = &r.glyphs;
        if r.text_unit_bytes != 2
            || r.text_record_size < 8
            || r.text_variants_offset >= r.text_record_size
            || r.text_metrics_offset > r.text_record_size - 4
            || r.text_max_units == 0
            || r.supplementary_max_units == 0
            || r.supplementary_first > r.last_caption
            || r.private_text_first > r.private_text_last
            || r.glyph_first > r.glyph_last
            || glyphs.registry_address == 0
            || glyphs.record_size < 4
            || glyphs.data_address_offset > glyphs.record_size - 4
            || glyphs.data_size_offset > glyphs.record_size - 4
        {
            return Err("invalid remote resource layout".into());
        }
        let s = &self.scanout;
        if s.width == 0
            || s.height == 0
            || u64::from(s.stride_bytes) < u64::from(s.width) * 4
            || u64::from(s.stride_bytes) * u64::from(s.height) > u32::MAX.into()
        {
            return Err("invalid remote scanout extent".into());
        }
        Ok(())
    }
}

impl BuildMenuManifest {
    /// Check declaration identities and their artifact bindings.
    pub fn validate(&self) -> Result<(), String> {
        if self.schema != 1 || self.schema_uri != MENU_MANIFEST_SCHEMA_URI {
            return Err("unsupported build menu schema".into());
        }
        fujimod_catalog_identity::validate_id(&self.firmware_id)?;
        for digest in [&self.firmware_sha256, &self.elf_sha256] {
            fujimod_catalog_identity::validate_content_sha256(&digest.clone())?;
        }
        let mut files = BTreeSet::new();
        for artifact in &self.artifacts {
            let path = std::path::Path::new(&artifact.file);
            if path.is_absolute()
                || path
                    .components()
                    .any(|c| !matches!(c, std::path::Component::Normal(_)))
                || !files.insert(&artifact.file)
            {
                return Err("invalid or repeated build artifact path".into());
            }
            fujimod_catalog_identity::validate_content_sha256(&artifact.sha256.clone())?;
        }
        for span in &self.memory {
            let artifact = self
                .artifacts
                .iter()
                .find(|a| a.file == span.file)
                .ok_or("memory span has no artifact")?;
            if span.address.checked_add(span.size).is_none()
                || span
                    .offset
                    .checked_add(span.size)
                    .is_none_or(|end| end > artifact.size)
            {
                return Err("build memory span exceeds artifact".into());
            }
        }
        let mut ids = BTreeSet::new();
        let mut addresses = BTreeSet::new();
        for item in &self.items {
            if item.identity != format!("{}/{}", item.owning_mod, item.name)
                || item.name.is_empty()
                || item.owning_mod.is_empty()
                || !ids.insert(&item.identity)
                || !addresses.insert(item.address)
                || item.choice_count > item.choice_capacity
            {
                return Err("invalid or repeated build menu identity/capacity".into());
            }
            let mut names = BTreeSet::new();
            if item
                .options
                .iter()
                .any(|o| o.name.is_empty() || !names.insert(&o.name))
            {
                return Err("empty or repeated stable option name".into());
            }
        }
        if self
            .walkthrough
            .requirements
            .iter()
            .any(|(_, id)| !ids.contains(id))
            || self
                .walkthrough
                .option_requirements
                .iter()
                .any(|(id, _)| !ids.contains(id))
            || self
                .walkthrough
                .screen_requirements
                .iter()
                .any(|(_, id, _)| !ids.contains(id))
        {
            return Err("coverage refers to an unknown menu identity".into());
        }
        Ok(())
    }
}

/// Firmware addresses used by a supported run observer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct FirmwareObservation {
    /// Firmware address containing the UI mode.
    pub ui_mode: u64,
    /// Firmware address containing the system mode.
    pub system_mode: u64,
    /// Firmware address containing the menu-active flag.
    pub menu_active: u64,
    /// Firmware address containing the pending UI mode.
    pub pending_mode: u64,
    /// Firmware address containing the current menu node.
    pub menu_node: u64,
    /// REC mode value.
    pub rec_mode: u32,
    /// Setup mode value.
    pub setup_mode: u32,
    /// Readiness observer addresses.
    pub readiness: ReadinessAddresses,
}

/// Addresses used to check REC readiness.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct ReadinessAddresses {
    /// Firmware address for readiness category one.
    pub category_one: u64,
    /// Firmware address for readiness category two.
    pub category_two: u64,
    /// Firmware address for pending input state.
    pub input_pending: u64,
    /// Mask of readiness bits excluded from the check.
    pub excluded_mask: u32,
    /// Firmware address for the operation-alert timer.
    pub operation_alert_timer: u64,
    /// Firmware address used to override readiness.
    pub override_address: u64,
}
