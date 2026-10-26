use super::*;
use crate::{Catalog, Document, Kind};
use std::sync::atomic::{AtomicU64, Ordering};

struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let target = std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("target"));
        let parent = target.join("input-provider-tests");
        std::fs::create_dir_all(&parent).unwrap();
        loop {
            let path = parent.join(format!(
                "{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match std::fs::create_dir(&path) {
                Ok(()) => {
                    std::fs::create_dir(path.join("docs")).unwrap();
                    std::fs::create_dir(path.join("assets")).unwrap();
                    return Self(path);
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("test directory: {error}"),
            }
        }
    }
    fn files(&self, location: Location, limit: u64) -> Files {
        Files::new(
            &self.0.join("docs"),
            &self.0.join("assets"),
            BTreeMap::new(),
            BTreeMap::from([("input".into(), location)]),
            limit,
        )
        .unwrap()
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
fn content(bytes: &[u8]) -> Content {
    Content::identify(bytes)
}
fn asset(bytes: &[u8]) -> Asset {
    Asset {
        id: "input".into(),
        content: content(bytes),
    }
}

#[test]
fn locations_are_separate_from_catalog_facts_and_every_load_checks_identity() {
    let directory = Directory::new();
    let bytes = b"authored input";
    std::fs::write(directory.0.join("assets/input.data"), bytes).unwrap();
    let asset = asset(bytes);
    let document = Document {
        schema: 1,
        key: Key {
            kind: Kind::Body,
            id: "demo.body".into(),
        },
        references: vec![],
        assets: vec![asset.clone()],
        facts: BTreeMap::from([("clock_hz", 24000000u64)]),
    };
    let encoded = serde_json::to_vec(&document).unwrap();
    std::fs::write(directory.0.join("docs/body.json"), &encoded).unwrap();
    let reference = Reference {
        key: document.key,
        content: content(&encoded),
    };
    let provider = Files::new(
        &directory.0.join("docs"),
        &directory.0.join("assets"),
        BTreeMap::from([(
            reference.key.clone(),
            Location::File {
                path: "body.json".into(),
            },
        )]),
        BTreeMap::from([(
            "input".into(),
            Location::File {
                path: "input.data".into(),
            },
        )]),
        100,
    )
    .unwrap();
    let catalog = Catalog::resolve(&reference, &provider).unwrap();
    assert_eq!(
        catalog
            .facts::<BTreeMap<String, u64>>(&reference.key)
            .unwrap()["clock_hz"],
        24000000
    );
    assert_eq!(catalog.asset("input", &provider).unwrap(), bytes);
    std::fs::write(directory.0.join("assets/input.data"), b"changed input!").unwrap();
    assert!(catalog.asset("input", &provider).is_err());
}

#[test]
fn declared_windows_are_bounded_and_whole_files_require_the_exact_extent() {
    let directory = Directory::new();
    std::fs::write(directory.0.join("assets/combined.data"), b"HEADbodyTAIL").unwrap();
    let window = directory.files(
        Location::Window {
            path: "combined.data".into(),
            offset: 4,
        },
        4,
    );
    assert_eq!(asset(b"body").load(&window).unwrap(), b"body");
    let whole = directory.files(
        Location::File {
            path: "combined.data".into(),
        },
        100,
    );
    assert!(asset(b"body").load(&whole).is_err());
    let overflow = directory.files(
        Location::Window {
            path: "combined.data".into(),
            offset: u64::MAX,
        },
        100,
    );
    assert!(asset(b"body").load(&overflow).is_err());
    assert!(asset(b"body!").load(&window).is_err());
}

#[test]
fn absent_inputs_changed_bytes_and_caller_limits_refuse() {
    let directory = Directory::new();
    let provider = directory.files(
        Location::File {
            path: "absent".into(),
        },
        4,
    );
    assert!(asset(b"data").load(&provider).is_err());
    std::fs::write(directory.0.join("assets/absent"), b"DATA").unwrap();
    assert!(asset(b"data").load(&provider).is_err());
    let mut missing = asset(b"DATA");
    missing.id = "other".into();
    assert!(missing.load(&provider).is_err());
    assert!(
        asset(b"longer")
            .load(&provider)
            .unwrap_err()
            .contains("limit")
    );
}

#[test]
fn absolute_parent_and_foreign_separator_paths_refuse_before_io() {
    let directory = Directory::new();
    for path in [
        "../outside",
        "nested/../outside",
        "/outside",
        "nested\\outside",
        "",
    ] {
        let result = Files::new(
            &directory.0.join("docs"),
            &directory.0.join("assets"),
            BTreeMap::new(),
            BTreeMap::from([("input".into(), Location::File { path: path.into() })]),
            100,
        );
        assert!(result.is_err(), "{path}");
    }
}

#[cfg(unix)]
#[test]
fn symlink_escape_and_non_regular_sources_refuse() {
    let directory = Directory::new();
    std::fs::write(directory.0.join("outside"), b"data").unwrap();
    std::os::unix::fs::symlink(directory.0.join("outside"), directory.0.join("assets/link"))
        .unwrap();
    let provider = directory.files(
        Location::File {
            path: "link".into(),
        },
        100,
    );
    assert!(
        asset(b"data")
            .load(&provider)
            .unwrap_err()
            .contains("escapes")
    );
    std::fs::create_dir(directory.0.join("assets/directory")).unwrap();
    let provider = directory.files(
        Location::File {
            path: "directory".into(),
        },
        100,
    );
    assert!(
        asset(b"data")
            .load(&provider)
            .unwrap_err()
            .contains("regular")
    );
}

#[test]
fn unknown_location_kinds_and_fields_refuse() {
    for json in [
        r#"{"kind":"guessed","path":"input"}"#,
        r#"{"kind":"file","path":"input","extra":true}"#,
    ] {
        assert!(serde_json::from_str::<Location>(json).is_err());
    }
}
