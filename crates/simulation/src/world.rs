//! Physical state and observer controls. No browser, renderer, or firmware knowledge.
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionMode {
    Walk,
    Inspect,
}

/// Declared optical inputs, supplied by the selected camera/scenario.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Optics {
    pub focus_distance: f32,
    pub aperture: f32,
    pub focal_length: f32,
    pub sensor_height: f32,
}
impl Default for Optics {
    fn default() -> Self {
        Self {
            focus_distance: 3.,
            aperture: 2.8,
            focal_length: 0.05,
            sensor_height: 0.024,
        }
    }
}
impl Optics {
    pub fn is_valid(&self) -> bool {
        [
            self.focus_distance,
            self.aperture,
            self.focal_length,
            self.sensor_height,
        ]
        .iter()
        .all(|v| v.is_finite() && *v > 0.)
    }
}

#[derive(Resource, Clone, Debug)]
pub struct Simulation {
    pub position: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub height: f32,
    pub camera_height: f32,
    pub mode: InteractionMode,
    pub elapsed: f64,
    pub movement: Vec2,
    pub orbit: Vec2,
    pub distance: f32,
    pub transition: f32,
    pub optics: Optics,
}
impl Default for Simulation {
    fn default() -> Self {
        Self {
            position: Vec3::ZERO,
            yaw: 0.,
            pitch: 0.,
            height: 1.75,
            camera_height: 1.45,
            mode: InteractionMode::Inspect,
            elapsed: 0.,
            movement: Vec2::ZERO,
            orbit: Vec2::new(0., 0.08),
            distance: 0.13,
            transition: 1.,
            optics: Optics::default(),
        }
    }
}
impl Simulation {
    /// Advance explicitly: real elapsed time interactively, scenario time in tests.
    /// Guest execution speed never scales this interval.
    pub fn advance(&mut self, seconds: f32) {
        assert!(seconds.is_finite() && seconds >= 0.);
        self.elapsed += f64::from(seconds);
        if self.mode == InteractionMode::Walk {
            let direction = self.movement.clamp_length_max(1.);
            self.position += Quat::from_rotation_y(self.yaw)
                * Vec3::new(direction.x, 0., -direction.y)
                * 1.4
                * seconds;
        }
        let target = if self.mode == InteractionMode::Inspect {
            1.
        } else {
            0.
        };
        self.transition += (target - self.transition).clamp(-seconds / 0.25, seconds / 0.25);
    }
    pub fn toggle(&mut self) {
        self.mode = match self.mode {
            InteractionMode::Walk => InteractionMode::Inspect,
            InteractionMode::Inspect => InteractionMode::Walk,
        };
        self.movement = Vec2::ZERO;
        if self.mode == InteractionMode::Inspect {
            self.orbit = Vec2::new(0., 0.08);
        }
    }
    pub fn look(&mut self, delta: Vec2) {
        if self.mode == InteractionMode::Walk {
            self.yaw -= delta.x * 0.0025;
            self.pitch = (self.pitch - delta.y * 0.0025).clamp(-1.35, 1.35);
        }
    }
    pub fn orbit(&mut self, delta: Vec2) {
        if self.mode == InteractionMode::Inspect {
            self.orbit.x -= delta.x * 0.006;
            self.orbit.y = (self.orbit.y + delta.y * 0.006).clamp(-1.4, 1.4);
        }
    }
    pub fn rotation(&self) -> Quat {
        Quat::from_euler(EulerRot::YXZ, self.yaw, self.pitch, 0.)
    }
    fn eye_position(&self) -> Vec3 {
        // Standing height includes the crown of the head, not the eyes.
        self.position + Vec3::Y * (self.height - 0.11)
    }
    pub fn camera_pose(&self) -> Transform {
        let held = Vec3::new(0.055, self.camera_height - (self.height - 0.11), -0.40);
        Transform::from_translation(self.eye_position() + self.rotation() * held)
            .with_rotation(self.rotation())
    }
    pub fn observer_pose(&self, vertical_fov: f32, aspect: f32) -> Transform {
        let physical = self.camera_pose();
        let eye = self.eye_position();
        let held = self.rotation().inverse() * (physical.translation - eye);
        let tangent = (vertical_fov * 0.5).tan();
        // The head glances down at the held body. Keep its center slightly
        // right of center and its nearest bottom corner 5% above the edge.
        // Both views still observe the very same physical camera in the world.
        let head_yaw = (0.2 * tangent * aspect).atan() - held.x.atan2(-held.z);
        let bottom = Quat::from_rotation_y(-head_yaw)
            * (held + Vec3::new(0.0565 * head_yaw.signum(), -0.035, 0.025));
        let head_pitch = bottom.y.atan2(-bottom.z) + (0.9 * tangent).atan();
        let walking_rotation =
            self.rotation() * Quat::from_euler(EulerRot::YXZ, head_yaw, head_pitch, 0.);
        let offset = Quat::from_euler(EulerRot::YXZ, self.orbit.x, -self.orbit.y, 0.)
            * Vec3::Z
            * self.distance;
        let close = physical.translation + physical.rotation * offset;
        let inspecting =
            Transform::from_translation(close).looking_at(physical.translation, Vec3::Y);
        let a = self.transition * self.transition * (3. - 2. * self.transition);
        Transform::from_translation(eye.lerp(inspecting.translation, a))
            .with_rotation(walking_rotation.slerp(inspecting.rotation, a))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn walking_keeps_the_physical_body_above_the_lower_edge_across_viewports_and_look_angles() {
        for aspect in [0.6, 1., 1.3, 1.8, 2.4] {
            for pitch in [-1.3, 0., 1.3] {
                let s = Simulation {
                    mode: InteractionMode::Walk,
                    transition: 0.,
                    yaw: 1.2,
                    pitch,
                    ..default()
                };
                let fov = 45_f32.to_radians();
                let observer = s.observer_pose(fov, aspect);
                let physical = s.camera_pose();
                let project = |p: Vec3| {
                    let p = observer.rotation.inverse()
                        * (physical.transform_point(p) - observer.translation);
                    assert!(p.z < -0.005);
                    Vec2::new(
                        0.5 + p.x / (-p.z * (fov * 0.5).tan() * aspect) * 0.5,
                        0.5 - p.y / (-p.z * (fov * 0.5).tan()) * 0.5,
                    )
                };
                let center = project(Vec3::ZERO);
                assert!(
                    (0.53..0.66).contains(&center.x),
                    "center: {center:?}, aspect: {aspect}"
                );
                let mut bottom = 0_f32;
                for x in [-0.0565, 0.0565] {
                    for y in [-0.035, 0.035] {
                        for z in [-0.025, 0.025] {
                            let p = project(Vec3::new(x, y, z));
                            assert!(
                                (0. ..=1.).contains(&p.x) && (0. ..=1.).contains(&p.y),
                                "body clipped: {p:?}"
                            );
                            bottom = bottom.max(p.y);
                        }
                    }
                }
                assert!((bottom - 0.95).abs() < 0.001, "bottom: {bottom}");
                assert_eq!(physical, s.camera_pose());
            }
        }
    }
    #[test]
    fn inspecting_changes_only_observer_even_during_transition() {
        let mut s = Simulation {
            mode: InteractionMode::Walk,
            transition: 0.,
            ..default()
        };
        s.look(Vec2::new(100., -40.));
        let camera = s.camera_pose();
        let observer = s.observer_pose(45_f32.to_radians(), 1.5);
        s.toggle();
        s.advance(0.1);
        s.orbit(Vec2::new(80., 30.));
        assert_eq!(camera, s.camera_pose());
        assert_ne!(observer, s.observer_pose(45_f32.to_radians(), 1.5));
        s.advance(1.);
        assert_eq!(camera, s.camera_pose());
    }
    #[test]
    fn explicit_time_replays_independently_of_guest_speed_and_display_rate() {
        let mut a = Simulation {
            mode: InteractionMode::Walk,
            movement: Vec2::Y,
            ..default()
        };
        let mut b = a.clone();
        for _ in 0..60 {
            a.advance(1. / 60.);
        }
        for _ in 0..30 {
            b.advance(1. / 30.);
        }
        assert!(a.position.distance(b.position) < 0.00001);
        assert!((a.position.z + 1.4).abs() < 0.00001);
    }
    #[test]
    fn diagonal_speed_and_inspect_movement_are_bounded() {
        let mut s = Simulation {
            mode: InteractionMode::Walk,
            movement: Vec2::ONE,
            ..default()
        };
        s.advance(1.);
        assert!((s.position.length() - 1.4).abs() < 0.00001);
        s.toggle();
        let p = s.position;
        s.movement = Vec2::ONE;
        s.advance(10.);
        assert_eq!(s.position, p);
    }
}
