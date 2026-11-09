//! Browser ownership and typed arrays; card construction runs in Rust.
use fujimod_recovery as core;
use wasm_bindgen::prelude::*;

fn error(error: core::image::Error) -> JsValue {
    js_sys::Error::new(&error.to_string()).into()
}

#[wasm_bindgen]
pub struct SdCardAvailability {
    inner: core::Availability,
}

#[wasm_bindgen]
impl SdCardAvailability {
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
    #[wasm_bindgen(getter, js_name = systemStartupLamp)]
    pub fn system_startup_lamp(&self) -> bool {
        self.inner.system_startup_lamp
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
    /// Firmwares with the same value share one card's EXR II boot files.
    #[wasm_bindgen(getter, js_name = sdBootV2Header)]
    pub fn ota_startup_header(&self) -> Option<String> {
        self.inner.ota_startup_header.clone()
    }
    #[wasm_bindgen(getter, js_name = greenLedFilenames)]
    pub fn green_led_filenames(&self) -> Result<Vec<String>, JsValue> {
        match core::Recipe::bundled(&self.inner.firmware_id).map_err(error)? {
            Some(recipe) => core::green_led::filenames(&recipe).map_err(error),
            None => Ok(vec![]),
        }
    }
    #[wasm_bindgen(getter, js_name = sdBootCombination, unchecked_return_type = "BootCombination | null")]
    pub fn sd_boot_combination(&self) -> Result<JsValue, JsValue> {
        js_sys::JSON::parse(
            &self
                .inner
                .sd_boot_combination
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
pub struct SdCardFile {
    inner: core::image::File,
}
#[wasm_bindgen]
impl SdCardFile {
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
pub struct SdCardFiles {
    files: Vec<core::image::File>,
}
#[wasm_bindgen]
impl SdCardFiles {
    /// Transfer all file ownership to JS in one call.
    #[wasm_bindgen(js_name = takeFiles, unchecked_return_type = "SdCardFile[]")]
    pub fn take_files(&mut self) -> js_sys::Array {
        std::mem::take(&mut self.files)
            .into_iter()
            .map(|inner| JsValue::from(SdCardFile { inner }))
            .collect()
    }
}

#[wasm_bindgen(js_name = sdCardAvailability)]
pub fn availability(firmware_id: &str) -> Result<SdCardAvailability, JsValue> {
    core::availability(firmware_id)
        .map(|inner| SdCardAvailability { inner })
        .map_err(error)
}

#[wasm_bindgen(js_name = buildSdCardRecovery)]
pub fn build(firmware_id: &str, source: &[u8]) -> Result<SdCardFiles, JsValue> {
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
        .ok_or_else(|| js_sys::Error::new("Missing SD card recipe"))?;
    recipe
        .build(source)
        .map(|files| SdCardFiles { files })
        .map_err(error)
}

/// The EXR II card root that loads stage 1 from NAND, then FujiMod stage 2.
#[wasm_bindgen(js_name = buildSdBootV2SdCard)]
pub fn build_ota_startup(firmware_id: &str, source: &[u8]) -> Result<SdCardFiles, JsValue> {
    let recipe = core::Recipe::bundled(firmware_id)
        .map_err(error)?
        .ok_or_else(|| js_sys::Error::new("Missing SD card recipe"))?;
    recipe
        .build_ota_startup(source)
        .map(|files| SdCardFiles { files })
        .map_err(error)
}

#[wasm_bindgen(js_name = buildGreenLedCard)]
pub fn build_green_led(firmware_id: &str, source: &[u8]) -> Result<SdCardFiles, JsValue> {
    let recipe = core::Recipe::bundled(firmware_id)
        .map_err(error)?
        .ok_or_else(|| js_sys::Error::new("Missing SD card recipe"))?;
    core::green_led::build(&recipe, source)
        .map(|files| SdCardFiles { files })
        .map_err(error)
}
