//! Thin browser entry points. JS sends input commands, never scene graphs.
#[cfg(any(
    all(feature = "webgl2", feature = "webgpu"),
    not(any(feature = "webgl2", feature = "webgpu"))
))]
compile_error!("Select exactly one browser renderer: webgl2 or webgpu");
use crate::rendering::{Bridge, Command, Presentation, SimulationPlugin};
use bevy::{
    asset::AssetMetaCheck,
    prelude::*,
    window::{PresentMode, WindowResolution},
    winit::WinitSettings,
};
use std::cell::RefCell;
use wasm_bindgen::prelude::*;
thread_local! {static HOST:RefCell<Option<Bridge>>=const{RefCell::new(None)};}
#[wasm_bindgen]
pub fn backend() -> String {
    if cfg!(feature = "webgpu") {
        "webgpu"
    } else {
        "webgl2"
    }
    .into()
}
fn bridge() -> Result<Bridge, JsValue> {
    HOST.with(|h| {
        h.borrow()
            .clone()
            .ok_or_else(|| JsValue::from_str("Simulator has not started"))
    })
}
#[wasm_bindgen]
pub fn start(canvas_selector: String, asset_root: String) -> Result<(), JsValue> {
    if HOST.with(|h| h.borrow().is_some()) {
        return Err(JsValue::from_str("Simulator already started"));
    }
    std::panic::set_hook(Box::new(|info| {
        let message = JsValue::from_str(&info.to_string());
        web_sys::console::error_1(&message);
        let init = web_sys::CustomEventInit::new();
        init.set_detail(&message);
        if let (Some(window), Ok(event)) = (
            web_sys::window(),
            web_sys::CustomEvent::new_with_event_init_dict("simulationerror", &init),
        ) {
            let _ = window.dispatch_event(&event);
        }
    }));
    let bridge = Bridge::default();
    HOST.with(|h| *h.borrow_mut() = Some(bridge.clone()));
    App::new()
        .add_plugins(
            DefaultPlugins
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        canvas: Some(canvas_selector),
                        fit_canvas_to_parent: true,
                        // One render pixel per CSS pixel on every browser. Device DPR
                        // otherwise multiplies every color/depth/MSAA attachment.
                        resolution: WindowResolution::new(640, 480).with_scale_factor_override(1.),
                        prevent_default_event_handling: false,
                        present_mode: PresentMode::AutoVsync,
                        ..default()
                    }),
                    ..default()
                })
                .set(AssetPlugin {
                    file_path: asset_root,
                    meta_check: AssetMetaCheck::Never,
                    ..default()
                }),
        )
        .insert_resource(WinitSettings::continuous())
        .add_plugins(SimulationPlugin {
            bridge,
            presentation: Presentation::Observer,
        })
        .run();
    Ok(())
}
#[wasm_bindgen]
pub fn command(json: &str) -> Result<(), JsValue> {
    let command: Command =
        serde_json::from_str(json).map_err(|e| JsValue::from_str(&e.to_string()))?;
    bridge()?.send(command);
    Ok(())
}
#[wasm_bindgen]
pub fn snapshot() -> Result<String, JsValue> {
    Ok(bridge()?.snapshot().to_string())
}
#[wasm_bindgen]
pub fn response(request: u32) -> Result<Option<String>, JsValue> {
    Ok(bridge()?.response(request).map(|v| v.to_string()))
}
#[wasm_bindgen]
pub fn lcd(width: u32, height: u32, rgba: Vec<u8>) -> Result<(), JsValue> {
    bridge()?
        .lcd(width, height, rgba)
        .map_err(JsValue::from_str)
}
#[wasm_bindgen]
pub fn take_sensor() -> Result<Option<js_sys::Uint8Array>, JsValue> {
    Ok(bridge()?
        .take_sensor()
        .map(|frame| js_sys::Uint8Array::from(frame.rgb.as_slice())))
}
