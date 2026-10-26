//! The supported sRGB encoding lookup, calculated from its transfer function.
//!
//! This GPU input uses the earlier sRGB curve (offset 0.0556, threshold 0.00304),
//! with truncation to eight bits. The later 0.055/0.0031308 revision has different
//! quantized values and must not silently replace it.

use super::commands::{Command, Write};
use fujimod_emulator_devices::record::{Result, Unsupported};

pub fn encode_srgb(linear: f64) -> u8 {
    let encoded = if linear <= 0.00304 {
        12.92 * linear
    } else {
        1.0556 * fujimod_emulator_devices::math::pow(linear, 1.0 / 2.4) - 0.0556
    };
    (255.0 * encoded).clamp(0.0, 255.0) as u8
}

/// Validate a complete supported lookup upload when one is present. Framing
/// and lane writes are decoded independently; unrelated state can be interleaved.
/// Arbitrary gamma curves require a renderer that implements those curves.
pub fn validate_upload(commands: &[Command<'_>]) -> Result<()> {
    let mut index = 0;
    for command in commands {
        if let Command::Write(Write {
            index: 0x188,
            lanes,
            value,
        }) = *command
        {
            let expected = u32::from(encode_srgb(f64::from(index % 256) / 255.0)) * 0x010101;
            if lanes != 7 || value & 0xffffff != expected {
                return Err(Unsupported(
                    "Graphics gamma upload requires the supported sRGB curve and RGB lanes".into(),
                ));
            }
            index += 1;
        }
    }
    if index % 256 != 0 {
        return Err(Unsupported(
            "Graphics gamma lookup upload is incomplete".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transfer_endpoints_quantization_and_older_curve_are_explicit() {
        assert_eq!(encode_srgb(0.0), 0);
        assert_eq!(encode_srgb(1.0), 255);
        assert_eq!(encode_srgb(1.0 / 255.0), 12);
        assert_eq!(encode_srgb(17.0 / 255.0), 72);
        assert_eq!(encode_srgb(128.0 / 255.0), 187);
        let ramp: Vec<_> = (0..=255)
            .map(|i| encode_srgb(f64::from(i) / 255.0))
            .collect();
        assert!(ramp.windows(2).all(|pair| pair[0] <= pair[1]));
    }

    #[test]
    fn uploads_are_values_with_explicit_lanes_and_complete_channel_counts() {
        let mut upload: Vec<_> = (0..256)
            .map(|i| {
                Command::Write(Write {
                    index: 0x188,
                    lanes: 7,
                    value: u32::from(encode_srgb(f64::from(i) / 255.0)) * 0x010101,
                })
            })
            .collect();
        upload.insert(
            20,
            Command::Write(Write {
                index: 0x140,
                lanes: 1,
                value: 0,
            }),
        );
        assert!(validate_upload(&upload).is_ok());
        upload.pop();
        assert!(validate_upload(&upload).is_err());
        upload[0] = Command::Write(Write {
            index: 0x188,
            lanes: 3,
            value: 0,
        });
        assert!(validate_upload(&upload).is_err());
    }
}
