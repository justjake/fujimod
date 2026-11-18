//! The XP5 still path restates this crate's declared X-Trans still law in
//! `fujimod_emulator_imaging::xtrans`; both must develop the same RAW to the same picture.
use fujimod_emulator_imaging::xtrans;
use fujimod_emulator_xpp::{development, imaging};

#[test]
fn the_imaging_xtrans_law_matches_the_xpp_declared_development() {
    assert_eq!(xtrans::XTRANS, development::XTRANS);
    assert_eq!((xtrans::BLACK, xtrans::WHITE), (imaging::BLACK, imaging::WHITE));
    let (width, lines, pitch) = (30, 18, 72);
    let mut raw = vec![0u8; pitch * lines];
    for y in 0..lines {
        // A picture with edges and every color, mosaicked by the imaging law.
        let rgb: Vec<u8> = (0..width)
            .flat_map(|x| [(x * 9 + y * 3) as u8, (255 - x * 7) as u8, ((x * y) % 256) as u8])
            .collect();
        xtrans::mosaic_line(&rgb, y, &mut raw[y * pitch..y * pitch + 2 * width]);
    }
    for phase in [(0, 0), (4, 2), (5, 1)] {
        let expected = development::develop(&raw, pitch, lines, width, phase, None).unwrap();
        let window = xtrans::Raw::new(&raw, pitch, width, lines, phase).unwrap();
        for (y, line) in expected.iter().enumerate() {
            assert_eq!(&xtrans::develop_422_line(&window, y, width, |x| x), line, "line {y}");
        }
    }
}
