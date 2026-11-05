use super::*;
#[test]
fn rounding_original_edge_extension_and_packed_chroma_exclusion() {
    let luma: Vec<u8> = (0..8)
        .flat_map(|y| (0..8).map(move |x| ((x * x * 17 + y * y * 13 + x * y * 3) % 256) as u8))
        .collect();
    let expected = [
        [
            32, 76, 112, 112, 83, 107, 124, 134, 134, 126, 117, 133, 151, 142, 127, 127,
        ],
        [
            39, 78, 108, 111, 84, 105, 121, 131, 130, 125, 121, 134, 148, 142, 130, 128,
        ],
    ];
    for smooth in [false, true] {
        let mut output = [0; 16];
        half_luma(
            &luma,
            Raster {
                width: 8,
                height: 8,
                pitch: 8,
                packed: false,
            },
            smooth,
            &mut output,
        )
        .unwrap();
        assert_eq!(output, expected[smooth as usize]);
        for chroma in [0, 255] {
            let mut packed = vec![0xa5; 24 * 8];
            for y in 0..8 {
                for x in 0..8 {
                    packed[y * 24 + x / 2 * 4 + x % 2] = luma[y * 8 + x];
                    packed[y * 24 + x / 2 * 4 + 2 + x % 2] = chroma;
                }
            }
            half_luma(
                &packed,
                Raster {
                    width: 8,
                    height: 8,
                    pitch: 24,
                    packed: true,
                },
                smooth,
                &mut output,
            )
            .unwrap();
            assert_eq!(output, expected[smooth as usize]);
        }
    }
}
#[test]
fn tiny_and_narrow_edges_do_not_replicate_filtered_intermediates() {
    for (width, height) in [(2, 2), (2, 16), (16, 2), (32, 18)] {
        for value in [0, 1, 17, 128, 255] {
            let input = vec![value; width * height];
            let mut out = vec![0; width * height / 4];
            for smooth in [false, true] {
                half_luma(
                    &input,
                    Raster {
                        width,
                        height,
                        pitch: width,
                        packed: false,
                    },
                    smooth,
                    &mut out,
                )
                .unwrap();
                assert!(out.iter().all(|v| *v == value));
            }
        }
    }
}
#[test]
fn malformed_rasters_leave_output_unchanged() {
    let mut out = [0xa5; 16];
    for raster in [
        Raster {
            width: 7,
            height: 8,
            pitch: 8,
            packed: false,
        },
        Raster {
            width: 8,
            height: 8,
            pitch: 4,
            packed: false,
        },
        Raster {
            width: 8190,
            height: 8190,
            pitch: 8192,
            packed: false,
        },
    ] {
        assert!(half_luma(&[0; 64], raster, true, &mut out).is_err());
        assert_eq!(out, [0xa5; 16]);
    }
}
