use super::fixtures::*;
use super::*;

#[test]
fn authored_clear_decodes_fences_color_and_scratch_without_a_writer_template() {
    let (path, fill) = lists();
    let (descriptor, _) = parse_clear(&path, &fill, 32768, 32768, None, None).unwrap();
    assert_eq!(int(&descriptor, "path_sequence"), 42);
    assert_eq!(int(&descriptor, "fill_sequence"), 42);
    assert_eq!(int(&descriptor, "destination"), 256);
    let Some(Value::Bytes(pixel)) = get(&descriptor, "pixel_bytes") else {
        panic!("pixel")
    };
    assert_eq!(pixel, &[0x80, 0x40, 0x20, 0x10]);
    let source = vec![0x5a; 256];
    let rendered = render_clear(&source, 8, [1, 1, 5, 5], pixel);
    assert_eq!(
        rendered.iter().filter(|&&byte| byte != 0x5a).count(),
        4 * 4 * 4
    );
    assert_eq!(&rendered[..4], &source[..4]);
}

#[test]
fn equivalent_byte_lanes_reordering_and_redundant_state_are_accepted() {
    let (path, fill) = lists();
    let expected = parse_clear(&path, &fill, 32768, 32768, None, None)
        .unwrap()
        .0;
    let mut rewritten = Vec::new();
    // Reorder all independent fill-state writes. Split full writes into
    // low/high halfword operations with irrelevant bits in unselected lanes.
    for pair in fill_state().chunks_exact(2).rev() {
        let index = pair[0] & 0xffff;
        write(&mut rewritten, index, 3, pair[1] | 0xffff0000);
        write(&mut rewritten, index, 12, pair[1] | 0x0000ffff);
    }
    let prefix = fill_state().len();
    rewritten.extend_from_slice(&fill[prefix..]);
    while rewritten.last() == Some(&0) {
        rewritten.pop();
    }
    let actual = parse_clear(&path, &align(rewritten), 32768, 32768, None, None)
        .unwrap()
        .0;
    assert_eq!(actual, expected);
}

#[test]
fn malformed_or_unsupported_state_refuses_before_any_rendering() {
    let (path, mut fill) = lists();
    let blend = fill
        .chunks_exact(2)
        .position(|pair| pair[0] & 0xffff == 0x1a0)
        .unwrap()
        * 2
        + 1;
    fill[blend] = 0x10000;
    assert!(parse_clear(&path, &fill, 32768, 32768, None, None).is_err());
    let (_, mut fill) = lists();
    let fence = fill
        .chunks_exact(2)
        .position(|pair| pair[0] & 0xffff == 0xad)
        .unwrap()
        * 2
        + 1;
    fill[fence] = 0x1002b;
    assert!(parse_clear(&path, &fill, 32768, 32768, None, None).is_err());
    assert!(path_descriptor(&[0; 16], 32768, 32768).is_err());
}

#[test]
fn a_fill_with_a_partially_written_ramp_filter_state_is_refused() {
    let (path, fill) = lists();
    let mut rewritten = Vec::new();
    // Only the low halfword of the inactive ramp/filter state is written.
    for pair in fill_state().chunks_exact(2) {
        let (index, lanes) = (
            pair[0] & 0xffff,
            if pair[0] & 0xffff == 0x16c { 3 } else { 15 },
        );
        write(&mut rewritten, index, lanes, pair[1]);
    }
    let prefix = fill_state().len();
    rewritten.extend_from_slice(&fill[prefix..]);
    while rewritten.last() == Some(&0) {
        rewritten.pop();
    }
    let error = parse_clear(&path, &align(rewritten), 32768, 32768, None, None).unwrap_err();
    assert!(
        error.0.contains("unsupported mask, paint or sampler state"),
        "{}",
        error.0
    );
}
