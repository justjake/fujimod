//! FFF38000 radial gain supports the isotropic (1,1) and (2,2) selectors,
//! nine cubic segments, and floored 10-bit interpolation.
pub(super) struct Shading {
    center: [i64; 2],
    scale: u64,
    shift: u32,
    gains: Vec<u16>,
}

impl Shading {
    pub fn from_registers(r: impl Fn(usize) -> u32) -> Result<Option<Self>, String> {
        if r(0) & 0x10 == 0 {
            return Ok(None);
        }
        let center = r(0x14);
        let control = r(0x18);
        let mode = control >> 24 & 63;
        let flags = control >> 16 & 63;
        if center & !0x1fff1fff != 0
            || control & !0x3f3f1fff != 0
            || mode != flags
            || !matches!(mode, 1 | 2)
        {
            return Err(
                "unmodeled radial shading selectors; measured isotropic modes are (1,1) and (2,2)"
                    .into(),
            );
        }
        let mut points = Vec::with_capacity(28);
        for o in (0x1c..0x54).step_by(4) {
            let word = r(o);
            if word & !0x3fff3fff != 0 {
                return Err("radial shading point exceeds its 14-bit field".into());
            }
            points.extend([u64::from(word >> 16), u64::from(word & 0xffff)]);
        }
        let mut gains = Vec::with_capacity(9 * 1024 + 1);
        for k in 0..9 {
            let p = &points[3 * k..3 * k + 4];
            for t in 0..1024u64 {
                let u = 1024 - t;
                gains.push(
                    ((u * u * u * p[0]
                        + 3 * u * u * t * p[1]
                        + 3 * u * t * t * p[2]
                        + t * t * t * p[3])
                        >> 30) as u16,
                );
            }
        }
        gains.push(points[27] as u16);
        Ok(Some(Self {
            center: [i64::from(center >> 16), i64::from(center & 0xffff)],
            scale: u64::from(control & 8191),
            shift: mode + flags,
            gains,
        }))
    }

    /// Coordinates are bounded by RawView's maximum 8192-by-8192 raster.
    pub fn gain(&self, x: usize, y: usize) -> u16 {
        let dx = x as i64 - self.center[0];
        let dy = y as i64 - self.center[1];
        let radius = (dx * dx + dy * dy) as u64;
        let s = ((radius * self.scale) << self.shift).saturating_sub(1 << 21);
        self.gains[((s >> 20) as usize).min(9 * 1024)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registers(mode: u32, flags: u32, scale: u32) -> Vec<u32> {
        let mut r = vec![0; 0x60 / 4];
        r[0] = 0x111;
        r[0x14 / 4] = 2472 << 16 | 1648;
        r[0x18 / 4] = mode << 24 | flags << 16 | scale;
        // The authored spike pattern is not a linear interpolation table.
        let mut points = [2048; 28];
        for i in [6, 14, 22] {
            points[i] = 3072;
        }
        for (i, p) in points.chunks_exact(2).enumerate() {
            r[0x1c / 4 + i] = p[0] << 16 | p[1];
        }
        r
    }

    #[test]
    fn spike_pattern_matches_floored_bezier_and_radius_scaling() {
        // Expected values cover the declared curve and radius cases.
        // Cover the center offset, all three spikes, scale and final clamp.
        let cases = [
            (1, 243, [2048, 2048, 2048, 2518, 2048, 2048, 2048]),
            (1, 486, [2048, 2048, 2587, 2048, 2473, 2048, 2048]),
            (1, 121, [2048, 2048, 2048, 2048, 2573, 2048, 2048]),
            (2, 243, [2048, 2048, 2048, 2286, 2048, 2048, 2048]),
        ];
        for (mode, scale, expected) in cases {
            let r = registers(mode, mode, scale);
            let s = Shading::from_registers(|o| r[o / 4]).unwrap().unwrap();
            assert_eq!(
                [0, 32, 1000, 1400, 2000, 2600, 5000].map(|d| s.gain(2472 + d, 1648)),
                expected
            );
        }
    }

    #[test]
    fn full_field_matches_nonconstant_curves_and_limits() {
        // FNV-1a of gain outputs on a 257x193 grid of raw coordinates
        // (31*x,41*y), including endpoint clamping.
        let cases = [
            (1, 0, [2472, 1648], 0xb4d26adade08c8bcu64),
            (1, 121, [2472, 1648], 0x672926780dd15257u64),
            (1, 243, [2472, 1648], 0xf3fdbbbc7e853c54u64),
            (1, 486, [2472, 1648], 0x33093013de6c63f3u64),
            (2, 243, [2472, 1648], 0x67c7a4783c577293u64),
            (2, 8191, [2472, 1648], 0x81002e026faf684cu64),
            (1, 1, [0, 0], 0x44b62b64db6adfefu64),
            (2, 121, [8191, 8191], 0x9587ed4d2565519du64),
        ];
        for (mode, scale, center, expected) in cases {
            let mut r = registers(mode, mode, scale);
            r[0x14 / 4] = center[0] << 16 | center[1];
            let points: Vec<u32> = (0..28)
                .map(|i| (i * i * 727 + i * 31 + 1000) % 16384)
                .collect();
            for (i, p) in points.chunks_exact(2).enumerate() {
                r[0x1c / 4 + i] = p[0] << 16 | p[1];
            }
            let s = Shading::from_registers(|o| r[o / 4]).unwrap().unwrap();
            let mut hash = 0xcbf29ce484222325u64;
            for y in 0..193 {
                for x in 0..257 {
                    for byte in s.gain(x * 31, y * 41).to_le_bytes() {
                        hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
                    }
                }
            }
            assert_eq!(hash, expected);
        }
    }
    #[test]
    fn disabled_shading_is_passive_but_unknown_enabled_fields_refuse() {
        let mut r = registers(1, 2, 243);
        r[0] = 0;
        assert!(Shading::from_registers(|o| r[o / 4]).unwrap().is_none());
        r[0] = 0x10;
        assert!(Shading::from_registers(|o| r[o / 4]).is_err());
        for mode in [0, 3, 63] {
            r = registers(mode, mode, 243);
            assert!(Shading::from_registers(|o| r[o / 4]).is_err());
        }
        for (o, bit) in [(0x14, 1 << 13), (0x18, 1 << 13), (0x1c, 1 << 14)] {
            r = registers(1, 1, 243);
            r[o / 4] |= bit;
            assert!(Shading::from_registers(|o| r[o / 4]).is_err());
        }
    }
}
