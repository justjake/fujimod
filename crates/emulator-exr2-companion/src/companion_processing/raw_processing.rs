//! Packed12 phase raster reduction and addressed tone mapping.
//!
//! Spatial reduction, defective-site interpolation and phase-gain estimates are
//! declared numerical approximations. The packet owns the crop, pitches, tone
//! table, mask, calibration gains and result destinations; no software state or
//! uploaded-program identity selects an effect.
use super::{DmaOutput, word};
use fujimod_emulator_devices::Cx;
use fujimod_emulator_imaging::exr2_isp::dma;
use std::collections::BTreeMap;

pub(super) struct Request {
    source: u32,
    source_pitch: usize,
    destination: u32,
    pitch: usize,
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    statistics: u32,
    samples: u32,
    mask: u32,
    tone: u32,
    gains: [u8; 4],
    ceiling: u16,
}
impl Request {
    pub(super) fn parse(p: &[u8]) -> Result<Self, String> {
        if p.len() != 80 {
            return Err("FE80 RAW parameters require 80 bytes".into());
        }
        let pair = |offset| {
            let v = word(p, offset);
            [(v & 65535) as usize, (v >> 16) as usize]
        };
        let [source_pitch, mode] = pair(4);
        let [pitch, planes] = pair(12);
        let [x, y] = pair(16);
        let [width, height] = pair(20);
        if mode != 1
            || planes != 0
            || source_pitch == 0
            || source_pitch > 8192
            || !source_pitch.is_multiple_of(2)
            || width == 0
            || !width.is_multiple_of(12)
            || height == 0
            || !height.is_multiple_of(3)
            || x + width > source_pitch
            || y + height > 8192
            || pitch != width / 3
            || pitch > 8192
        {
            return Err("FE80 RAW packet has unsupported packing, reduction or geometry".into());
        }
        let request = Self {
            source: word(p, 0),
            source_pitch,
            destination: word(p, 8),
            pitch,
            x,
            y,
            width,
            height,
            statistics: word(p, 48),
            samples: word(p, 52),
            mask: word(p, 64),
            tone: word(p, 68),
            gains: word(p, 72).to_le_bytes(),
            ceiling: (word(p, 60) & 65535) as u16,
        };
        // Validate every addressed input/output before starting a job. The
        // source pitch is in packed samples, not bytes or a software buffer size.
        dma::physical(request.source.into(), source_pitch * 3 / 2 * (y + height))?;
        dma::physical(request.tone.into(), 1024)?;
        dma::physical(request.mask.into(), 256)?;
        request.output_spans()?;
        Ok(request)
    }
    fn columns(&self) -> usize {
        self.width.div_ceil(96)
    }
    fn rows(&self) -> usize {
        self.height.div_ceil(12)
    }
    fn output_height(&self) -> usize {
        self.height / 3 * 2
    }
    pub(super) fn pixels(&self) -> u64 {
        (self.width * self.height) as u64
    }
    pub(super) fn output_spans(&self) -> Result<Vec<(u32, usize)>, String> {
        let spans = vec![
            (self.destination, self.pitch * self.output_height() * 2),
            (self.statistics, self.columns() * 4 * 2),
            (self.samples, self.columns() * self.rows() * 2),
        ];
        let ranges = spans
            .iter()
            .map(|&(address, length)| dma::span(address.into(), length))
            .collect::<Result<Vec<_>, _>>()?;
        for (i, a) in ranges.iter().enumerate() {
            for b in &ranges[i + 1..] {
                if a.start < b.end && b.start < a.end {
                    return Err("FE80 RAW result destinations overlap".into());
                }
            }
        }
        Ok(spans)
    }
    pub(super) fn calculate(&self, cx: &Cx<'_>) -> Result<Vec<DmaOutput>, String> {
        let pitch = self.source_pitch * 3 / 2;
        let raw = cx.ram_bytes(
            dma::physical(self.source.into(), pitch * (self.y + self.height))?,
            pitch * (self.y + self.height),
        )?;
        let tone = cx.ram_bytes(dma::physical(self.tone.into(), 1024)?, 1024)?;
        let packed = cx.ram_bytes(dma::physical(self.mask.into(), 256)?, 256)?;
        let mut mask = BTreeMap::new();
        for entry in packed.chunks_exact(4) {
            let v = word(entry, 0);
            if v == u32::MAX {
                break;
            }
            let cell = (v & 65535) as usize;
            let bits = v >> 16;
            if cell >= self.width / 12 * (self.height / 3) || bits & !4095 != 0 {
                return Err("FE80 RAW defective-site mask escapes crop".into());
            }
            *mask.entry(cell).or_insert(0u32) |= bits;
        }
        let sample = |x: usize, y: usize| -> u16 {
            let i = (self.y + y) * pitch + (self.x + x) * 3 / 2;
            let v = u16::from_le_bytes([raw[i], raw[i + 1]]);
            if (self.x + x) & 1 == 0 {
                v & 4095
            } else {
                v >> 4
            }
        };
        let corrected = |x: usize, y: usize| -> u16 {
            let cell = y / 3 * (self.width / 12) + x / 12;
            let bit = (y % 3) * 4 + (x % 12) / 3;
            if mask.get(&cell).copied().unwrap_or(0) & (1 << bit) == 0 {
                return sample(x, y);
            }
            let left = if x >= 3 {
                x - 3
            } else {
                (x + 3).min(self.width - 1)
            };
            let right = if x + 3 < self.width { x + 3 } else { left };
            (u32::from(sample(left, y)) + u32::from(sample(right, y))).div_ceil(2) as u16
        };
        let spans = self.output_spans()?;
        let mut raster = vec![0u8; spans[0].1];
        let columns = self.columns();
        let rows = self.rows();
        let mut sums = vec![0u64; columns * rows];
        let mut counts = vec![0u32; columns * rows];
        // A three-column by three-row sensor cell yields two scalar samples.
        // Keep the two row phases separate for split-image reconstruction.
        // Two luma samples share neutral chroma in packed Y0 Y1 Cb Cr.
        for oy in 0..self.output_height() {
            let y = oy / 2 * 3 + oy % 2;
            for ox in 0..self.width / 3 {
                let x = ox * 3;
                let phase = (oy % 2) * 2 + ox % 2;
                let raw = u32::from(corrected(x, y));
                let gained = (raw * u32::from(self.gains[phase]) + 64) / 128;
                let value = tone[(gained.min(4095) >> 2) as usize];
                let i = oy * self.pitch * 2 + ox / 2 * 4;
                raster[i + ox % 2] = value;
                raster[i + 2] = 128;
                raster[i + 3] = 128;
                let bin = (y / 12) * columns + x / 96;
                sums[bin] += u64::from(raw);
                counts[bin] += 1;
            }
        }
        let mut statistics = vec![0u64; columns * 4];
        let mut statistic_counts = vec![0u32; columns * 4];
        let mut samples = Vec::with_capacity(spans[2].1);
        // Tagged Q7 phase-gain estimates share the RAW pixels consumed by the
        // raster. Normalize each row phase against its column, with unity for
        // a dark column, and bound it by the programmed calibration ceiling.
        let references = (0..columns)
            .map(|column| {
                let (total, n) = (0..rows).fold((0u64, 0u64), |(sum, n), r| {
                    let j = r * columns + column;
                    (sum + sums[j], n + u64::from(counts[j]))
                });
                total / n.max(1)
            })
            .collect::<Vec<_>>();
        for row in 0..rows {
            for column in 0..columns {
                let i = row * columns + column;
                let average = sums[i] / u64::from(counts[i].max(1));
                let reference = references[column];
                let gain = if reference == 0 {
                    128
                } else {
                    (average * 128 + reference / 2) / reference
                };
                let limit = (u64::from(self.ceiling) * 128).clamp(128, 4094);
                let gain = gain.clamp(128, limit) as u16;
                let phase = row % 4;
                samples.extend_from_slice(&(gain | ((phase as u16) << 12)).to_le_bytes());
                statistics[phase * columns + column] += u64::from(gain);
                statistic_counts[phase * columns + column] += 1;
            }
        }
        let statistics = statistics
            .into_iter()
            .zip(statistic_counts)
            .flat_map(|(sum, n)| {
                (if n == 0 {
                    128
                } else {
                    (sum + u64::from(n) / 2) / u64::from(n)
                } as u16)
                    .to_le_bytes()
            })
            .collect();
        Ok(vec![
            DmaOutput {
                address: self.destination,
                bytes: raster,
            },
            DmaOutput {
                address: self.statistics,
                bytes: statistics,
            },
            DmaOutput {
                address: self.samples,
                bytes: samples,
            },
        ])
    }
}
