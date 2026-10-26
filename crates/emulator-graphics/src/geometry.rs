//! Path recovery, curve flattening and stroke construction
//! (`_ui_transform`, `_ui_curve`, `_ui_paths`, `_ui_stroke`).
//!
//! Declared software laws, not measured hardware: 0.125px flattening
//! tolerance, adaptive de Casteljau subdivision to depth 14, round/bevel/miter
//! (limit 4) joins, butt/round/square caps and polygonal circles.

use super::batch::{Affine, Draw};
use fujimod_emulator_devices::math::{cos, dist2, hypot2, pow, sin, sum};
use fujimod_emulator_devices::record::{Result, Unsupported};

pub type Point = (f64, f64);

/// Inputs read by stroke construction, independent of command/paint state.
pub struct StrokeStyle {
    pub radius: f64,
    pub matrix: Affine,
    pub device_radius: bool,
    pub cap: u32,
    pub join: u32,
}

impl From<&Draw> for StrokeStyle {
    fn from(draw: &Draw) -> Self {
        Self {
            radius: draw.radius,
            matrix: draw.matrix,
            device_radius: draw.device_radius,
            cap: draw.cap,
            join: draw.join,
        }
    }
}

fn fail<T>(message: &str) -> Result<T> {
    Err(Unsupported(message.into()))
}

/// Python `max(a, b)` for floats: the first unless the second is greater.
fn pmax(a: f64, b: f64) -> f64 {
    if b > a { b } else { a }
}

pub fn transform(point: Point, matrix: &Affine) -> Result<Point> {
    let (x, y) = point;
    let [a, b, c, d, e, f] = *matrix;
    let result = (a * x + b * y + c, d * x + e * y + f);
    if [result.0, result.1]
        .iter()
        .any(|v| !v.is_finite() || v.abs() > 16777216.0)
    {
        return fail("Graphics UI transformed geometry exceeds the declared bound");
    }
    Ok(result)
}

/// Bounded adaptive de Casteljau subdivision (declared software law).
pub fn curve(points: Vec<Point>, tolerance: f64) -> Result<Vec<Point>> {
    let mut output = Vec::new();
    let mut pending: Vec<(Vec<Point>, u32)> = vec![(points, 0)];
    while let Some((points, depth)) = pending.pop() {
        let (start, end) = (points[0], points[points.len() - 1]);
        let (dx, dy) = (end.0 - start.0, end.1 - start.1);
        let denominator = hypot2(dx, dy);
        let inner = &points[1..points.len() - 1];
        let mut error: Option<f64> = None;
        for point in inner {
            let value = if denominator != 0.0 {
                // Python min(1, max(0, x)) keeps the int on ties; the products agree.
                let x =
                    ((point.0 - start.0) * dx + (point.1 - start.1) * dy) / pow(denominator, 2.0);
                let t = if x > 0.0 { x } else { 0.0 };
                let t = if t < 1.0 { t } else { 1.0 };
                dist2(*point, (start.0 + t * dx, start.1 + t * dy))
            } else {
                dist2(start, *point)
            };
            error = Some(match error {
                None => value,
                Some(e) => pmax(e, value),
            });
        }
        let error = error.expect("curves have interior control points");
        if error <= tolerance {
            output.push(end);
        } else if depth == 14 {
            return fail("Graphics UI curve subdivision exceeds the declared depth");
        } else {
            let mut layers: Vec<Vec<Point>> = vec![points.clone()];
            while layers.last().unwrap().len() > 1 {
                let last = layers.last().unwrap();
                let next: Vec<Point> = last
                    .windows(2)
                    .map(|w| ((w[0].0 + w[1].0) / 2.0, (w[0].1 + w[1].1) / 2.0))
                    .collect();
                layers.push(next);
            }
            let left: Vec<Point> = layers.iter().map(|layer| layer[0]).collect();
            let right: Vec<Point> = layers
                .iter()
                .rev()
                .map(|layer| layer[layer.len() - 1])
                .collect();
            pending.push((right, depth + 1));
            pending.push((left, depth + 1));
        }
        if output.len() + pending.len() > 65536 {
            return fail("Graphics UI curve flattening exceeds the declared edge bound");
        }
    }
    Ok(output)
}

/// The draw's polylines: (points, closed) (`_ui_paths`).
pub fn paths(draw: &Draw) -> Result<Vec<(Vec<Point>, bool)>> {
    let matrix = &draw.matrix;
    let object_stroke = draw.stroke && !draw.device_radius;
    let scale = pmax(
        pmax(hypot2(matrix[0], matrix[3]), hypot2(matrix[1], matrix[4])),
        1.0,
    );
    let tolerance = if object_stroke { 0.125 / scale } else { 0.125 };
    let mut paths: Vec<(Vec<Point>, bool)> = Vec::new();
    let mut current: Vec<Point> = Vec::new();
    let mut closed = false;
    let mut edges = 0usize;
    fn flush(paths: &mut Vec<(Vec<Point>, bool)>, current: &mut Vec<Point>, closed: &mut bool) {
        if current.len() > 1 {
            let is_closed = *closed || current[0] == current[current.len() - 1];
            paths.push((std::mem::take(current), is_closed));
        }
        current.clear();
        *closed = false;
    }
    for (code, flags, values) in &draw.geometry {
        let mut points = Vec::with_capacity(values.len() / 2);
        for pair in values
            .chunks_exact(2)
            .filter(|_| matches!(code, 0x28 | 0x29 | 0x30 | 0x31))
        {
            let point = (pair[0], pair[1]);
            points.push(if object_stroke {
                point
            } else {
                transform(point, matrix)?
            });
        }
        match *code {
            0x28 => {
                flush(&mut paths, &mut current, &mut closed);
                current = vec![points[0]];
            }
            0x29 | 0x30 | 0x31 => {
                if current.is_empty() {
                    return fail("Graphics UI line/curve has no preceding move");
                }
                let added = if *code == 0x29 {
                    points
                } else {
                    let mut control = vec![current[current.len() - 1]];
                    control.extend(points);
                    curve(control, tolerance)?
                };
                edges += added.len();
                current.extend(added);
            }
            _ if !draw.stroke => {
                return fail("Graphics UI extrusion metadata is invalid in a fill path");
            }
            0x2b => {
                if current.is_empty() {
                    return fail("Graphics UI stroke close has no centerline");
                }
                closed = true;
                if current[current.len() - 1] != current[0] {
                    let first = current[0];
                    current.push(first);
                }
                flush(&mut paths, &mut current, &mut closed);
            }
            0x2a if flags & 2 != 0 => flush(&mut paths, &mut current, &mut closed),
            _ => {}
        }
        if edges > 65536 {
            return fail("Graphics UI batch path exceeds the declared flattened edge bound");
        }
    }
    flush(&mut paths, &mut current, &mut closed);
    if paths.is_empty() {
        return fail("Graphics UI draw has no supported fill edges/stroke centerline");
    }
    Ok(paths)
}

/// Segment rectangles with declared round/bevel/miter joins and caps
/// (`_ui_stroke`); each polygon is closed (first point repeated).
pub fn stroke(paths: &[(Vec<Point>, bool)], draw: &Draw) -> Result<Vec<Vec<Point>>> {
    stroke_with_style(paths, &StrokeStyle::from(draw))
}

pub fn stroke_with_style(
    paths: &[(Vec<Point>, bool)],
    draw: &StrokeStyle,
) -> Result<Vec<Vec<Point>>> {
    let radius = draw.radius;
    let matrix = draw.matrix;
    let device = draw.device_radius;
    let mut polygons: Vec<Vec<Point>> = Vec::new();
    let mut polygon = |points: Vec<Point>| -> Result<()> {
        let mut points: Vec<Point> = if device {
            points
        } else {
            points
                .into_iter()
                .map(|p| transform(p, &matrix))
                .collect::<Result<_>>()?
        };
        let n = points.len();
        let area = sum((0..n).map(|i| {
            let (a, b) = (points[i], points[(i + 1) % n]);
            a.0 * b.1 - b.0 * a.1
        }));
        if area < 0.0 {
            points.reverse();
        }
        let first = points[0];
        points.push(first);
        polygons.push(points);
        Ok(())
    };
    let circle_points = |center: Point| -> Vec<Point> {
        let scale = if device {
            1.0
        } else {
            pmax(hypot2(matrix[0], matrix[3]), hypot2(matrix[1], matrix[4]))
        };
        let extent = pmax(radius * scale, 1.0);
        let count = (std::f64::consts::PI * (extent / 0.125).sqrt()).ceil() as i64;
        let count = count.clamp(12, 256);
        (0..count)
            .map(|i| {
                let angle = 2.0 * std::f64::consts::PI * i as f64 / count as f64;
                (
                    center.0 + radius * cos(angle),
                    center.1 + radius * sin(angle),
                )
            })
            .collect()
    };
    for (points, closed) in paths {
        let mut points: Vec<Point> = points
            .iter()
            .enumerate()
            .filter(|(index, p)| *index == 0 || **p != points[index - 1])
            .map(|(_, p)| *p)
            .collect();
        if points.len() < 2 {
            continue;
        }
        if *closed && points[points.len() - 1] != points[0] {
            let first = points[0];
            points.push(first);
        }
        let mut tangents: Vec<Point> = Vec::new();
        for pair in points.windows(2) {
            let (start, end) = (pair[0], pair[1]);
            let length = dist2(start, end);
            if length == 0.0 {
                continue;
            }
            let (tx, ty) = ((end.0 - start.0) / length, (end.1 - start.1) / length);
            let (nx, ny) = (-ty * radius, tx * radius);
            tangents.push((tx, ty));
            polygon(vec![
                (start.0 + nx, start.1 + ny),
                (end.0 + nx, end.1 + ny),
                (end.0 - nx, end.1 - ny),
                (start.0 - nx, start.1 - ny),
            ])?;
        }
        let vertices: Vec<usize> = if *closed {
            (0..points.len() - 1).collect()
        } else {
            (1..points.len() - 1).collect()
        };
        let count = tangents.len() as isize;
        for index in vertices {
            let vertex = points[index];
            if draw.join == 1 {
                polygon(circle_points(vertex))?;
                continue;
            }
            // Python indexing: tangents[index - 1] wraps at -1.
            let before = tangents[((index as isize - 1).rem_euclid(count)) as usize];
            let after = tangents[(index as isize % count) as usize];
            let cross = before.0 * after.1 - before.1 * after.0;
            if cross.abs() < 1e-12 {
                continue;
            }
            let sign = if cross > 0.0 { -1.0 } else { 1.0 };
            let first = (
                vertex.0 - before.1 * radius * sign,
                vertex.1 + before.0 * radius * sign,
            );
            let last = (
                vertex.0 - after.1 * radius * sign,
                vertex.1 + after.0 * radius * sign,
            );
            let mut join = vec![vertex, first, last];
            if draw.join == 0 {
                let delta = (last.0 - first.0, last.1 - first.1);
                let amount = (delta.0 * after.1 - delta.1 * after.0) / cross;
                let miter = (first.0 + amount * before.0, first.1 + amount * before.1);
                if dist2(vertex, miter) <= 4.0 * radius {
                    join = vec![vertex, first, miter, last];
                }
            }
            polygon(join)?;
        }
        if !closed {
            let ends = [
                (points[0], tangents[0], -1.0),
                (points[points.len() - 1], tangents[tangents.len() - 1], 1.0),
            ];
            for (point, tangent, direction) in ends {
                if draw.cap == 1 {
                    polygon(circle_points(point))?;
                } else if draw.cap == 2 {
                    let (tx, ty) = (
                        tangent.0 * radius * direction,
                        tangent.1 * radius * direction,
                    );
                    let (nx, ny) = (-tangent.1 * radius, tangent.0 * radius);
                    polygon(vec![
                        (point.0 + nx, point.1 + ny),
                        (point.0 + nx + tx, point.1 + ny + ty),
                        (point.0 - nx + tx, point.1 - ny + ty),
                        (point.0 - nx, point.1 - ny),
                    ])?;
                }
            }
        }
    }
    Ok(polygons)
}
