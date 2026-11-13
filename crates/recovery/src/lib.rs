//! SD card construction from explicit source identities and located recipes.
#![forbid(unsafe_code)]
pub mod image;
pub mod green_led;
pub mod early_led;
mod exr;
mod exr_loader;
mod exr_firmware;
mod exr_load_maps;
pub mod rom_sd;
pub use exr::Exr;
mod exr2;
pub use exr2::lamp::StartupLamp;
pub use exr2::footer::LoaderFooter;
pub use exr2::defaults::ConfigurationDefaults;
pub use exr2::startup_reads::StartupReads;
pub mod xpp;
mod xp4;
mod xp5;
use image::{File, Packaging, Plan, Result, require};
use serde::{Deserialize, Serialize};
pub use xp4::{ColdStart, FileApi, SdCardRead, Setup, sd_card_fast_read::PackedStage};
pub use xp5::{
    Board as SdBoard, CodeProof as Xp5CodeProof, Flash as Xp5Flash, Lamp as Xp5Lamp,
    Operation as SdOperation, RegisterWrite as Xp5RegisterWrite, Setup as Xp5Setup,
    SourceProof as Xp5SourceProof,
};

/// Optional diagnostics for XP5 startup construction. Zero preserves normal entry.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StartupOptions {
    pub success_hold_seconds: u32,
    /// XP5 only: keep the installed flash attached. The card supplies the RAM
    /// image alone; the prelude initializes and scans the installed flash as
    /// stage one does, and no card-resident tables, namespace or unit storage
    /// image are used (so none is needed).
    pub internal_flash: bool,
    /// With `internal_flash`: when the FFBOOT load fails (no file, a length or CRC
    /// mismatch, a read error), wait this many seconds and continue the stock boot from the
    /// installed flash. `None` keeps the loader's halt on any failure.
    pub fallback_wait_seconds: Option<u32>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub startup_lamp: Option<StartupLamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loader_footer: Option<LoaderFooter>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configuration_defaults: Option<ConfigurationDefaults>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub startup_reads: Option<StartupReads>,
    pub setup: Option<Setup>,
    pub xp5: Option<Xp5Setup>,
    pub cold_start: Option<ColdStart>,
    pub file_api: Option<FileApi>,
    pub sd_boot_combination: Option<serde_json::Value>,
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
        read: SdCardRead,
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
        let mut bytes = xp4::ffinit(
            &fujimod_firmware::invert(source),
            setup,
            api,
            payload,
            base,
            Some(workspace),
            read,
        )?;
        early_led::stage(self, source, &mut bytes, base, xp4::indicator_entry()?, ((api.workspace - setup.linked_sram_base) & !0xfff) as usize - 0x1000)?;
        let mut layout = xp4::layout(setup, api, base, read)?;
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
    /// XP4 recovery cards use the full-image layout that booted X-S10, X100V
    /// and X-Pro3 cameras, with the loader holding the green lamp on; the
    /// preloaded layout from `build_startup` has not booted.
    pub fn build(&self, source: &[u8]) -> Result<Vec<File>> {
        self.build_with_sd_card_read(source, SdCardRead::Rom)
    }
    /// Record FFBOOT's packed stage in an `SdCardRead::Fast` FFINIT from
    /// this recipe, so FFINIT performs the stage before entering RAM.
    pub fn bind_sd_card_packed_stage(&self, ffinit: &mut [u8], stage: &PackedStage) -> Result<()> {
        require(
            matches!(self.generation, Generation::Xp4),
            "The fast SD card read requires XP4",
        )?;
        let api = self
            .file_api
            .as_ref()
            .ok_or_else(|| image::Error::Invalid("Missing XP4 file API".into()))?;
        xp4::sd_card_fast_read::bind_packed_stage(ffinit, api.load_address, stage)
    }
    /// `build`, with an XP4 SD card's FFBOOT read selected. Other generations
    /// accept only `SdCardRead::Rom`.
    pub fn build_with_sd_card_read(&self, source: &[u8], read: SdCardRead) -> Result<Vec<File>> {
        require(
            read == SdCardRead::Rom || matches!(self.generation, Generation::Xp4),
            "The fast SD card read requires XP4",
        )?;
        let missing = self.missing_facts();
        require(
            missing.is_empty(),
            &format!("Missing recovery facts: {}", missing.join(", ")),
        )?;
        require(
            self.updater.is_some(),
            "Missing version-independent stock updater acceptance",
        )?;
        if matches!(self.generation, Generation::Xp4) {
            let missing = || image::Error::Invalid("Missing XP4 recovery facts".into());
            let mut files = xp4::sd_card_recovery(
                self,
                source,
                &fujimod_firmware::invert(source),
                self.plan.as_ref().ok_or_else(missing)?,
                self.cold_start.as_ref().ok_or_else(missing)?,
                self.setup.as_ref().ok_or_else(missing)?,
                self.file_api.as_ref().ok_or_else(missing)?,
                read,
            )?;
            files.push(File::new(&self.update_filename, source.to_vec())?);
            return Ok(files);
        }
        self.build_startup(source)
    }
    /// The XP4 full-image cold card: one RAM image read by FFINIT, no
    /// preloaded data file or startup indicator.
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
    /// `build_startup_with_options` without the update: exactly the two files the
    /// boot ROM and the loader read, `marble_init.bin` and `ffboot.bin` (an XP5
    /// startup). The update is an independent updater input; this never returns it,
    /// and any other output name is an error rather than silently passed on.
    pub fn build_boot_files(&self, source: &[u8], options: StartupOptions) -> Result<Vec<File>> {
        let files = self.build_startup_with_options(source, options)?;
        boot_files_only(files, &self.update_filename)
    }
    /// Include the unit's retained storage data in a checked card startup.
    pub fn build_startup_with_storage(
        &self, source: &[u8], storage: &[u8], options: StartupOptions,
    ) -> Result<Vec<File>> {
        require(matches!(self.generation, Generation::Xp5), "Card storage requires XP5")?;
        self.startup_inputs(source, options, Some(storage))
    }
    /// Identifies the `FFSYS.BIN` that `build_ota_startup` writes, without the
    /// update: firmwares with the same value share one card's boot files.
    pub fn ota_startup_header(&self) -> Option<String> {
        let packaging = self.packaging.as_ref()?;
        matches!(self.generation, Generation::Exr2).then(|| {
            let identity = serde_json::json!([packaging.header_sha256, packaging.edits]);
            fujimod_firmware::sha256(identity.to_string().as_bytes())
        })
    }
    /// Build an EXR II card entry that loads stage 1 from NAND, then stage 2.
    /// The board's memory setup header is verified against the supplied update;
    /// the packaged loaders are the same for every EXR II body.
    pub fn build_ota_startup(&self, source: &[u8]) -> Result<Vec<File>> {
        require(
            matches!(self.generation, Generation::Exr2),
            "OTA startup requires EXR II",
        )?;
        require(
            source.len() == self.source_size,
            "Update size differs from recipe",
        )?;
        image::hash(source, &self.source_sha256)?;
        let packaging = self
            .packaging
            .as_ref()
            .ok_or_else(|| image::Error::Invalid("Missing SD header packaging".into()))?;
        let inverted = fujimod_firmware::invert(source);
        // One universal stage-1 loader for every EXR II body: no early LED
        // table is staged in, so the same FFBOOT.BIN boots any of them.
        let stage1 = include_bytes!("../assets/sd-boot-v2-stage1-sd-card.generated.bin").to_vec();
        let mut files = packaging.package_bytes(&inverted, packaging.entry, stage1)?;
        files.push(File::new(
            "FM_EXR2.BIN",
            include_bytes!("../assets/sd-boot-v2-stage2.generated.bin").to_vec(),
        )?);
        Ok(files)
    }

    fn startup_inputs(&self, source: &[u8], options: StartupOptions, storage: Option<&[u8]>)
        -> Result<Vec<File>> {
        require(
            options.success_hold_seconds == 0 || matches!(self.generation, Generation::Xp5),
            "Success hold requires XP5",
        )?;
        require(
            !options.internal_flash
                || (matches!(self.generation, Generation::Xp5) && storage.is_none()),
            "Internal-flash startup requires XP5 and takes no storage image",
        )?;
        require(
            options.fallback_wait_seconds.is_none() || options.internal_flash,
            "The load-failure fallback requires an internal-flash startup",
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
                    let mut ram = image::transform(&inverted, plan)?;
                    green_led::preserve_loader_green(self, source, &mut ram)?;
                    let mut files = exr::package(
                        api,
                        header,
                        &inverted,
                        plan,
                        ram,
                    )?;
                    early_led::stage(self, source, &mut files[1].bytes, 0, 0, 0xffff)?;
                    files
                }
                Generation::Exr2 | Generation::Xpp => {
                    let packaging = self.packaging.as_ref().ok_or_else(|| {
                        image::Error::Invalid("Missing SD header packaging".into())
                    })?;
                    let mut ram = image::transform(&inverted, plan)?;
                    if matches!(self.generation, Generation::Exr2) {
                        exr2::install(plan, &mut ram)?;
                        if let Some(footer) = &self.loader_footer {
                            footer.install(&inverted, plan, &mut ram)?;
                        }
                        if let Some(lamp) = &self.startup_lamp {
                            lamp.install(plan, &mut ram)?;
                        }
                        if let Some(reads) = &self.startup_reads {
                            reads.install(&inverted, plan, &mut ram)?;
                        }
                        if let Some(defaults) = &self.configuration_defaults {
                            defaults.install(&inverted, plan, &mut ram)?;
                        }
                    } else {
                        xpp::install(&inverted, plan, &mut ram)?;
                    }
                    if self.startup_lamp.is_none() {
                        early_led::image(self, source, &mut ram)?;
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
                    let mut files = xp4::package(self, source, &inverted, plan, cold, setup, api)?;
                    early_led::stage(self, source, &mut files[0].bytes, api.load_address, xp4::indicator_entry()?,
                        ((api.workspace - setup.linked_sram_base) & !0xfff) as usize - 0x1000)?;
                    files
                }
                Generation::Xp5 => {
                    let setup = self.xp5.as_ref().ok_or_else(|| {
                        image::Error::Invalid(
                            "Missing XP5 post-setup SD loader and handoff facts".into(),
                        )
                    })?;
                    let ram = image::transform(&inverted, plan)?;
                    if options.internal_flash {
                        xp5::package_internal_flash(
                            &inverted,
                            setup,
                            ram,
                            options.success_hold_seconds,
                            options.fallback_wait_seconds,
                        )?
                    } else {
                        xp5::package_with_storage(
                            &inverted,
                            setup,
                            Some(ram),
                            options.success_hold_seconds,
                            storage,
                        )?
                    }
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
/// The only files `build_boot_files` returns.
const BOOT_FILES: [&str; 2] = ["marble_init.bin", "ffboot.bin"];

/// Drop the update from `files` and require exactly the boot files, once each, in that order.
fn boot_files_only(files: Vec<File>, update_filename: &str) -> Result<Vec<File>> {
    let mut boot = Vec::new();
    for file in files {
        if file.name.eq_ignore_ascii_case(update_filename) {
            continue;
        }
        require(
            BOOT_FILES.contains(&file.name.as_str()),
            "A startup output is neither a boot file nor the update",
        )?;
        boot.push(file);
    }
    require(
        boot.iter().map(|f| f.name.as_str()).eq(BOOT_FILES),
        "A startup lacks one of its boot files",
    )?;
    Ok(boot)
}

#[derive(Debug, Serialize)]
pub struct Availability {
    pub firmware_id: String,
    pub generation: Option<Generation>,
    pub can_build: bool,
    pub missing: Vec<String>,
    pub output_filenames: Vec<String>,
    pub sd_boot_combination: Option<serde_json::Value>,
    pub firmware_update: Option<serde_json::Value>,
    /// The system card turns the green lamp on during SD startup.
    pub system_startup_lamp: bool,
    /// See `Recipe::ota_startup_header`.
    pub ota_startup_header: Option<String>,
}
pub fn availability(firmware_id: &str) -> Result<Availability> {
    let id = firmware_id;
    let recipe = Recipe::bundled(id)?;
    let sd_boot_combination = recipe.as_ref().and_then(|r| r.sd_boot_combination.clone());
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
            Generation::Xp4 => {
                output_filenames.extend(["ffinit.bin".into(), "ffboot.bin".into()])
            }
            Generation::Xp5 => {
                output_filenames.extend(["marble_init.bin".into(), "ffboot.bin".into()])
            }
        }
        output_filenames.push(recipe.update_filename.clone());
    }
    let system_startup_lamp = recipe
        .as_ref()
        .is_some_and(|r| !matches!(r.generation, Generation::Xp5));
    let ota_startup_header = recipe.as_ref().and_then(Recipe::ota_startup_header);
    Ok(Availability {
        firmware_id: id.into(),
        generation: recipe.map(|r| r.generation),
        system_startup_lamp,
        can_build: missing.is_empty(),
        missing,
        output_filenames,
        sd_boot_combination,
        firmware_update,
        ota_startup_header,
    })
}

#[cfg(test)]
mod tests;
