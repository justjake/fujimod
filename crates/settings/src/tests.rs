use super::*;
use fujimod_catalog_settings::{
    NamedSetting, ProfileRecord, SETTINGS_SCHEMA_URI, SectionLength, SettingsBank, SettingsField,
    SettingsProfile, SettingsSection, SettingsSource,
};

/// An update holding one literal-only method-4 stream, stored inverted.
fn update(section: &[u8], flash_offset: usize, native: usize) -> Vec<u8> {
    let mut body = Vec::new();
    for chunk in section.chunks(127) {
        body.push(chunk.len() as u8);
        body.extend(chunk);
    }
    let packed = body.len() as u32 + 20;
    let mut stream: Vec<u8> = [
        section.len() as u32,
        packed,
        packed.div_ceil(0x4000),
        0x4000,
        4,
    ]
    .into_iter()
    .flat_map(u32::to_le_bytes)
    .collect();
    stream.extend(body);
    let mut raw = vec![0; flash_offset + native];
    raw.extend(stream);
    fujimod_firmware::invert(&raw)
}
fn range(address: u64, bytes: &[u8]) -> UpdateRange {
    UpdateRange {
        address,
        size: bytes.len() as u64,
        sha256: fujimod_firmware::sha256(bytes),
    }
}
fn fixture() -> (Vec<u8>, FirmwareSettings) {
    let descriptor: Vec<u8> = [2u32, 40, 931]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect();
    let table = [0x10, 0, 9, 0, 0xf0, 0, 7, 0, 0xff, 0xff, 0, 0];
    let mut section = descriptor.clone();
    section.extend([0; 4]);
    section.extend(table);
    let layout = FirmwareSettings {
        schema_uri: SETTINGS_SCHEMA_URI.into(),
        schema: 1,
        id: "sample-1-settings".into(),
        firmware_id: "sample-1".into(),
        source: SettingsSource {
            flash_offset: 0x10,
            address: 0x1000,
            size: section.len() as u64,
            sha256: fujimod_firmware::sha256(&section),
        },
        bank: SettingsBank {
            descriptor: range(0x1000, &descriptor),
            kind: 2,
            blocks: 40,
            first_block: 931,
            logical_first: 41,
            bytes: 0x400,
        },
        defaults: Some(range(0x1010, &table)),
        fields: vec![SettingsField {
            offset: 0x202,
            bytes: vec![0x55, 0xaa],
            basis: "Settings signature.".into(),
        }],
        profile: None,
        sections: vec![
            SettingsSection {
                name: "configuration".into(),
                offset: 0,
                length: SectionLength::Fixed { bytes: 0x300 },
                basis: "Sample boot read.".into(),
            },
            SettingsSection {
                name: "table".into(),
                offset: 0x380,
                length: SectionLength::Counted {
                    counts: 0x280,
                    bits: 34,
                },
                basis: "Sample counted read.".into(),
            },
            SettingsSection {
                name: "list".into(),
                offset: 0x3a0,
                length: SectionLength::CountedList {
                    count: 0x290,
                    bits: 34,
                },
                basis: "Sample single-list read.".into(),
            },
        ],
        settings: BTreeMap::from([(
            "debug".into(),
            NamedSetting {
                store: "bank".into(),
                offset: 0xf7,
                values: BTreeMap::from([("off".into(), vec![0]), ("on".into(), vec![1])]),
            },
        )]),
    };
    (update(&section, 4, 0x10), layout)
}

#[test]
fn banks_take_stock_defaults_fields_and_requested_settings() {
    let (source, layout) = fixture();
    let flash = flash_image(&source, 4).unwrap();
    let requested = BTreeMap::from([("debug".into(), "on".into())]);
    let settings = build(&layout, &flash, &requested, None).unwrap();
    assert_eq!(settings.profile, None);
    let bank = settings.bank;
    assert_eq!(
        (bank.first_block, bank.blocks, bank.logical_first),
        (931, 40, 41)
    );
    assert_eq!(bank.bytes.len(), 0x400);
    assert_eq!((bank.bytes[0x10], bank.bytes[0xf0]), (9, 7));
    assert_eq!(&bank.bytes[0x202..0x204], [0x55, 0xaa]);
    assert_eq!(value(&layout, &bank.bytes, "debug").as_deref(), Some("on"));
}

#[test]
fn unknown_settings_and_changed_updates_refuse() {
    let (source, layout) = fixture();
    let flash = flash_image(&source, 4).unwrap();
    let unknown = BTreeMap::from([("language".into(), "en".into())]);
    assert!(
        build(&layout, &flash, &unknown, None)
            .unwrap_err()
            .contains("language")
    );
    let value = BTreeMap::from([("debug".into(), "maybe".into())]);
    assert!(
        build(&layout, &flash, &value, None)
            .unwrap_err()
            .contains("maybe")
    );
    let mut changed = flash.clone();
    *changed.last_mut().unwrap() ^= 1;
    assert!(build(&layout, &changed, &BTreeMap::new(), None).is_err());
}

/// The fixture with a profile: a header and a record of eight default bytes
/// after the bank defaults, with `language` in the record and `setup` in the header.
fn profile_fixture() -> (Vec<u8>, FirmwareSettings) {
    let (_, mut layout) = fixture();
    let descriptor: Vec<u8> = [2u32, 40, 931]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect();
    let table = [0x10, 0, 9, 0, 0xf0, 0, 7, 0, 0xff, 0xff, 0, 0];
    let header = [0, 0, 0, 0, 0xd0, 7, 0, 0xff, 0, 0x80, 0, 4, 0, 0, 0xaa, 0];
    let record = [1, 2, 3, 4, 5, 6, 7, 8];
    let mut section = descriptor;
    section.extend([0; 4]);
    section.extend(table);
    section.extend(header);
    section.extend(record);
    layout.source.size = section.len() as u64;
    layout.source.sha256 = fujimod_firmware::sha256(&section);
    layout.profile = Some(SettingsProfile {
        derived_fields: vec![],
        header: range(0x101c, &header),
        records: BTreeMap::from([(
            "main".into(),
            ProfileRecord {
                block: 976,
                managed_index: 86,
                bytes: 0x100,
                defaults: range(0x102c, &record),
                defaults_offset: 0x10,
                parity_offset: 0xe0,
                fields: vec![SettingsField {
                    offset: 2,
                    bytes: vec![0xff, 0xff],
                    basis: "Record marker.".into(),
                }],
            },
        )]),
    });
    for (name, store, offset, values) in [
        ("language", "main", 0x12, [("en", 1u8), ("ja", 0)]),
        ("setup", "header", 11, [("done", 7), ("pending", 4)]),
    ] {
        layout.settings.insert(
            name.into(),
            NamedSetting {
                store: store.into(),
                offset,
                values: values
                    .into_iter()
                    .map(|(n, v)| (n.into(), vec![v]))
                    .collect(),
            },
        );
    }
    (update(&section, 4, 0x10), layout)
}

#[test]
fn profiles_take_stock_defaults_settings_and_sealed_records() {
    let (source, layout) = profile_fixture();
    let flash = flash_image(&source, 4).unwrap();
    let requested = BTreeMap::from([
        ("language".into(), "en".into()),
        ("setup".into(), "done".into()),
    ]);
    let profile = build(&layout, &flash, &requested, None)
        .unwrap()
        .profile
        .unwrap();
    assert_eq!(profile.header[11], 7);
    assert_eq!(
        profile.header.iter().fold(0u8, |s, b| s.wrapping_add(*b)),
        0
    );
    let record = &profile.records[0];
    assert_eq!((record.block, record.managed_index), (976, 86));
    assert_eq!(&record.bytes[..4], [0, 0, 0xff, 0xff]);
    assert_eq!(&record.bytes[0x10..0x18], [1, 2, 1, 4, 5, 6, 7, 8]);
    let mut sealed = record.bytes.clone();
    seal_journal(&mut sealed, 0xe0).unwrap();
    assert_eq!(sealed, record.bytes);
    let checksum = (0..0xe0 / 4).fold(0u32, |sum, i| sum ^ word(&record.bytes, i * 4));
    assert_eq!(record.bytes[0xfc..], checksum.to_le_bytes());
    // Parity bit for the word holding 0xff 0xff is clear (sixteen bits set).
    assert_eq!(record.bytes[0xe0] & 1, 0);
    let mut moved = layout.clone();
    moved.settings.get_mut("setup").unwrap().offset = 14;
    assert!(build(&moved, &flash, &requested, None).is_err());
}

#[test]
fn camera_sections_replace_the_stock_bank_and_take_requested_settings() {
    let (source, layout) = fixture();
    let flash = flash_image(&source, 4).unwrap();
    let mut configuration = vec![0x11; 0x300];
    configuration[0x202..0x204].copy_from_slice(&[0x55, 0xaa]);
    // Two 34-bit entries in the first list (two 64-bit words, 16 bytes), one in the second (5 bytes).
    configuration[0x280..0x286].copy_from_slice(&[2, 0, 0, 1, 0, 0]);
    // Three 34-bit entries in one list: 102 bits, 13 bytes.
    configuration[0x290..0x293].copy_from_slice(&[3, 0, 0]);
    let camera = BTreeMap::from([
        ("configuration".to_owned(), configuration.clone()),
        ("table".to_owned(), vec![0x22; 21]),
        ("list".to_owned(), vec![0x33; 13]),
    ]);
    let requested = BTreeMap::from([("debug".into(), "on".into())]);
    let bank = build(&layout, &flash, &requested, Some(&camera))
        .unwrap()
        .bank
        .bytes;
    assert_eq!(&bank[..0xf7], &configuration[..0xf7]);
    assert_eq!(bank[0xf7], 1, "requested bank settings still apply");
    assert_eq!(bank[0x10], 0x11, "the stock defaults table does not");
    assert_eq!(&bank[0x380..0x395], &[0x22; 21]);
    assert_eq!(&bank[0x3a0..0x3ad], &[0x33; 13]);
    assert!(
        bank[0x300..0x380]
            .iter()
            .chain(&bank[0x395..0x3a0])
            .chain(&bank[0x3ad..])
            .all(|&b| b == 0xff)
    );

    let short = BTreeMap::from([
        ("configuration".to_owned(), configuration.clone()),
        ("table".to_owned(), vec![0x22; 20]),
        ("list".to_owned(), vec![0x33; 13]),
    ]);
    assert!(
        build(&layout, &flash, &requested, Some(&short))
            .unwrap_err()
            .contains("21")
    );
    let short_list = BTreeMap::from([
        ("configuration".to_owned(), configuration.clone()),
        ("table".to_owned(), vec![0x22; 21]),
        ("list".to_owned(), vec![0x33; 12]),
    ]);
    assert!(
        build(&layout, &flash, &requested, Some(&short_list))
            .unwrap_err()
            .contains("13")
    );
    let unknown = BTreeMap::from([("extra".to_owned(), vec![0])]);
    assert!(
        build(&layout, &flash, &requested, Some(&unknown))
            .unwrap_err()
            .contains("extra")
    );
}

#[test]
fn profile_fields_use_the_requested_bank_and_preserve_masked_bits() {
    use fujimod_catalog_settings::{BankByteValue, ProfileField, ProfileFieldSource};
    let (source, mut layout) = profile_fixture();
    let flash = flash_image(&source, 4).unwrap();
    let field = |offset, bytes, source| ProfileField {
        store: "main".into(),
        offset,
        bytes,
        source,
        basis: "Synthetic initialization law.".into(),
    };
    layout.profile.as_mut().unwrap().derived_fields = vec![
        field(0x30, 16, ProfileFieldSource::Fill { value: 255 }),
        field(0x30, 4, ProfileFieldSource::BankCopy { offset: 0x200 }),
        field(
            0x35,
            1,
            ProfileFieldSource::BankMap {
                offset: 0xf7,
                mask: 0x60,
                values: vec![BankByteValue {
                    input: 1,
                    output: 0x40,
                }],
                otherwise: 0x20,
                otherwise_high: Some(0),
            },
        ),
        field(
            0x36,
            1,
            ProfileFieldSource::BankBits {
                offset: 0x10,
                mask: 15,
            },
        ),
    ];
    let language = layout.settings.get_mut("language").unwrap();
    language.values.insert("en".into(), vec![0xc1]);
    layout
        .settings
        .get_mut("debug")
        .unwrap()
        .values
        .insert("negative".into(), vec![0x80]);
    for (debug, mapped) in [("on", 0xdf), ("off", 0xbf), ("negative", 0x9f)] {
        let requested = BTreeMap::from([
            ("language".into(), "en".into()),
            ("debug".into(), debug.into()),
        ]);
        let result = build(&layout, &flash, &requested, None).unwrap();
        let record = &result.profile.unwrap().records[0].bytes;
        assert_eq!(&record[0x30..0x34], &result.bank.bytes[0x200..0x204]);
        assert_eq!((record[0x35], record[0x36]), (mapped, 0xf9));
        assert_eq!(record[0x12], 0xc1);
        let mut sealed = record.clone();
        seal_journal(&mut sealed, 0xe0).unwrap();
        assert_eq!(&sealed, record);
    }
    let mut invalid = layout.clone();
    invalid.profile.as_mut().unwrap().derived_fields[1].bytes = 0x400;
    assert!(invalid.validate().is_err());
    let mut invalid = layout;
    if let ProfileFieldSource::BankMap { values, .. } =
        &mut invalid.profile.as_mut().unwrap().derived_fields[2].source
    {
        values.push(values[0].clone());
    }
    assert!(invalid.validate().is_err());
}
