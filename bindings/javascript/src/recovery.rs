//! Browser ownership and typed arrays; card construction runs in Rust.
use fujimod_recovery as core;
use wasm_bindgen::prelude::*;

fn error(error: core::image::Error) -> JsValue {
    js_sys::Error::new(&error.to_string()).into()
}

#[wasm_bindgen]
pub struct RecoveryAvailability {
    inner: core::Availability,
}

#[wasm_bindgen]
impl RecoveryAvailability {
    #[wasm_bindgen(getter, js_name = firmwareId)]
    pub fn firmware_id(&self) -> String {
        self.inner.firmware_id.clone()
    }
    #[wasm_bindgen(getter, js_name = canBuild)]
    pub fn can_build(&self) -> bool {
        self.inner.can_build
    }
    #[wasm_bindgen(getter)]
    pub fn missing(&self) -> Vec<String> {
        self.inner.missing.clone()
    }
    #[wasm_bindgen(getter, js_name = outputFilenames)]
    pub fn output_filenames(&self) -> Vec<String> {
        self.inner.output_filenames.clone()
    }
    #[wasm_bindgen(getter)]
    pub fn generation(&self) -> Option<String> {
        self.inner.generation.as_ref().map(|g| {
            match g {
                core::Generation::Exr => "exr",
                core::Generation::ExrPro => "exrpro",
                core::Generation::Exr2 => "exr2",
                core::Generation::Xpp => "xpp",
                core::Generation::Xp4 => "xp4",
                core::Generation::Xp5 => "xp5",
            }
            .into()
        })
    }
    #[wasm_bindgen(getter, js_name = sdBoot, unchecked_return_type = "BootCombination | null")]
    pub fn sd_boot(&self) -> Result<JsValue, JsValue> {
        js_sys::JSON::parse(
            &self
                .inner
                .sd_boot
                .as_ref()
                .map_or("null".into(), ToString::to_string),
        )
    }
    #[wasm_bindgen(getter, js_name = firmwareUpdate, unchecked_return_type = "BootCombination | null")]
    pub fn firmware_update(&self) -> Result<JsValue, JsValue> {
        js_sys::JSON::parse(
            &self
                .inner
                .firmware_update
                .as_ref()
                .map_or("null".into(), ToString::to_string),
        )
    }
}

#[wasm_bindgen]
pub struct RecoveryFile {
    inner: core::image::File,
}
#[wasm_bindgen]
impl RecoveryFile {
    #[wasm_bindgen(getter)]
    pub fn name(&self) -> String {
        self.inner.name.clone()
    }
    #[wasm_bindgen(getter)]
    pub fn bytes(&self) -> Vec<u8> {
        self.inner.bytes.clone()
    }
    #[wasm_bindgen(getter)]
    pub fn size(&self) -> usize {
        self.inner.bytes.len()
    }
}

#[wasm_bindgen]
pub struct RecoveryCard {
    files: Vec<core::image::File>,
}
#[wasm_bindgen]
impl RecoveryCard {
    /// Transfer all file ownership to JS in one call.
    #[wasm_bindgen(js_name = takeFiles, unchecked_return_type = "RecoveryFile[]")]
    pub fn take_files(&mut self) -> js_sys::Array {
        std::mem::take(&mut self.files)
            .into_iter()
            .map(|inner| JsValue::from(RecoveryFile { inner }))
            .collect()
    }
}

#[wasm_bindgen(js_name = recoveryAvailability)]
pub fn availability(firmware_id: &str) -> Result<RecoveryAvailability, JsValue> {
    core::availability(firmware_id)
        .map(|inner| RecoveryAvailability { inner })
        .map_err(error)
}

#[wasm_bindgen(js_name = buildRecoveryCard)]
pub fn build(firmware_id: &str, source: &[u8]) -> Result<RecoveryCard, JsValue> {
    let status = core::availability(firmware_id).map_err(error)?;
    if !status.can_build {
        return Err(js_sys::Error::new(&format!(
            "Missing recovery facts: {}",
            status.missing.join(", ")
        ))
        .into());
    }
    let recipe = core::Recipe::bundled(firmware_id)
        .map_err(error)?
        .ok_or_else(|| js_sys::Error::new("Missing recovery recipe"))?;
    recipe
        .build(source)
        .map(|files| RecoveryCard { files })
        .map_err(error)
}
