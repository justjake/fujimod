//! The X-Trans 6x6 color filter array.

pub const RED: u8 = 0;
pub const GREEN: u8 = 1;
pub const BLUE: u8 = 2;

/// Colors of one 6x6 X-Trans tile, indexed `[y % 6][x % 6]` in the
/// coordinates of the plane it describes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct XTrans {
    rows: [[u8; 6]; 6],
}

impl XTrans {
    pub fn new(rows: [[u8; 6]; 6]) -> Result<Self, String> {
        let mut counts = [0usize; 3];
        for row in &rows {
            for &c in row {
                if c > BLUE {
                    return Err(format!("CFA color {c} is not red, green or blue"));
                }
                counts[c as usize] += 1;
            }
        }
        if counts != [8, 20, 8] {
            return Err(format!(
                "CFA tile has {}R {}G {}B; an X-Trans tile has 8R 20G 8B",
                counts[0], counts[1], counts[2]
            ));
        }
        let pattern = Self { rows };
        pattern.block_origin()?;
        Ok(pattern)
    }

    /// The common X-Trans layout, used for synthetic frames.
    pub fn standard() -> Self {
        const G: u8 = GREEN;
        const R: u8 = RED;
        const B: u8 = BLUE;
        Self {
            rows: [
                [G, G, R, G, G, B],
                [G, G, B, G, G, R],
                [B, R, G, R, B, G],
                [G, G, B, G, G, R],
                [G, G, R, G, G, B],
                [R, B, G, B, R, G],
            ],
        }
    }

    pub fn rows(&self) -> [[u8; 6]; 6] {
        self.rows
    }

    #[inline(always)]
    pub fn color(&self, x: usize, y: usize) -> u8 {
        self.rows[y % 6][x % 6]
    }

    /// The pattern of a plane whose origin is at `(left, top)` in this one.
    pub fn shifted(&self, left: usize, top: usize) -> Self {
        let mut rows = [[0u8; 6]; 6];
        for (y, row) in rows.iter_mut().enumerate() {
            for (x, c) in row.iter_mut().enumerate() {
                *c = self.color(x + left, y + top);
            }
        }
        Self { rows }
    }

    /// Offset `(ox, oy)` in `0..3` of the 3x3 block grid whose every block
    /// holds two red, five green and two blue sites.
    pub fn block_origin(&self) -> Result<(usize, usize), String> {
        for oy in 0..3 {
            for ox in 0..3 {
                let balanced = (0..2).all(|by| {
                    (0..2).all(|bx| {
                        let mut counts = [0usize; 3];
                        for y in 0..3 {
                            for x in 0..3 {
                                counts[self.color(ox + 3 * bx + x, oy + 3 * by + y) as usize] += 1;
                            }
                        }
                        counts == [2, 5, 2]
                    })
                });
                if balanced {
                    return Ok((ox, oy));
                }
            }
        }
        Err("CFA tile has no 3x3 block grid with 2R 5G 2B per block".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_pattern_is_valid_and_block_aligned() {
        let p = XTrans::new(XTrans::standard().rows()).unwrap();
        assert_eq!(p.block_origin().unwrap(), (0, 0));
        let s = p.shifted(1, 2);
        assert_eq!(s.color(0, 0), p.color(1, 2));
        // Any returned origin gives balanced blocks over the whole tile.
        let (ox, oy) = s.block_origin().unwrap();
        for by in 0..2 {
            for bx in 0..2 {
                let mut counts = [0; 3];
                for y in 0..3 {
                    for x in 0..3 {
                        counts[s.color(ox + 3 * bx + x, oy + 3 * by + y) as usize] += 1;
                    }
                }
                assert_eq!(counts, [2, 5, 2]);
            }
        }
    }

    #[test]
    fn rejects_bayer_counts() {
        let mut rows = [[GREEN; 6]; 6];
        rows[0][0] = RED;
        assert!(XTrans::new(rows).is_err());
    }
}
