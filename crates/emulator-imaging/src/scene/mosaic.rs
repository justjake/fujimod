//! Sensor-site radiance of a CFA mosaic. Each site contributes its own color
//! exactly; the other two colors are the mean of that color's sites in the
//! site's 6×6 cell. Cell means are computed once per immutable frame.
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub type Cfa = [[u8; 6]; 6];

/// Black-subtracted sensor counts in one shared little-endian buffer. The
/// normalization reconstructs the original f32 radiance exactly. Checkpoints
/// share this buffer with their restores to preserve pixel identity.
#[derive(Clone, Serialize, Deserialize)]
pub struct Radiance {
    #[serde(with = "fujimod_emulator_devices::checkpoint::shared_bytes")]
    counts: Arc<[u8]>,
    pub norm: f64,
}

impl Radiance {
    pub fn from_counts(counts: impl IntoIterator<Item = u16>, norm: f64) -> Self {
        let counts = counts.into_iter();
        let mut bytes = Vec::with_capacity(counts.size_hint().0 * 2);
        for count in counts {
            bytes.extend_from_slice(&count.to_le_bytes());
        }
        Self {
            counts: bytes.into(),
            norm,
        }
    }
    /// Little-endian u16 counts, as stored in checkpoints.
    pub fn from_bytes(counts: Arc<[u8]>, norm: f64) -> Result<Self, String> {
        if !counts.len().is_multiple_of(2) {
            return Err("radiance bytes are not whole u16 counts".into());
        }
        Ok(Self { counts, norm })
    }
    pub fn bytes(&self) -> &Arc<[u8]> {
        &self.counts
    }
    pub fn len(&self) -> usize {
        self.counts.len() / 2
    }
    pub fn is_empty(&self) -> bool {
        self.counts.is_empty()
    }
    #[inline(always)]
    pub fn get(&self, index: usize) -> f32 {
        let at = index * 2;
        let count = u16::from_le_bytes([self.counts[at], self.counts[at + 1]]);
        (f64::from(count) / self.norm) as f32
    }
    pub fn values(&self) -> impl Iterator<Item = f32> + '_ {
        (0..self.len()).map(|i| self.get(i))
    }
}

/// Immutable mosaic radiance with its precomputed per-cell color means.
pub struct MosaicSites {
    cfa: Cfa,
    width: usize,
    cells_wide: usize,
    radiance: Radiance,
    /// Per cell, the mean radiance of each color over the cell's sites.
    cell_means: Vec<[f32; 3]>,
    /// Largest site radiance in the frame. Scene normalization and the
    /// response scale both derive from it, so the two stay consistent.
    pub max: f32,
}

impl MosaicSites {
    /// Radiance must already be validated finite and non-negative, with
    /// `width * height` sites and every CFA entry in 0..=2.
    pub fn new(cfa: Cfa, width: usize, height: usize, radiance: Radiance) -> Self {
        let cells_wide = width.div_ceil(6);
        let cells_high = height.div_ceil(6);
        let mut sums = vec![[0f64; 3]; cells_wide * cells_high];
        let mut counts = vec![[0u32; 3]; cells_wide * cells_high];
        let mut max = 0f32;
        for y in 0..height {
            for x in 0..width {
                let value = radiance.get(y * width + x);
                max = max.max(value);
                let color = usize::from(cfa[y % 6][x % 6]);
                let cell = (y / 6) * cells_wide + x / 6;
                sums[cell][color] += f64::from(value);
                counts[cell][color] += 1;
            }
        }
        let cell_means = sums
            .iter()
            .zip(&counts)
            .map(|(sum, count)| {
                std::array::from_fn(|c| {
                    if count[c] == 0 {
                        0.
                    } else {
                        (sum[c] / f64::from(count[c])) as f32
                    }
                })
            })
            .collect();
        Self {
            cfa,
            width,
            cells_wide,
            radiance,
            cell_means,
            max,
        }
    }

    /// Radiance seen at sensor site (x, y) for each color channel.
    #[inline(always)]
    pub fn site(&self, x: usize, y: usize) -> [f32; 3] {
        let mut values = self.cell_means[(y / 6) * self.cells_wide + x / 6];
        let own = usize::from(self.cfa[y % 6][x % 6]);
        if own < 3 {
            values[own] = self.radiance.get(y * self.width + x);
        }
        values
    }
    /// The sites under a per-value law. Every site value is a cell mean or
    /// the radiance of one of 65536 counts, so the law runs once per cell and
    /// once per count; `MappedSites::row` equals `law` applied to `site`.
    pub fn mapped(&self, law: impl Fn(f32) -> u16) -> MappedSites {
        MappedSites {
            cfa: self.cfa,
            width: self.width,
            cells_wide: self.cells_wide,
            counts: self.radiance.counts.clone(),
            cell_means: self.cell_means.iter().map(|m| m.map(&law)).collect(),
            counts_mapped: (0..=u16::MAX)
                .map(|count| law((f64::from(count) / self.radiance.norm) as f32))
                .collect(),
        }
    }
}

/// Mosaic sites after a per-value law, from `MosaicSites::mapped`.
pub struct MappedSites {
    cfa: Cfa,
    width: usize,
    cells_wide: usize,
    counts: Arc<[u8]>,
    cell_means: Vec<[u16; 3]>,
    counts_mapped: Vec<u16>,
}

impl MappedSites {
    #[inline(always)]
    pub fn site(&self, x: usize, y: usize) -> [u16; 3] {
        let mut values = self.cell_means[(y / 6) * self.cells_wide + x / 6];
        let own = usize::from(self.cfa[y % 6][x % 6]);
        if own < 3 {
            let at = (y * self.width + x) * 2;
            values[own] = self.counts_mapped
                [usize::from(u16::from_le_bytes([self.counts[at], self.counts[at + 1]]))];
        }
        values
    }
    /// Running sums of the mapped sites of row `y`: `out[0]` is zero and
    /// `out[x + 1]` sums sites `0..=x`.
    pub fn row_prefix(&self, y: usize, out: &mut [[u32; 3]]) {
        // Index loops over whole cells: iterator adapters are not inlined in
        // this crate's optimization profile.
        let cfa = &self.cfa[y % 6];
        let cells = &self.cell_means[(y / 6) * self.cells_wide..];
        let counts = &self.counts[y * self.width * 2..(y + 1) * self.width * 2];
        let mapped: &[u16; 65536] = self.counts_mapped[..].try_into().unwrap();
        let out = &mut out[..self.width + 1];
        out[0] = [0; 3];
        let mut sums = [0u32; 3];
        let mut x = 0;
        while x < self.width {
            let sites = (self.width - x).min(6);
            let [mr, mg, mb] = cells[x / 6];
            let means = [u32::from(mr), u32::from(mg), u32::from(mb)];
            let counts = &counts[x * 2..x * 2 + sites * 2];
            let out = &mut out[x + 1..x + 1 + sites];
            for phase in 0..sites {
                let site = u32::from(
                    mapped[usize::from(u16::from_le_bytes([
                        counts[phase * 2],
                        counts[phase * 2 + 1],
                    ]))],
                );
                // Select rather than index, so the sums stay in registers.
                let own = cfa[phase];
                sums[0] += if own == 0 { site } else { means[0] };
                sums[1] += if own == 1 { site } else { means[1] };
                sums[2] += if own == 2 { site } else { means[2] };
                out[phase] = sums;
            }
            x += 6;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_reconstruct_every_u16_with_the_original_float_rounding() {
        for norm in [1., 2., 1. / 800. / 81., 0.123456789, 1e-12, 1e12] {
            let radiance = Radiance::from_counts(0..=u16::MAX, norm);
            assert_eq!(radiance.bytes().len(), 65536 * 2);
            for (raw, value) in (0..=u16::MAX).zip(radiance.values()) {
                let original = (f64::from(raw) / norm) as f32;
                assert_eq!(value.to_bits(), original.to_bits());
                assert_eq!(radiance.get(usize::from(raw)).to_bits(), original.to_bits());
            }
            let original: Vec<_> = (0..=u16::MAX)
                .flat_map(|raw| ((f64::from(raw) / norm) as f32).to_le_bytes())
                .collect();
            let identity = fujimod_emulator_devices::fingerprint::float32s(radiance.values());
            assert_eq!(identity["floats"], 65536);
            assert_eq!(
                identity["sha256"],
                fujimod_emulator_devices::fingerprint::bytes(&original)["sha256"]
            );
        }
        assert!(Radiance::from_bytes(Arc::from([0u8]), 1.).is_err());
    }
}
