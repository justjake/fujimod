//! Located stock on-screen display scene variants, sprites, and selectors.
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const OSD_ITEMS_SCHEMA_URI: &str =
    "https://fujimod.com/schemas/osd-items.generated.schema.json";

/// Generated OSD records located in a firmware image.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(missing_as_null)
)]
pub struct OsdItems {
    #[serde(rename = "$schema")]
    pub schema_uri: String,
    pub schema: u32,
    pub id: String,
    pub firmware_id: String,
    /// SHA-256 of the update from which the records were located.
    pub firmware_sha256: String,
    pub source: OsdItemsSource,
    pub scene_variants: Vec<OsdSceneVariant>,
    pub sprite_records: Vec<OsdSpriteRecord>,
    pub disp_custom_selectors: Vec<DispCustomSelector>,
    /// Joined panel items are present when their anchors have been cataloged.
    #[serde(default)]
    pub items: Option<Vec<OsdItem>>,
}

impl OsdItems {
    /// Validate the generated document identity and its located records.
    pub fn validate(&self) -> Result<(), String> {
        fujimod_catalog_identity::validate_id(&self.id)?;
        fujimod_catalog_identity::validate_id(&self.firmware_id)?;
        fujimod_catalog_identity::validate_sha256(&self.firmware_sha256)?;
        if self.schema_uri != OSD_ITEMS_SCHEMA_URI
            || self.schema != 1
            || self.id != format!("{}-osd-items", self.firmware_id)
            || self.source.row_bytes == 0
            || self.source.row_count == 0
            || self.source.index.is_empty()
            || self.source.canvas_packet_bytes == 0
        {
            return Err("invalid generated OSD identity or source layout".into());
        }
        for address in [
            &self.source.table_address,
            &self.source.canvas_packet_draw,
            &self.source.disp_custom_function,
            &self.source.disp_custom_selector_table,
        ] {
            validate_address(address)?;
        }
        if self.scene_variants.is_empty()
            || self.sprite_records.is_empty()
            || self.disp_custom_selectors.is_empty()
        {
            return Err("generated OSD records must not be empty".into());
        }

        let mut event_ids = BTreeSet::new();
        for variant in &self.scene_variants {
            if variant.event_id == 0
                || variant.event_id > self.source.row_count
                || !event_ids.insert(variant.event_id)
            {
                return Err("invalid or repeated OSD scene event id".into());
            }
            for address in [
                variant.predicate.as_ref(),
                variant.header.as_ref(),
                variant.draw.as_ref(),
                variant.footer.as_ref(),
            ]
            .into_iter()
            .flatten()
            {
                validate_address(address)?;
            }
            let mut panels = BTreeSet::new();
            for panel in &variant.regional_panels {
                validate_address(panel)?;
                if !panels.insert(panel) {
                    return Err("repeated regional OSD panel address".into());
                }
            }
        }

        let mut record_ids = BTreeSet::new();
        for record in &self.sprite_records {
            if record.id.is_empty() || !record_ids.insert(record.id.as_str()) {
                return Err("empty or repeated OSD sprite record id".into());
            }
            validate_address(&record.panel)?;
        }

        let mut selector_ids = BTreeSet::new();
        if self
            .disp_custom_selectors
            .iter()
            .any(|selector| !selector_ids.insert(selector.selector_id))
        {
            return Err("repeated DISP. CUSTOM selector id".into());
        }
        Ok(())
    }
}

/// Source routines and table layout used to locate the generated records.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(missing_as_null)
)]
pub struct OsdItemsSource {
    pub table_address: String,
    pub row_bytes: u32,
    pub row_count: u32,
    pub index: String,
    pub canvas_packet_draw: String,
    pub canvas_packet_bytes: u32,
    pub disp_custom_function: String,
    pub disp_custom_selector_table: String,
}

/// One event-selected stock OSD scene variant.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(missing_as_null)
)]
pub struct OsdSceneVariant {
    pub event_id: u32,
    pub family_id: u32,
    pub predicate: Option<String>,
    pub header: Option<String>,
    pub draw: Option<String>,
    pub footer: Option<String>,
    pub regional_panels: Vec<String>,
}

/// A stock canvas sprite record and its packet-template geometry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(missing_as_null)
)]
pub struct OsdSpriteRecord {
    pub id: String,
    pub panel: String,
    pub kind: OsdSpriteKind,
    pub artwork_id: Option<u32>,
    pub position: OsdPosition,
    pub size: OsdSize,
    pub palette: Option<u32>,
}

/// Kind of stock canvas record.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify)
)]
pub enum OsdSpriteKind {
    Image,
    Text,
    Fill,
}

/// Packet-template position in signed canvas coordinates.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(missing_as_null)
)]
pub struct OsdPosition {
    pub x: i32,
    pub y: i32,
}

/// Packet-template width and height in canvas pixels.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(missing_as_null)
)]
pub struct OsdSize {
    pub width: u32,
    pub height: u32,
}

/// Selector index and stock LCD CUSTOM setting id.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(missing_as_null)
)]
pub struct DispCustomSelector {
    pub selector_id: u32,
    pub setting_id: u32,
}

/// Joined records and canvas boxes associated with one named panel item.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct OsdItem {
    pub anchor: String,
    pub record_ids: Vec<String>,
    pub artwork_ids: Vec<u32>,
    pub setting_id: Option<u32>,
    pub boxes: std::collections::BTreeMap<String, OsdBox>,
}

/// Canvas box for one panel variant.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(missing_as_null)
)]
pub struct OsdBox {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

fn validate_address(address: &str) -> Result<(), String> {
    if address.len() != 10
        || !address.starts_with("0x")
        || !address[2..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(format!("invalid OSD address {address}"));
    }
    Ok(())
}
