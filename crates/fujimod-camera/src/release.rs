//! The published FujiMod SD card for a firmware, downloaded once into the
//! state directory.
use crate::Error;
use fujimod_emulator_camera::{Library, StateDir};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};

/// Where the website publishes release metadata and files.
pub const RELEASES_URL: &str = "https://fujimod.com/releases/";
/// The current release's SD card tree for a firmware, downloaded once into
/// `<state>/sd-cards/<release>/<firmware>`.
pub fn card(state: &StateDir, library: &Library, firmware: &str) -> Result<PathBuf, Error> {
    if !name_ok(firmware) {
        return Err(Error::Release(format!(
            "invalid firmware identity {firmware:?}"
        )));
    }
    let tag = current_tag()?;
    let directory = state.sd_cards().join(&tag).join(firmware);
    let cache_manifest = directory
        .parent()
        .ok_or_else(|| Error::Release("card cache has no parent".into()))?
        .join(format!("{firmware}.json"));
    if cached(&directory, &cache_manifest, &tag, firmware) {
        return Ok(directory);
    }
    let index = index(&tag)?;
    let manifest = ota_manifest(&tag, firmware)?;
    let soc = library
        .resolved(firmware)
        .ok()
        .map(|resolved| resolved.reference.soc.id);
    let files = plan(&tag, &index, &manifest, firmware, soc.as_deref())?;
    write(&tag, firmware, &directory, &cache_manifest, &files)?;
    Ok(directory)
}

#[derive(Deserialize)]
struct Current {
    tag: String,
}

fn current_tag() -> Result<String, Error> {
    let url = format!("{RELEASES_URL}current");
    let bytes = fetch(&url)?;
    let current: Current = serde_json::from_slice(&bytes)
        .map_err(|error| Error::Release(format!("{url}: {error}")))?;
    if !tag_ok(&current.tag) {
        return Err(Error::Release(format!(
            "{url}: invalid release tag {:?}",
            current.tag
        )));
    }
    Ok(current.tag)
}

#[derive(Deserialize)]
struct Index {
    release: String,
    firmwares: Vec<IndexFirmware>,
    #[serde(default)]
    sd_card_files: Vec<CardFileSpec>,
}

#[derive(Deserialize)]
struct IndexFirmware {
    id: String,
    #[serde(default)]
    processor: Option<String>,
}

#[derive(Deserialize)]
struct CardFileSpec {
    path: String,
    asset: String,
    sha256: String,
    bytes: u64,
    #[serde(default)]
    processor: Option<String>,
}

fn index(tag: &str) -> Result<Index, Error> {
    let url = format!("{RELEASES_URL}{tag}/patches.json");
    let bytes = fetch(&url)?;
    serde_json::from_slice(&bytes).map_err(|error| Error::Release(format!("{url}: {error}")))
}

#[derive(Deserialize)]
struct OtaManifest {
    firmware: String,
    version: String,
    fs: BTreeMap<String, StoreFile>,
}

#[derive(Deserialize)]
struct StoreFile {
    sha256: String,
    size: u64,
}

fn ota_manifest(tag: &str, firmware: &str) -> Result<OtaManifest, Error> {
    let firmware = firmware.to_uppercase();
    let url = format!("{RELEASES_URL}{tag}/{firmware}-latest.json");
    let bytes = fetch(&url)?;
    let manifest: OtaManifest = serde_json::from_slice(&bytes)
        .map_err(|error| Error::Release(format!("{url}: {error}")))?;
    if manifest.firmware != firmware {
        return Err(Error::Release(format!(
            "{url}: names firmware {}, not {firmware}",
            manifest.firmware
        )));
    }
    if manifest.version.is_empty() || manifest.version.len() > 127 {
        return Err(Error::Release(format!(
            "{url}: invalid version {:?}",
            manifest.version
        )));
    }
    Ok(manifest)
}

#[derive(Debug)]
enum Source {
    Asset(String),
    Data(Vec<u8>),
}

#[derive(Debug)]
struct PlannedFile {
    path: String,
    sha256: String,
    bytes: u64,
    source: Source,
}

#[derive(Serialize)]
struct FirstInstall<'a> {
    #[serde(rename = "gen")]
    generation: u32,
    version: &'a str,
    fs: BTreeMap<String, String>,
}

#[derive(Serialize)]
struct CardState {
    a: StateSlot,
    b: StateSlot,
    stage2: StageTwo,
}

#[derive(Serialize)]
struct StateSlot {
    #[serde(rename = "gen")]
    generation: i32,
    attempts: u32,
    confirmed: bool,
}

#[derive(Serialize)]
struct StageTwo {
    attempts: u32,
    confirmed: bool,
}

fn plan(
    tag: &str,
    index: &Index,
    manifest: &OtaManifest,
    firmware: &str,
    soc: Option<&str>,
) -> Result<Vec<PlannedFile>, Error> {
    if index.release != tag {
        return Err(Error::Release(format!(
            "release {tag} index names release {}",
            index.release
        )));
    }
    let entry = index
        .firmwares
        .iter()
        .find(|entry| entry.id == firmware)
        .ok_or_else(|| Error::Release(format!("release {tag} does not support {firmware}")))?;
    let processor = entry
        .processor
        .as_deref()
        .or_else(|| soc.and_then(processor_for_soc))
        .ok_or_else(|| {
            Error::Release(format!(
                "release {tag} does not name {firmware}'s processor"
            ))
        })?;
    let root = root_files(processor).ok_or_else(|| {
        Error::Release(format!(
            "release {tag} has no SD boot files for {processor}"
        ))
    })?;
    let mut files = Vec::new();
    let mut paths = BTreeSet::new();
    for spec in index
        .sd_card_files
        .iter()
        .filter(|file| file.processor.as_deref() == Some(processor))
    {
        if !paths.insert(spec.path.clone()) {
            return Err(Error::Release(format!(
                "release {tag} lists card file {} twice",
                spec.path
            )));
        }
        files.push(card_file(tag, spec)?);
    }
    for name in root {
        if !paths.contains(*name) {
            return Err(Error::Release(format!(
                "release {tag} is missing {name} for {processor}"
            )));
        }
    }
    let id = firmware.to_uppercase();
    let mut fs = BTreeMap::new();
    let mut store = BTreeMap::<String, (String, u64)>::new();
    for (path, file) in &manifest.fs {
        if !layer_path_ok(path) {
            return Err(Error::Release(format!(
                "release {tag} manifest path {path:?} is invalid"
            )));
        }
        let sha256 = file.sha256.to_lowercase();
        if !digest_ok(&sha256) || file.size > u64::from(u32::MAX) {
            return Err(Error::Release(format!(
                "release {tag} manifest entry {path} is invalid"
            )));
        }
        let name = store_name(&sha256);
        if let Some((previous, _)) = store.get(&name)
            && previous != &sha256
        {
            return Err(Error::Release(format!(
                "release {tag} names two files {name}"
            )));
        }
        store.insert(name, (sha256.clone(), file.size));
        fs.insert(path.clone(), sha256);
    }
    let first = json_line(&FirstInstall {
        generation: 1,
        version: &manifest.version,
        fs,
    })?;
    files.push(data_file(format!("FUJIMOD/SYSTEM/{id}/A.JSN"), first));
    let state = json_line(&CardState {
        a: StateSlot {
            generation: 1,
            attempts: 0,
            confirmed: false,
        },
        b: StateSlot {
            generation: -1,
            attempts: 0,
            confirmed: false,
        },
        stage2: StageTwo {
            attempts: 0,
            confirmed: false,
        },
    })?;
    files.push(data_file(format!("FUJIMOD/SYSTEM/{id}/STATE.JSN"), state));
    for (name, (sha256, bytes)) in store {
        files.push(PlannedFile {
            path: format!("FUJIMOD/SYSTEM/STORE/{name}"),
            sha256,
            bytes,
            source: Source::Asset(name),
        });
    }
    Ok(files)
}

fn card_file(tag: &str, spec: &CardFileSpec) -> Result<PlannedFile, Error> {
    let sha256 = spec.sha256.to_lowercase();
    if !card_path_ok(&spec.path) || !name_ok(&spec.asset) || !digest_ok(&sha256) {
        return Err(Error::Release(format!(
            "release {tag} card file {} is invalid",
            spec.path
        )));
    }
    Ok(PlannedFile {
        path: spec.path.clone(),
        sha256,
        bytes: spec.bytes,
        source: Source::Asset(spec.asset.clone()),
    })
}

fn data_file(path: String, bytes: Vec<u8>) -> PlannedFile {
    PlannedFile {
        path,
        sha256: digest(&bytes),
        bytes: bytes.len() as u64,
        source: Source::Data(bytes),
    }
}

fn json_line<T: Serialize>(value: &T) -> Result<Vec<u8>, Error> {
    let mut bytes =
        serde_json::to_vec_pretty(value).map_err(|error| Error::Release(error.to_string()))?;
    bytes.push(b'\n');
    Ok(bytes)
}

#[derive(Deserialize, Serialize)]
struct CardManifest {
    tag: String,
    firmware: String,
    files: BTreeMap<String, CardFile>,
}

#[derive(Deserialize, Serialize)]
struct CardFile {
    sha256: String,
    bytes: u64,
}

fn cached(directory: &Path, manifest_path: &Path, tag: &str, firmware: &str) -> bool {
    let Ok(bytes) = fs::read(manifest_path) else {
        return false;
    };
    let Ok(manifest) = serde_json::from_slice::<CardManifest>(&bytes) else {
        return false;
    };
    if manifest.tag != tag || manifest.firmware != firmware || manifest.files.is_empty() {
        return false;
    }
    manifest.files.iter().all(|(path, file)| {
        card_path_ok(path)
            && fs::read(directory.join(path)).is_ok_and(|bytes| {
                bytes.len() as u64 == file.bytes && digest(&bytes) == file.sha256
            })
    })
}

fn write(
    tag: &str,
    firmware: &str,
    directory: &Path,
    manifest_path: &Path,
    files: &[PlannedFile],
) -> Result<(), Error> {
    let _ = fs::remove_file(manifest_path);
    if directory.exists() {
        fs::remove_dir_all(directory).map_err(|error| io_error(directory, error))?;
    }
    fs::create_dir_all(directory).map_err(|error| io_error(directory, error))?;
    let mut manifest = CardManifest {
        tag: tag.into(),
        firmware: firmware.into(),
        files: BTreeMap::new(),
    };
    for file in files {
        let bytes = match &file.source {
            Source::Data(bytes) => bytes.clone(),
            Source::Asset(asset) => {
                let url = format!("{RELEASES_URL}{tag}/{asset}");
                let bytes = fetch(&url)?;
                if bytes.len() as u64 != file.bytes || digest(&bytes) != file.sha256 {
                    return Err(Error::Release(format!("{url} failed its integrity check")));
                }
                bytes
            }
        };
        let path = directory.join(&file.path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| io_error(parent, error))?;
        }
        fs::write(&path, &bytes).map_err(|error| io_error(&path, error))?;
        manifest.files.insert(
            file.path.clone(),
            CardFile {
                sha256: file.sha256.clone(),
                bytes: file.bytes,
            },
        );
    }
    let bytes =
        serde_json::to_vec_pretty(&manifest).map_err(|error| Error::Release(error.to_string()))?;
    fs::write(manifest_path, bytes).map_err(|error| io_error(manifest_path, error))?;
    Ok(())
}

fn fetch(url: &str) -> Result<Vec<u8>, Error> {
    let response = ureq::get(url)
        .set("User-Agent", "fujimod-emulator")
        .timeout(Duration::from_secs(60))
        .call()
        .map_err(|error| Error::Release(format!("{url}: {error}")))?;
    let mut bytes = Vec::new();
    response
        .into_reader()
        .read_to_end(&mut bytes)
        .map_err(|error| Error::Release(format!("{url}: {error}")))?;
    Ok(bytes)
}

fn io_error(path: &Path, error: std::io::Error) -> Error {
    Error::Release(format!("{}: {error}", path.display()))
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// The store name the camera derives from a digest.
fn store_name(sha256: &str) -> String {
    format!("{}.{}", &sha256[..8], &sha256[8..11]).to_uppercase()
}

fn processor_for_soc(soc: &str) -> Option<&'static str> {
    match soc {
        "exr2" => Some("EXR Processor II"),
        _ => None,
    }
}

fn root_files(processor: &str) -> Option<&'static [&'static str]> {
    match processor {
        "EXR Processor II" => Some(&["FFBOOT.BIN", "FFSYS.BIN", "FM_EXR2.BIN"]),
        _ => None,
    }
}

fn tag_ok(tag: &str) -> bool {
    let mut chars = tag.chars();
    matches!(chars.next(), Some(first) if first.is_ascii_alphanumeric())
        && tag.len() <= 96
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

fn name_ok(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

fn segment_ok(segment: &str) -> bool {
    !segment.is_empty()
        && segment != "."
        && segment != ".."
        && !segment.contains('\\')
        && !segment.chars().any(char::is_control)
}

fn card_path_ok(path: &str) -> bool {
    !path.is_empty() && path.split('/').all(segment_ok)
}

fn layer_path_ok(path: &str) -> bool {
    path.starts_with('/') && path.len() <= 127 && path.split('/').skip(1).all(segment_ok)
}

fn digest_ok(sha256: &str) -> bool {
    sha256.len() == 64 && sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn spec(path: &str, asset: &str) -> CardFileSpec {
        CardFileSpec {
            path: path.into(),
            asset: asset.into(),
            sha256: SHA_A.into(),
            bytes: 12,
            processor: Some("EXR Processor II".into()),
        }
    }

    fn index() -> Index {
        Index {
            release: "1.0".into(),
            firmwares: [IndexFirmware {
                id: "x70-110".into(),
                processor: Some("EXR Processor II".into()),
            }]
            .into(),
            sd_card_files: [
                spec("FFBOOT.BIN", "FFBOOT.BIN"),
                spec("FFSYS.BIN", "FFSYS.BIN"),
                spec("FM_EXR2.BIN", "FM_EXR2.BIN"),
                spec("FUJIMOD/RECIPES/A.JSN", "A.JSN"),
            ]
            .into(),
        }
    }

    fn manifest() -> OtaManifest {
        OtaManifest {
            firmware: "X70-110".into(),
            version: "1.0".into(),
            fs: [(
                "/firmware.patch".into(),
                StoreFile {
                    sha256: "1111111111111111111111111111111111111111111111111111111111111111"
                        .into(),
                    size: 10,
                },
            )]
            .into(),
        }
    }

    #[test]
    fn a_release_card_has_boot_files_the_system_tree_and_store() {
        let files = plan("1.0", &index(), &manifest(), "x70-110", Some("exr2")).unwrap();
        let paths: Vec<&str> = files.iter().map(|file| file.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "FFBOOT.BIN",
                "FFSYS.BIN",
                "FM_EXR2.BIN",
                "FUJIMOD/RECIPES/A.JSN",
                "FUJIMOD/SYSTEM/X70-110/A.JSN",
                "FUJIMOD/SYSTEM/X70-110/STATE.JSN",
                "FUJIMOD/SYSTEM/STORE/11111111.111",
            ]
        );
        let Source::Data(first) = &files[4].source else {
            panic!("the first-install manifest is generated");
        };
        assert_eq!(
            std::str::from_utf8(first).unwrap(),
            concat!(
                "{\n",
                "  \"gen\": 1,\n",
                "  \"version\": \"1.0\",\n",
                "  \"fs\": {\n",
                "    \"/firmware.patch\": \"1111111111111111111111111111111111111111111111111111111111111111\"\n",
                "  }\n",
                "}\n",
            )
        );
        assert_eq!(files[6].sha256, "1".repeat(64));
        assert_eq!(files[6].bytes, 10);
    }

    #[test]
    fn a_release_card_needs_every_root_boot_file() {
        let mut index = index();
        index.sd_card_files.pop();
        index.sd_card_files.remove(1);
        let error = plan("1.0", &index, &manifest(), "x70-110", None).unwrap_err();
        assert!(error.to_string().contains("missing FFSYS.BIN"), "{error}");
    }

    #[test]
    fn release_paths_stay_inside_the_card() {
        assert!(card_path_ok("FUJIMOD/SYSTEM/X70-110/A.JSN"));
        assert!(!card_path_ok("../escape"));
        assert!(!card_path_ok("/absolute"));
        assert!(!card_path_ok("a//b"));
        assert!(!card_path_ok("a\\b"));
        assert!(layer_path_ok("/firmware.patch"));
        assert!(!layer_path_ok("firmware.patch"));
        assert!(!layer_path_ok("/../firmware.patch"));
        assert!(tag_ok("0.2.2"));
        assert!(!tag_ok("../0.2.2"));
    }
}
