//! Canonical formatting for the hand-authored catalog records.
//!
//! Every record is parsed into the Rust type that declares it and serialized
//! back, so its key order is the type's field order and hand edits and machine
//! edits produce the same bytes. Generated files are never formatted.

use fujimod_catalog::{Body, Firmware, Ip, Part, Soc};
use fujimod_catalog_controls::PhysicalControls;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::fs;
use std::path::{Path, PathBuf};

/// One hand-authored catalog record shape.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecordKind {
    /// `soc/<id>/soc.json`.
    Soc,
    /// `ip/<id>.json`.
    Ip,
    /// `bodies/<id>/body.json`.
    Body,
    /// `bodies/<id>/controls.json`.
    Controls,
    /// `parts/<id>/part.json`.
    Part,
    /// `firmware/<body>/<version>/firmware.json`.
    Firmware,
}

impl RecordKind {
    /// The record a path names inside a catalog root, or `None` for a
    /// generated file or a path the formatter does not own.
    pub fn for_path(root: &Path, path: &Path) -> Option<Self> {
        let relative = path.strip_prefix(root).ok()?;
        let parts: Vec<&str> = relative.iter().filter_map(|part| part.to_str()).collect();
        match parts.as_slice() {
            ["soc", _, "soc.json"] => Some(Self::Soc),
            ["ip", file] if file.ends_with(".json") && !file.ends_with(".generated.json") => {
                Some(Self::Ip)
            }
            ["bodies", _, "body.json"] => Some(Self::Body),
            ["bodies", _, "controls.json"] => Some(Self::Controls),
            ["parts", _, "part.json"] => Some(Self::Part),
            ["firmware", _, _, "firmware.json"] => Some(Self::Firmware),
            _ => None,
        }
    }
}

/// Format one record of a known kind.
pub fn format_record(kind: RecordKind, bytes: &[u8]) -> Result<Vec<u8>, String> {
    match kind {
        RecordKind::Soc => format::<Soc>(bytes),
        RecordKind::Ip => format::<Ip>(bytes),
        RecordKind::Body => format::<Body>(bytes),
        RecordKind::Controls => format::<PhysicalControls>(bytes),
        RecordKind::Part => format::<Part>(bytes),
        RecordKind::Firmware => format::<Firmware>(bytes),
    }
}

fn format<T: DeserializeOwned + Serialize>(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let record: T =
        serde_json::from_slice(bytes).map_err(|error| format!("cannot parse record: {error}"))?;
    let mut canonical = serde_json::to_vec_pretty(&record)
        .map_err(|error| format!("cannot serialize record: {error}"))?;
    canonical.push(b'\n');
    Ok(canonical)
}

/// Every hand-authored record under `root`, in path order.
pub fn records(root: &Path) -> Result<Vec<(PathBuf, RecordKind)>, String> {
    let mut found = Vec::new();
    walk(root, root, &mut found)?;
    found.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(found)
}

fn walk(
    root: &Path,
    directory: &Path,
    found: &mut Vec<(PathBuf, RecordKind)>,
) -> Result<(), String> {
    let entries = fs::read_dir(directory)
        .map_err(|error| format!("cannot read {}: {error}", directory.display()))?;
    for entry in entries {
        let entry =
            entry.map_err(|error| format!("cannot read {}: {error}", directory.display()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
        if file_type.is_dir() {
            walk(root, &path, found)?;
        } else if file_type.is_file()
            && let Some(kind) = RecordKind::for_path(root, &path)
        {
            found.push((path, kind));
        }
    }
    Ok(())
}

/// The outcome of formatting or checking a catalog root.
#[derive(Debug, Default)]
pub struct Report {
    /// Records whose bytes change, or would change when checking.
    pub changed: Vec<PathBuf>,
    /// Records that already match their canonical form.
    pub unchanged: usize,
    /// Records the formatter could not parse, with the reason.
    pub problems: Vec<(PathBuf, String)>,
}

/// Write every record that differs from its canonical form.
pub fn write_tree(root: &Path) -> Result<Report, String> {
    visit(root, true)
}

/// Report every record that differs from its canonical form without writing.
pub fn check_tree(root: &Path) -> Result<Report, String> {
    visit(root, false)
}

fn visit(root: &Path, write: bool) -> Result<Report, String> {
    let mut report = Report::default();
    for (path, kind) in records(root)? {
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) => {
                report
                    .problems
                    .push((path, format!("cannot read: {error}")));
                continue;
            }
        };
        let canonical = match format_record(kind, &bytes) {
            Ok(canonical) => canonical,
            Err(error) => {
                report.problems.push((path, error));
                continue;
            }
        };
        if canonical == bytes {
            report.unchanged += 1;
            continue;
        }
        report.changed.push(path.clone());
        if write && let Err(error) = fs::write(&path, &canonical) {
            report
                .problems
                .push((path, format!("cannot write: {error}")));
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{"$schema":"https://fujimod.com/schemas/ip.schema.json","schema":1,"id":"example","evidence":[{"firmware_ids":["xt3-511"],"kind":"inferred"}]}"#;

    #[test]
    fn formatting_a_record_twice_gives_the_same_bytes() {
        let once = format_record(RecordKind::Ip, SAMPLE.as_bytes()).unwrap();
        let twice = format_record(RecordKind::Ip, &once).unwrap();
        assert_eq!(once, twice);
        assert!(once.ends_with(b"\n"));
        let text = String::from_utf8(once).unwrap();
        assert!(text.contains("\n  \"schema\": 1"));
    }

    #[test]
    fn only_hand_authored_record_paths_are_formatted() {
        let root = Path::new("catalog");
        assert_eq!(
            RecordKind::for_path(root, Path::new("catalog/ip/shared.json")),
            Some(RecordKind::Ip)
        );
        assert_eq!(
            RecordKind::for_path(root, Path::new("catalog/soc/xp4/soc.json")),
            Some(RecordKind::Soc)
        );
        assert_eq!(
            RecordKind::for_path(root, Path::new("catalog/bodies/xt3/controls.json")),
            Some(RecordKind::Controls)
        );
        assert_eq!(
            RecordKind::for_path(root, Path::new("catalog/firmware/xt3/5.11/firmware.json")),
            Some(RecordKind::Firmware)
        );
        assert_eq!(
            RecordKind::for_path(
                root,
                Path::new("catalog/firmware/xt3/5.11/bulk.generated.json")
            ),
            None
        );
    }

    #[test]
    fn writing_a_tree_twice_changes_nothing_the_second_time() {
        let root =
            std::env::temp_dir().join(format!("fujimod-catalog-format-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("ip")).unwrap();
        fs::write(root.join("ip/example.json"), SAMPLE).unwrap();

        let first = write_tree(&root).unwrap();
        assert_eq!(first.changed.len(), 1);
        assert!(first.problems.is_empty());
        let second = write_tree(&root).unwrap();
        assert!(second.changed.is_empty());
        assert_eq!(second.unchanged, 1);
        assert!(check_tree(&root).unwrap().changed.is_empty());

        fs::remove_dir_all(&root).unwrap();
    }
}
