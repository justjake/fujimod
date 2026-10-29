use super::*;

#[test]
fn recompress_preserves_unmodified_tokens_and_prevents_dictionary_edit_propagation() {
    let original = stream(&[1, b'A', 0x80, 0x1f, 0x80, 0x1f], 31);
    let mut replacement = vec![b'A'; 31];
    assert_eq!(recompress(&original, &replacement).unwrap(), original);
    for index in [0, 1, 15, 16, 30] {
        replacement[index] = b'B';
        let packed = recompress(&original, &replacement).unwrap();
        assert_eq!(decompress(&packed).unwrap().0, replacement);
        replacement[index] = b'A';
    }
    assert!(recompress(&original, b"short").is_err());
    assert!(recompress(&original[..21], &replacement).is_err());
}
#[test]
fn sha256_matches_standard_vectors() {
    assert_eq!(
        sha256(b""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(
        sha256(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}
fn stream(body: &[u8], size: u32) -> Vec<u8> {
    let packed = body.len() as u32 + 20;
    [
        [size, packed, packed.div_ceil(0x4000), 0x4000, 4]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect::<Vec<_>>(),
        body.to_vec(),
    ]
    .concat()
}
fn put(data: &mut [u8], offset: usize, values: &[u32]) {
    for (i, v) in values.iter().enumerate() {
        data[offset + i * 4..offset + i * 4 + 4].copy_from_slice(&v.to_le_bytes());
    }
}
#[test]
fn tokens_and_scan() {
    let encoded = stream(&[1, b'A', 0x80, 0x1f], 16);
    let (data, c) = decompress(&encoded).unwrap();
    assert_eq!(data, b"AAAAAAAAAAAAAAAA");
    assert_eq!((c.literals, c.matches), (1, 1));
    let candidates = scan(&invert(&encoded));
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].decoded, data);
    assert_eq!(invert(&invert(&encoded)), encoded);
    let raw: Vec<u8> = (0..2047).map(|i| (i * 37 + 19) as u8).collect();
    let mut body = Vec::new();
    for c in raw.chunks(127) {
        body.push(c.len() as u8);
        body.extend(c);
    }
    body.extend([255, 255]);
    assert_eq!(
        decompress(&stream(&body, 2062)).unwrap().0,
        [raw.clone(), raw[..15].to_vec()].concat()
    );
}
#[test]
fn malformed_tokens() {
    for (body, size) in [
        (vec![0], 0),
        (vec![3, b'A'], 3),
        (vec![0x80], 1),
        (vec![0x80, 0x1f], 15),
        (vec![1, b'A', 0x80, 0x0f], 16),
        (vec![1, b'A', 0x80, 0x10], 1),
        (vec![1, b'A'], 0),
        (vec![1, b'A'], 2),
        (vec![], u32::MAX),
    ] {
        assert!(decompress(&stream(&body, size)).is_err());
    }
    let valid = stream(&[1, b'A'], 1);
    for n in 0..valid.len() {
        assert!(decompress(&valid[..n]).is_err());
    }
    for offset in [8, 12, 16] {
        let mut bad = valid.clone();
        put(&mut bad, offset, &[7]);
        assert!(decompress(&bad).is_err());
    }
}
#[test]
fn headers_raw_and_typed_image() {
    for bits in [32, 64] {
        let mut data = Vec::new();
        for v in [0u64, 0x1000, 64, 0x1020, 0, 0, 0, 0xaaaaffff] {
            data.extend_from_slice(&v.to_le_bytes()[..bits / 8]);
        }
        data.resize(64, 0);
        let spec = RawSpec {
            offset: 0,
            size: 64,
            base: 0x1000,
            sha256: &sha256(&data),
            header_bits: Some(bits as u8),
            entry: Some(0x1020),
            architecture_bits: bits as u8,
        };
        assert_eq!(raw(&data, &spec).unwrap(), data);
        assert!(raw(&data, &RawSpec { size: 65, ..spec }).is_err());
        assert!(
            raw(
                &data,
                &RawSpec {
                    architecture_bits: 0,
                    ..spec
                }
            )
            .is_err()
        );
        let image = decode(
            &invert(&data),
            &[Mapping {
                name: "code".into(),
                offset: 0,
                size: 64,
                base: 0x1000,
                executable: true,
                encoding: Encoding::Raw,
            }],
        )
        .unwrap();
        assert_eq!(image.units[0].bytes, data);
        assert_eq!(image.source_sha256, sha256(&invert(&data)));
    }
    assert!(inner_header(&[0; 64]).is_none());
    assert!(extent(&[], u64::MAX, 2).is_err());
}
fn overlay_fixture() -> Vec<u8> {
    let mut data = vec![0; 0x20004];
    put(&mut data, 0, &[0x40001040, 0x40001100]);
    put(
        &mut data,
        0x40,
        &[4, 0x2000, 4, 0, 0, 0x200, 0x20000, 0x20, 1],
    );
    put(&mut data, 0x64, &[6, 0, 0, 0, 0, 0x20000, 0x20000, 0x20, 0]);
    put(&mut data, 0x88, &[u32::MAX]);
    put(&mut data, 0x100, &[0x40001120, 0x40000, 0x20000]);
    put(&mut data, 0x120, &[0, 0x40000, 4, 0, 0, 0x48]);
    put(&mut data, 0x138, &[1, 0x40000, 4, 0, 0, 0x49]);
    put(&mut data, 0x150, &[u32::MAX]);
    data[0x20000..].copy_from_slice(b"code");
    data
}
fn parse_overlays(data: &[u8]) -> Result<Vec<Overlay>> {
    overlays(
        data,
        &OverlayTable {
            offset: 0,
            base: 0x1000,
            size: 0x200,
            sha256: &sha256(&data[..0x200]),
            alias: 0x40000000,
            flash_dat_bias: 0,
            section_id: 6,
            streams: &[(0x2000, 0x200)],
        },
    )
}
#[test]
fn bounded_overlay_tables() {
    let data = overlay_fixture();
    let rows = parse_overlays(&data).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].dat_offset, 0x20000);
    assert_eq!(rows[0].address, rows[1].address);
    for (offset, value) in [
        (0, 0x40000900),
        (0, 0x40001041),
        (0x120 + 12, 1),
        (0x120 + 8, 0x20001),
        (0x100 + 8, 0),
        (0x64, 4),
        (0x40 + 20, 0x201),
        (0x150, 0),
    ] {
        let mut bad = data.clone();
        put(&mut bad, offset, &[value]);
        assert!(parse_overlays(&bad).is_err(), "{offset:x}");
    }
}
#[test]
fn outer_headers_and_signed_trailer() {
    for (kind, size) in [(4, 0x25c), (6, 0x27c), (8, 0x2bc)] {
        let mut data = vec![0; size];
        put(&mut data, 0, &[kind]);
        put(&mut data, 0x204, &[1, 0x23]);
        if kind == 8 {
            data.extend([0x30, 6, 2, 1, 1, 2, 1, 2]);
        }
        let h = outer_header(&data).unwrap();
        assert_eq!(h.payload_offset, size);
        assert_eq!(h.version_minor, 0x23);
        assert_eq!(h.signature_der.is_some(), kind == 8);
        if kind == 8 {
            assert_eq!(h.signed_bytes.unwrap().len(), size);
            data.pop();
            assert!(outer_header(&data).is_err());
        }
        assert!(outer_header(&data[..size - 1]).is_err());
    }
    for der in [
        &[0x30, 6, 2, 1, 0x80, 2, 1, 1][..],
        &[0x30, 7, 2, 2, 0, 1, 2, 1, 1][..],
    ] {
        assert!(!valid_signature_der(der));
    }
}

#[test]
fn structured_errors_keep_positions_and_values() {
    assert!(matches!(
        extent(&[0; 4], 3, 2),
        Err(Error::Extent {
            offset: 3,
            size: 2,
            available: 4
        })
    ));
    assert!(matches!(
        decompress(&stream(&[0], 0)),
        Err(Error::Literal {
            offset: 20,
            length: 0,
            ..
        })
    ));
    let mut bad = stream(&[1, b'A'], 1);
    put(&mut bad, 16, &[9]);
    assert!(matches!(
        decompress(&bad),
        Err(Error::HeaderValue {
            offset: 16,
            value: 9,
            expected: 4,
            ..
        })
    ));
    let mut data = overlay_fixture();
    put(&mut data, 0, &[0x40001041]);
    assert!(matches!(
        parse_overlays(&data),
        Err(Error::DescriptorPointer {
            address: 0x1041,
            read_size: 4,
            ..
        })
    ));
}

#[test]
fn overlay_descriptors_address_bytes_not_hardware_flash_blocks() {
    let mut data = overlay_fixture();
    data.resize(0x20008, 0);
    put(&mut data, 0x64 + 20, &[0x20004]);
    put(&mut data, 0x100 + 4, &[0x40004]);
    put(&mut data, 0x120 + 4, &[0x40004]);
    put(&mut data, 0x138 + 4, &[0x40004]);
    data[0x20004..].copy_from_slice(b"code");
    let rows = parse_overlays(&data).unwrap();
    assert_eq!(rows[0].dat_offset, 0x20004);
    assert_eq!(rows[0].address, 0x40004);
    assert_eq!(rows[0].sha256, sha256(b"code"));
    assert!(parse_overlays(&data[..0x20007]).is_err());
}
