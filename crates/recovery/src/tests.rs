use super::*;
use image::{Bss, Compression, Edit, HeaderWord, Packaging, Part, Region, Source};
fn source(bytes: &[u8]) -> Source {
    Source {
        dat_offset: 0,
        size: bytes.len(),
        compression: Compression::Raw,
        decoded_size: bytes.len(),
        sha256: fujimod_firmware::sha256(bytes),
        initialized_prefix: None,
    }
}
fn plan(bytes: &[u8]) -> Plan {
    Plan {
        size: 16,
        entry: 0,
        regions: vec![Region {
            name: "loader".into(),
            address: 0,
            source: source(bytes),
        }],
        bss: vec![],
        edits: vec![],
    }
}
#[test]
fn placement_and_word_edits_are_checked_before_packaging() {
    let bytes = 7u32.to_le_bytes();
    let mut p = plan(&bytes);
    p.edits.push(Edit {
        address: 0,
        old: 7,
        new: 8,
        what: "mode".into(),
    });
    p.bss.push(Bss {
        name: "zero".into(),
        address: 4,
        size: 12,
    });
    let ram = image::transform(&bytes, &p).unwrap();
    assert_eq!(&ram[..4], &8u32.to_le_bytes());
    assert_eq!(&ram[4..], &[0; 12]);
    let packaging = Packaging {
        header_parts: vec![Part {
            dat_offset: 0,
            size: 4,
        }],
        header_sha256: fujimod_firmware::sha256(&bytes),
        header_words: vec![HeaderWord {
            offset: 0,
            value: 7,
        }],
        edits: vec![],
        header_filename: "ffsys.bin".into(),
        image_filename: "ffboot.bin".into(),
        entry: 0,
        load_address: 0,
    };
    let files = packaging.package(&bytes, &p, ram).unwrap();
    assert_eq!(files[0].bytes, bytes);
    assert_eq!(files[1].bytes.len(), 16);
    p.edits[0].old = 9;
    assert!(image::transform(&bytes, &p).is_err());
}
#[test]
fn rejects_overlaps_invalid_bss_duplicate_edits_and_source_mutation() {
    let bytes = 7u32.to_le_bytes();
    let p = plan(&bytes);
    let mut bad = p.clone();
    bad.regions.push(bad.regions[0].clone());
    assert!(image::transform(&bytes, &bad).is_err());
    bad = p.clone();
    bad.bss.push(Bss {
        name: "overlap".into(),
        address: 2,
        size: 1,
    });
    assert!(image::transform(&bytes, &bad).is_err());
    bad = p.clone();
    bad.edits = vec![
        Edit {
            address: 0,
            old: 7,
            new: 7,
            what: "one".into()
        };
        2
    ];
    assert!(image::transform(&bytes, &bad).is_err());
    bad = p.clone();
    bad.edits.push(Edit {
        address: 4,
        old: 0,
        new: 1,
        what: "BSS".into(),
    });
    assert!(image::transform(&bytes, &bad).is_err());
    assert!(image::transform(&8u32.to_le_bytes(), &p).is_err());
    assert!(image::transform(&[], &p).is_err());
    bad = p.clone();
    bad.regions[0].address = usize::MAX;
    assert!(image::transform(&bytes, &bad).is_err());
    bad = p;
    bad.bss.push(Bss {
        name: "outside".into(),
        address: 16,
        size: 1,
    });
    assert!(image::transform(&bytes, &bad).is_err());
}
#[test]
fn header_old_words_power_policy_and_filenames_are_checked() {
    let bytes = 0u32.to_le_bytes();
    let mut packaging = Packaging {
        header_parts: vec![Part {
            dat_offset: 0,
            size: 4,
        }],
        header_sha256: fujimod_firmware::sha256(&bytes),
        header_words: vec![HeaderWord {
            offset: 0,
            value: 0,
        }],
        edits: vec![Edit {
            address: 0,
            old: 0,
            new: 1,
            what: "power hold".into(),
        }],
        header_filename: "ffsys.bin".into(),
        image_filename: "ffboot.bin".into(),
        entry: 0,
        load_address: 0,
    };
    assert_eq!(packaging.header(&bytes).unwrap(), 1u32.to_le_bytes());
    packaging.edits[0].old = 1;
    assert!(packaging.header(&bytes).is_err());
    for name in ["", "..", "../ffboot.bin", "A:\\DAT", "ffboot\0.bin"] {
        assert!(File::new(name, vec![]).is_err());
    }
}
#[test]
fn crc32_matches_the_transfer_polynomial() {
    assert_eq!(xp4::crc32(b"123456789"), 0xcbf43926);
    assert_eq!(xp4::crc32(b""), 0);
}
#[test]
fn recipes_match_catalog_identities_and_generation() {
    let catalog = fujimod_catalog::ReferenceCatalog::bundled().unwrap();
    for id in catalog.firmware_ids() {
        let selected = catalog.resolve_firmware(id).unwrap();
        if let Some(recipe) = Recipe::bundled(id).unwrap() {
            let identity = selected.firmware.update.as_ref().unwrap();
            assert_eq!(recipe.source_sha256, identity.sha256);
            assert_eq!(recipe.source_size as u64, identity.size);
            assert_eq!(
                selected.soc.id,
                match recipe.generation {
                    Generation::Exr => "exr",
                    Generation::ExrPro => "exrpro",
                    Generation::Exr2 => "exr2",
                    Generation::Xpp => "xpp",
                    Generation::Xp4 => "xp4",
                    Generation::Xp5 => "xp5",
                }
            );
        }
        let status = availability(id).unwrap();
        assert_eq!(status.can_build, status.missing.is_empty());
        assert_eq!(
            serde_json::to_value(status.sd_boot).unwrap(),
            selected
                .physical_controls
                .as_ref()
                .and_then(|c| c.pointer("/boot/sd"))
                .cloned()
                .unwrap_or(serde_json::Value::Null),
            "SD boot controls for {id}"
        );
    }
}

#[test]
fn cold_worker_requires_the_checked_condition_call_and_barrier_join() {
    let mut code = vec![0; 0x100];
    for (index, value) in [
        0xe3003030, 0xe3403000, 0xe5933000, 0xe3530001, 0x0a000012, 0xe3003070, 0xe3403000,
        0xe3a02000, 0xe583200c, 0xf57ff04f,
    ]
    .into_iter()
    .enumerate()
    {
        image::put(&mut code, index * 4, value).unwrap();
    }
    image::put(&mut code, 0x60, 0xeb000006).unwrap();
    image::put(&mut code, 0x64, 0xeaffffea).unwrap();
    let worker = xp4::worker(&code, 0x30).unwrap();
    assert_eq!(
        (worker.compare, worker.call, worker.entry, worker.join),
        (12, 0x60, 0x80, 20)
    );
    for address in [12, 0x64] {
        let mut changed = code.clone();
        changed[address] ^= 1;
        assert!(xp4::worker(&changed, 0x30).is_err());
    }
    assert!(xp4::worker(&code, 0x34).is_err());
}
