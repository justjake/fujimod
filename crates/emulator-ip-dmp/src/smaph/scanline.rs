//! Coverage on the same pixel/subsample grid as the DMP software rasterizer.
//! Resolve fill crossings once per subrow instead of once per sample point.
type Point = [f64; 2];

pub fn coverage(
    contours: &[Vec<Point>],
    x0: usize,
    x1: usize,
    y: usize,
    samples: u32,
    stroke: Option<f64>,
    nonzero: bool,
) -> Vec<u32> {
    let mut out = vec![0; x1.saturating_sub(x0)];
    for sy in 0..samples {
        let py = y as f64 + (sy as f64 + 0.5) / samples as f64;
        if let Some(radius) = stroke {
            // Preserve round-cap/round-join distance tests, but remove edges
            // whose expanded y bounds cannot cover this subrow.
            let edges: Vec<_> = contours
                .iter()
                .flat_map(|p| p.windows(2))
                .filter(|e| {
                    py >= e[0][1].min(e[1][1]) - radius && py <= e[0][1].max(e[1][1]) + radius
                })
                .collect();
            for (n, covered) in out.iter_mut().enumerate() {
                for sx in 0..samples {
                    let px = (x0 + n) as f64 + (sx as f64 + 0.5) / samples as f64;
                    *covered += u32::from(edges.iter().any(|e| {
                        let [a, b] = [e[0], e[1]];
                        let dx = b[0] - a[0];
                        let dy = b[1] - a[1];
                        let len = dx * dx + dy * dy;
                        let t = if len == 0. {
                            0.
                        } else {
                            ((px - a[0]) * dx + (py - a[1]) * dy) / len
                        }
                        .clamp(0., 1.);
                        (px - a[0] - t * dx).hypot(py - a[1] - t * dy) <= radius
                    }));
                }
            }
        } else {
            let mut crossings = Vec::new();
            for points in contours.iter().filter(|p| p.len() >= 3) {
                for (a, b) in points
                    .iter()
                    .zip(points.iter().cycle().skip(1))
                    .take(points.len())
                {
                    if (a[1] <= py && b[1] > py) || (b[1] <= py && a[1] > py) {
                        let at = a[0] + (py - a[1]) * (b[0] - a[0]) / (b[1] - a[1]);
                        crossings.push((at, if b[1] > a[1] { 1 } else { -1 }));
                    }
                }
            }
            crossings.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
            // Existing winding law casts the ray rightwards: crossings at
            // exactly the sample's x are excluded, and y is half-open.
            let mut winding: i32 = crossings.iter().map(|c| c.1).sum();
            let mut edge = 0;
            for (n, covered) in out.iter_mut().enumerate() {
                for sx in 0..samples {
                    let px = (x0 + n) as f64 + (sx as f64 + 0.5) / samples as f64;
                    while edge < crossings.len() && crossings[edge].0 <= px {
                        winding -= crossings[edge].1;
                        edge += 1;
                    }
                    *covered += u32::from(if nonzero {
                        winding != 0
                    } else {
                        winding & 1 != 0
                    });
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    fn reference(
        contours: &[Vec<Point>],
        x: f64,
        y: f64,
        stroke: Option<f64>,
        nonzero: bool,
    ) -> bool {
        let mut winding = 0i32;
        for points in contours {
            if let Some(radius) = stroke {
                for e in points.windows(2) {
                    let [a, b] = [e[0], e[1]];
                    let dx = b[0] - a[0];
                    let dy = b[1] - a[1];
                    let len = dx * dx + dy * dy;
                    let t = if len == 0. {
                        0.
                    } else {
                        ((x - a[0]) * dx + (y - a[1]) * dy) / len
                    }
                    .clamp(0., 1.);
                    if (x - a[0] - t * dx).hypot(y - a[1] - t * dy) <= radius {
                        return true;
                    }
                }
            } else if points.len() >= 3 {
                for (a, b) in points
                    .iter()
                    .zip(points.iter().cycle().skip(1))
                    .take(points.len())
                {
                    if (a[1] <= y && b[1] > y) || (b[1] <= y && a[1] > y) {
                        let at = a[0] + (y - a[1]) * (b[0] - a[0]) / (b[1] - a[1]);
                        if at > x {
                            winding += if b[1] > a[1] { 1 } else { -1 };
                        }
                    }
                }
            }
        }
        stroke.is_none()
            && if nonzero {
                winding != 0
            } else {
                winding & 1 != 0
            }
    }
    fn compare(c: &[Vec<Point>]) {
        for samples in [1, 2, 4] {
            for stroke in [None, Some(0.), Some(0.5), Some(2.)] {
                for nonzero in [false, true] {
                    for y in 0..12 {
                        let got = coverage(c, 1, 14, y, samples, stroke, nonzero);
                        let want: Vec<u32> = (1..14)
                            .map(|x| {
                                (0..samples)
                                    .flat_map(|sy| (0..samples).map(move |sx| (sy, sx)))
                                    .map(|(sy, sx)| {
                                        u32::from(reference(
                                            c,
                                            x as f64 + (sx as f64 + 0.5) / samples as f64,
                                            y as f64 + (sy as f64 + 0.5) / samples as f64,
                                            stroke,
                                            nonzero,
                                        ))
                                    })
                                    .sum()
                            })
                            .collect();
                        assert_eq!(
                            got, want,
                            "y={y} samples={samples} stroke={stroke:?} nonzero={nonzero}"
                        );
                    }
                }
            }
        }
    }
    #[test]
    fn holes_shared_vertices_open_paths_and_degenerate_strokes() {
        compare(&[
            vec![[1.5, 1.5], [11.5, 1.5], [11.5, 9.5], [1.5, 9.5]],
            vec![[3.5, 3.5], [8.5, 3.5], [8.5, 7.5], [3.5, 7.5]],
        ]);
        compare(&[
            vec![[2., 2.], [10., 10.], [2., 10.], [10., 2.]],
            vec![[5., 5.], [5., 5.]],
            vec![[0., 4.], [13., 4.]],
        ]);
        compare(&[
            vec![[6.125, 0.125], [6.125, 11.125]],
            vec![[0.125, 6.125], [13.125, 6.125]],
            vec![[6., 6.]],
        ]);
    }
    #[test]
    fn varied_concave_and_intersecting_contours_match_point_law() {
        let mut seed = 123456789u32;
        for _ in 0..24 {
            let mut c = Vec::new();
            for _ in 0..3 {
                let mut p = Vec::new();
                for _ in 0..8 {
                    let mut v = [0.; 2];
                    for x in &mut v {
                        seed ^= seed << 13;
                        seed ^= seed >> 17;
                        seed ^= seed << 5;
                        *x = (seed % 128) as f64 / 8. - 2.;
                    }
                    p.push(v);
                }
                c.push(p);
            }
            compare(&c);
        }
    }
}
