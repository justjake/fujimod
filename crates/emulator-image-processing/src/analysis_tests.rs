use super::histogram_luma;
use fujimod_emulator_imaging::analysis::ImageLayout;

fn scalar(source: &[u8], input: ImageLayout, grid: [usize; 2], window: [usize; 4]) -> [u8; 1024] {
    let [left, top, width, height] = window;
    let mut counts = [0u32; 256];
    for y in top..(top + height).min(grid[1]) {
        for x in (left..(left + width).min(grid[0])).step_by(2) {
            let sx = ((2 * x + 1) * input.width / (2 * grid[0])).min(input.width - 1);
            let sy = ((2 * y + 1) * input.height / (2 * grid[1])).min(input.height - 1);
            counts[source[sy * input.stride + sx / 2 * 4 + sx % 2] as usize] += 1;
        }
    }
    let mut result = [0; 1024];
    for (word, count) in result.chunks_exact_mut(4).zip(counts) {
        word.copy_from_slice(&count.to_le_bytes());
    }
    result
}

#[test]
fn histogram_packed_pairs_preserve_local_sample_phase_clipping_and_padding() {
    for [width, height] in [[2, 1], [4, 3], [18, 11], [40, 7]] {
        let input = ImageLayout {
            width,
            height,
            stride: width * 2 + 12,
        };
        let mut source = vec![0xfd; input.bytes().unwrap()];
        for y in 0..height {
            for x in 0..width {
                source[y * input.stride + x / 2 * 4 + x % 2] =
                    ((x * 71 + y * 43 + x * y * 11) % 251) as u8;
            }
        }
        for grid in [
            [1, 1],
            [3, 2],
            [4, 4],
            [7, 5],
            [31, 27],
            [71, 53],
            [16384, 16384],
        ] {
            for left in [0, 1, 2, grid[0] / 2, grid[0].min(8192)] {
                for top in [0, 1, grid[1] / 2, grid[1].min(8192)] {
                    for [w, h] in [[1, 1], [2, 3], [5, 7], [33, 19]] {
                        let window = [left, top, w, h];
                        let mut actual = [0xa5; 1024];
                        histogram_luma(&source, input, grid, window, &mut actual).unwrap();
                        assert_eq!(
                            actual,
                            scalar(&source, input, grid, window),
                            "input {width}x{height}, grid {grid:?}, window {window:?}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn histogram_keeps_the_maximum_window_count() {
    let input = ImageLayout {
        width: 2,
        height: 1,
        stride: 4,
    };
    let mut actual = [0; 1024];
    histogram_luma(
        &[19, 173, 251, 251],
        input,
        [16384, 16384],
        [8192, 8192, 8192, 8192],
        &mut actual,
    )
    .unwrap();
    let bins: Vec<u32> = actual
        .chunks_exact(4)
        .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
        .collect();
    assert_eq!(bins[173], 4096 * 8192);
    assert_eq!(bins.iter().sum::<u32>(), 4096 * 8192);
}

#[test]
fn histogram_refuses_invalid_input_before_sampling() {
    let input = ImageLayout {
        width: 4,
        height: 2,
        stride: 12,
    };
    for (source, grid, window) in [
        (vec![0; 23], [4, 2], [0, 0, 4, 2]),
        (vec![0; 24], [0, 2], [0, 0, 4, 2]),
        (vec![0; 24], [16385, 2], [0, 0, 4, 2]),
        (vec![0; 24], [4, 2], [0, 0, 0, 2]),
        (vec![0; 24], [4, 2], [8193, 0, 4, 2]),
    ] {
        let mut actual = [0xa5; 1024];
        assert!(histogram_luma(&source, input, grid, window, &mut actual).is_err());
        assert_eq!(actual, [0xa5; 1024]);
    }
}
