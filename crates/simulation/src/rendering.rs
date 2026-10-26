//! One Bevy world renders both the sensor and the observer. Camera-body entities
//! occupy a separate render layer and can never appear in the sensor image.
use crate::{InteractionMode, Simulation, navigation::Navigation, world::Optics};
#[cfg(not(target_arch = "wasm32"))]
use bevy::post_process::dof::DepthOfField;
use bevy::{
    anti_alias::fxaa::Fxaa,
    asset::RenderAssetUsages,
    camera::{RenderTarget, primitives::Aabb, visibility::RenderLayers},
    ecs::system::{SystemParam, SystemState},
    gltf::{GltfLoaderSettings, convert_coordinates::GltfConvertCoordinates},
    image::{ImageSampler, ImageSamplerDescriptor},
    mesh::VertexAttributeValues,
    picking::mesh_picking::ray_cast::{MeshRayCast, MeshRayCastSettings, RayCastVisibility},
    prelude::*,
    render::{
        gpu_readback::{ReadbackComplete, ReadbackOnce},
        render_resource::{TextureFormat, TextureUsages},
        view::{DebandDither, Tonemapping},
    },
    world_serialization::WorldInstanceReady,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
};

pub const SENSOR_WIDTH: u32 = 768;
pub const SENSOR_HEIGHT: u32 = 512;

#[derive(Clone, Debug, Deserialize)]
pub struct MaterialDefinition {
    pub color: [f32; 3],
    pub metallic: f32,
    pub roughness: f32,
}
#[derive(Clone, Debug, Deserialize)]
pub struct ModelDefinition {
    pub url: String,
    pub yaw: f32,
    pub physical_width: f32,
    pub optics: Optics,
    #[serde(default)]
    pub materials: BTreeMap<String, MaterialDefinition>,
    pub hidden: Vec<String>,
    pub screen: String,
    pub inputs: BTreeMap<String, Value>,
    #[serde(default)]
    pub rotaries: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub order: Vec<String>,
    #[serde(default)]
    pub regions: BTreeMap<String, BTreeMap<String, String>>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct EnvironmentDefinition {
    #[serde(default = "sky_background")]
    pub background: [f32; 3],
    pub id: String,
    pub scene: String,
    #[serde(default)]
    pub translation: [f32; 3],
    #[serde(default)]
    pub rotation: [f32; 3],
    pub scale: f32,
    pub spawn: [f32; 3],
    #[serde(default)]
    pub yaw: f32,
    #[serde(default)]
    pub unlit: bool,
}
fn sky_background() -> [f32; 3] {
    [0.55, 0.65, 0.75]
}
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Command {
    Model {
        data: ModelDefinition,
    },
    Environment {
        data: EnvironmentDefinition,
    },
    Move {
        right: f32,
        forward: f32,
    },
    Look {
        x: f32,
        y: f32,
    },
    Orbit {
        x: f32,
        y: f32,
    },
    Zoom {
        amount: f32,
    },
    ToggleInspect,
    Face {
        lens: bool,
    },
    Highlight {
        ids: Vec<String>,
    },
    Buttons {
        ids: Vec<String>,
    },
    DefineRotary {
        id: String,
        aliases: Vec<String>,
    },
    Rotary {
        id: String,
        position: f32,
        immediate: bool,
    },
    Pick {
        request: u32,
        x: f32,
        y: f32,
        radius: f32,
    },
    Sensor {
        enabled: bool,
    },
    Viewport {
        width: u32,
        height: u32,
    },
    Observe {
        enabled: bool,
    },
    Optics {
        data: Optics,
    },
    PauseInput,
}
#[derive(Clone, Default)]
pub struct Bridge(Arc<Mutex<BridgeData>>);
#[derive(Default)]
struct BridgeData {
    commands: VecDeque<Command>,
    lcd: Option<(u32, u32, Vec<u8>)>,
    snapshot: Value,
    responses: BTreeMap<u32, Value>,
    sensor: Option<SensorFrame>,
}
#[derive(Clone, Debug)]
pub struct SensorFrame {
    pub rgb: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub sequence: u64,
    pub world_seconds: f64,
    pub camera_pose: Transform,
}
impl Bridge {
    pub fn send(&self, command: Command) {
        self.0.lock().unwrap().commands.push_back(command);
    }
    pub fn snapshot(&self) -> Value {
        self.0.lock().unwrap().snapshot.clone()
    }
    pub fn response(&self, request: u32) -> Option<Value> {
        self.0.lock().unwrap().responses.remove(&request)
    }
    pub fn take_sensor(&self) -> Option<SensorFrame> {
        self.0.lock().unwrap().sensor.take()
    }
    pub fn lcd(&self, width: u32, height: u32, rgba: Vec<u8>) -> Result<(), &'static str> {
        if width == 0
            || height == 0
            || width > 8192
            || height > 8192
            || rgba.len() != width as usize * height as usize * 4
        {
            return Err("Invalid LCD dimensions");
        }
        self.0.lock().unwrap().lcd = Some((width, height, rgba));
        Ok(())
    }
}
#[derive(Resource)]
struct Host(Bridge);
#[derive(Resource, Default)]
struct ViewerState {
    model: Option<ModelDefinition>,
    root: Option<Entity>,
    prepared: bool,
    environment: Option<Entity>,
    environment_ready: bool,
    navigation_pending: Option<Entity>,
    targets: BTreeMap<String, Entity>,
    rotaries: BTreeMap<String, Entity>,
    rotary_definitions: BTreeMap<String, Vec<String>>,
    highlighted: Vec<String>,
    projection_key: Vec<f32>,
    projected: BTreeMap<String, Value>,
    sensor_enabled: bool,
    viewport: Option<Vec2>,
    button_ids: Vec<String>,
    readback_pending: bool,
    readback_elapsed: f32,
    sensor_dirty: bool,
    grounded: bool,
    last_sensor_key: Vec<f32>,
    sequence: u64,
    error: Option<String>,
}
#[derive(Resource)]
struct Textures {
    lcd: Handle<Image>,
    sensor: Handle<Image>,
}
#[derive(Component)]
struct Observer;
#[derive(Resource)]
struct StudioLight(EnvironmentMapLight);
#[derive(Component)]
struct Sensor;
#[derive(Component)]
struct CameraBody;
#[derive(Component)]
struct ModelRoot;
#[derive(Component)]
struct EnvironmentRoot {
    unlit: bool,
}
#[derive(Component)]
struct Prepared;
#[derive(Component, Clone)]
struct ControlMesh {
    node: Entity,
    name: String,
    ids: Vec<String>,
}
#[derive(Component)]
struct CameraMesh;
#[derive(Component)]
struct OriginalEmission(LinearRgba);
#[derive(Component)]
struct Rotary {
    axis: Vec3,
    angle: f32,
    target: f32,
    immediate: bool,
}

#[derive(Clone, Copy)]
pub enum Presentation {
    Observer,
    SensorOnly,
}
#[derive(Resource)]
struct PresentationSettings(Presentation);
pub struct SimulationPlugin {
    pub bridge: Bridge,
    pub presentation: Presentation,
}
impl Plugin for SimulationPlugin {
    fn build(&self, app: &mut App) {
        #[cfg(not(target_arch = "wasm32"))]
        app.add_systems(Update, update_focus.after(update_poses));
        app.insert_resource(PresentationSettings(self.presentation))
            .insert_resource(Host(self.bridge.clone()))
            .init_resource::<Simulation>()
            .init_resource::<Navigation>()
            .init_resource::<ViewerState>()
            .add_systems(Startup, setup)
            .add_observer(scene_ready)
            .add_systems(
                Update,
                (
                    apply_studio_light,
                    process_commands,
                    advance,
                    update_rotaries,
                    update_lcd,
                    request_sensor,
                    update_poses,
                )
                    .chain(),
            )
            .add_systems(
                PostUpdate,
                publish.after(bevy::transform::TransformSystems::Propagate),
            );
    }
}
fn setup(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut state: ResMut<ViewerState>,
    presentation: Res<PresentationSettings>,
    simulation: Res<Simulation>,
) {
    let material = materials.add(StandardMaterial {
        base_color: Color::srgb(0.06, 0.065, 0.075),
        metallic: 0.35,
        perceptual_roughness: 0.6,
        ..default()
    });
    let body = commands
        .spawn((CameraBody, Transform::default(), Visibility::default()))
        .id();
    commands.spawn((
        Mesh3d(meshes.add(Cuboid::new(0.12, 0.07, 0.04))),
        MeshMaterial3d(material.clone()),
        Transform::default(),
        RenderLayers::layer(1),
        ChildOf(body),
    ));
    commands.spawn((
        Mesh3d(meshes.add(Cylinder::new(0.024, 0.025))),
        MeshMaterial3d(material),
        Transform::from_xyz(0., 0., -0.028)
            .with_rotation(Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)),
        RenderLayers::layer(1),
        ChildOf(body),
    ));
    state.root = Some(body);
    let mut lcd = Image::new_fill(
        bevy::render::render_resource::Extent3d {
            width: 720,
            height: 480,
            depth_or_array_layers: 1,
        },
        bevy::render::render_resource::TextureDimension::D2,
        &[12, 12, 14, 255],
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    lcd.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor::linear());
    let lcd = images.add(lcd);
    let mut sensor = Image::new_target_texture(
        SENSOR_WIDTH,
        SENSOR_HEIGHT,
        TextureFormat::Rgba8UnormSrgb,
        None,
    );
    sensor.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    let sensor = images.add(sensor);
    let observer = commands
        .spawn((
            Camera3d::default(),
            Observer,
            bevy::camera::Hdr,
            Msaa::Off,
            Fxaa::default(),
            Camera {
                clear_color: Color::srgb(0.035, 0.04, 0.05).into(),
                is_active: matches!(presentation.0, Presentation::Observer),
                ..default()
            },
            Projection::Perspective(PerspectiveProjection {
                fov: 45_f32.to_radians(),
                near: 0.005,
                far: 2000.,
                ..default()
            }),
            Tonemapping::ReinhardLuminance,
            DebandDither::Disabled,
            RenderLayers::layer(0).with(1),
            Transform::default(),
        ))
        .id();
    // Bound initial allocations before the web layout has a visible canvas.
    #[cfg(target_arch = "wasm32")]
    commands
        .entity(observer)
        .insert(bevy::camera::MainPassResolutionOverride(UVec2::new(
            640, 480,
        )));
    #[cfg(not(target_arch = "wasm32"))]
    let _ = observer;
    let sensor_camera = commands
        .spawn((
            Camera3d::default(),
            Sensor,
            Msaa::Off,
            Camera {
                order: -1,
                is_active: false,
                clear_color: Color::srgb(0.55, 0.65, 0.75).into(),
                ..default()
            },
            RenderTarget::Image(sensor.clone().into()),
            Projection::Perspective(PerspectiveProjection {
                fov: 2.
                    * (simulation.optics.sensor_height / (2. * simulation.optics.focal_length))
                        .atan(),
                aspect_ratio: 1.5,
                near: 0.01,
                far: 2000.,
                ..default()
            }),
            Tonemapping::None,
            DebandDither::Disabled,
            Transform::default(),
        ))
        .id();
    #[cfg(not(target_arch = "wasm32"))]
    commands.entity(sensor_camera).insert(DepthOfField {
        focal_distance: simulation.optics.focus_distance,
        aperture_f_stops: simulation.optics.aperture,
        sensor_height: simulation.optics.sensor_height,
        ..default()
    });
    #[cfg(target_arch = "wasm32")]
    let _ = sensor_camera;
    commands.spawn((
        DirectionalLight {
            illuminance: 2600.,
            shadow_maps_enabled: false,
            ..default()
        },
        Transform::from_xyz(3., 6., 4.).looking_at(Vec3::ZERO, Vec3::Y),
        RenderLayers::layer(0).with(1),
    ));
    let environment = crate::lighting::studio_light(&mut images);
    // Broad reflected sky/ground lights metal as well as diffuse surfaces.
    commands.spawn((
        DirectionalLight {
            illuminance: 1600.,
            color: Color::srgb(0.65, 0.75, 1.),
            shadow_maps_enabled: false,
            ..default()
        },
        Transform::from_xyz(-3., 2., -4.).looking_at(Vec3::ZERO, Vec3::Y),
        RenderLayers::layer(1),
    ));
    commands.spawn((
        DirectionalLight {
            illuminance: 1000.,
            shadow_maps_enabled: false,
            ..default()
        },
        Transform::from_xyz(-2., 3., 5.).looking_at(Vec3::ZERO, Vec3::Y),
        RenderLayers::layer(1),
    ));
    commands.insert_resource(StudioLight(environment));
    commands.insert_resource(GlobalAmbientLight {
        color: Color::WHITE,
        brightness: 25.,
        ..default()
    });
    commands.insert_resource(Textures { lcd, sensor });
}
fn apply_studio_light(
    mut commands: Commands,
    light: Res<StudioLight>,
    cameras: Query<Entity, (With<Observer>, Without<EnvironmentMapLight>)>,
) {
    for camera in &cameras {
        commands.entity(camera).insert(light.0.clone());
    }
}
fn load_scene(world: &mut World, path: String) -> Handle<bevy::world_serialization::WorldAsset> {
    world
        .resource::<AssetServer>()
        .load_builder()
        .with_settings(|settings: &mut GltfLoaderSettings| {
            settings.load_cameras = false;
            settings.load_lights = false;
            settings.load_animations = false;
            // Imported textures are immutable. Retain mesh CPU data for picking
            // and navigation, but release texture pixels after GPU upload.
            settings.load_materials = RenderAssetUsages::RENDER_WORLD;
            settings.convert_coordinates = Some(GltfConvertCoordinates::default());
        })
        .load(GltfAssetLabel::Scene(0).from_asset(path))
}
fn process_commands(world: &mut World) {
    let bridge = world.resource::<Host>().0.clone();
    let commands: Vec<_> = bridge.0.lock().unwrap().commands.drain(..).collect();
    for command in commands {
        match command {
            Command::Model { data } => {
                if !data.physical_width.is_finite()
                    || data.physical_width <= 0.
                    || !data.optics.is_valid()
                {
                    world.resource_mut::<ViewerState>().error =
                        Some("Camera requires positive physical dimensions and optics".into());
                    continue;
                }
                world.resource_mut::<Simulation>().optics = data.optics;
                let old = world.resource_mut::<ViewerState>().root.take();
                if let Some(old) = old {
                    world.despawn(old);
                }
                let handle = load_scene(world, data.url.clone());
                let body = world
                    .spawn((CameraBody, Transform::default(), Visibility::default()))
                    .id();
                world.spawn((
                    WorldAssetRoot(handle),
                    ModelRoot,
                    Transform::default(),
                    Visibility::default(),
                    ChildOf(body),
                ));
                let mut state = world.resource_mut::<ViewerState>();
                state.root = Some(body);
                state.model = Some(data);
                state.prepared = false;
                state.targets.clear();
                state.projection_key.clear();
                state.projected.clear();
                state.rotaries.clear();
                state.error = None;
            }
            Command::Environment { data } => {
                for mut camera in world
                    .query_filtered::<&mut Camera, Or<(With<Observer>, With<Sensor>)>>()
                    .iter_mut(world)
                {
                    camera.clear_color =
                        Color::srgb(data.background[0], data.background[1], data.background[2])
                            .into();
                }
                let old = world.resource_mut::<ViewerState>().environment.take();
                if let Some(old) = old {
                    world.despawn(old);
                }
                let handle = load_scene(world, data.scene);
                let e = world
                    .spawn((
                        WorldAssetRoot(handle),
                        EnvironmentRoot { unlit: data.unlit },
                        Transform::from_translation(Vec3::from_array(data.translation))
                            .with_rotation(Quat::from_euler(
                                EulerRot::XYZ,
                                data.rotation[0],
                                data.rotation[1],
                                data.rotation[2],
                            ))
                            .with_scale(Vec3::splat(data.scale)),
                    ))
                    .id();
                {
                    let mut s = world.resource_mut::<Simulation>();
                    s.position = Vec3::from_array(data.spawn);
                    s.yaw = data.yaw;
                }
                let mut state = world.resource_mut::<ViewerState>();
                state.environment = Some(e);
                state.environment_ready = false;
                state.navigation_pending = None;
                state.grounded = false;
                state.sensor_dirty = true;
                state.last_sensor_key.clear();
            }
            Command::Move { right, forward } => {
                if right.is_finite() && forward.is_finite() {
                    world.resource_mut::<Simulation>().movement = Vec2::new(right, forward);
                }
            }
            Command::Look { x, y } => {
                if x.is_finite() && y.is_finite() {
                    world.resource_mut::<Simulation>().look(Vec2::new(x, y));
                }
            }
            Command::Orbit { x, y } => {
                if x.is_finite() && y.is_finite() {
                    world.resource_mut::<Simulation>().orbit(Vec2::new(x, y));
                }
            }
            Command::Zoom { amount } => {
                if amount.is_finite() {
                    let mut s = world.resource_mut::<Simulation>();
                    s.distance = (s.distance * (amount * 0.001).exp()).clamp(0.08, 0.8);
                }
            }
            Command::ToggleInspect => world.resource_mut::<Simulation>().toggle(),
            Command::Face { lens } => {
                let mut s = world.resource_mut::<Simulation>();
                s.mode = InteractionMode::Inspect;
                s.movement = Vec2::ZERO;
                s.orbit = Vec2::new(if lens { std::f32::consts::PI } else { 0. }, 0.08);
            }
            Command::Highlight { ids } => highlight(world, ids),
            Command::Buttons { ids } => world.resource_mut::<ViewerState>().button_ids = ids,
            Command::Viewport { width, height } => {
                if width > 0 && height > 0 && width <= 8192 && height <= 8192 {
                    world.resource_mut::<ViewerState>().viewport =
                        Some(Vec2::new(width as f32, height as f32));
                    let observers: Vec<_> = world
                        .query_filtered::<Entity, With<Observer>>()
                        .iter(world)
                        .collect();
                    for e in observers {
                        world
                            .entity_mut(e)
                            .insert(bevy::camera::MainPassResolutionOverride(UVec2::new(
                                width, height,
                            )));
                    }
                }
            }
            Command::DefineRotary { id, aliases } => {
                world
                    .resource_mut::<ViewerState>()
                    .rotary_definitions
                    .insert(id.clone(), aliases.clone());
                define_rotary(world, id, aliases);
            }
            Command::Rotary {
                id,
                position,
                immediate,
            } => {
                if position.is_finite()
                    && let Some(e) = world.resource::<ViewerState>().rotaries.get(&id).copied()
                    && let Some(mut part) = world.get_mut::<Rotary>(e)
                {
                    part.target = position * std::f32::consts::PI / 12.;
                    part.immediate = immediate;
                }
            }
            Command::Pick {
                request,
                x,
                y,
                radius,
            } => {
                let result = pick(world, x, y, radius);
                bridge.0.lock().unwrap().responses.insert(request, result);
            }
            Command::Sensor { enabled } => {
                let mut state = world.resource_mut::<ViewerState>();
                state.sensor_enabled = enabled;
                state.last_sensor_key.clear()
            }
            Command::Observe { enabled } => {
                for mut camera in world
                    .query_filtered::<&mut Camera, With<Observer>>()
                    .iter_mut(world)
                {
                    camera.is_active = enabled;
                }
            }
            Command::Optics { data } => {
                if data.is_valid() {
                    world.resource_mut::<Simulation>().optics = data;
                }
            }
            Command::PauseInput => world.resource_mut::<Simulation>().movement = Vec2::ZERO,
        }
    }
}
fn scene_ready(event: On<WorldInstanceReady>, mut commands: Commands) {
    // Defer until the spawned hierarchy has propagated transforms.
    commands.entity(event.entity).insert(Prepared);
}
fn advance(world: &mut World) {
    let seconds = world.resource::<Time<Real>>().delta_secs();
    let simulation = world.resource::<Simulation>();
    let moving = simulation.mode == InteractionMode::Walk && simulation.movement != Vec2::ZERO;
    // Integrate all host time. Short collision steps prevent tunnelling without
    // making walking speed depend on the display or the firmware's guest speed.
    let steps = if moving {
        (seconds / 0.1).ceil().clamp(1., 100.) as u32
    } else {
        1
    };
    for _ in 0..steps {
        advance_step(world, seconds / steps as f32);
    }
}
fn advance_step(world: &mut World, seconds: f32) {
    let previous = world.resource::<Simulation>().position;
    world.resource_mut::<Simulation>().advance(seconds);
    let state = world.resource::<ViewerState>();
    if !state.environment_ready {
        return;
    }
    let initial = !state.grounded;
    let position = world.resource::<Simulation>().position;
    if !initial && position == previous {
        return;
    }
    let navigation = world.resource::<Navigation>();
    let movement = position - previous;
    let mut blocked = false;
    if let Ok(direction) = Dir3::new(movement) {
        for height in [0.35, 1.3] {
            blocked |= navigation
                .raycast(
                    previous + Vec3::Y * height,
                    *direction,
                    movement.length() + 0.2,
                    false,
                )
                .is_some();
        }
    }
    let origin = position + Vec3::Y * if initial { 2. } else { 0.4 };
    let ground = navigation.raycast(origin, Vec3::NEG_Y, if initial { 4. } else { 0.8 }, true);
    let mut simulation = world.resource_mut::<Simulation>();
    match ground {
        Some(ground) if !blocked && (initial || (ground.y - previous.y).abs() <= 0.4) => {
            simulation.position.y = ground.y
        }
        _ if !initial => simulation.position = previous,
        _ => {}
    }
    world.resource_mut::<ViewerState>().grounded |= ground.is_some();
}

type BodyTransforms<'w, 's> = Query<'w, 's, &'static mut Transform, With<CameraBody>>;
type ObserverTransforms<'w, 's> =
    Query<'w, 's, (&'static mut Transform, &'static Projection), With<Observer>>;
type SensorTransforms<'w, 's> = Query<
    'w,
    's,
    (
        &'static mut Transform,
        &'static mut Camera,
        &'static mut Projection,
    ),
    With<Sensor>,
>;

fn update_poses(
    s: Res<Simulation>,
    state: Res<ViewerState>,
    mut transforms: ParamSet<(BodyTransforms, ObserverTransforms, SensorTransforms)>,
) {
    for mut t in &mut transforms.p0() {
        *t = s.camera_pose();
    }
    for (mut t, projection) in &mut transforms.p1() {
        let mut observer = s.clone();
        if let Projection::Perspective(p) = projection {
            // Models normalize to 113 mm wide. Fit both axes with a small margin,
            // allowing for the rear surface being closer than the model's center.
            let half_height = 0.035_f32.max(0.0565 / p.aspect_ratio);
            observer.distance *= (half_height / (p.fov * 0.5).tan() + 0.015) * 1.06 / 0.13;
        }
        *t = match projection {
            Projection::Perspective(p) => observer.observer_pose(p.fov, p.aspect_ratio),
            _ => continue,
        };
    }
    for (mut t, mut camera, mut projection) in &mut transforms.p2() {
        *t = s.camera_pose();
        camera.is_active =
            state.sensor_enabled && state.environment_ready && state.readback_pending;
        if let Projection::Perspective(p) = &mut *projection {
            p.fov = 2. * (s.optics.sensor_height / (2. * s.optics.focal_length)).atan();
        }
    }
}
#[cfg(not(target_arch = "wasm32"))]
fn update_focus(s: Res<Simulation>, mut cameras: Query<&mut DepthOfField, With<Sensor>>) {
    for mut dof in &mut cameras {
        dof.focal_distance = s.optics.focus_distance;
        dof.aperture_f_stops = s.optics.aperture;
        dof.sensor_height = s.optics.sensor_height;
    }
}
fn update_rotaries(time: Res<Time<Real>>, mut parts: Query<(&mut Rotary, &mut Transform)>) {
    for (mut part, mut t) in &mut parts {
        let remaining = part.target - part.angle;
        part.angle += if part.immediate {
            remaining
        } else {
            remaining.clamp(-time.delta_secs() * 4.4, time.delta_secs() * 4.4)
        };
        t.rotation = Quat::from_axis_angle(part.axis, part.angle);
    }
}
fn update_lcd(host: Res<Host>, textures: Res<Textures>, mut images: ResMut<Assets<Image>>) {
    let Some((width, height, rgba)) = host.0.0.lock().unwrap().lcd.take() else {
        return;
    };
    let Some(mut image) = images.get_mut(&textures.lcd) else {
        return;
    };
    if image.width() != width || image.height() != height {
        image.resize(bevy::render::render_resource::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        });
    }
    image.data = Some(rgba);
}
type ChangedSensorMeshes<'w, 's> = Query<
    'w,
    's,
    Option<&'static RenderLayers>,
    (With<Mesh3d>, Without<CameraMesh>, Changed<GlobalTransform>),
>;

#[derive(SystemParam)]
struct SensorScene<'w, 's> {
    simulation: Res<'w, Simulation>,
    presentation: Res<'w, PresentationSettings>,
    changed_world: ChangedSensorMeshes<'w, 's>,
}

fn request_sensor(
    mut commands: Commands,
    time: Res<Time<Real>>,
    textures: Res<Textures>,
    mut state: ResMut<ViewerState>,
    host: Res<Host>,
    scene: SensorScene,
) {
    let mut key = scene
        .simulation
        .camera_pose()
        .to_matrix()
        .to_cols_array()
        .to_vec();
    key.extend([
        scene.simulation.optics.focus_distance,
        scene.simulation.optics.aperture,
        scene.simulation.optics.focal_length,
        scene.simulation.optics.sensor_height,
    ]);
    state.sensor_dirty |= key != state.last_sensor_key
        || scene
            .changed_world
            .iter()
            .any(|layers| layers.is_none_or(|layers| layers.intersects(&RenderLayers::layer(0))));
    if matches!(scene.presentation.0, Presentation::Observer) && !state.sensor_dirty {
        return;
    }
    state.readback_elapsed += time.delta_secs();
    if !state.sensor_enabled
        || !state.environment_ready
        || state.readback_pending
        || state.readback_elapsed < 1. / 15.
    {
        return;
    }
    state.readback_elapsed = 0.;
    state.last_sensor_key = key;
    state.sensor_dirty = false;
    state.readback_pending = true;
    state.sequence += 1;
    let sequence = state.sequence;
    let world_seconds = scene.simulation.elapsed;
    let pose = scene.simulation.camera_pose();
    let bridge = host.0.clone();
    commands
        .spawn(ReadbackOnce::texture(textures.sensor.clone()))
        .observe(
            move |event: On<ReadbackComplete>, mut state: ResMut<ViewerState>| {
                let row_bytes = (SENSOR_WIDTH * 4) as usize;
                let stride = row_bytes.div_ceil(256) * 256;
                let mut rgb = Vec::with_capacity((SENSOR_WIDTH * SENSOR_HEIGHT * 3) as usize);
                for row in event.data.chunks(stride).take(SENSOR_HEIGHT as usize) {
                    for pixel in row[..row_bytes].as_chunks::<4>().0 {
                        rgb.extend_from_slice(&pixel[..3]);
                    }
                }
                if rgb.len() == (SENSOR_WIDTH * SENSOR_HEIGHT * 3) as usize {
                    bridge.0.lock().unwrap().sensor = Some(SensorFrame {
                        rgb,
                        width: SENSOR_WIDTH,
                        height: SENSOR_HEIGHT,
                        sequence,
                        world_seconds,
                        camera_pose: pose,
                    });
                }
                state.readback_pending = false;
            },
        );
}
fn descendants(world: &World, root: Entity) -> Vec<Entity> {
    let mut all = Vec::new();
    let mut queue = vec![root];
    while let Some(e) = queue.pop() {
        all.push(e);
        if let Some(children) = world.get::<Children>(e) {
            queue.extend(children.iter());
        }
    }
    all
}
fn local_bounds(world: &World, node: Entity) -> Option<(Vec3, Vec3)> {
    let inverse = world.get::<GlobalTransform>(node)?.affine().inverse();
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    for e in descendants(world, node) {
        let Some(aabb) = world.get::<Aabb>(e) else {
            continue;
        };
        let Some(t) = world.get::<GlobalTransform>(e) else {
            continue;
        };
        let a = Vec3::from(aabb.center - aabb.half_extents);
        let b = Vec3::from(aabb.center + aabb.half_extents);
        let matrix = inverse * t.affine();
        for i in 0..8 {
            let p = matrix.transform_point3(Vec3::new(
                if i & 1 == 0 { a.x } else { b.x },
                if i & 2 == 0 { a.y } else { b.y },
                if i & 4 == 0 { a.z } else { b.z },
            ));
            min = min.min(p);
            max = max.max(p);
        }
    }
    if min.is_finite() && max.is_finite() {
        Some((min, max))
    } else {
        None
    }
}
fn prepare_model(world: &mut World, root: Entity) {
    let Some(definition) = world.resource::<ViewerState>().model.clone() else {
        return;
    };
    let Some((min, max)) = local_bounds(world, root) else {
        return;
    };
    let scale = definition.physical_width / (max - min).max_element();
    let rotation = Quat::from_rotation_y(definition.yaw);
    if let Some(mut transform) = world.get_mut::<Transform>(root) {
        transform.scale = Vec3::splat(scale);
        transform.rotation = rotation;
        transform.translation = rotation * (-(max + min) * 0.5 * scale);
    }
    let meshes: Vec<_> = descendants(world, root)
        .into_iter()
        .filter(|e| world.get::<Mesh3d>(*e).is_some())
        .collect();
    let lcd = world.resource::<Textures>().lcd.clone();
    for e in meshes {
        let mut node = e;
        let mut hidden = false;
        let mut screen = false;
        let mut control = None;
        let mut material_override = None;
        loop {
            if let Some(name) = world.get::<Name>(node) {
                let name = name.as_str();
                if material_override.is_none() {
                    material_override = definition.materials.get(name);
                }
                hidden |= definition.hidden.iter().any(|n| n == name);
                screen |= name == definition.screen;
                let mut ids: Vec<String> = definition
                    .inputs
                    .iter()
                    .filter_map(|(id, v)| {
                        let nodes = v
                            .as_array()
                            .or_else(|| v.get("nodes").and_then(Value::as_array));
                        nodes
                            .filter(|a| a.iter().any(|n| n.as_str() == Some(name)))
                            .map(|_| id.clone())
                    })
                    .collect();
                ids.sort_by_key(|id| {
                    definition
                        .order
                        .iter()
                        .position(|candidate| candidate == id)
                        .unwrap_or(usize::MAX)
                });
                if control.is_none() && !ids.is_empty() {
                    control = Some(ControlMesh {
                        node,
                        name: name.to_owned(),
                        ids,
                    });
                }
            }
            if node == root {
                break;
            }
            let Some(parent) = world.get::<ChildOf>(node) else {
                break;
            };
            node = parent.parent();
        }
        world
            .entity_mut(e)
            .insert((CameraMesh, RenderLayers::layer(1)));
        if hidden {
            world.entity_mut(e).insert(Visibility::Hidden);
        }
        if let Some(c) = control {
            for id in &c.ids {
                world
                    .resource_mut::<ViewerState>()
                    .targets
                    .entry(id.clone())
                    .or_insert(c.node);
            }
            world.entity_mut(e).insert(c);
        }
        if screen {
            let mesh = world.get::<Mesh3d>(e).unwrap().0.clone();
            let mut copied = world.resource::<Assets<Mesh>>().get(&mesh).unwrap().clone();
            if let Some(VertexAttributeValues::Float32x3(positions)) =
                copied.attribute(Mesh::ATTRIBUTE_POSITION)
            {
                let a = positions.iter().fold(Vec3::splat(f32::INFINITY), |a, p| {
                    a.min(Vec3::from_array(*p))
                });
                let b = positions
                    .iter()
                    .fold(Vec3::splat(f32::NEG_INFINITY), |b, p| {
                        b.max(Vec3::from_array(*p))
                    });
                let extent = b - a;
                let mut axes = [0, 1, 2];
                axes.sort_by(|a, b| extent[*b].total_cmp(&extent[*a]));
                let uv: Vec<[f32; 2]> = positions
                    .iter()
                    .map(|p| {
                        [
                            1. - (p[axes[0]] - a[axes[0]]) / extent[axes[0]],
                            1. - (p[axes[1]] - a[axes[1]]) / extent[axes[1]],
                        ]
                    })
                    .collect();
                copied.insert_attribute(Mesh::ATTRIBUTE_UV_0, uv);
            }
            let mesh = world.resource_mut::<Assets<Mesh>>().add(copied);
            let material = world
                .resource_mut::<Assets<StandardMaterial>>()
                .add(StandardMaterial {
                    base_color_texture: Some(lcd.clone()),
                    unlit: true,
                    cull_mode: None,
                    ..default()
                });
            world
                .entity_mut(e)
                .insert((Mesh3d(mesh), MeshMaterial3d(material)));
        } else if let Some(material) = world.get::<MeshMaterial3d<StandardMaterial>>(e).cloned() {
            let copied = world
                .resource::<Assets<StandardMaterial>>()
                .get(&material.0)
                .cloned();
            if let Some(mut copied) = copied {
                if let Some(material) = material_override {
                    copied.base_color =
                        Color::srgb(material.color[0], material.color[1], material.color[2]);
                    copied.metallic = material.metallic;
                    copied.perceptual_roughness = material.roughness;
                    copied.base_color_texture = None;
                    copied.metallic_roughness_texture = None;
                }
                let emission = copied.emissive;
                let handle = world.resource_mut::<Assets<StandardMaterial>>().add(copied);
                world
                    .entity_mut(e)
                    .insert((MeshMaterial3d(handle), OriginalEmission(emission)));
            }
        }
    }
    world.entity_mut(root).remove::<Prepared>();
    world.resource_mut::<ViewerState>().prepared = true;
}
fn build_navigation(world: &World, root: Entity) -> Navigation {
    let mut navigation = Navigation::default();
    for e in descendants(world, root) {
        let (Some(mesh), Some(transform)) =
            (world.get::<Mesh3d>(e), world.get::<GlobalTransform>(e))
        else {
            continue;
        };
        let Some(mesh) = world.resource::<Assets<Mesh>>().get(&mesh.0) else {
            continue;
        };
        if let Ok(triangles) = mesh.triangles() {
            for triangle in triangles {
                navigation.insert(Triangle3d {
                    vertices: triangle.vertices.map(|p| transform.transform_point(p)),
                });
            }
        } else if mesh.indices().is_none() {
            // glTF meshes without normals may be expanded for flat shading.
            if let Some(VertexAttributeValues::Float32x3(positions)) =
                mesh.attribute(Mesh::ATTRIBUTE_POSITION)
            {
                for triangle in positions.as_chunks::<3>().0 {
                    navigation.insert(Triangle3d::new(
                        transform.transform_point(Vec3::from_array(triangle[0])),
                        transform.transform_point(Vec3::from_array(triangle[1])),
                        transform.transform_point(Vec3::from_array(triangle[2])),
                    ));
                }
            }
        }
    }
    navigation
}
fn prepare_environment(world: &mut World, root: Entity) {
    let unlit = world.get::<EnvironmentRoot>(root).unwrap().unlit;
    for e in descendants(world, root) {
        if let Some(material) = world.get::<MeshMaterial3d<StandardMaterial>>(e).cloned()
            && let Some(mut m) = world
                .resource_mut::<Assets<StandardMaterial>>()
                .get_mut(&material.0)
        {
            m.unlit = unlit;
        }
    }
    world.entity_mut(root).remove::<Prepared>();
    world.resource_mut::<ViewerState>().navigation_pending = Some(root);
}
fn define_rotary(world: &mut World, id: String, aliases: Vec<String>) {
    if world.resource::<ViewerState>().rotaries.contains_key(&id) {
        return;
    }
    let Some(node) = world.resource::<ViewerState>().targets.get(&id).copied() else {
        return;
    };
    let Some((min, max)) = local_bounds(world, node) else {
        return;
    };
    let Some(parent) = world.get::<ChildOf>(node).map(ChildOf::parent) else {
        return;
    };
    let old = *world.get::<Transform>(node).unwrap();
    let size = max - min;
    let axis_index = if size.x < size.y && size.x < size.z {
        0
    } else if size.y < size.z {
        1
    } else {
        2
    };
    let axis = old.rotation * [Vec3::X, Vec3::Y, Vec3::Z][axis_index];
    let center = old.transform_point((min + max) * 0.5);
    let pivot = world
        .spawn((
            Transform::from_translation(center),
            Visibility::default(),
            ChildOf(parent),
            Rotary {
                axis,
                angle: 0.,
                target: 0.,
                immediate: false,
            },
        ))
        .id();
    let mut shifted = old;
    shifted.translation -= center;
    world.entity_mut(node).insert((ChildOf(pivot), shifted));
    let members = world
        .resource::<ViewerState>()
        .model
        .as_ref()
        .and_then(|d| d.rotaries.get(&id))
        .cloned()
        .unwrap_or_default();
    let siblings: Vec<_> = world
        .query::<(Entity, &Name, &ChildOf)>()
        .iter(world)
        .filter(|(_, name, p)| p.parent() == parent && members.iter().any(|m| m == name.as_str()))
        .map(|(e, _, _)| e)
        .collect();
    for sibling in siblings {
        let mut transform = *world.get::<Transform>(sibling).unwrap();
        transform.translation -= center;
        world
            .entity_mut(sibling)
            .insert((ChildOf(pivot), transform));
        for mesh in descendants(world, sibling) {
            if world.get::<Mesh3d>(mesh).is_some() {
                let name = world
                    .get::<Name>(node)
                    .map(|n| n.to_string())
                    .unwrap_or_default();
                world.entity_mut(mesh).insert(ControlMesh {
                    node,
                    name,
                    ids: vec![id.clone()],
                });
            }
        }
    }
    let mut state = world.resource_mut::<ViewerState>();
    state.rotaries.insert(id, pivot);
    for alias in aliases {
        state.rotaries.insert(alias, pivot);
    }
}
fn highlight(world: &mut World, ids: Vec<String>) {
    if world.resource::<ViewerState>().highlighted == ids {
        return;
    }
    let region_nodes: std::collections::HashSet<String> = world
        .resource::<ViewerState>()
        .model
        .as_ref()
        .map(|d| d.regions.keys().cloned().collect())
        .unwrap_or_default();
    let mut query = world.query::<(
        &ControlMesh,
        &MeshMaterial3d<StandardMaterial>,
        &OriginalEmission,
    )>();
    let records: Vec<_> = query
        .iter(world)
        .map(|(c, m, o)| (c.clone(), m.0.clone(), o.0))
        .collect();
    for (c, m, original) in records {
        if let Some(mut material) = world.resource_mut::<Assets<StandardMaterial>>().get_mut(&m) {
            material.emissive =
                if !region_nodes.contains(&c.name) && c.ids.iter().any(|id| ids.contains(id)) {
                    LinearRgba::new(0.25, 0.25, 0.25, 1.)
                } else {
                    original
                };
        }
    }
    world.resource_mut::<ViewerState>().highlighted = ids;
}
fn ray_hits(
    world: &mut World,
    ray: Ray3d,
) -> Vec<(Entity, bevy::picking::mesh_picking::ray_cast::RayMeshHit)> {
    let camera_entities: std::collections::HashSet<_> = world
        .query_filtered::<Entity, With<CameraMesh>>()
        .iter(world)
        .collect();
    let filter = |e: Entity| camera_entities.contains(&e);
    let mut system = SystemState::<MeshRayCast>::new(world);
    let hits = system
        .get_mut(world)
        .expect("raycast resources installed by renderer")
        .cast_ray(
            ray,
            &MeshRayCastSettings::default()
                .with_visibility(RayCastVisibility::Visible)
                .with_filter(&filter),
        )
        .to_vec();
    system.apply(world);
    hits
}
fn pick(world: &mut World, x: f32, y: f32, radius: f32) -> Value {
    if world.resource::<Simulation>().mode != InteractionMode::Inspect {
        return Value::Null;
    }
    let camera = world
        .query_filtered::<(&Camera, &GlobalTransform), With<Observer>>()
        .iter(world)
        .next()
        .map(|(c, t)| (c.clone(), *t));
    let Some((camera, transform)) = camera else {
        return Value::Null;
    };
    let logical = camera.logical_viewport_size().unwrap_or(Vec2::ONE);
    let viewport = world.resource::<ViewerState>().viewport.unwrap_or(logical);
    let scale = logical / viewport;
    let Ok(ray) = camera.viewport_to_world(&transform, Vec2::new(x, y) * scale) else {
        return Value::Null;
    };
    let hits = ray_hits(world, ray);
    let hit = hits
        .iter()
        .find(|(e, _)| world.get::<CameraMesh>(*e).is_some());
    if let Some((e, h)) = hit
        && let Some(c) = world.get::<ControlMesh>(*e).cloned()
    {
        let mut id = c.ids[0].clone();
        if !world.resource::<ViewerState>().button_ids.contains(&id)
            && !c.ids.iter().any(|id| id == "touchscreen")
        {
            let nearby = nearby_button(world, &camera, &transform, Vec2::new(x, y), radius, scale);
            if !nearby.is_null() {
                return nearby;
            }
        }
        let mut point = h.uv.map(|uv| json!({"x":uv.x,"y":uv.y}));
        if let Some((min, max)) = local_bounds(world, c.node) {
            let p = world
                .get::<GlobalTransform>(c.node)
                .unwrap()
                .affine()
                .inverse()
                .transform_point3(h.point);
            let local = (p - min) / (max - min).max(Vec3::splat(0.000001));
            if let Some(regions) = world
                .resource::<ViewerState>()
                .model
                .as_ref()
                .and_then(|d| d.regions.get(&c.name))
            {
                let d = Vec2::new(local.x, local.y) - Vec2::splat(0.5);
                let region = if d.abs().max_element() <= 0.22 {
                    "center"
                } else if d.x.abs() > d.y.abs() {
                    if d.x < 0. { "left" } else { "right" }
                } else if d.y < 0. {
                    "down"
                } else {
                    "up"
                };
                if let Some(mapped) = regions.get(region) {
                    id = mapped.clone();
                    point = None;
                }
            }
        }
        let direction = world
            .resource::<ViewerState>()
            .rotaries
            .get(&id)
            .and_then(|e| {
                let t = world.get::<GlobalTransform>(*e)?;
                let axis = t
                    .affine()
                    .transform_vector3(world.get::<Rotary>(*e)?.axis)
                    .normalize();
                let tangent = axis.cross(h.point - t.translation());
                let start = camera.world_to_viewport(&transform, h.point).ok()? / scale;
                let end = camera
                    .world_to_viewport(&transform, h.point + tangent)
                    .ok()?
                    / scale;
                let d = (end - start).try_normalize().unwrap_or(Vec2::X);
                Some(json!({"x":d.x,"y":d.y}))
            });
        return json!({"id":id,"point":point,"surfacePoint":h.point.to_array(),"direction":direction});
    }
    nearby_button(world, &camera, &transform, Vec2::new(x, y), radius, scale)
}
fn nearby_button(
    world: &mut World,
    camera: &Camera,
    transform: &GlobalTransform,
    point: Vec2,
    radius: f32,
    scale: Vec2,
) -> Value {
    let (x, y) = (point.x, point.y);
    if radius > 0. {
        let targets = world.resource::<ViewerState>().targets.clone();
        let mut nearest = None;
        for (id, e) in targets {
            if !world.resource::<ViewerState>().button_ids.contains(&id) {
                continue;
            }
            let Some(p) = target_point(world, e, &id) else {
                continue;
            };
            let Ok(screen) = camera.world_to_viewport(transform, p) else {
                continue;
            };
            let distance = (screen / scale).distance(Vec2::new(x, y));
            if distance > radius
                || nearest
                    .as_ref()
                    .is_some_and(|(_, d): &(String, f32)| distance >= *d)
            {
                continue;
            }
            if target_visible(world, e, p, transform.translation()) {
                nearest = Some((id, distance));
            }
        }
        if let Some((mut id, _)) = nearest {
            let node = world.resource::<ViewerState>().targets[&id];
            let name = world.get::<Name>(node).map(Name::as_str).unwrap_or("");
            if let Some(regions) = world
                .resource::<ViewerState>()
                .model
                .as_ref()
                .and_then(|m| m.regions.get(name))
            {
                let project = |id: &str| {
                    target_point(world, node, id)
                        .and_then(|p| camera.world_to_viewport(transform, p).ok())
                        .map(|p| p / scale)
                };
                if let (Some(center), Some(right), Some(up)) = (
                    project(&regions["center"]),
                    project(&regions["right"]),
                    project(&regions["up"]),
                ) {
                    let r = right - center;
                    let u = up - center;
                    let d = point - center;
                    let determinant = r.perp_dot(u);
                    if determinant.abs() > 0.01 {
                        let local = Vec2::new(d.perp_dot(u), r.perp_dot(d)) / determinant;
                        let region = if local.abs().max_element() <= 0.7 {
                            "center"
                        } else if local.x.abs() > local.y.abs() {
                            if local.x < 0. { "left" } else { "right" }
                        } else if local.y < 0. {
                            "down"
                        } else {
                            "up"
                        };
                        id = regions[region].clone();
                    }
                }
            }
            return json!({"id":id});
        }
    }
    Value::Null
}
fn target_point(world: &World, node: Entity, id: &str) -> Option<Vec3> {
    let (min, max) = local_bounds(world, node)?;
    let size = max - min;
    let name = world.get::<Name>(node).map(Name::as_str).unwrap_or("");
    let regions = world
        .resource::<ViewerState>()
        .model
        .as_ref()
        .and_then(|d| d.regions.get(name));
    let region = regions.and_then(|r| r.iter().find(|(_, v)| *v == id).map(|(k, _)| k.as_str()));
    let [x, y] = match region {
        Some("up") => [0.5, 0.81],
        Some("down") => [0.5, 0.19],
        Some("left") => [0.19, 0.5],
        Some("right") => [0.81, 0.5],
        _ => [0.5, 0.5],
    };
    Some(
        world
            .get::<GlobalTransform>(node)?
            .transform_point(Vec3::new(
                min.x + size.x * x,
                min.y + size.y * y,
                min.z + size.z * 0.08,
            )),
    )
}
fn target_visible(world: &mut World, node: Entity, point: Vec3, origin: Vec3) -> bool {
    let delta = point - origin;
    let Ok(direction) = Dir3::new(delta) else {
        return false;
    };
    let own = descendants(world, node);
    let hits = ray_hits(world, Ray3d::new(origin, direction));
    let body_hits: Vec<_> = hits
        .iter()
        .filter(|(e, _)| world.get::<CameraMesh>(*e).is_some())
        .collect();
    let Some((_, first)) = body_hits.first() else {
        return false;
    };
    body_hits
        .iter()
        .any(|(e, h)| own.contains(e) && h.distance <= first.distance + 0.0006)
}
fn publish(world: &mut World) {
    let mut failures = Vec::new();
    for scene in world.query::<&WorldAssetRoot>().iter(world) {
        let server = world.resource::<AssetServer>();
        if let Some(bevy::asset::LoadState::Failed(error)) = server.get_load_state(scene.0.id()) {
            failures.push(error.to_string());
        } else if let Some(bevy::asset::RecursiveDependencyLoadState::Failed(error)) =
            server.get_recursive_dependency_load_state(scene.0.id())
        {
            failures.push(error.to_string());
        }
    }
    if let Some(error) = failures.into_iter().next() {
        world.resource_mut::<ViewerState>().error = Some(error);
    }

    // WorldInstanceReady may arrive after transform propagation. Wait for the
    // following frame before indexing triangles in world space.
    if let Some(root) = world
        .resource_mut::<ViewerState>()
        .navigation_pending
        .take()
    {
        world.insert_resource(build_navigation(world, root));
        world.resource_mut::<ViewerState>().environment_ready = true;
    }
    let pending: Vec<_> = world
        .query_filtered::<Entity, With<Prepared>>()
        .iter(world)
        .collect();
    for e in pending {
        if world.get::<ModelRoot>(e).is_some() {
            prepare_model(world, e);
        } else if world.get::<EnvironmentRoot>(e).is_some() {
            prepare_environment(world, e);
        }
    }
    if world.resource::<ViewerState>().prepared {
        let definitions = world.resource::<ViewerState>().rotary_definitions.clone();
        for (id, aliases) in definitions {
            define_rotary(world, id, aliases);
        }
    }
    let camera = world
        .query_filtered::<(&Camera, &GlobalTransform), With<Observer>>()
        .iter(world)
        .next()
        .map(|(c, t)| (c.clone(), *t));
    let observer_position = camera.as_ref().map(|(_, t)| t.translation().to_array());
    let mut key = world
        .resource::<ViewerState>()
        .viewport
        .unwrap_or_default()
        .to_array()
        .to_vec();
    if let Some((ref camera, transform)) = camera {
        key.extend(transform.to_matrix().to_cols_array());
        key.extend(
            camera
                .logical_viewport_size()
                .unwrap_or_default()
                .to_array(),
        );
    }
    let pose = world.resource::<Simulation>().camera_pose();
    key.extend(pose.to_matrix().to_cols_array());
    // Model normalization happens after propagation on the scene-ready frame.
    // Reproject once its world transform catches up, even with a stationary eye.
    for transform in world
        .query_filtered::<&GlobalTransform, With<ModelRoot>>()
        .iter(world)
    {
        key.extend(transform.to_matrix().to_cols_array());
    }
    for part in world.query::<&Rotary>().iter(world) {
        key.push(part.angle);
    }
    if key != world.resource::<ViewerState>().projection_key {
        let mut points = BTreeMap::new();
        if world.resource::<Simulation>().mode == InteractionMode::Inspect
            && let Some((camera, transform)) = camera
        {
            let targets = world.resource::<ViewerState>().targets.clone();
            for (id, node) in targets {
                if let Some(p) = target_point(world, node, &id)
                    && let Ok(screen) = camera.world_to_viewport(&transform, p)
                {
                    let visible = target_visible(world, node, p, transform.translation());
                    let logical = camera.logical_viewport_size().unwrap_or(Vec2::ONE);
                    let viewport = world.resource::<ViewerState>().viewport.unwrap_or(logical);
                    let screen = screen * viewport / logical;
                    points.insert(id, json!({"x":screen.x,"y":screen.y,"visible":visible}));
                }
            }
        }
        let mut state = world.resource_mut::<ViewerState>();
        state.projection_key = key;
        state.projected = points;
    }
    let state = world.resource::<ViewerState>();
    let sim = world.resource::<Simulation>();
    let pose = sim.camera_pose();
    let snapshot = json!({"renderer":"bevy","version":"0.20.0","ready":state.prepared,"environmentReady":state.environment_ready,
        "mode":sim.mode,"worldSeconds":sim.elapsed,"cameraPosition":pose.translation.to_array(),"cameraRotation":pose.rotation.to_array(),
        "observerPosition":observer_position,"sensorSequence":state.sequence,"grounded":state.grounded,"renderSize":state.viewport.map(|v| v.to_array()),"targets":state.projected,"error":state.error});
    world.resource::<Host>().0.0.lock().unwrap().snapshot = snapshot;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn body_origin_dial_gets_a_centered_pivot_and_does_not_move_when_rotated() {
        let mut world = World::new();
        world.init_resource::<ViewerState>();
        let parent = world
            .spawn((
                Transform::from_xyz(1., 2., 3.),
                GlobalTransform::from_xyz(1., 2., 3.),
            ))
            .id();
        let node = world
            .spawn((
                Transform::from_xyz(10., 2., 3.),
                GlobalTransform::from_xyz(11., 4., 6.),
                ChildOf(parent),
            ))
            .id();
        world.spawn((
            Aabb {
                center: Vec3::new(3., 4., 5.).into(),
                half_extents: Vec3::new(1., 0.1, 1.).into(),
            },
            GlobalTransform::from_xyz(11., 4., 6.),
            ChildOf(node),
        ));
        world
            .resource_mut::<ViewerState>()
            .targets
            .insert("dial".into(), node);
        world.resource_mut::<ViewerState>().model = Some(
            serde_json::from_value(json!({
                "url":"camera.glb", "yaw":0., "physical_width":0.113, "optics": Optics::default(),
                "hidden":[], "screen":"lcd", "inputs":{}, "rotaries":{"dial":["face"]}
            }))
            .unwrap(),
        );
        let face = world
            .spawn((
                Name::new("face"),
                Transform::from_xyz(13., 6.1, 8.),
                ChildOf(parent),
            ))
            .id();
        define_rotary(
            &mut world,
            "dial".into(),
            vec!["left".into(), "right".into()],
        );
        let pivot = world.resource::<ViewerState>().rotaries["dial"];
        assert_eq!(world.get::<ChildOf>(face).unwrap().parent(), pivot);
        assert!(
            world
                .get::<Transform>(face)
                .unwrap()
                .translation
                .distance(Vec3::new(0., 0.1, 0.))
                < 0.00001
        );
        assert_eq!(
            world.get::<Transform>(pivot).unwrap().translation,
            Vec3::new(13., 6., 8.)
        );
        assert_eq!(world.get::<Rotary>(pivot).unwrap().axis, Vec3::Y);
        let initial_center = Vec3::new(14., 8., 11.);
        for angle in [0.25, 1., -0.5] {
            let mut t = *world.get::<Transform>(pivot).unwrap();
            t.rotation = Quat::from_rotation_y(angle);
            let child = world.get::<Transform>(node).unwrap();
            let center = world
                .get::<Transform>(parent)
                .unwrap()
                .transform_point(t.transform_point(child.transform_point(Vec3::new(3., 4., 5.))));
            assert!(center.distance(initial_center) < 0.00001);
        }
        assert_eq!(world.resource::<ViewerState>().rotaries["left"], pivot);
    }
    #[test]
    fn walking_uses_scene_geometry_even_when_no_camera_has_rendered_it() {
        let mut world = World::new();
        world.init_resource::<Assets<Mesh>>();
        world.init_resource::<ViewerState>();
        world.init_resource::<Time<Real>>();
        world.insert_resource(Simulation {
            mode: InteractionMode::Walk,
            movement: Vec2::Y,
            ..default()
        });
        let root = world.spawn_empty().id();
        let add_box = |world: &mut World, size: Vec3, position: Vec3| {
            let mut geometry = Mesh::from(Cuboid::from_size(size));
            if size.y < 0.2 {
                geometry.duplicate_vertices();
            }
            let mesh = world.resource_mut::<Assets<Mesh>>().add(geometry);
            world.spawn((
                Mesh3d(mesh),
                Aabb::from_min_max(-size * 0.5, size * 0.5),
                Transform::from_translation(position),
                GlobalTransform::from_translation(position),
                InheritedVisibility::VISIBLE,
                ViewVisibility::default(),
                ChildOf(root),
            ));
        };
        add_box(
            &mut world,
            Vec3::new(10., 0.1, 10.),
            Vec3::new(0., -0.05, 0.),
        );
        add_box(
            &mut world,
            Vec3::new(10., 3., 0.1),
            Vec3::new(0., 1.5, -0.6),
        );
        world.insert_resource(build_navigation(&world, root));
        world.resource_mut::<ViewerState>().environment = Some(root);
        world.resource_mut::<ViewerState>().environment_ready = true;
        for _ in 0..20 {
            world
                .resource_mut::<Time<Real>>()
                .advance_by(std::time::Duration::from_secs_f32(0.1));
            advance(&mut world);
        }
        let position = world.resource::<Simulation>().position;
        assert!(
            position.z < -0.1 && position.z > -0.5,
            "wall blocks walking: {position:?}"
        );
        assert!(position.y.abs() < 0.00001);
        world.resource_mut::<Simulation>().movement = Vec2::NEG_X;
        for _ in 0..100 {
            advance(&mut world);
        }
        assert!(
            world.resource::<Simulation>().position.x > -5.,
            "cannot walk past the scanned floor"
        );
    }
    #[test]
    fn invalid_lcd_sizes_never_enter_the_render_queue() {
        let bridge = Bridge::default();
        assert!(bridge.lcd(0, 480, vec![]).is_err());
        assert!(bridge.lcd(720, 480, vec![0; 4]).is_err());
        assert!(bridge.lcd(1, 1, vec![0; 4]).is_ok());
    }
}
