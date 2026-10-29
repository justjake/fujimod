use crate::*;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
fn function(
    index: &mut Index,
    fw: &str,
    unit: &str,
    a: u64,
    text: &[&str],
    calls: &[(usize, u64)],
    names: &[&str],
) {
    let tokens = text
        .iter()
        .map(|s| {
            let n = index.tokens.len() as u32;
            *index.intern.entry(s.to_string()).or_insert_with(|| {
                index.tokens.push(s.to_string());
                n
            })
        })
        .collect();
    index.functions.push(Function {
        header: FunctionHeader {
            key: FunctionKey {
                firmware: fw.into(),
                unit: unit.into(),
                address: a,
            },
            architecture: Architecture::Arm32,
            names: names.iter().map(|s| s.to_string()).collect(),
            annotations: vec![],
            ranges: vec![(a, a + 4 * text.len() as u64)],
            signature: Sha256::digest(text.join("\n")).into(),
            instructions: text.len(),
            calls: calls.into(),
        },
        tokens,
        references: vec![],
        constants: BTreeSet::new(),
        strings: BTreeSet::new(),
        ngrams: BTreeSet::new(),
    });
}
/// Save `index`, then check that every store lookup answers exactly as the
/// in-memory index does and that loading restores the same answers.
fn assert_store_matches(index: &Index, label: &str) {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/corpus-tests");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{label}-{}.sqlite", std::process::id()));
    index.save(&path).unwrap();
    let store = Store::open(&path).unwrap();
    let loaded = store.load().unwrap();
    fn same<A: serde::Serialize, B: serde::Serialize>(what: &str, a: A, b: B) {
        assert_eq!(
            serde_json::to_value(a).unwrap(),
            serde_json::to_value(b).unwrap(),
            "{what}"
        );
    }
    same("stats", index.stats().unwrap(), store.stats().unwrap());
    same(
        "loaded stats",
        index.stats().unwrap(),
        loaded.stats().unwrap(),
    );
    same(
        "all",
        index.find(None, None, None, None).unwrap(),
        store.find(None, None, None, None).unwrap(),
    );
    let names: BTreeSet<String> = index
        .functions
        .iter()
        .flat_map(|f| f.names.clone())
        .collect();
    let strings: BTreeSet<String> = index
        .functions
        .iter()
        .flat_map(|f| f.strings.clone())
        .collect();
    let firmwares: BTreeSet<String> = index
        .functions
        .iter()
        .map(|f| f.key.firmware.clone())
        .collect();
    for name in &names {
        for firmware in firmwares.iter().map(|f| Some(f.as_str())).chain([None]) {
            same(
                name,
                index.find(Some(name), None, firmware, None).unwrap(),
                store.find(Some(name), None, firmware, None).unwrap(),
            );
            same(
                name,
                index.find(Some(name), None, firmware, None).unwrap(),
                loaded.find(Some(name), None, firmware, None).unwrap(),
            );
        }
    }
    for firmware in &firmwares {
        same(
            firmware,
            index
                .find(None, None, Some(firmware), Some("main"))
                .unwrap(),
            store
                .find(None, None, Some(firmware), Some("main"))
                .unwrap(),
        );
        same(
            firmware,
            index.find_strings(&strings, Some(firmware)).unwrap(),
            store.find_strings(&strings, Some(firmware)).unwrap(),
        );
        same(
            firmware,
            index.symbols(None, Some(firmware), None).unwrap(),
            store.symbols(None, Some(firmware), None).unwrap(),
        );
    }
    same(
        "strings",
        index.string_frequencies(&strings).unwrap(),
        store.string_frequencies(&strings).unwrap(),
    );
    for family in 0..index.families.len() + 1 {
        same(
            "family",
            index.family(family).unwrap(),
            store.family(family).unwrap(),
        );
        same(
            "members",
            index.members(family, None).unwrap(),
            store.members(family, None).unwrap(),
        );
    }
    for (id, f) in index.functions.iter().enumerate() {
        let k = &f.key;
        same(
            "find",
            index
                .find(None, Some(k.address), Some(&k.firmware), Some(&k.unit))
                .unwrap(),
            store
                .find(None, Some(k.address), Some(&k.firmware), Some(&k.unit))
                .unwrap(),
        );
        same(
            "names",
            index.inherited_names(id, None, false).unwrap(),
            store.inherited_names(id, None, false).unwrap(),
        );
        same(
            "normalized names",
            index.inherited_names(id, None, true).unwrap(),
            store.inherited_names(id, None, true).unwrap(),
        );
        same("calls", index.calls(id).unwrap(), store.calls(id).unwrap());
        same(
            "features",
            index.function(id).unwrap(),
            store.function(id).unwrap(),
        );
        same(
            "loaded features",
            index.function(id).unwrap(),
            loaded.function(id).unwrap(),
        );
        same(
            "tokens",
            index.normalized_tokens(id).unwrap(),
            store.normalized_tokens(id).unwrap(),
        );
        same(
            "body",
            index.body_sha256(id).unwrap(),
            store.body_sha256(id).unwrap(),
        );
        for r in &f.references {
            same(
                "references",
                index.references_to(&k.firmware, None, r.target).unwrap(),
                store.references_to(&k.firmware, None, r.target).unwrap(),
            );
            same(
                "read",
                index.read(&k.firmware, &k.unit, r.target, 8).unwrap(),
                store.read(&k.firmware, &k.unit, r.target, 8).unwrap(),
            );
        }
    }
    let pattern = BytePattern::new(vec![0; 4], Some(vec![0, 0, 0, 0xff])).unwrap();
    for firmware in &firmwares {
        same(
            "search",
            index.search(firmware, None, &pattern).unwrap(),
            store.search(firmware, None, &pattern).unwrap(),
        );
        let only = BTreeSet::from([firmware.clone()]);
        same(
            "subset",
            index.subset(&only).stats().unwrap(),
            store.subset(&only).unwrap().stats().unwrap(),
        );
    }
    same("pairs", index.pairs("a", "b"), loaded.pairs("a", "b"));
    drop(store);
    std::fs::remove_file(path).unwrap();
}
const CALLER: &[&str] = &["push R", "mov R", "bl T", "bl T", "pop pc"];
const LEAF: &[&str] = &["push R", "mov R #1", "str R", "pop pc"];
const CHANGED: &[&str] = &["push R", "mov R #2", "str R", "pop pc"];

#[test]
fn relocation_candidates_keep_all_banks_weak_and_do_not_merge_or_name_them() {
    let mut i = Index::new();
    function(
        &mut i,
        "a",
        "main",
        100,
        &[
            "movwne R, #0x1234",
            "movtne R, #0x12",
            "ldr R, [R]",
            "bx lr",
        ],
        &[],
        &["role"],
    );
    for unit in ["bank-one", "bank-two"] {
        function(
            &mut i,
            "b",
            unit,
            1000,
            &[
                "movwne R, #0x5678",
                "movtne R, #0x34",
                "ldr R, [R]",
                "bx lr",
            ],
            &[],
            &[],
        );
    }
    i.build();
    let source = i.find(Some("role"), None, Some("a"), None).unwrap()[0].id;
    let families = i.families.len();
    assert!(
        i.find(Some("role"), None, Some("b"), None)
            .unwrap()
            .is_empty()
    );
    let matches = i.candidates(source, "b");
    assert_eq!(matches.len(), 2);
    assert!(
        matches
            .iter()
            .all(|m| m.grade == Grade::Weak && m.origins == ["relocation-shape"])
    );
    assert_ne!(matches[0].function.key.unit, matches[1].function.key.unit);
    assert_eq!(i.families.len(), families);
    assert!(
        i.find(Some("role"), None, Some("b"), None)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn mov_low_encoding_variant_only_nominates_with_a_following_matching_movt() {
    let mut i = Index::new();
    function(
        &mut i,
        "a",
        "main",
        100,
        &["movw R, K", "movt R, K", "strb R, [R]", "bx lr"],
        &[],
        &["role"],
    );
    function(
        &mut i,
        "b",
        "main",
        200,
        &["mov R, #0x20c", "movt R, K", "strb R, [R]", "bx lr"],
        &[],
        &[],
    );
    function(
        &mut i,
        "b",
        "main",
        300,
        &["mov R, #0x20c", "movtne R, K", "strb R, [R]", "bx lr"],
        &[],
        &[],
    );
    i.build();
    let source = i.find(Some("role"), None, Some("a"), None).unwrap()[0].id;
    let matches = i.candidates(source, "b");
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].function.key.address, 200);
    assert_eq!(matches[0].grade, Grade::Weak);
    assert!(i.candidates(usize::MAX, "b").is_empty());
}

#[test]
fn propagates_across_three_firmwares_and_keeps_grades() {
    let mut i = Index::new();
    function(
        &mut i,
        "a",
        "main",
        100,
        CALLER,
        &[(2, 200), (3, 300)],
        &["caller"],
    );
    function(&mut i, "a", "main", 200, LEAF, &[], &["leaf"]);
    function(&mut i, "a", "main", 300, LEAF, &[], &["other"]);
    function(
        &mut i,
        "b",
        "main",
        1000,
        CALLER,
        &[(2, 2000), (3, 3000)],
        &[],
    );
    function(&mut i, "b", "main", 2000, LEAF, &[], &[]);
    function(&mut i, "b", "main", 3000, CHANGED, &[], &["bridge"]);
    // The changed leaf is uniquely shared by B and C, and receives A's name through B.
    function(&mut i, "c", "main", 30000, CHANGED, &[], &[]);
    for f in &mut i.functions {
        if f.key.address == 3000 || f.key.address == 30000 {
            f.strings.insert("synthetic bridge role".into());
        }
    }
    i.build();
    let pairs = i.pairs("a", "b");
    assert_eq!(pairs.len(), 3);
    assert_eq!(pairs[0].grade, Grade::Identical);
    assert_eq!(pairs[1].grade, Grade::Identical);
    assert_eq!(pairs[2].grade, Grade::CallSite);
    let c = i.find(None, Some(30000), Some("c"), None).unwrap()[0].id;
    assert!(
        i.inherited_names(c, None, false)
            .unwrap()
            .contains_key("bridge")
    );
    assert_eq!(i.members(i.membership[c], None).unwrap().len(), 3);
    let baseline = i.subset(&BTreeSet::from(["a".into(), "c".into()]));
    assert_eq!(baseline.functions.len(), 4);
    assert!(baseline.pairs("a", "c").is_empty());
}
#[test]
fn overlapping_banks_and_unresolved_calls_stay_distinct() {
    let mut i = Index::new();
    function(&mut i, "a", "main", 100, CALLER, &[(2, 200), (3, 999)], &[]);
    function(&mut i, "a", "one", 200, LEAF, &[], &[]);
    function(&mut i, "a", "two", 200, CHANGED, &[], &[]);
    function(
        &mut i,
        "b",
        "main",
        1000,
        CALLER,
        &[(2, 2000), (3, 999)],
        &[],
    );
    function(&mut i, "b", "main", 2000, LEAF, &[], &[]);
    i.build();
    let caller = i.find(None, Some(100), Some("a"), None).unwrap()[0].id;
    assert!(i.calls(caller).unwrap().iter().all(|(_, v)| v.is_none()));
    assert_eq!(i.pairs("a", "b")[0].grade, Grade::SameCode);
    let one = i.find(None, Some(200), Some("a"), Some("one")).unwrap()[0].id;
    assert_eq!(i.callee(one, 200), Some(one));
}
#[test]
fn tiny_code_is_not_a_seed_and_weak_names_do_not_propagate_calls() {
    let mut i = Index::new();
    function(&mut i, "a", "main", 1, &["bx lr"], &[], &[]);
    function(&mut i, "b", "main", 2, &["bx lr"], &[], &[]);
    i.build();
    assert!(i.pairs("a", "b").is_empty());
    let mut i = Index::new();
    function(&mut i, "a", "main", 100, CALLER, &[(2, 200)], &[]);
    function(&mut i, "b", "main", 1000, CALLER, &[(2, 2000)], &[]);
    function(&mut i, "a", "main", 200, LEAF, &[(1, 300)], &["weak_name"]);
    let big: Vec<_> = (0..40).map(|_| "different").collect();
    function(&mut i, "b", "main", 2000, &big, &[(1, 3000)], &[]);
    function(&mut i, "a", "main", 300, &["one", "two"], &[], &[]);
    function(&mut i, "b", "main", 3000, &["one", "three"], &[], &[]);
    i.build();
    assert_eq!(i.pairs("a", "b").len(), 1);
    assert!(
        i.find(Some("weak_name"), None, Some("b"), None)
            .unwrap()
            .is_empty()
    );
    // A second call site in the same caller is not independent support.
    i.functions[0].calls.push((3, 200));
    i.functions[3].calls.push((3, 2000));
    i.build();
    assert_eq!(i.pairs("a", "b").len(), 1);
    // A separate caller family can nominate the same dissimilar callees.
    let second = &["push R", "xor R #7", "bl T", "pop pc"];
    function(&mut i, "a", "main", 150, second, &[(2, 200)], &[]);
    function(&mut i, "b", "main", 1500, second, &[(2, 2000)], &[]);
    i.build();
    assert_eq!(i.pairs("a", "b").len(), 3);
    let found = i.find(Some("weak_name"), None, Some("b"), None).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].key.address, 2000);
    assert_eq!(found[0].name_donors[0].grade, Grade::Weak);
    assert!(
        i.inherited_names(found[0].id, None, true)
            .unwrap()
            .is_empty()
    );
    assert_eq!(i.sample_links(50, 42)["weak"].links.len(), 1);
    assert_eq!(i.pairs("a", "b").last().unwrap().grade, Grade::Weak);
    let complete = i.pairs("a", "b");
    for (id, source) in i
        .functions
        .iter()
        .enumerate()
        .filter(|(_, function)| function.key.firmware == "a")
    {
        let expected: Vec<_> = complete
            .iter()
            .filter(|pair| pair.reference == source.key)
            .cloned()
            .collect();
        assert_eq!(
            serde_json::to_value(i.pairs_for(id, "b")).unwrap(),
            serde_json::to_value(expected).unwrap(),
            "scoped pairs for {:?}",
            source.key
        );
    }
    assert_store_matches(&i, "weak");
}
fn input(fw: &str, arch: Architecture) -> UnitInput {
    UnitInput {
        layout: UnitLayout {
            firmware: fw.into(),
            unit: "main".into(),
            architecture: arch,
            address_ranges: vec![(0x40000, 0x10000000)],
            address_aliases: vec![],
        },
        functions: vec![Boundary {
            address: 0x1000,
            ranges: vec![(0x1000, 0x1014)],
            names: vec!["synthetic".into()],
            annotations: vec![],
            references: vec![],
        }],
    }
}
#[test]
fn decoding_normalization_pools_and_locator_queries() {
    // Synthetic A32: mov r0,#1; ldr r1,[pc,#4]; bx lr; nop; literal.
    let words = [
        0xe3a00001u32,
        0xe59f1004,
        0xe12fff1e,
        0xe320f000,
        0x00800000,
    ];
    let bytes: Vec<_> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    let mut i = Index::new();
    i.add_unit(
        input("a", Architecture::Arm32),
        vec![Region {
            address: 0x1000,
            bytes: bytes.clone(),
        }],
    )
    .unwrap();
    i.build();
    assert_eq!(i.tokens[i.functions[0].tokens[0] as usize], "mov R, #1");
    assert_eq!(
        i.tokens[i.functions[0].tokens[1] as usize],
        "ldr R, [pc, =A]"
    );
    assert_eq!(i.tokens[i.functions[0].tokens[4] as usize], "|");
    assert!(
        i.data_references(0)
            .unwrap()
            .iter()
            .any(|r| r.target == 0x800000)
    );
    assert_eq!(
        i.references_to("a", Some("main"), 0x800000).unwrap().len(),
        1
    );
    let pattern = BytePattern::new(bytes[0..4].to_vec(), None).unwrap();
    assert_eq!(i.search("a", None, &pattern).unwrap()[0].address, 0x1000);
    assert!(BytePattern::new(vec![], None).is_err());
    assert!(BytePattern::new(vec![0; 4], Some(vec![0])).is_err());
    let anything = BytePattern::new(vec![0; 4], Some(vec![0; 4])).unwrap();
    assert_eq!(i.search("a", None, &anything).unwrap().len(), 17);
    assert!(i.add_unit(input("a", Architecture::Arm32), vec![]).is_err());
    // Synthetic AArch64: mov w0,#1; add w0,w0,#2; nop; nop; ret.
    let words = [
        0x52800020u32,
        0x11000800,
        0xd503201f,
        0xd503201f,
        0xd65f03c0,
    ];
    let mut j = Index::new();
    j.add_unit(
        input("b", Architecture::Aarch64),
        vec![Region {
            address: 0x1000,
            bytes: words.iter().flat_map(|w| w.to_le_bytes()).collect(),
        }],
    )
    .unwrap();
    j.build();
    assert_eq!(j.functions[0].instructions, 5);
    i.merge(j).unwrap();
    assert_eq!(i.functions.len(), 2);
}
#[test]
fn persisted_index_retains_annotations_and_rejects_bad_version() {
    let mut i = Index::new();
    function(&mut i, "a", "main", 100, LEAF, &[], &["leaf"]);
    i.build();
    i.annotate(
        0,
        Annotation {
            kind: "note".into(),
            text: "synthetic".into(),
            source: "test".into(),
        },
    )
    .unwrap();
    let path = {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/corpus-tests");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(format!("test-{}.sqlite", std::process::id()))
    };
    i.save(&path).unwrap();
    let store = Store::open(&path).unwrap();
    assert_eq!(
        store.family(0).unwrap().unwrap().annotations[0].text,
        "synthetic"
    );
    assert_eq!(store.find(Some("leaf"), None, None, None).unwrap().len(), 1);
    let j = store.load().unwrap();
    assert_eq!(j.families[0].annotations[0].text, "synthetic");
    assert_eq!(j.find(Some("leaf"), None, None, None).unwrap().len(), 1);
    drop(store);
    std::fs::write(&path, b"INVALID!").unwrap();
    assert!(Store::open(&path).is_err());
    std::fs::remove_file(path).unwrap();
}

#[test]
fn feature_seeds_bridge_architectures_but_do_not_claim_equal_code() {
    let mut i = Index::new();
    function(&mut i, "a", "main", 100, LEAF, &[], &["from_a"]);
    function(&mut i, "b", "main", 200, CHANGED, &[], &[]);
    i.functions[1].architecture = Architecture::Aarch64;
    for f in &mut i.functions {
        f.strings = BTreeSet::from([
            "synthetic unique alpha".into(),
            "synthetic unique beta".into(),
        ]);
        f.constants = BTreeSet::from([123456, 456789]);
    }
    i.build();
    assert_eq!(i.pairs("a", "b")[0].grade, Grade::CallSite);
    assert_eq!(
        i.find(Some("from_a"), None, Some("b"), None).unwrap().len(),
        1
    );
    let b = i.find(None, Some(200), Some("b"), None).unwrap()[0].id;
    assert!(i.inherited_names(b, None, true).unwrap().is_empty());
    assert_eq!(
        i.inherited_names(b, None, false).unwrap()["from_a"][0].grade,
        Grade::CallSite
    );
    assert!(!i.name_support(0, b).unwrap().corroborated);
}

#[test]
fn cross_isa_seeds_require_string_coverage_and_never_align_instruction_positions() {
    let mut i = Index::new();
    function(&mut i, "a", "main", 100, CALLER, &[(2, 200)], &[]);
    function(&mut i, "a", "main", 200, LEAF, &[], &["unrelated_leaf"]);
    function(&mut i, "b", "main", 1000, CALLER, &[(2, 2000)], &[]);
    function(&mut i, "b", "main", 2000, CHANGED, &[], &[]);
    for f in &mut i.functions {
        if f.key.firmware == "b" {
            f.architecture = Architecture::Aarch64;
        }
        if f.key.address == 100 || f.key.address == 1000 {
            f.strings = BTreeSet::from(["shared alpha".into(), "shared beta".into()]);
            f.constants = BTreeSet::from([123456, 456789]);
        }
    }
    i.build();
    assert_eq!(i.pairs("a", "b").len(), 1);
    assert!(
        i.find(Some("unrelated_leaf"), None, Some("b"), None)
            .unwrap()
            .is_empty()
    );
    // Two coincident labels inside a large resolver cannot seed a small consumer.
    i.functions[0]
        .strings
        .extend((0..20).map(|n| format!("unrelated label {n}")));
    i.build();
    assert!(i.pairs("a", "b").is_empty());
}

#[test]
fn merge_is_atomic_on_duplicate_unit_and_annotations_survive_rebuild() {
    let mut i = Index::new();
    let body: Vec<u8> = [
        0xe3a00001u32,
        0xe2800002,
        0xe2800003,
        0xe2800004,
        0xe12fff1e,
    ]
    .iter()
    .flat_map(|w| w.to_le_bytes())
    .collect();
    i.add_unit(
        input("a", Architecture::Arm32),
        vec![Region {
            address: 0x1000,
            bytes: body,
        }],
    )
    .unwrap();
    i.build();
    i.annotate(
        0,
        Annotation {
            kind: "type".into(),
            text: "synthetic signature".into(),
            source: "test".into(),
        },
    )
    .unwrap();
    assert!(i.merge(i.clone()).is_err());
    assert_eq!(i.functions.len(), 1);
    i.build();
    assert_eq!(i.families[0].annotations[0].text, "synthetic signature");
}

#[test]
fn split_address_references_and_aliases_recover_strings() {
    let words = [
        0xe3010080u32,
        0xe3440000,
        0xe12fff1e,
        0xe320f000,
        0xe320f000,
    ];
    let mut bytes = vec![0; 0x100];
    for (i, w) in words.iter().enumerate() {
        bytes[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes());
    }
    bytes[0x80..0x95].copy_from_slice(b"synthetic task label\0");
    let mut meta = input("a", Architecture::Arm32);
    meta.layout.address_aliases = vec![(0x40001000, 0x40001100, 0x1000)];
    meta.layout.address_ranges.push((0x40001000, 0x40001100));
    let mut i = Index::new();
    i.add_unit(
        meta,
        vec![Region {
            address: 0x1000,
            bytes: bytes.clone(),
        }],
    )
    .unwrap();
    i.build();
    assert!(i.functions[0].strings.contains("synthetic task label"));
    assert!(
        i.data_references(0)
            .unwrap()
            .iter()
            .any(|r| r.kind == "movw-movt" && r.target == 0x1080)
    );
    let unit = &i.units[0];
    assert!(unit.read(0x400010ff, 2).is_none());
    assert_eq!(unit.read(0x40001080, 20), unit.read(0x1080, 20));
    let words = [
        0x90000000u32,
        0x91020000,
        0xd65f03c0,
        0xd503201f,
        0xd503201f,
    ];
    for (n, w) in words.iter().enumerate() {
        bytes[n * 4..n * 4 + 4].copy_from_slice(&w.to_le_bytes());
    }
    let mut j = Index::new();
    j.add_unit(
        input("b", Architecture::Aarch64),
        vec![Region {
            address: 0x1000,
            bytes,
        }],
    )
    .unwrap();
    j.build();
    assert!(j.functions[0].strings.contains("synthetic task label"));
    i.add_symbol(DataSymbol {
        firmware: "a".into(),
        unit: "main".into(),
        address: 0x1080,
        length: 4,
        name: "label".into(),
        kind: "string".into(),
        source: "test".into(),
    })
    .unwrap();
    i.merge(j.clone()).unwrap();
    assert_store_matches(&i, "strings");
    assert!(
        j.data_references(0)
            .unwrap()
            .iter()
            .any(|r| r.kind == "adrp-add" && r.target == 0x1080)
    );
}

#[test]
fn uncorroborated_names_propagate_with_their_grade() {
    let mut i = Index::new();
    function(&mut i, "a", "main", 100, LEAF, &[], &["source_name"]);
    function(&mut i, "b", "main", 200, LEAF, &[], &[]);
    i.build();
    assert_eq!(i.pairs("a", "b")[0].grade, Grade::Identical);
    assert!(!i.name_support(0, 1).unwrap().corroborated);
    assert_eq!(
        i.inherited_names(1, None, false).unwrap()["source_name"][0].grade,
        Grade::Identical
    );
    assert_eq!(
        i.find(Some("source_name"), None, Some("b"), None)
            .unwrap()
            .len(),
        1
    );
    i.add_name(1, "local_name".into(), "synthetic".into())
        .unwrap();
    assert_eq!(
        i.find(Some("local_name"), None, Some("b"), None)
            .unwrap()
            .len(),
        1
    );
    let first = serde_json::to_value(i.sample_links(50, 42)).unwrap();
    i.functions.reverse();
    i.build();
    assert_eq!(first, serde_json::to_value(i.sample_links(50, 42)).unwrap());
}
#[test]
fn cross_isa_corroboration_uses_a_separate_constant_table() {
    let mut i = Index::new();
    for (fw, arch, words) in [
        (
            "a",
            Architecture::Arm32,
            vec![
                0xe3011234u32,
                0xe3401000,
                0xe3022345,
                0xe3402000,
                0xe12fff1e,
            ],
        ),
        (
            "b",
            Architecture::Aarch64,
            vec![
                0xd2824681u32,
                0xd28468a2,
                0xd503201f,
                0xd503201f,
                0xd65f03c0,
            ],
        ),
    ] {
        let mut meta = input(fw, arch);
        meta.functions[0].names = if fw == "a" {
            vec!["table_algorithm".into()]
        } else {
            vec![]
        };
        meta.functions[0].references = vec![
            Reference {
                site: 0x1000,
                target: 0x2000,
                kind: "data".into(),
                text: None,
            },
            Reference {
                site: 0x1000,
                target: 0x3000,
                kind: "string".into(),
                text: Some("synthetic alpha role".into()),
            },
            Reference {
                site: 0x1004,
                target: 0x3100,
                kind: "string".into(),
                text: Some("synthetic beta role".into()),
            },
        ];
        let table: Vec<u8> = (0..16u32)
            .flat_map(|n| {
                0xdeadbeefu32
                    .wrapping_add(n.wrapping_mul(0x1234567))
                    .to_le_bytes()
            })
            .collect();
        i.add_unit(
            meta,
            vec![
                Region {
                    address: 0x1000,
                    bytes: words.into_iter().flat_map(u32::to_le_bytes).collect(),
                },
                Region {
                    address: 0x2000,
                    bytes: table,
                },
            ],
        )
        .unwrap();
    }
    i.build();
    assert_eq!(i.pairs("a", "b").len(), 1);
    let support = i.name_support(0, 1).unwrap();
    assert!(support.corroborated);
    assert_eq!(support.constant_tables.len(), 1);
    assert!(!support.exact_body_bytes);
    assert!(
        i.inherited_names(1, None, false)
            .unwrap()
            .contains_key("table_algorithm")
    );
}
