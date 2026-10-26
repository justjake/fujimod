//! SD card construction from explicit source identities and located recipes.
#![forbid(unsafe_code)]
pub mod image;
mod exr;
mod exr_loader;
mod exr_firmware;
mod exr_load_maps;
pub mod rom_sd;
pub use exr::Exr;
mod exr2;
pub mod xpp;
mod xp4;
mod xp5;
use image::{File, Packaging, Plan, Result, require};
use serde::{Deserialize, Serialize};
pub use xp4::{ColdStart, FileApi, Setup};
pub use xp5::{
    Board as SdBoard, CodeProof as Xp5CodeProof, Flash as Xp5Flash, Lamp as Xp5Lamp,
    Operation as SdOperation, RegisterWrite as Xp5RegisterWrite, Setup as Xp5Setup,
    SourceProof as Xp5SourceProof,
};

/// Optional diagnostics for XP5 startup construction. Zero preserves normal entry.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StartupOptions {
    pub success_hold_seconds: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Generation {
    Exr,
    #[serde(rename = "exrpro")]
    ExrPro,
    Exr2,
    #[serde(rename = "xpp")]
    Xpp,
    Xp4,
    Xp5,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Proof {
    pub address: usize,
    pub size: usize,
    pub sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields, tag = "kind", rename_all = "snake_case")]
pub enum VersionConfiguration {
    Reader {
        address: usize,
    },
    Memory {
        pointer: usize,
        selector_offset: usize,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VersionInput {
    pub address: usize,
    pub value: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VersionContract {
    pub public_version: [u8; 2],
    pub version_getter: Proof,
    pub configuration: VersionConfiguration,
    pub getter_arguments: u8,
    pub comparison: Proof,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub comparison_inputs: Vec<VersionInput>,
    pub version_producer: Proof,
    pub producer_stop: usize,
    pub version_context_pointer: usize,
    pub version_context: usize,
    pub current_major_setter: Proof,
    pub current_minor_setter: Proof,
    pub target_major: usize,
    pub target_minor: usize,
    pub current_major: usize,
    pub current_minor: usize,
}
impl VersionContract {
    fn verify(&self, ram: &[u8]) -> Result<()> {
        for proof in [
            &self.version_getter,
            &self.comparison,
            &self.version_producer,
            &self.current_major_setter,
            &self.current_minor_setter,
        ] {
            image::hash(
                image::extent(ram, proof.address, proof.size)?,
                &proof.sha256,
            )?;
        }
        require(
            self.current_major == self.target_major + 8
                && self.current_minor == self.target_minor + 8
                && self.target_minor == self.target_major + 4,
            "Updater version pair layout differs",
        )?;
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recipe {
    pub firmware_id: String,
    pub source_size: usize,
    pub updater: Option<VersionContract>,
    pub source_sha256: String,
    pub update_filename: String,
    pub generation: Generation,
    pub plan: Option<Plan>,
    pub packaging: Option<Packaging>,
    pub exr: Option<Exr>,
    pub setup: Option<Setup>,
    pub xp5: Option<Xp5Setup>,
    pub cold_start: Option<ColdStart>,
    pub file_api: Option<FileApi>,
    pub sd_boot: Option<serde_json::Value>,
    pub firmware_update: Option<serde_json::Value>,
    pub missing: Vec<String>,
}
impl Recipe {
    pub fn missing_facts(&self) -> Vec<String> {
        let mut missing = self.missing.clone();
        if self.updater.is_none()
            && !missing
                .iter()
                .any(|s| s == "version-independent stock updater acceptance")
        {
            missing.push("version-independent stock updater acceptance".into());
        }
        missing
    }
    pub fn bundled(firmware_id: &str) -> Result<Option<Self>> {
        let recipes: Vec<Self> = serde_json::from_str(include_str!(concat!(
            env!("OUT_DIR"),
            "/recipes.generated.json"
        )))
        .map_err(|e| image::Error::Invalid(e.to_string()))?;
        Ok(recipes.into_iter().find(|r| r.firmware_id == firmware_id))
    }
    pub fn cold_worker(&self, source: &[u8]) -> Result<serde_json::Value> {
        require(
            source.len() == self.source_size,
            "Update size differs from recipe",
        )?;
        image::hash(source, &self.source_sha256)?;
        let plan = self
            .plan
            .as_ref()
            .ok_or_else(|| image::Error::Invalid("Missing RAM plan".into()))?;
        let cold = self
            .cold_start
            .as_ref()
            .ok_or_else(|| image::Error::Invalid("Missing cold startup".into()))?;
        let loader = plan
            .regions
            .first()
            .ok_or_else(|| image::Error::Invalid("Missing loader".into()))?
            .source
            .read(&fujimod_firmware::invert(source))?;
        serde_json::to_value(xp4::worker(&loader, cold.mode_flag)?)
            .map_err(|e| image::Error::Invalid(e.to_string()))
    }
    pub fn cold_plan(&self, source: &[u8]) -> Result<Plan> {
        require(
            source.len() == self.source_size,
            "Update size differs from recipe",
        )?;
        image::hash(source, &self.source_sha256)?;
        let plan = self
            .plan
            .as_ref()
            .ok_or_else(|| image::Error::Invalid("Missing RAM plan".into()))?;
        let cold = self
            .cold_start
            .as_ref()
            .ok_or_else(|| image::Error::Invalid("Missing cold startup".into()))?;
        xp4::cold_plan(&fujimod_firmware::invert(source), plan, cold)
    }
    /// Explicit SRAM placement and file ABI for isolated loader execution.
    pub fn compile_ffinit(
        &self,
        source: &[u8],
        payload: &[u8],
        base: u32,
        api: &FileApi,
        workspace: u32,
    ) -> Result<(Vec<u8>, serde_json::Value)> {
        require(
            source.len() == self.source_size,
            "Update size differs from recipe",
        )?;
        image::hash(source, &self.source_sha256)?;
        let setup = self
            .setup
            .as_ref()
            .ok_or_else(|| image::Error::Invalid("Missing DDR setup".into()))?;
        let bytes = xp4::ffinit(
            &fujimod_firmware::invert(source),
            setup,
            api,
            payload,
            base,
            Some(workspace),
        )?;
        let mut layout = xp4::layout(setup, api, base);
        layout["workspace"] = workspace.into();
        layout["file_api"] = serde_json::json!(api);
        layout["payload_size"] = payload.len().into();
        layout["payload_crc32"] = xp4::crc32(payload).into();
        layout["bytes"] = bytes.len().into();
        layout["sha256"] = fujimod_firmware::sha256(&bytes).into();
        Ok((bytes, layout))
    }
    /// Prepare XP5 flash and resume the update's original internal startup.
    pub fn build_stage_one(&self, source: &[u8]) -> Result<Vec<File>> {
        self.build_stage_one_with_options(source, StartupOptions::default())
    }
    pub fn build_stage_one_with_options(
        &self,
        source: &[u8],
        options: StartupOptions,
    ) -> Result<Vec<File>> {
        require(
            matches!(self.generation, Generation::Xp5),
            "Stage one requires XP5",
        )?;
        require(
            source.len() == self.source_size,
            "Update size differs from recipe",
        )?;
        image::hash(source, &self.source_sha256)?;
        let setup = self.xp5.as_ref().ok_or_else(|| {
            image::Error::Invalid(format!(
                "Missing XP5 flash/setup bindings: {}",
                self.missing.join(", ")
            ))
        })?;
        xp5::package(
            &fujimod_firmware::invert(source),
            setup,
            None,
            options.success_hold_seconds,
        )
    }
    /// Boot files retain the stock updater and storage drivers. DAT is copied
    /// verbatim to its required root filename, including signed containers.
    pub fn build(&self, source: &[u8]) -> Result<Vec<File>> {
        let missing = self.missing_facts();
        require(
            missing.is_empty(),
            &format!("Missing recovery facts: {}", missing.join(", ")),
        )?;
        require(
            self.updater.is_some(),
            "Missing version-independent stock updater acceptance",
        )?;
        self.build_startup(source)
    }
    /// Reproduce the original XP4 cold card pair for regression comparisons.
    pub fn build_rescue_baseline(&self, source: &[u8]) -> Result<Vec<File>> {
        require(
            matches!(self.generation, Generation::Xp4),
            "Rescue baseline requires XP4",
        )?;
        require(
            source.len() == self.source_size,
            "Update size differs from recipe",
        )?;
        image::hash(source, &self.source_sha256)?;
        let missing = || image::Error::Invalid("Missing XP4 rescue baseline facts".into());
        let mut files = xp4::rescue_baseline(
            &fujimod_firmware::invert(source),
            self.plan.as_ref().ok_or_else(missing)?,
            self.cold_start.as_ref().ok_or_else(missing)?,
            self.setup.as_ref().ok_or_else(missing)?,
            self.file_api.as_ref().ok_or_else(missing)?,
        )?;
        files.push(File::new(&self.update_filename, source.to_vec())?);
        Ok(files)
    }
    /// Construct established startup files independently of updater readiness.
    pub fn build_startup(&self, source: &[u8]) -> Result<Vec<File>> {
        self.build_startup_with_options(source, StartupOptions::default())
    }
    pub fn build_startup_with_options(
        &self,
        source: &[u8],
        options: StartupOptions,
    ) -> Result<Vec<File>> {
        self.startup_inputs(source, options, None)
    }
    /// Include the unit's retained storage data in a checked card startup.
    pub fn build_startup_with_storage(
        &self, source: &[u8], storage: &[u8], options: StartupOptions,
    ) -> Result<Vec<File>> {
        require(matches!(self.generation, Generation::Xp5), "Card storage requires XP5")?;
        self.startup_inputs(source, options, Some(storage))
    }
    fn startup_inputs(&self, source: &[u8], options: StartupOptions, storage: Option<&[u8]>)
        -> Result<Vec<File>> {
        require(
            options.success_hold_seconds == 0 || matches!(self.generation, Generation::Xp5),
            "Success hold requires XP5",
        )?;
        require(
            source.len() == self.source_size,
            "Update size differs from recipe",
        )?;
        image::hash(source, &self.source_sha256)?;
        let inverted = fujimod_firmware::invert(source);
        let plan = self.plan.as_ref().ok_or_else(|| {
            image::Error::Invalid(format!(
                "Missing RAM image plan: {}",
                self.missing.join(", ")
            ))
        })?;
        let mut files =
            match self.generation {
                Generation::Exr | Generation::ExrPro => {
                    let header = self
                        .packaging
                        .as_ref()
                        .ok_or_else(|| image::Error::Invalid("Missing EXR header".into()))?;
                    let api = self.exr.as_ref().ok_or_else(|| {
                        image::Error::Invalid("Missing EXR filesystem ABI".into())
                    })?;
                    exr::package(
                        api,
                        header,
                        &inverted,
                        plan,
                        image::transform(&inverted, plan)?,
                    )?
                }
                Generation::Exr2 | Generation::Xpp => {
                    let packaging = self.packaging.as_ref().ok_or_else(|| {
                        image::Error::Invalid("Missing SD header packaging".into())
                    })?;
                    let mut ram = image::transform(&inverted, plan)?;
                    if matches!(self.generation, Generation::Exr2) {
                        exr2::install(&inverted, plan, &mut ram)?;
                    } else {
                        xpp::install(&inverted, plan, &mut ram)?;
                    }
                    packaging.package_bytes(&inverted, plan.entry, ram)?
                }
                Generation::Xp4 => {
                    let cold = self
                        .cold_start
                        .as_ref()
                        .ok_or_else(|| image::Error::Invalid("Missing XP4 cold startup".into()))?;
                    let setup = self
                        .setup
                        .as_ref()
                        .ok_or_else(|| image::Error::Invalid("Missing XP4 DDR setup".into()))?;
                    let api = self.file_api.as_ref().ok_or_else(|| {
                        image::Error::Invalid("Missing Topaz filesystem ABI".into())
                    })?;
                    xp4::package(&inverted, plan, cold, setup, api)?
                }
                Generation::Xp5 => {
                    let setup = self.xp5.as_ref().ok_or_else(|| {
                        image::Error::Invalid(
                            "Missing XP5 post-setup SD loader and handoff facts".into(),
                        )
                    })?;
                    xp5::package_with_storage(
                        &inverted,
                        setup,
                        Some(image::transform(&inverted, plan)?),
                        options.success_hold_seconds,
                        storage,
                    )?
                }
            };
        require(
            !files
                .iter()
                .any(|f| f.name.eq_ignore_ascii_case(&self.update_filename)),
            "DAT filename collides with boot files",
        )?;
        if let Some(contract) = &self.updater {
            let ram_filename = if matches!(self.generation, Generation::Exr | Generation::ExrPro) {
                "ffuser.bin"
            } else {
                "ffboot.bin"
            };
            let ram = files
                .iter()
                .find(|f| f.name.eq_ignore_ascii_case(ram_filename))
                .ok_or_else(|| {
                    image::Error::Invalid("Missing RAM image for updater proof".into())
                })?;
            contract.verify(&ram.bytes)?;
        }
        files.push(File::new(&self.update_filename, source.to_vec())?);
        Ok(files)
    }
}
#[derive(Debug, Serialize)]
pub struct Availability {
    pub firmware_id: String,
    pub generation: Option<Generation>,
    pub can_build: bool,
    pub missing: Vec<String>,
    pub output_filenames: Vec<String>,
    pub sd_boot: Option<serde_json::Value>,
    pub firmware_update: Option<serde_json::Value>,
}
pub fn availability(firmware_id: &str) -> Result<Availability> {
    let id = firmware_id;
    let recipe = Recipe::bundled(id)?;
    let sd_boot = recipe.as_ref().and_then(|r| r.sd_boot.clone());
    let firmware_update = recipe.as_ref().and_then(|r| r.firmware_update.clone());
    let missing = recipe
        .as_ref()
        .map(|r| r.missing_facts())
        .unwrap_or_else(|| vec!["recovery recipe".into()]);
    let mut output_filenames = Vec::new();
    if let Some(recipe) = &recipe {
        match recipe.generation {
            Generation::Exr | Generation::ExrPro | Generation::Exr2 | Generation::Xpp => {
                if let Some(packaging) = &recipe.packaging {
                    output_filenames.extend([
                        packaging.header_filename.clone(),
                        packaging.image_filename.clone(),
                    ]);
                }
                if matches!(recipe.generation, Generation::Exr | Generation::ExrPro) {
                    output_filenames.push("ffuser.bin".into());
                }
            }
            Generation::Xp4 => output_filenames.extend(["ffinit.bin".into(), "ffboot.bin".into()]),
            Generation::Xp5 => {
                output_filenames.extend(["marble_init.bin".into(), "ffboot.bin".into()])
            }
        }
        output_filenames.push(recipe.update_filename.clone());
    }
    Ok(Availability {
        firmware_id: id.into(),
        generation: recipe.map(|r| r.generation),
        can_build: missing.is_empty(),
        missing,
        output_filenames,
        sd_boot,
        firmware_update,
    })
}

#[cfg(test)]
mod tests;
