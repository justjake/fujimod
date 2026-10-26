//! Horizontal integer-radius means are independent of fractional focus,
//! exposure and film parameters. Retain a bounded set for the selected source
//! columns and vertical halo; the vertical pass still rounds the blended means.
use super::{
    readout::Coordinates,
    scene::{Scene, blend},
};
use std::{
    collections::VecDeque,
    sync::{Arc, Weak},
};

const CACHE_PIXELS: usize = 4 * 1024 * 1024;
struct Plane {
    scene: Arc<Scene>,
    coordinates: Arc<Coordinates>,
    radius: usize,
    begin: usize,
    end: usize,
    pixels: Vec<[u16; 3]>,
    first: Vec<[u16; 3]>,
    last: Vec<[u16; 3]>,
}
impl Plane {
    fn new(
        scene: Arc<Scene>,
        coordinates: Arc<Coordinates>,
        radius: usize,
        begin: usize,
        end: usize,
    ) -> Self {
        let mut pixels = Vec::with_capacity(coordinates.xs.len() * (end - begin));
        for y in begin..end {
            let row = scene.row(y);
            for &x in &coordinates.xs {
                pixels.push(row.horizontal(x, radius, 0));
            }
        }
        let first_row = scene.row(0);
        let first = coordinates
            .xs
            .iter()
            .map(|&x| first_row.horizontal(x, radius, 0))
            .collect();
        let last_row = scene.row(scene.dimensions[1] - 1);
        let last = coordinates
            .xs
            .iter()
            .map(|&x| last_row.horizontal(x, radius, 0))
            .collect();
        drop(first_row);
        drop(last_row);
        Self {
            scene,
            coordinates,
            radius,
            begin,
            end,
            pixels,
            first,
            last,
        }
    }
    fn len(&self) -> usize {
        self.pixels.len() + self.first.len() + self.last.len()
    }
    #[inline(always)]
    fn sample(&self, column: usize, y: usize) -> [u16; 3] {
        if y < self.begin {
            debug_assert_eq!(y, 0);
            self.first[column]
        } else if y >= self.end {
            debug_assert_eq!(y, self.scene.dimensions[1] - 1);
            self.last[column]
        } else {
            self.pixels[(y - self.begin) * self.coordinates.xs.len() + column]
        }
    }
}
pub(super) struct Prepared {
    inner: Arc<Plane>,
    outer: Arc<Plane>,
    fraction: u32,
}
impl Prepared {
    #[inline(always)]
    pub fn sample(&self, column: usize, y: usize) -> [u16; 3] {
        let inner = self.inner.sample(column, y);
        if self.fraction == 0 {
            return inner;
        }
        let outer = self.outer.sample(column, y);
        std::array::from_fn(|c| blend(u64::from(inner[c]), self.fraction, u64::from(outer[c])))
    }
}
struct Request {
    scene: Weak<Scene>,
    coordinates: Weak<Coordinates>,
    radius: usize,
}
pub(super) struct Cache {
    entries: VecDeque<Arc<Plane>>,
    pixels: usize,
    budget: usize,
    last: VecDeque<Request>,
}
impl Default for Cache {
    fn default() -> Self {
        Self {
            entries: VecDeque::new(),
            pixels: 0,
            budget: CACHE_PIXELS,
            last: VecDeque::new(),
        }
    }
}
impl Cache {
    pub fn prepare(
        &mut self,
        scene: Arc<Scene>,
        coordinates: Arc<Coordinates>,
        radius: [usize; 2],
        fraction: u32,
    ) -> Option<Prepared> {
        // Do not extend the lifetime of a scene prefix beyond its own cache
        // policy. The retained RGB16 means use at most 24 MiB per projection.
        if scene.dimensions[0] * scene.dimensions[1] > super::CACHE_PIXELS {
            self.entries.clear();
            self.pixels = 0;
            self.last.clear();
            return None;
        }
        if self
            .entries
            .front()
            .is_some_and(|p| !p.scene.frame.same_pixels(&scene.frame))
        {
            self.entries.clear();
            self.pixels = 0;
            self.last.clear();
        }
        if coordinates.xs.is_empty() {
            return None;
        }
        let (&first, &last) = (coordinates.ys.first()?, coordinates.ys.last()?);
        let begin = first.saturating_sub(radius[1] + 1);
        let end = (last + radius[1] + 2).min(scene.dimensions[1]);
        // Two retained planes fit the limit even while both are borrowed.
        // Large readouts keep the original bounded block scratch algorithm.
        if (radius[0] == 0 && fraction == 0)
            || coordinates.xs.len() * (end - begin + 2) > self.budget / 2
        {
            return None;
        }
        // Coarse focus jumps often consume a radius only once. Avoid building
        // two full mean planes on a miss unless that radius repeats. An
        // adjacent cached box already repays one pass and is reused at once.
        let request = Request {
            scene: Arc::downgrade(&scene),
            coordinates: Arc::downgrade(&coordinates),
            radius: radius[0],
        };
        let same_readout = |old: &Request| {
            Weak::ptr_eq(&old.scene, &request.scene)
                && Weak::ptr_eq(&old.coordinates, &request.coordinates)
        };
        let repeated = self.last.iter().any(|old| {
            old.radius == radius[0]
                && Weak::ptr_eq(&old.scene, &request.scene)
                && Weak::ptr_eq(&old.coordinates, &request.coordinates)
        });
        self.last.retain(|old| !same_readout(old));
        if self.last.len() == 16 {
            self.last.pop_front();
        }
        self.last.push_back(request);
        if !repeated
            && !self.entries.iter().any(|p| {
                Arc::ptr_eq(&p.scene, &scene)
                    && Arc::ptr_eq(&p.coordinates, &coordinates)
                    && (p.radius == radius[0] || p.radius == radius[0] + 1)
                    && p.begin == begin
                    && p.end == end
            })
        {
            return None;
        }
        let inner = self.plane(scene.clone(), coordinates.clone(), radius[0], begin, end);
        let outer = if fraction == 0 {
            inner.clone()
        } else {
            self.plane(scene, coordinates, radius[0] + 1, begin, end)
        };
        Some(Prepared {
            inner,
            outer,
            fraction,
        })
    }
    fn plane(
        &mut self,
        scene: Arc<Scene>,
        coordinates: Arc<Coordinates>,
        radius: usize,
        begin: usize,
        end: usize,
    ) -> Arc<Plane> {
        if let Some(index) = self.entries.iter().position(|p| {
            Arc::ptr_eq(&p.scene, &scene)
                && Arc::ptr_eq(&p.coordinates, &coordinates)
                && p.radius == radius
                && p.begin == begin
                && p.end == end
        }) {
            let plane = self.entries.remove(index).unwrap();
            self.entries.push_back(plane.clone());
            return plane;
        }
        // Evict before allocation: replacing a radius does not need a third
        // full plane on top of the two admitted by the cache budget.
        let pixels = coordinates.xs.len() * (end - begin + 2);
        while self.pixels + pixels > self.budget
            || self.retained_scene_pixels(&scene) > super::CACHE_PIXELS
        {
            self.pixels -= self.entries.pop_front().unwrap().len();
        }
        let plane = Arc::new(Plane::new(scene, coordinates, radius, begin, end));
        self.pixels += plane.len();
        self.entries.push_back(plane.clone());
        plane
    }
    fn retained_scene_pixels(&self, scene: &Arc<Scene>) -> usize {
        let mut scenes = vec![scene];
        for p in &self.entries {
            if !scenes.iter().any(|old| Arc::ptr_eq(old, &p.scene)) {
                scenes.push(&p.scene);
            }
        }
        scenes
            .iter()
            .map(|s| s.dimensions[0] * s.dimensions[1])
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::super::readout::project;
    use super::*;
    use fujimod_emulator_imaging::scene::{Frame, FramePixels};
    fn source() -> Arc<Scene> {
        let bytes: Vec<u8> = (0..8 * 5 * 3).map(|i| (i * 79) as u8).collect();
        let frame = Frame::new(8, 5, FramePixels::Rgb24(bytes.into())).unwrap();
        Arc::new(Scene::new(Arc::new(frame)).unwrap())
    }
    #[test]
    fn cached_integer_means_preserve_every_fractional_box_pass_and_window() {
        let scene = source();
        for (grid, window) in [
            ([8, 5], [0, 0, 8, 5]),
            ([31, 19], [3, 5, 7, 4]),
            ([3, 2], [1, 0, 1, 2]),
        ] {
            let coordinates = Arc::new(Coordinates::new([8, 5], grid, window).unwrap());
            let mut cache = Cache::default();
            for radius in [0, 1, 2, 5, 8] {
                for fraction in [0, 1, 16384, 32768, 65535] {
                    let prepared =
                        cache.prepare(scene.clone(), coordinates.clone(), [radius; 2], fraction);
                    assert_eq!(
                        project(&scene, &coordinates, radius, fraction, prepared.as_ref()),
                        project(&scene, &coordinates, radius, fraction, None)
                    );
                }
            }
        }
    }
    #[test]
    fn large_readouts_fall_back_while_small_programmed_windows_fit_the_budget() {
        let scene = source();
        let full = Arc::new(Coordinates::full([8, 5]));
        let mut cache = Cache {
            budget: 64,
            ..Cache::default()
        };
        assert!(cache.prepare(scene.clone(), full, [1; 2], 123).is_none());
        assert!(cache.entries.is_empty());
        let window = Arc::new(Coordinates::new([8, 5], [8, 5], [3, 2, 1, 1]).unwrap());
        assert!(
            cache
                .prepare(scene.clone(), window.clone(), [1; 2], 123)
                .is_none()
        );
        assert!(cache.entries.is_empty());
        let prepared = cache
            .prepare(scene.clone(), window.clone(), [1; 2], 123)
            .unwrap();
        assert_eq!(
            project(&scene, &window, 1, 123, Some(&prepared)),
            project(&scene, &window, 1, 123, None)
        );
        assert!(cache.pixels <= cache.budget);
        assert!(cache.entries.iter().all(|p| p.pixels.len() < 8 * 5));
    }
    #[test]
    fn coarse_focus_jumps_use_scratch_until_a_new_radius_repeats() {
        let scene = source();
        let coordinates = Arc::new(Coordinates::full([8, 5]));
        let mut cache = Cache::default();
        assert!(
            cache
                .prepare(scene.clone(), coordinates.clone(), [1; 2], 123)
                .is_none()
        );
        assert!(cache.entries.is_empty());
        let first = cache
            .prepare(scene.clone(), coordinates.clone(), [1; 2], 123)
            .unwrap();
        for radius in [5, 9, 13] {
            assert!(
                cache
                    .prepare(scene.clone(), coordinates.clone(), [radius; 2], 123)
                    .is_none()
            );
            assert_eq!(cache.entries.len(), 2);
            assert!(Arc::ptr_eq(&cache.entries[0], &first.inner));
            assert!(Arc::ptr_eq(&cache.entries[1], &first.outer));
        }
        let repeated = cache
            .prepare(scene.clone(), coordinates.clone(), [13; 2], 456)
            .unwrap();
        assert_eq!(
            project(&scene, &coordinates, 13, 456, Some(&repeated)),
            project(&scene, &coordinates, 13, 456, None)
        );
    }
    #[test]
    fn fractional_focus_and_adjacent_radii_reuse_planes_with_bounded_eviction() {
        let scene = source();
        let coordinates = Arc::new(Coordinates::full([8, 5]));
        let mut cache = Cache {
            budget: 3 * 8 * 7,
            ..Cache::default()
        };
        assert!(
            cache
                .prepare(scene.clone(), coordinates.clone(), [1; 2], 123)
                .is_none()
        );
        let a = cache
            .prepare(scene.clone(), coordinates.clone(), [1; 2], 123)
            .unwrap();
        let b = cache
            .prepare(scene.clone(), coordinates.clone(), [1; 2], 567)
            .unwrap();
        assert!(Arc::ptr_eq(&a.inner, &b.inner));
        assert!(Arc::ptr_eq(&a.outer, &b.outer));
        let c = cache
            .prepare(scene.clone(), coordinates.clone(), [2; 2], 123)
            .unwrap();
        assert!(Arc::ptr_eq(&a.outer, &c.inner));
        for radius in 3..20 {
            cache
                .prepare(scene.clone(), coordinates.clone(), [radius; 2], 123)
                .unwrap();
            assert!(cache.pixels <= cache.budget);
        }
        let replacement = source();
        assert!(
            cache
                .prepare(replacement.clone(), coordinates.clone(), [1; 2], 123)
                .is_none()
        );
        assert!(cache.entries.is_empty());
        let d = cache
            .prepare(replacement.clone(), coordinates.clone(), [1; 2], 123)
            .unwrap();
        assert!(
            cache
                .entries
                .iter()
                .all(|p| Arc::ptr_eq(&p.scene, &replacement))
        );
        assert!(!Arc::ptr_eq(&a.inner, &d.inner));
        let sparse = Arc::new(Coordinates::new([8, 5], [8, 5], [0, 0, 1, 1]).unwrap());
        assert!(
            cache
                .prepare(replacement.clone(), sparse.clone(), [1; 2], 123)
                .is_none()
        );
        let sparse_projection = cache
            .prepare(replacement, sparse.clone(), [1; 2], 123)
            .unwrap();
        assert!(Arc::ptr_eq(&sparse_projection.inner.coordinates, &sparse));
        assert!(Arc::ptr_eq(&sparse_projection.outer.coordinates, &sparse));
        assert!(cache.pixels <= cache.budget);
    }

    #[test]
    fn anisotropic_windows_share_integer_means_without_crossing_geometry() {
        use super::super::readout::{project_axes, project_prepared_axes};
        let scene = source();
        let coordinates = Arc::new(Coordinates::full([8, 5]));
        let alternate = Arc::new(Coordinates::new([8, 5], [13, 9], [3, 2, 4, 3]).unwrap());
        let mut cache = Cache::default();
        for radius in [[0, 2], [1, 0], [2, 3], [5, 1]] {
            for fraction in [[1, 65535], [32768, 0], [65535, 16384]] {
                for coords in [&coordinates, &alternate, &coordinates] {
                    let mut prepared =
                        cache.prepare(scene.clone(), coords.clone(), radius, fraction[0]);
                    if prepared.is_none() {
                        prepared =
                            cache.prepare(scene.clone(), coords.clone(), radius, fraction[0]);
                    }
                    assert!(prepared.is_some());
                    assert_eq!(
                        project_prepared_axes(&scene, coords, radius, fraction, prepared.as_ref()),
                        project_axes(&scene, coords, radius, fraction)
                    );
                    assert!(cache.pixels <= cache.budget);
                    assert!(cache.retained_scene_pixels(&scene) <= super::super::CACHE_PIXELS);
                }
            }
        }
    }
}
