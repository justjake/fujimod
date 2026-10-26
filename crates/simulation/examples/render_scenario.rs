//! Native, windowless scenario rendering using the same world and renderer as the web.
//! cargo run --manifest-path crates/simulation/Cargo.toml --example render_scenario -- ENVIRONMENT_JSON FRESH_OUTPUT
use bevy::{
    asset::AssetMetaCheck,
    prelude::*,
    render::{RenderPlugin, render_resource::PollType, renderer::RenderDevice},
    time::TimeUpdateStrategy,
    window::ExitCondition,
};
use fujimod_simulation::{
    Simulation,
    rendering::{Bridge, Command, EnvironmentDefinition, Presentation, SimulationPlugin},
};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if !(2..=3).contains(&args.len()) {
        return Err(
            "Expected ENVIRONMENT_JSON, a fresh output directory, and optional CAMERA_JSON".into(),
        );
    }
    let environment: EnvironmentDefinition = serde_json::from_slice(&std::fs::read(&args[0])?)?;
    let output = PathBuf::from(&args[1]);
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::create_dir(&output)?;
    let bridge = Bridge::default();
    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(WindowPlugin {
                primary_window: None,
                exit_condition: ExitCondition::DontExit,
                ..default()
            })
            .set(AssetPlugin {
                file_path: env!("CARGO_MANIFEST_DIR").into(),
                meta_check: AssetMetaCheck::Never,
                ..default()
            })
            .set(RenderPlugin {
                synchronous_pipeline_compilation: true,
                ..default()
            }),
    )
    .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::ZERO))
    .add_plugins(SimulationPlugin {
        bridge: bridge.clone(),
        presentation: Presentation::SensorOnly,
    });
    app.finish();
    app.cleanup();
    bridge.send(Command::Environment { data: environment });
    if let Some(path) = args.get(2) {
        let camera: serde_json::Value = serde_json::from_slice(&std::fs::read(path)?)?;
        bridge.send(Command::Optics {
            data: serde_json::from_value(camera["optics"].clone())?,
        });
    }
    bridge.send(Command::Sensor { enabled: true });
    let start = Instant::now();
    while bridge.snapshot()["environmentReady"] != true {
        app.update();
        if start.elapsed() > Duration::from_secs(60) {
            return Err(format!("Environment load failed: {}", bridge.snapshot()).into());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    app.world_mut().resource_mut::<Simulation>().elapsed = 0.;
    let mut records = Vec::new();
    for _ in 0..10 {
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1. / 15.,
        )));
        app.update();
        // GPU latency must not change scenario time or the next exposure pose.
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::ZERO));
        let waiting = Instant::now();
        let frame = loop {
            app.world()
                .resource::<RenderDevice>()
                .poll(PollType::Wait {
                    timeout: None,
                    submission_index: None,
                })?;
            app.update();
            if let Some(frame) = bridge.take_sensor() {
                break frame;
            }
            if waiting.elapsed() > Duration::from_secs(30) {
                return Err("Sensor readback timed out".into());
            }
        };
        let name = format!("frame-{:04}.ppm", frame.sequence);
        let mut bytes = format!("P6\n{} {}\n255\n", frame.width, frame.height).into_bytes();
        bytes.extend_from_slice(&frame.rgb);
        std::fs::write(output.join(&name), bytes)?;
        records.push(serde_json::json!({"file":name,"world_seconds":frame.world_seconds,
            "position":frame.camera_pose.translation.to_array(),"grounded":bridge.snapshot()["grounded"],"rotation":frame.camera_pose.rotation.to_array()}));
    }
    if records.is_empty() {
        return Err("Renderer produced no sensor frames".into());
    }
    std::fs::write(
        output.join("scenario.json"),
        serde_json::to_vec_pretty(&records)?,
    )?;
    println!("{} frames written to {}", records.len(), output.display());
    Ok(())
}
