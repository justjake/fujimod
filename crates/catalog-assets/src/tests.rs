use super::*;
use serde_json::json;

#[derive(Default)]
struct Memory {
    documents: BTreeMap<Key, Vec<u8>>,
    assets: BTreeMap<String, Vec<u8>>,
}
impl Provider for Memory {
    fn document(&self, reference: &Reference) -> Result<Vec<u8>, String> {
        self.documents
            .get(&reference.key)
            .cloned()
            .ok_or("missing document".into())
    }
    fn asset(&self, asset: &Asset) -> Result<Vec<u8>, String> {
        self.assets
            .get(&asset.id)
            .cloned()
            .ok_or("missing input".into())
    }
}
fn content(bytes: &[u8]) -> Content {
    Content {
        size: bytes.len() as u64,
        sha256: "unused cache key".into(),
    }
}
fn key(kind: Kind, id: &str) -> Key {
    Key {
        kind,
        id: id.into(),
    }
}
fn add(provider: &mut Memory, document: Document<serde_json::Value>) -> Reference {
    let bytes = serde_json::to_vec(&document).unwrap();
    let reference = Reference {
        key: document.key.clone(),
        content: content(&bytes),
    };
    provider.documents.insert(document.key, bytes);
    reference
}
fn document(kind: Kind, id: &str) -> Document<serde_json::Value> {
    Document {
        schema: 1,
        key: key(kind, id),
        references: vec![],
        assets: vec![],
        facts: json!({"clock_hz":24000000}),
    }
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct Facts {
    clock_hz: u64,
}

#[test]
fn resolves_typed_documents_and_loads_separate_assets() {
    let mut provider = Memory::default();
    let processor = add(&mut provider, document(Kind::Processor, "demo.processor"));
    let bytes = b"authored scene input".to_vec();
    let mut body = document(Kind::Body, "demo.body");
    body.references.push(processor);
    body.assets.push(Asset {
        id: "scene".into(),
        content: content(&bytes),
    });
    let root = add(&mut provider, body);
    provider.assets.insert("scene".into(), bytes.clone());
    let catalog = Catalog::resolve(&root, &provider).unwrap();
    assert_eq!(
        catalog.facts::<Facts>(&root.key).unwrap(),
        Facts { clock_hz: 24000000 }
    );
    assert_eq!(catalog.asset("scene", &provider).unwrap(), bytes);
    provider.assets.get_mut("scene").unwrap()[0] ^= 1;
    let changed = provider.assets["scene"].clone();
    assert_eq!(catalog.asset("scene", &provider).unwrap(), changed);
    provider.assets.get_mut("scene").unwrap().push(0);
    assert!(
        catalog
            .asset("scene", &provider)
            .unwrap_err()
            .contains("size")
    );
    assert!(catalog.asset("absent", &provider).is_err());
}

#[test]
fn missing_wrong_size_and_identity_are_distinct_failures() {
    let mut provider = Memory::default();
    let root = add(&mut provider, document(Kind::Firmware, "demo.update"));
    let mut wrong = root.clone();
    wrong.content.size += 1;
    assert!(Catalog::resolve(&wrong, &provider).is_err());
    wrong = root.clone();
    wrong.content.sha256 = "different unused cache key".into();
    assert!(Catalog::resolve(&wrong, &provider).is_ok());
    wrong = root.clone();
    wrong.key.id = "different".into();
    provider
        .documents
        .insert(wrong.key.clone(), provider.documents[&root.key].clone());
    assert!(
        Catalog::resolve(&wrong, &provider)
            .err()
            .unwrap()
            .contains("identity")
    );
    assert!(Catalog::resolve(&root, &Memory::default()).is_err());
}

#[test]
fn schemas_reject_unknown_kinds_fields_and_unconsumed_facts() {
    let mut provider = Memory::default();
    let mut value = serde_json::to_value(document(Kind::Body, "demo.body")).unwrap();
    value["key"]["kind"] = json!("unknown");
    let bytes = serde_json::to_vec(&value).unwrap();
    let root = Reference {
        key: key(Kind::Body, "demo.body"),
        content: content(&bytes),
    };
    provider.documents.insert(root.key.clone(), bytes);
    assert!(Catalog::resolve(&root, &provider).is_err());
    value["key"]["kind"] = json!("body");
    value["path"] = json!("unconsumed");
    let bytes = serde_json::to_vec(&value).unwrap();
    let root = Reference {
        content: content(&bytes),
        ..root
    };
    provider.documents.insert(root.key.clone(), bytes);
    assert!(Catalog::resolve(&root, &provider).is_err());
    let mut body = document(Kind::Body, "typed.body");
    body.facts["unconsumed"] = json!(true);
    let root = add(&mut provider, body);
    let catalog = Catalog::resolve(&root, &provider).unwrap();
    assert!(catalog.facts::<Facts>(&root.key).is_err());
}

#[test]
fn declared_cycles_and_conflicting_asset_identities_refuse() {
    let mut provider = Memory::default();
    let mut child = document(Kind::Body, "cycle.body");
    child.references.push(Reference {
        key: key(Kind::Firmware, "cycle.update"),
        content: Content {
            size: 1,
            sha256: "0".repeat(64),
        },
    });
    let child = add(&mut provider, child);
    let mut root = document(Kind::Firmware, "cycle.update");
    root.references.push(child);
    let root = add(&mut provider, root);
    assert!(
        Catalog::resolve(&root, &provider)
            .err()
            .unwrap()
            .contains("cyclic")
    );
    let mut child = document(Kind::Processor, "conflict.processor");
    child.assets.push(Asset {
        id: "same".into(),
        content: content(b"first"),
    });
    let child = add(&mut provider, child);
    let mut root = document(Kind::Body, "conflict.body");
    root.references.push(child);
    root.assets.push(Asset {
        id: "same".into(),
        content: content(b"second"),
    });
    let root = add(&mut provider, root);
    assert!(
        Catalog::resolve(&root, &provider)
            .err()
            .unwrap()
            .contains("conflicting")
    );
}

#[test]
fn malformed_content_ids_and_duplicate_references_refuse() {
    assert!(
        Content {
            size: 1,
            sha256: "A".repeat(64)
        }
        .validate()
        .is_err()
    );
    for id in ["", "../outside", "/absolute", "drive:path", "space name"] {
        assert!(validate_id(id).is_err());
    }
    let mut provider = Memory::default();
    let child = add(
        &mut provider,
        document(Kind::Processor, "duplicate.processor"),
    );
    let mut root = document(Kind::Body, "duplicate.body");
    root.references = vec![child.clone(), child];
    let root = add(&mut provider, root);
    assert!(
        Catalog::resolve(&root, &provider)
            .err()
            .unwrap()
            .contains("duplicate")
    );
    let empty = content(b"");
    empty.check(b"").unwrap();
}

#[test]
fn loading_checks_exact_byte_counts_without_a_content_digest() {
    let identity = content(b"abc");
    identity.check(b"abc").unwrap();
    identity.check(b"xyz").unwrap();
    assert!(identity.check(b"ab").is_err());
    assert!(identity.check(b"abcd").is_err());
}

#[test]
fn shared_references_and_assets_check_sizes_and_ignore_cache_keys() {
    for conflicting_size in [false, true] {
        let mut provider = Memory::default();
        let processor = add(&mut provider, document(Kind::Processor, "shared.processor"));
        let mut body = document(Kind::Body, "shared.body");
        let mut shared = processor.clone();
        shared.content.sha256 = "another unused cache key".into();
        if conflicting_size {
            shared.content.size += 1;
        }
        body.references.push(shared);
        let body = add(&mut provider, body);
        let mut root = document(Kind::Firmware, "shared.update");
        root.references = vec![processor, body];
        let root = add(&mut provider, root);
        let result = Catalog::resolve(&root, &provider);
        if conflicting_size {
            assert!(result.err().unwrap().contains("conflicting"));
        } else {
            assert!(result.is_ok());
        }
    }

    let mut provider = Memory::default();
    let mut child = document(Kind::Processor, "shared.processor");
    child.assets.push(Asset {
        id: "shared".into(),
        content: content(b"data"),
    });
    let child = add(&mut provider, child);
    let mut root = document(Kind::Body, "shared.body");
    root.references.push(child);
    root.assets.push(Asset {
        id: "shared".into(),
        content: Content {
            size: 4,
            sha256: "another unused cache key".into(),
        },
    });
    let root = add(&mut provider, root);
    provider.assets.insert("shared".into(), b"DATA".to_vec());
    let catalog = Catalog::resolve(&root, &provider).unwrap();
    assert_eq!(catalog.asset("shared", &provider).unwrap(), b"DATA");
}

#[test]
fn duplicate_assets_and_malformed_document_bytes_refuse() {
    let mut provider = Memory::default();
    let mut root = document(Kind::Body, "duplicate.body");
    let asset = Asset {
        id: "duplicate".into(),
        content: content(b"data"),
    };
    root.assets = vec![asset.clone(), asset];
    let root = add(&mut provider, root);
    assert!(
        Catalog::resolve(&root, &provider)
            .err()
            .unwrap()
            .contains("duplicate")
    );
    provider
        .documents
        .insert(root.key.clone(), b"invalid".to_vec());
    let root = Reference {
        content: content(b"invalid"),
        ..root
    };
    assert!(
        Catalog::resolve(&root, &provider)
            .err()
            .unwrap()
            .contains("invalid catalog document")
    );
}

#[test]
fn published_x70_connection_limits_are_nonempty_strings() {
    let bodies =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../catalog/bodies/x70");
    for name in ["x70", "x70-rec-usb"] {
        let path = bodies.join("body.json");
        let bytes =
            std::fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        let document: serde_json::Value = serde_json::from_slice(&bytes)
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        let connections = document["hardware"]["profiles"][name]
            .get("connections")
            .and_then(serde_json::Value::as_array)
            .unwrap_or_else(|| panic!("{}: missing connections", path.display()));
        assert!(
            !connections.is_empty(),
            "{}: no connections",
            path.display()
        );
        for connection in connections {
            let id = connection
                .get("id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("<missing id>");
            let limits = connection
                .get("limits")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_else(|| {
                    panic!(
                        "{} connection {id}: limits must be a string",
                        path.display()
                    )
                });
            assert!(
                !limits.trim().is_empty(),
                "{} connection {id}: limits must not be empty",
                path.display()
            );
        }
    }
}
