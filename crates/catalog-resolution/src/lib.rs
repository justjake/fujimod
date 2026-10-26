//! Run configurations resolved against the reference catalog.
//!
//! A run configuration names a body and firmware; resolution selects their
//! reference records, applies the named overlays, and checks that every
//! block, connection, part and profile identity the run uses exists in the
//! selected hardware.

use fujimod_catalog::{ReferenceCatalog, ResolvedSelection, reference::SocBlockKind};
use fujimod_catalog_configuration::application_inputs::*;
use fujimod_catalog_configuration::*;
use fujimod_catalog_construction::HardwareSelection;
use fujimod_catalog_run_inputs::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// A run configuration and the hardware and firmware identities it resolves.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(deny_unknown_fields))]
#[cfg_attr(
    all(target_arch = "wasm32", not(target_os = "emscripten")),
    derive(tsify::Tsify),
    tsify(hashmap_as_object, missing_as_null)
)]
pub struct ResolvedConfiguration {
    /// Validated run configuration selected by identity.
    pub configuration: RunConfiguration,
    /// Resolved SoC, body, firmware and installed parts.
    pub reference: ResolvedSelection,
}

impl ResolvedConfiguration {
    /// Load a bundled run configuration and resolve all selected catalog IDs.
    pub fn bundled(id: &str) -> Result<Self, String> {
        let configuration = RunConfiguration::bundled(id)?;
        let overlays = configuration.overlays.clone();
        Self::resolve_and_apply(configuration, &overlays)
    }

    /// Resolve a base run configuration and apply its named input overlays.
    pub fn with_overlays(configuration_id: &str, overlay_ids: &[String]) -> Result<Self, String> {
        let configuration = RunConfiguration::bundled(configuration_id)?;
        let mut selected = configuration.overlays.clone();
        selected.extend(overlay_ids.iter().cloned());
        Self::resolve_and_apply(configuration, &selected)
    }

    /// Resolve the selected caller-neutral body reset inputs, if the run names one.
    pub fn hardware_inputs(&self) -> Result<Option<HardwareInputs>, String> {
        let Some(id) = self
            .configuration
            .preset
            .as_ref()
            .and_then(|preset| preset.hardware_inputs_id.as_deref())
        else {
            return Ok(None);
        };
        let inputs = HardwareInputs::bundled(id)?;
        if inputs.body_id != self.configuration.body_id {
            return Err(format!(
                "hardware-input profile {id} belongs to body {}, not {}",
                inputs.body_id, self.configuration.body_id
            ));
        }
        Ok(Some(inputs))
    }

    fn resolve_and_apply(
        mut configuration: RunConfiguration,
        overlay_ids: &[String],
    ) -> Result<Self, String> {
        let catalog = ReferenceCatalog::bundled()?;
        let reference = catalog.resolve(&configuration.body_id, &configuration.firmware_id)?;
        validate_selection(&configuration, &reference)?;
        let mut applied = BTreeSet::new();
        let mut changed = BTreeSet::new();
        configuration.overlays.clear();
        for overlay_id in overlay_ids {
            if !applied.insert(overlay_id.as_str()) {
                return Err(format!(
                    "run overlay {overlay_id} is selected more than once"
                ));
            }
            let overlay = RunOverlay::bundled(overlay_id)?;
            overlay.validate(overlay_id, &configuration.id)?;
            match overlay.change {
                RunOverlayChange::Preset { mut preset } => {
                    if !changed.insert("preset") {
                        return Err("run overlays select a device preset more than once".into());
                    }
                    preset.validate()?;
                    if let Some(base) = &configuration.preset {
                        if preset.execution_scope.is_none() {
                            preset.execution_scope = base.execution_scope.clone();
                        }
                        if preset.hardware_selection.is_none() {
                            preset.hardware_selection = base.hardware_selection.clone();
                        }
                        if preset.hardware_inputs_id.is_none() {
                            preset.hardware_inputs_id = base.hardware_inputs_id.clone();
                        }
                        if preset.firmware_profile_id.is_none() {
                            preset.firmware_profile_id = base.firmware_profile_id.clone();
                        }
                    }
                    if let Some(purpose) = preset.purpose {
                        if !changed.insert("purpose") {
                            return Err("run overlays change purpose more than once".into());
                        }
                        configuration.purpose = purpose;
                    }
                    if let Some(start) = preset.start {
                        if !changed.insert("start") {
                            return Err("run overlays change the start entry more than once".into());
                        }
                        configuration.start = start;
                    }
                    if let Some(connected) = preset.usb_connected {
                        if !changed.insert("usb") {
                            return Err("run overlays change USB connection more than once".into());
                        }
                        if let Some(RunInputs::Exr2(inputs)) = &mut configuration.inputs {
                            inputs.usb.connected = connected;
                        }
                    }
                    if let Some(entry) = &preset.factory_options.entry {
                        if !changed.insert("start") {
                            return Err("run overlays change the start entry more than once".into());
                        }
                        configuration.start = match entry {
                            RunEntry::Loader => RunStart::Loader,
                        };
                    }
                    if let Some(camera) = &preset.browser_camera {
                        if !changed.insert("browser_camera") {
                            return Err(
                                "run overlays declare the browser camera more than once".into()
                            );
                        }
                        configuration.browser_camera = Some(camera.clone());
                    }
                    if let Some(execution) = preset.execution {
                        if !changed.insert("execution") {
                            return Err(
                                "run overlays change the execution policy more than once".into()
                            );
                        }
                        configuration.execution = Some(execution);
                    }
                    configuration.preset = Some(preset);
                }
                RunOverlayChange::UsbConnection { connected } => {
                    if !changed.insert("usb") {
                        return Err("run overlays change USB connection more than once".into());
                    }
                    let Some(RunInputs::Exr2(inputs)) = &mut configuration.inputs else {
                        return Err(format!(
                            "run overlay {overlay_id} requires declared EXR2 USB input state"
                        ));
                    };
                    inputs.usb.connected = connected;
                }
                RunOverlayChange::Start { start } => {
                    if !changed.insert("start") {
                        return Err("run overlays change the start entry more than once".into());
                    }
                    configuration.start = start;
                }
                RunOverlayChange::Purpose { purpose } => {
                    if !changed.insert("purpose") {
                        return Err(
                            "run overlays change the operation purpose more than once".into()
                        );
                    }
                    configuration.purpose = purpose;
                }
                RunOverlayChange::Execution { execution } => {
                    if !changed.insert("execution") {
                        return Err(
                            "run overlays change the execution policy more than once".into()
                        );
                    }
                    if execution.quantum == 0 {
                        return Err("run execution quantum must be positive".into());
                    }
                    execution.instructions_per_tick.validate()?;
                    configuration.execution = Some(execution);
                }
                RunOverlayChange::BrowserCamera { browser_camera } => {
                    if !changed.insert("browser_camera") {
                        return Err("run overlays declare the browser camera more than once".into());
                    }
                    browser_camera.validate()?;
                    configuration.browser_camera = Some(browser_camera);
                }
            }
            configuration.overlays.push(overlay.id);
        }
        validate_selection(&configuration, &reference)?;
        Ok(Self {
            configuration,
            reference,
        })
    }
}

/// Check that every run-input identity exists in the selected hardware.
pub fn validate_selection(
    configuration: &RunConfiguration,
    selection: &ResolvedSelection,
) -> Result<(), String> {
    if selection.body.id != configuration.body_id
        || selection.soc.id != selection.body.soc_id
        || selection.firmware.id != configuration.firmware_id
        || selection.firmware.body_id != configuration.body_id
    {
        return Err("run configuration identities differ from resolved hardware".into());
    }

    if let Some(hardware) = configuration
        .preset
        .as_ref()
        .and_then(|preset| preset.hardware_selection.as_ref())
    {
        hardware_profiles(hardware, selection)?;
    }

    if let Some(preset) = &configuration.preset {
        if let Some(id) = &preset.firmware_profile_id {
            if !selection
                .firmware
                .layouts
                .as_ref()
                .is_some_and(|layouts| layouts.profiles.contains_key(id))
            {
                return Err(format!(
                    "firmware {} has no layout profile {id}",
                    selection.firmware.id
                ));
            }
        }
        if !preset.native_device_inputs.is_empty() && selection.soc.id != "exr2" {
            return Err("native EXR2 input policies require an EXR2 SoC".into());
        }
        for medium in preset.nand_inputs.values() {
            let RunNandSceneInput::Raw { options, .. } = medium;
            let geometry = selection
                .body
                .nand
                .as_ref()
                .ok_or("run NAND input has no selected medium")?;
            if options.bytes != geometry.bytes
                || options.page_bytes != geometry.page_bytes
                || options.pages_per_block != geometry.pages_per_block
                || options.spare_bytes != geometry.spare_bytes
            {
                return Err("run NAND input geometry differs from the selected body".into());
            }
        }
    }

    let input_model_matches = match &configuration.inputs {
        None => true,
        Some(RunInputs::Exr2(_)) => selection.soc.id == "exr2",
        Some(RunInputs::Xp4(_)) => selection.soc.id == "xp4",
        Some(RunInputs::Xp5(_)) => selection.soc.id == "xp5",
    };
    let scene_model_matches = match &configuration.scene {
        None => true,
        Some(RunScene::Exr2(_)) => selection.soc.id == "exr2",
        Some(RunScene::Xp4(_)) => selection.soc.id == "xp4",
        Some(RunScene::Xp5(_)) => selection.soc.id == "xp5",
    };
    if !input_model_matches || !scene_model_matches {
        return Err("run inputs or scene do not match the selected SoC model".into());
    }
    if let Some(scene) = &configuration.scene {
        scene_selection(scene, selection)?;
    }

    let inputs = match &configuration.inputs {
        None => return Ok(()),
        Some(RunInputs::Exr2(inputs)) => inputs,
        Some(RunInputs::Xp4(inputs)) => return application_inputs_selection(inputs, selection),
        Some(RunInputs::Xp5(inputs)) => return application_inputs_selection(inputs, selection),
    };

    let body = &selection.body;
    let soc = &selection.soc;
    let block = |id: &str, kind| {
        soc.blocks
            .iter()
            .any(|candidate| candidate.id == id && candidate.kind == kind)
    };
    if !block(
        &inputs.gpio.block_id,
        fujimod_catalog::reference::SocBlockKind::Gpio,
    ) {
        return Err(format!(
            "run GPIO input refers to an absent or non-GPIO SoC block {}",
            inputs.gpio.block_id
        ));
    }
    if !block(
        &inputs.adc.block_id,
        fujimod_catalog::reference::SocBlockKind::Adc,
    ) {
        return Err(format!(
            "run ADC input refers to an absent or non-ADC SoC block {}",
            inputs.adc.block_id
        ));
    }

    let connection = |id: &str| body.connections.iter().find(|item| item.id == id);
    for key in &inputs.gpio.keys {
        if !connection(&key.connection_id)
            .is_some_and(|item| item.kind == fujimod_catalog::reference::ConnectionKind::Control)
        {
            return Err(format!(
                "run key input refers to an absent or non-control body connection {}",
                key.connection_id
            ));
        }
    }
    for encoder in &inputs.encoders {
        let input = connection(&encoder.connection_id)
            .filter(|item| item.kind == fujimod_catalog::reference::ConnectionKind::Control)
            .ok_or_else(|| {
                format!(
                    "run encoder input refers to an absent or non-control connection {}",
                    encoder.connection_id
                )
            })?;
        let gpio_signals = input
            .signals
            .iter()
            .filter(|signal| {
                matches!(
                    signal,
                    fujimod_catalog::reference::ConnectionSignal::Gpio { .. }
                )
            })
            .count();
        if gpio_signals != 2 {
            return Err(format!(
                "run encoder input {} must resolve to two GPIO signals",
                encoder.connection_id
            ));
        }
    }
    for output in &inputs.gpio.outputs {
        let target = connection(&output.connection_id).ok_or_else(|| {
            format!(
                "run GPIO output refers to an absent body connection {}",
                output.connection_id
            )
        })?;
        if !matches!(
            target.signals.get(usize::from(output.signal_index)),
            Some(fujimod_catalog::reference::ConnectionSignal::Gpio { .. })
        ) {
            return Err(format!(
                "run GPIO output {} signal {} is absent or non-GPIO",
                output.connection_id, output.signal_index
            ));
        }
    }
    for state in &inputs.gpio.connections {
        if connection(&state.connection_id).is_none() {
            return Err(format!(
                "run connection input refers to an absent body connection {}",
                state.connection_id
            ));
        }
    }
    if !connection(&inputs.usb.connection_id)
        .is_some_and(|item| item.kind == fujimod_catalog::reference::ConnectionKind::Usb)
    {
        return Err(format!(
            "run USB input refers to an absent or non-USB body connection {}",
            inputs.usb.connection_id
        ));
    }

    let part = |id: &str| selection.parts.iter().find(|item| item.id == id);
    let mcu = body
        .microcontroller
        .as_ref()
        .filter(|item| item.part_id == inputs.mcu.part_id)
        .ok_or("run MCU input does not target the installed body MCU")?;
    let power = body
        .power_control
        .as_ref()
        .filter(|item| item.part_id == inputs.power.part_id)
        .ok_or("run power input does not target the installed body power controller")?;
    let card = body
        .card_slot
        .as_ref()
        .filter(|item| item.connection_id == inputs.card.connection_id)
        .ok_or("run card input does not target the body's card connection")?;
    if inputs.card.present && card.controller_id.is_none() {
        return Err("selected body has no supported removable-card controller".into());
    }
    if inputs.gpio.connections.iter().any(|state| {
        state.connection_id == inputs.usb.connection_id
            || state.connection_id == card.detect_connection_id
            || state.connection_id == card.write_protect_connection_id
    }) {
        return Err("run connection states overlap USB or card-slot inputs".into());
    }

    let mcu_part = part(&mcu.part_id).ok_or("body MCU part is unresolved")?;
    let (mcu_registers, packet_registers, clock_registers) = match &mcu_part.model {
        fujimod_catalog::reference::PartModel::Microcontroller {
            registers,
            packet_registers,
            clock_registers,
            ..
        } => (registers, packet_registers, clock_registers),
        _ => return Err("body MCU attachment refers to a non-MCU part".into()),
    };
    if mcu_registers.width_bytes != 1
        || mcu_registers.count == 0
        || packet_registers.width_bytes != 1
        || packet_registers.count != 16
        || clock_registers.width_bytes != 1
        || clock_registers.count != 4
    {
        return Err("body MCU part has an unsupported packet or clock register layout".into());
    }
    let in_range = |range: &fujimod_catalog::reference::RegisterRange, register: u32| {
        register >= range.first && register < range.first.saturating_add(range.count)
    };
    let packet_overlaps_clock = (0..packet_registers.count)
        .map(|offset| packet_registers.first.saturating_add(offset))
        .any(|register| in_range(clock_registers, register));
    let packet_overlaps_extent = packet_registers.first < mcu_registers.first
        || packet_registers
            .first
            .saturating_add(packet_registers.count)
            > mcu_registers.first.saturating_add(mcu_registers.count);
    let clock_overlaps_extent = clock_registers.first < mcu_registers.first
        || clock_registers.first.saturating_add(clock_registers.count)
            > mcu_registers.first.saturating_add(mcu_registers.count);
    if packet_overlaps_clock || packet_overlaps_extent || clock_overlaps_extent {
        return Err("body MCU sub-register layout overlaps or exceeds its extent".into());
    }
    let power_part = part(&power.part_id).ok_or("body power-controller part is unresolved")?;
    let power_registers = match &power_part.model {
        fujimod_catalog::reference::PartModel::PowerControl { status_registers } => {
            status_registers
        }
        _ => return Err("body power attachment refers to a non-power part".into()),
    };
    let register_in_range = |register: u8, range: &fujimod_catalog::reference::RegisterRange| {
        let first = u64::from(range.first);
        let end = first + u64::from(range.count);
        u64::from(register) >= first && u64::from(register) < end
    };
    for input in &inputs.power.spi_status {
        if !power_registers
            .iter()
            .any(|register| register.address == input.register)
        {
            return Err(format!(
                "run power input targets an undeclared status register {}",
                input.register
            ));
        }
    }
    for serial_input in &inputs.serial_registers {
        let target = part(&serial_input.part_id).ok_or_else(|| {
            format!(
                "run input refers to an absent body part {}",
                serial_input.part_id
            )
        })?;
        if !body
            .parts
            .iter()
            .any(|part_id| part_id == &serial_input.part_id)
        {
            return Err(format!(
                "run input refers to an uninstalled body part {}",
                serial_input.part_id
            ));
        }
        let range = match &target.model {
            fujimod_catalog::reference::PartModel::Microcontroller {
                registers: register_range,
                packet_registers,
                clock_registers,
                status_register,
                initial_registers,
            } => {
                for value in &serial_input.values {
                    let address = u32::from(value.register);
                    if in_range(packet_registers, address)
                        || in_range(clock_registers, address)
                        || (*status_register != Some(address)
                            && !initial_registers.contains(&address))
                    {
                        return Err(format!(
                            "run serial input register {} has no declared initialization role",
                            value.register
                        ));
                    }
                }
                Some(register_range)
            }
            fujimod_catalog::reference::PartModel::SerialOutput {
                endpoint: fujimod_catalog::reference::SerialOutputKind::Registers,
            } => None,
            _ => {
                return Err(format!(
                    "run serial-register input targets a non-register part {}",
                    serial_input.part_id
                ));
            }
        };
        if let Some(range) = range {
            for value in &serial_input.values {
                if !register_in_range(value.register, range) {
                    return Err(format!(
                        "run serial input register {} is outside part {}'s register range",
                        value.register, serial_input.part_id
                    ));
                }
            }
        }
    }
    Ok(())
}

/// Resolve both profile identities against already selected reference records.
fn hardware_profiles<'a>(
    choice: &HardwareSelection,
    selection: &'a ResolvedSelection,
) -> Result<
    (
        &'a fujimod_catalog::profiles::SocProfile,
        &'a fujimod_catalog::profiles::BodyProfile,
    ),
    String,
> {
    fujimod_catalog_identity::validate_id(&choice.soc_profile_id)?;
    fujimod_catalog_identity::validate_id(&choice.body_profile_id)?;
    let soc = selection
        .soc
        .hardware
        .as_ref()
        .and_then(|hardware| hardware.profiles.get(&choice.soc_profile_id))
        .ok_or_else(|| {
            format!(
                "SoC {} has no hardware profile {}",
                selection.soc.id, choice.soc_profile_id
            )
        })?;
    let body = selection
        .body
        .hardware
        .as_ref()
        .and_then(|hardware| hardware.profiles.get(&choice.body_profile_id))
        .ok_or_else(|| {
            format!(
                "body {} has no hardware profile {}",
                selection.body.id, choice.body_profile_id
            )
        })?;
    Ok((soc, body))
}

fn scene_selection(scene: &RunScene, selection: &ResolvedSelection) -> Result<(), String> {
    match scene {
        RunScene::Exr2(_) => scene.validate(),
        RunScene::Xp4(scene) | RunScene::Xp5(scene) => {
            application_scene_selection(scene, selection)
        }
    }
}

/// Check every block and installed-part identity against the selected hardware.
fn application_inputs_selection(
    inputs: &ApplicationRunInputs,
    selection: &ResolvedSelection,
) -> Result<(), String> {
    inputs.validate()?;
    let block = |id: &str| {
        selection
            .soc
            .blocks
            .iter()
            .find(|block| block.id == id)
            .ok_or_else(|| format!("application input refers to missing SoC block {id}"))
    };
    let part = |id: &str| {
        if !selection.body.parts.iter().any(|part| part == id) {
            return Err(format!("application input refers to uninstalled part {id}"));
        }
        Ok(())
    };
    for input in &inputs.blocks {
        let selected = block(input.block_id())?;
        let valid = match input {
            ApplicationBlockInput::Adc { .. } => selected.kind == SocBlockKind::Adc,
            ApplicationBlockInput::Gpio { .. } => selected.kind == SocBlockKind::Gpio,
            ApplicationBlockInput::SerialLine { .. } => {
                selected.kind == SocBlockKind::SerialController
            }
            ApplicationBlockInput::Flash { part_id, .. } => {
                part(part_id)?;
                true
            }
            ApplicationBlockInput::Temperature { .. } => true,
        };
        if !valid {
            return Err(format!(
                "application input uses block {} with the wrong hardware category",
                selected.id
            ));
        }
    }
    for input in &inputs.parts {
        part(input.part_id())?;
    }
    for timing in &inputs.timings {
        match timing {
            ApplicationTiming::Delay { block_ids, .. }
            | ApplicationTiming::ParentClock { block_ids, .. } => {
                for id in block_ids {
                    block(id)?;
                }
            }
            ApplicationTiming::Throughput { block_id, .. } => {
                block(block_id)?;
            }
            ApplicationTiming::StorageProgramming { part_id, .. } => part(part_id)?,
        }
    }
    Ok(())
}

/// Validate all scene consumers against the selected SoC's block identities.
fn application_scene_selection(
    scene: &ApplicationRunScene,
    selection: &ResolvedSelection,
) -> Result<(), String> {
    scene.validate()?;
    for id in scene
        .frames
        .iter()
        .flat_map(|frame| frame.block_ids.iter().map(String::as_str))
        .chain(scene.statistics.iter().map(|input| input.block_id.as_str()))
        .chain(
            scene
                .command_images
                .iter()
                .flat_map(|input| input.block_ids.iter().map(String::as_str)),
        )
    {
        if !selection.soc.blocks.iter().any(|block| block.id == id) {
            return Err(format!(
                "application scene refers to missing SoC block {id}"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_inputs_resolve_to_typed_blocks_connections_and_parts() {
        let config = RunConfiguration::bundled("x70-110-rec").unwrap();
        let catalog = ReferenceCatalog::bundled().unwrap();
        let selection = catalog.resolve("x70", "x70-110").unwrap();
        assert!(validate_selection(&config, &selection).is_ok());

        let mut invalid = config.clone();
        let Some(RunInputs::Exr2(inputs)) = invalid.inputs.as_mut() else {
            panic!("X70 run inputs must be EXR2 inputs");
        };
        inputs.adc.block_id = inputs.gpio.block_id.clone();
        assert!(validate_selection(&invalid, &selection).is_err());

        let mut invalid = config.clone();
        let Some(RunInputs::Exr2(inputs)) = invalid.inputs.as_mut() else {
            panic!("X70 run inputs must be EXR2 inputs");
        };
        inputs.usb.connection_id = "hdmi".into();
        assert!(validate_selection(&invalid, &selection).is_err());

        let mut invalid = config.clone();
        let Some(RunInputs::Exr2(inputs)) = invalid.inputs.as_mut() else {
            panic!("X70 run inputs must be EXR2 inputs");
        };
        inputs.encoders[0].connection_id = "s1".into();
        assert!(validate_selection(&invalid, &selection).is_err());

        let mut invalid = config.clone();
        let Some(RunInputs::Exr2(inputs)) = invalid.inputs.as_mut() else {
            panic!("X70 run inputs must be EXR2 inputs");
        };
        inputs.gpio.outputs[0].signal_index = 2;
        assert!(validate_selection(&invalid, &selection).is_err());

        let mut invalid = config;
        let Some(RunInputs::Exr2(inputs)) = invalid.inputs.as_mut() else {
            panic!("X70 run inputs must be EXR2 inputs");
        };
        inputs.mcu.part_id = inputs.power.part_id.clone();
        assert!(validate_selection(&invalid, &selection).is_err());
    }

    #[test]
    fn usb_overlay_connects_the_cable() {
        let resolved =
            ResolvedConfiguration::with_overlays("x70-110-rec", &["x70-110-rec-usb".into()])
                .unwrap();
        let Some(RunInputs::Exr2(inputs)) = resolved.configuration.inputs.as_ref() else {
            panic!("X70 run inputs must be EXR2 inputs");
        };
        assert!(inputs.usb.connected);
        assert_eq!(resolved.configuration.overlays, ["x70-110-rec-usb"]);
    }

    #[test]
    fn browser_camera_overlay_declares_card_still_capture_and_preview() {
        let resolved =
            ResolvedConfiguration::with_overlays("x70-110-rec", &["x70-110-browser-camera".into()])
                .unwrap();
        let browser = resolved.configuration.browser_camera.unwrap();
        assert_eq!(browser.virtual_card.label, "CAMERA");
        assert_eq!(browser.virtual_card.sectors, 0x0100_0000);
        assert_eq!(browser.virtual_card.partition_start, 8192);
        assert_eq!(browser.virtual_card.sectors_per_cluster, 64);
        assert_eq!(browser.preview.frames_per_second, 10);
        assert_eq!(resolved.configuration.overlays, ["x70-110-browser-camera"]);
    }

    #[test]
    fn browser_camera_refuses_unsupported_card_and_cadence() {
        let mut browser =
            ResolvedConfiguration::with_overlays("x70-110-rec", &["x70-110-browser-camera".into()])
                .unwrap()
                .configuration
                .browser_camera
                .unwrap();
        browser.virtual_card.label = "TOO-LONG-LABEL".into();
        assert!(browser.validate().is_err());
        browser.virtual_card.label = "CAMERA".into();
        browser.preview.frames_per_second = 0;
        assert!(browser.validate().is_err());
    }

    #[test]
    fn execution_overlay_declares_policy_once_for_a_base_without_one() {
        let resolved = ResolvedConfiguration::with_overlays(
            "x20-103",
            &["x20-103-discovery-execution".into()],
        )
        .unwrap();
        let execution = resolved.configuration.execution.unwrap();
        assert_eq!(execution.mode, RunExecutionMode::Strict);
        assert_eq!(execution.engine, RunEngine::Unicorn);
        assert_eq!(execution.quantum, 1000);
        assert_eq!(
            execution.instructions_per_tick,
            RunInstructionRate {
                numerator: 24,
                denominator: 1
            }
        );
        assert!(
            ResolvedConfiguration::with_overlays(
                "x20-103",
                &[
                    "x20-103-discovery-execution".into(),
                    "x20-103-discovery-execution".into(),
                ],
            )
            .is_err()
        );
    }

    #[test]
    fn every_bundled_run_identity_and_overlay_resolves_without_path_aliases() {
        for id in RunConfiguration::bundled_ids() {
            let resolved = ResolvedConfiguration::bundled(id)
                .unwrap_or_else(|error| panic!("run configuration {id}: {error}"));
            assert_eq!(resolved.configuration.id, id);
            resolved
                .hardware_inputs()
                .unwrap_or_else(|error| panic!("run configuration {id}: {error}"));
        }
        for id in RunOverlay::bundled_ids() {
            let overlay =
                RunOverlay::bundled(id).unwrap_or_else(|error| panic!("run overlay {id}: {error}"));
            let resolved =
                ResolvedConfiguration::with_overlays(&overlay.configuration_id, &[id.to_owned()])
                    .unwrap_or_else(|error| panic!("run overlay {id}: {error}"));
            assert_eq!(resolved.configuration.overlays, [id]);
        }
    }
}
