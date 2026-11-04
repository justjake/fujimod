//! Pure packed display color conversion shared by all execution frontends.
use std::sync::OnceLock;

// Conversion is observational. These bounded tables depend only on the declared
// color matrix, never on device state or the pixels of a previous frame.
struct ColorMatrix {
    red: Box<[u8; 65536]>,
    blue: Box<[u8; 65536]>,
    y: [f64; 256],
    green_cb: [f64; 256],
    green_cr: [f64; 256],
}
fn channel(value: f64) -> u8 {
    value.round_ties_even().clamp(0.0, 255.0) as u8
}
impl ColorMatrix {
    fn new(kr: f64, kb: f64, limited: bool) -> Self {
        let kg = 1.0 - kr - kb;
        let red_factor = 2.0 * (1.0 - kr);
        let blue_factor = 2.0 * (1.0 - kb);
        let green_cb = 2.0 * kb * (1.0 - kb) / kg;
        let green_cr = 2.0 * kr * (1.0 - kr) / kg;
        let y = std::array::from_fn(|v| {
            if limited {
                (v as f64 - 16.0) * (255.0 / 219.0)
            } else {
                v as f64
            }
        });
        let chroma: [f64; 256] = std::array::from_fn(|v| {
            let value = v as f64 - 128.0;
            if limited {
                value * (255.0 / 224.0)
            } else {
                value
            }
        });
        let table = |factor| {
            (0..65536)
                .map(|i| channel(y[i >> 8] + factor * chroma[i & 255]))
                .collect::<Vec<_>>()
                .into_boxed_slice()
                .try_into()
                .unwrap()
        };
        Self {
            red: table(red_factor),
            blue: table(blue_factor),
            y,
            green_cb: chroma.map(|v| green_cb * v),
            green_cr: chroma.map(|v| green_cr * v),
        }
    }
    fn color(&self, y: u8, cb: u8, cr: u8) -> [u8; 3] {
        let (y, cb, cr) = (y as usize, cb as usize, cr as usize);
        [
            self.red[y << 8 | cr],
            // Keep the scalar decoder's subtraction order and ties-to-even.
            channel(self.y[y] - self.green_cb[cb] - self.green_cr[cr]),
            self.blue[y << 8 | cb],
        ]
    }
}
static MATRICES: [OnceLock<ColorMatrix>; 4] = [const { OnceLock::new() }; 4];

pub fn decode(
    data: &[u8],
    width: u64,
    height: u64,
    stride: u64,
    packing: &str,
    matrix: &str,
) -> Result<Vec<u8>, String> {
    let invalid = |message: &str| message.to_owned();
    let pixels = width
        .checked_mul(height)
        .filter(|&n| width > 0 && height > 0 && n <= 32 * 1024 * 1024)
        .ok_or_else(|| invalid("Display dimensions require 1..32Mi pixels"))?;
    let (index, kr, kb, limited): (usize, f64, f64, bool) = match matrix {
        "bt601-limited" => (0, 0.299, 0.114, true),
        "bt601-full" => (1, 0.299, 0.114, false),
        "bt709-limited" => (2, 0.2126, 0.0722, true),
        "bt709-full" => (3, 0.2126, 0.0722, false),
        _ => return Err(invalid("Unknown display color matrix")),
    };
    let rgb565 = packing == "rgb565";
    let channels = match packing {
        "yycbcr" | "rgb565" => [0, 1, 2, 3],
        "yycrcb" => [0, 1, 3, 2],
        "yuyv" => [0, 2, 1, 3],
        "uyvy" => [1, 3, 0, 2],
        _ => return Err(invalid("Unknown display packing")),
    };
    if !rgb565 && width % 2 != 0 {
        return Err(invalid("YUV 4:2:2 requires an even width"));
    }
    let row_bytes = width * 2;
    let required = (height - 1)
        .checked_mul(stride)
        .and_then(|n| n.checked_add(row_bytes));
    if stride < row_bytes || required.is_none_or(|n| n > data.len() as u64) {
        return Err(invalid("Display stride or source byte span differs"));
    }
    let matrix =
        (!rgb565).then(|| MATRICES[index].get_or_init(|| ColorMatrix::new(kr, kb, limited)));
    Ok({
        let mut result = vec![0u8; (pixels * 3) as usize];
        for row in 0..height {
            let source = &data[(row * stride) as usize..(row * stride + row_bytes) as usize];
            let target = &mut result[(row * width * 3) as usize..((row + 1) * width * 3) as usize];
            if rgb565 {
                for (word, rgb) in source.chunks_exact(2).zip(target.chunks_exact_mut(3)) {
                    let v = u16::from_le_bytes([word[0], word[1]]);
                    let (r, g, b) = ((v >> 11) & 31, (v >> 5) & 63, v & 31);
                    rgb.copy_from_slice(&[
                        ((r << 3) | (r >> 2)) as u8,
                        ((g << 2) | (g >> 4)) as u8,
                        ((b << 3) | (b >> 2)) as u8,
                    ]);
                }
            } else {
                let matrix = matrix.unwrap();
                for (word, rgb) in source.chunks_exact(4).zip(target.chunks_exact_mut(6)) {
                    let [y0, y1, cb, cr] = channels.map(|i| word[i]);
                    rgb[..3].copy_from_slice(&matrix.color(y0, cb, cr));
                    rgb[3..].copy_from_slice(&matrix.color(y1, cb, cr));
                }
            }
        }
        result
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_matrices_and_packings_preserve_scalar_color_corpus() {
        use sha2::{Digest, Sha256};
        // Recorded from the scalar decoder before lookup tables: every luma,
        // neutral chroma neighbors, endpoints, and limited-range boundaries.
        let palette = [0, 1, 15, 16, 127, 128, 129, 239, 240, 254, 255];
        for (packing, order) in [
            ("yycbcr", [0, 1, 2, 3]),
            ("yycrcb", [0, 1, 3, 2]),
            ("yuyv", [0, 2, 1, 3]),
            ("uyvy", [2, 0, 3, 1]),
        ] {
            let mut input = Vec::new();
            for cb in palette {
                for cr in palette {
                    for y in (0..=254u8).step_by(2) {
                        input.extend(order.map(|i| [y, y + 1, cb, cr][i]));
                    }
                }
            }
            for (matrix, expected) in [
                (
                    "bt601-limited",
                    "3ec9edc564a18b845da30696ed5a690dac516e33036fa91fcc3239c956b485de",
                ),
                (
                    "bt601-full",
                    "39af344423e9fe4a256d2bbfbce8c1ec4403ef39962025f5ab66f8587d1b332d",
                ),
                (
                    "bt709-limited",
                    "98a23d559da15e7d5b2db1bf0c4d96cd1afc316452b2b1acb298546968f60f05",
                ),
                (
                    "bt709-full",
                    "483179c8fda533c1ee4e2b9f450e6ee60821c650ee821b2515c7b6d9d68982ea",
                ),
            ] {
                let rgb = decode(&input, 256, 121, 512, packing, matrix).unwrap();
                assert_eq!(
                    format!("{:x}", Sha256::digest(rgb)),
                    expected,
                    "{packing}/{matrix}"
                );
            }
        }
    }
    #[test]
    fn packed_color_order_and_row_padding_use_only_addressed_pixels() {
        for (packing, data) in [
            (
                "yycbcr",
                vec![40, 60, 128, 128, 1, 2, 3, 4, 80, 100, 128, 128],
            ),
            (
                "yycrcb",
                vec![40, 60, 128, 128, 1, 2, 3, 4, 80, 100, 128, 128],
            ),
            (
                "yuyv",
                vec![40, 128, 60, 128, 1, 2, 3, 4, 80, 128, 100, 128],
            ),
            (
                "uyvy",
                vec![128, 40, 128, 60, 1, 2, 3, 4, 128, 80, 128, 100],
            ),
        ] {
            assert_eq!(
                decode(&data, 2, 2, 8, packing, "bt601-full").unwrap(),
                vec![40, 40, 40, 60, 60, 60, 80, 80, 80, 100, 100, 100]
            );
        }
        assert_eq!(
            decode(&[0, 248, 31, 0], 2, 1, 4, "rgb565", "bt709-full").unwrap(),
            vec![255, 0, 0, 0, 0, 255]
        );
    }
    #[test]
    fn incomplete_rows_and_odd_yuv_width_refuse_before_decoding() {
        assert!(decode(&[128; 7], 2, 2, 4, "yycbcr", "bt601-full").is_err());
        assert!(decode(&[128; 6], 3, 1, 6, "yycbcr", "bt601-full").is_err());
        assert!(decode(&[], 1, 1, u64::MAX, "rgb565", "bt601-full").is_err());
    }
}
