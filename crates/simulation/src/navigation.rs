//! Static environment collision index. Queries touch nearby cells, not the entire scan.
use bevy::prelude::*;
use std::collections::HashMap;

#[derive(Resource, Default)]
pub(crate) struct Navigation {
    triangles: Vec<Triangle3d>,
    cells: HashMap<(i32, i32), Vec<usize>>,
}
impl Navigation {
    pub fn insert(&mut self, triangle: Triangle3d) {
        let min = triangle.vertices.iter().copied().reduce(Vec3::min).unwrap();
        let max = triangle.vertices.iter().copied().reduce(Vec3::max).unwrap();
        let index = self.triangles.len();
        self.triangles.push(triangle);
        for x in min.x.floor() as i32..=max.x.floor() as i32 {
            for z in min.z.floor() as i32..=max.z.floor() as i32 {
                self.cells.entry((x, z)).or_default().push(index);
            }
        }
    }
    pub fn raycast(
        &self,
        origin: Vec3,
        direction: Vec3,
        distance: f32,
        floor: bool,
    ) -> Option<Vec3> {
        let end = origin + direction * distance;
        let min = origin.min(end);
        let max = origin.max(end);
        let mut nearest = distance;
        let mut found = false;
        for x in min.x.floor() as i32..=max.x.floor() as i32 {
            for z in min.z.floor() as i32..=max.z.floor() as i32 {
                for &index in self.cells.get(&(x, z)).into_iter().flatten() {
                    let [a, b, c] = self.triangles[index].vertices;
                    let e1 = b - a;
                    let e2 = c - a;
                    if floor && e1.cross(e2).normalize_or_zero().y <= 0.5 {
                        continue;
                    }
                    let p = direction.cross(e2);
                    let determinant = e1.dot(p);
                    if determinant.abs() < 1e-8 {
                        continue;
                    }
                    let inverse = determinant.recip();
                    let t = origin - a;
                    let u = t.dot(p) * inverse;
                    if !(0. ..=1.).contains(&u) {
                        continue;
                    }
                    let q = t.cross(e1);
                    let v = direction.dot(q) * inverse;
                    if v < 0. || u + v > 1. {
                        continue;
                    }
                    let hit = e2.dot(q) * inverse;
                    if hit >= 0. && hit <= nearest {
                        nearest = hit;
                        found = true;
                    }
                }
            }
        }
        found.then_some(origin + direction * nearest)
    }
}
