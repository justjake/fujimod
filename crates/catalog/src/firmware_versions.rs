//! Public releases and their internal firmware identities.
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Stable schema identity for firmware version mappings.
pub const SCHEMA_URI: &str = "https://fujimod.com/schemas/firmware-versions.generated.schema.json";

/// One known firmware release, independent of emulation or bulk-read support.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify)
)]
pub struct FirmwareVersion {
    /// Stable identity of the firmware release.
    pub firmware_id: String,
    /// Camera product name.
    pub product: String,
    /// Public release version shown on Fujifilm's download page.
    pub version: String,
    /// Native BCD version, LEN build identifier, or three-part HCE version.
    pub internal_version: String,
    /// SHA-256 of the official update from which the mapping was extracted.
    pub update_sha256: String,
    /// Official camera firmware page with current releases and update instructions.
    pub download_url: String,
    /// Supplied processor boot ROM associated with this firmware release.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boot_rom: Option<crate::reference::BootRomIdentity>,
}

/// Generated mappings for known updates, including bodies without emulator profiles.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct FirmwareVersions {
    /// Schema identity.
    #[serde(rename = "$schema")]
    pub schema_uri: String,
    /// Document version.
    pub schema: u32,
    /// Known releases. Internal versions are comparable only within one product.
    pub releases: Vec<FirmwareVersion>,
}

fn version(value: &str) -> Option<(u8, u8)> {
    let (major, minor) = value.split_once('.')?;
    if major.is_empty()
        || minor.len() != 2
        || !major
            .bytes()
            .chain(minor.bytes())
            .all(|v| v.is_ascii_digit())
    {
        return None;
    }
    let major = major.parse::<u8>().ok()?;
    let minor = minor.parse::<u8>().ok()?;
    (major <= 99).then_some((major, minor))
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum InternalVersion {
    Native(u8, u8),
    Len(String, u8),
    Hce(u8, u8, u8),
}

impl InternalVersion {
    fn parse(value: &str) -> Option<Self> {
        if let Some((major, minor)) = version(value) {
            return Some(Self::Native(major, minor));
        }
        if let Some((model, revision)) = value.split_once("-v") {
            if model.len() != 5
                || !model.starts_with('C')
                || revision.len() != 2
                || !model[1..]
                    .bytes()
                    .chain(revision.bytes())
                    .all(|b| b.is_ascii_digit() || (b'A'..=b'F').contains(&b))
            {
                return None;
            }
            return Some(Self::Len(
                model.into(),
                u8::from_str_radix(revision, 16).ok()?,
            ));
        }
        let (major, suffix) = value.split_once('.')?;
        if suffix.len() != 4 || !suffix.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let (major, minor) = version(&format!("{major}.{}", &suffix[..2]))?;
        Some(Self::Hce(major, minor, suffix[2..].parse().ok()?))
    }

    fn order(&self, next: &Self) -> Option<std::cmp::Ordering> {
        match (self, next) {
            (Self::Native(a, b), Self::Native(c, d)) => Some((a, b).cmp(&(c, d))),
            (Self::Hce(a, b, c), Self::Hce(d, e, f)) => Some((a, b, c).cmp(&(d, e, f))),
            (Self::Len(model, a), Self::Len(other, b)) if model == other => Some(a.cmp(b)),
            _ => None,
        }
    }
}

impl FirmwareVersions {
    /// Decode and check identities, version ordering and official download links.
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        let document: Self = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        if document.schema_uri != SCHEMA_URI || document.schema != 1 {
            return Err("firmware versions schema differs".into());
        }
        let mut ids = BTreeSet::new();
        let mut products: BTreeMap<&str, BTreeMap<(u8, u8), InternalVersion>> = BTreeMap::new();
        for release in &document.releases {
            fujimod_catalog_identity::validate_id(&release.firmware_id)?;
            fujimod_catalog_identity::validate_sha256(&release.update_sha256)?;
            if let Some(rom) = &release.boot_rom {
                rom.content.validate()?;
                if rom.address.checked_add(rom.content.size).is_none() {
                    return Err("boot ROM extent overflows".into());
                }
            }
            let public = version(&release.version).ok_or("invalid public firmware version")?;
            let internal = InternalVersion::parse(&release.internal_version)
                .ok_or("invalid internal firmware version")?;
            if release.product.trim().is_empty()
                || release.product.trim() != release.product
                || !ids.insert(&release.firmware_id)
                || !release
                    .download_url
                    .starts_with("https://www.fujifilm-x.com/")
                || !release
                    .download_url
                    .contains("/support/download/firmware/cameras/")
            {
                return Err(format!(
                    "invalid firmware version identity {}",
                    release.firmware_id
                ));
            }
            if products
                .entry(&release.product)
                .or_default()
                .insert(public, internal)
                .is_some()
            {
                return Err(format!(
                    "repeated public firmware version for {}",
                    release.product
                ));
            }
        }
        for (product, releases) in products {
            let mut previous: Option<InternalVersion> = None;
            for internal in releases.into_values() {
                if let Some(value) = previous.as_ref() {
                    let ordering = value.order(&internal).ok_or_else(|| {
                        format!("incomparable internal firmware identities for {product}")
                    })?;
                    if ordering == std::cmp::Ordering::Greater {
                        return Err(format!("internal firmware versions decrease for {product}"));
                    }
                }
                previous = Some(internal);
            }
        }
        Ok(document)
    }

    /// The same mappings compiled into Rust and WebAssembly consumers.
    pub fn bundled() -> Result<Self, String> {
        Self::decode(include_bytes!(
            "../../../catalog/firmware-versions.generated.json"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn internal_identities_preserve_build_and_patch_components() {
        let first = InternalVersion::parse("C1234-v99").unwrap();
        let second = InternalVersion::parse("C1234-vA0").unwrap();
        assert_eq!(first.order(&second), Some(std::cmp::Ordering::Less));
        assert_eq!(
            first.order(&InternalVersion::parse("C5678-v99").unwrap()),
            None
        );
        let first = InternalVersion::parse("2.0006").unwrap();
        let second = InternalVersion::parse("2.0100").unwrap();
        assert_eq!(first.order(&second), Some(std::cmp::Ordering::Less));
        for value in ["C1234-vZZ", "C1234-v1", "2.001", "2.00000", "nan"] {
            assert_eq!(InternalVersion::parse(value), None);
        }
    }

    #[test]
    fn bundled_map_covers_known_models_and_distinguishes_internal_from_public_versions() {
        let map = FirmwareVersions::bundled().unwrap();
        for (id, internal, public) in [
            ("xt1-552", "2.39", "5.52"),
            ("xh1-215", "1.90", "2.15"),
            ("xm1-101", "1.61", "1.01"),
        ] {
            let entry = map.releases.iter().find(|r| r.firmware_id == id).unwrap();
            assert_eq!(
                (&*entry.internal_version, &*entry.version),
                (internal, public)
            );
        }
    }

    #[test]
    fn processor_rom_identities_cover_releases_without_execution_graphs() {
        let map = FirmwareVersions::bundled().unwrap();
        let rom = |id: &str| {
            map.releases
                .iter()
                .find(|r| r.firmware_id == id)
                .unwrap()
                .boot_rom
                .as_ref()
                .unwrap()
        };
        assert_eq!(rom("x10-201"), rom("xpro1-382"));
        assert_eq!(
            (rom("x10-201").address, rom("x10-201").content.size),
            (0x80000000, 0x4000)
        );
        assert_eq!(
            (rom("xpro2-511").address, rom("xpro2-511").content.size),
            (0xfff00000, 0x10000)
        );
        let mut changed = map.clone();
        changed
            .releases
            .iter_mut()
            .find(|r| r.firmware_id == "xpro2-511")
            .unwrap()
            .boot_rom
            .as_mut()
            .unwrap()
            .address = u64::MAX;
        assert!(
            FirmwareVersions::decode(&serde_json::to_vec(&changed).unwrap())
                .unwrap_err()
                .contains("overflows")
        );
    }

    #[test]
    fn only_same_product_ordering_is_checked_and_decreasing_internal_versions_fail() {
        let mut document = FirmwareVersions::bundled().unwrap();
        let entry = document
            .releases
            .iter_mut()
            .find(|r| r.firmware_id == "xm1-150")
            .unwrap();
        entry.internal_version = "1.60".into();
        assert!(
            FirmwareVersions::decode(&serde_json::to_vec(&document).unwrap())
                .unwrap_err()
                .contains("decrease for X-M1")
        );
        assert_eq!(version("1.09"), Some((1, 9)));
        for text in ["1.9", "1.90beta", "nan", "1.100", "-1.00"] {
            assert_eq!(version(text), None);
        }
    }
}
