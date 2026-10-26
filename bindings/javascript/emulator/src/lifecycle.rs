//! Cold power boundaries and portable committed storage for an owned camera.
//! CPU contexts and pending device operations never enter the durable format.
use super::{results::SavedMedia, session::Session};
use fujimod_emulator_session::durable::Media;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Arc};

pub(super) const MAX_DURABLE: usize = 256 << 20;
/// A control held while the camera powers on.
#[derive(Default, Deserialize, tsify::Tsify)]
#[serde(deny_unknown_fields)]
#[tsify(hashmap_as_object, missing_as_null)]
pub struct BootInput {
    pub id: String,
    #[serde(default)]
    #[tsify(optional)]
    pub stage: Option<String>,
    #[serde(default)]
    #[tsify(optional)]
    pub position: Option<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    schema: u32,
    identity: [u8; 32],
    powered_on: bool,
    positions: BTreeMap<String, String>,
    media: Media,
}
impl Session {
    fn durable(&self) -> Result<Saved, String> {
        let m = self
            .camera
            .as_ref()
            .ok_or("session has no external storage graph")?;
        let positions = if let Some(driver) = &self.core.contact_driver {
            driver.persistent_positions(&self.core)
        } else {
            self.core
                .controls
                .as_ref()
                .map(|p| p.positions().clone())
                .unwrap_or_default()
        };
        Ok(Saved {
            schema: 1,
            identity: m.identity,
            powered_on: self.powered_on,
            positions,
            media: Media::capture(&self.core, &m.storage)?,
        })
    }
    pub fn save_durable(&self) -> Result<SavedMedia, String> {
        let saved = self.durable()?;
        let mut bytes = Vec::new();
        ciborium::into_writer(&saved, &mut bytes).map_err(|e| e.to_string())?;
        if bytes.len() > MAX_DURABLE {
            return Err("durable storage exceeds 256 MiB".into());
        }
        Ok(SavedMedia {
            durable: bytes,
            positions: saved.positions,
            powered_on: saved.powered_on,
        })
    }
    pub(super) fn restore_durable(&mut self, bytes: &[u8]) -> Result<(), String> {
        if bytes.is_empty() || bytes.len() > MAX_DURABLE {
            return Err("durable storage requires 1..256 MiB".into());
        }
        let mut input = bytes;
        let saved: Saved = ciborium::from_reader(&mut input)
            .map_err(|e| format!("durable storage decode: {e}"))?;
        if !input.is_empty() {
            return Err("durable storage has trailing bytes".into());
        }
        self.cold(saved, &[], None)
    }
    pub(super) fn boot_holds(&mut self, inputs: &[BootInput]) -> Result<(), String> {
        if self.core.board.instruction_counts().iter().any(|n| *n != 0) {
            return Err("boot inputs require an unexecuted board".into());
        }
        if inputs.len() > 32 {
            return Err("boot input count exceeds 32".into());
        }
        let mut seen = std::collections::BTreeSet::new();
        for input in inputs {
            if !seen.insert(&input.id) || input.stage.is_some() && input.position.is_some() {
                return Err("duplicate or ambiguous boot input".into());
            }
            // USB boot conditions drive the declared cable wire, rather than
            // looking up a button in the physical contact catalog.
            if input.id == "usb" {
                if input.stage.is_some() {
                    return Err("USB boot input requires an explicit cable position".into());
                }
                let connected = match input.position.as_deref() {
                    Some("on") => true,
                    Some("off") => false,
                    _ => return Err("USB boot position must be on or off".into()),
                };
                self.core.logic_input("usb_cable", connected)?;
                continue;
            }
            let position = if let Some(p) = &input.position {
                p.clone()
            } else {
                self.core
                    .contact_driver
                    .as_ref()
                    .ok_or("boot hold requires the generated contact catalog")?
                    .boot_position(&input.id, input.stage.as_deref())?
            };
            self.core.set_position(&input.id, &position)?;
        }
        Ok(())
    }
    pub(super) fn release_boot_inputs(
        &mut self,
        inputs: &[BootInput],
    ) -> Result<super::results::SessionState, String> {
        let driver = self
            .core
            .contact_driver
            .as_ref()
            .ok_or("boot release requires the generated contact catalog")?;
        let poses = inputs
            .iter()
            .filter(|input| input.position.is_none())
            .map(|input| Ok((input.id.clone(), driver.boot_release_position(&input.id)?)))
            .collect::<Result<Vec<_>, String>>()?;
        // Boot holds are physical setup, not queued gestures. Service mode can
        // stop scanning ordinary controls after recognizing its power-on chord.
        for (id, position) in poses {
            self.core.set_position(&id, &position)?;
        }
        self.session_state()
    }
    fn cold(
        &mut self,
        mut saved: Saved,
        inputs: &[BootInput],
        files: Option<&[(String, Vec<u8>)]>,
    ) -> Result<(), String> {
        let reset = self
            .reset_image
            .as_ref()
            .ok_or("session has no checked reset image")?
            .clone();
        let metadata = self
            .camera
            .as_ref()
            .ok_or("cold storage metadata is missing")?;
        if saved.schema != 1 || metadata.identity != saved.identity {
            return Err("durable firmware/storage identity differs".into());
        }
        let generation = self
            .generation
            .checked_add(1)
            .ok_or("cold generation overflow")?;
        if let Some(files) = files {
            let card = metadata
                .card
                .as_ref()
                .ok_or("camera has no declared SD card")?;
            saved.media.merge_card_files(&card.node, files)?;
        }
        saved.media.validate(&self.core, &metadata.storage)?;
        // Validate complete physical poses before resetting any execution state.
        let mut panel = self
            .core
            .controls
            .as_ref()
            .map(|p| {
                fujimod_emulator_devices::parts::switch_panel::SwitchPanel::new(p.spec().clone())
            })
            .transpose()?;
        let mut poses = saved
            .positions
            .iter()
            .map(|(id, p)| (id.clone(), p.clone()))
            .collect::<Vec<_>>();
        if inputs.len() > 32 {
            return Err("boot input count exceeds 32".into());
        }
        let mut seen = std::collections::BTreeSet::new();
        for input in inputs {
            if !seen.insert(&input.id) || input.stage.is_some() && input.position.is_some() {
                return Err("duplicate or ambiguous boot input".into());
            }
            if input.id == "usb" {
                if input.stage.is_some() || !matches!(input.position.as_deref(), Some("on" | "off"))
                {
                    return Err("USB boot position must be on or off".into());
                }
                self.core.resources.logic_level("usb_cable")?;
                continue;
            }
            let position = if let Some(p) = &input.position {
                p.clone()
            } else {
                self.core
                    .contact_driver
                    .as_ref()
                    .ok_or("boot hold requires the generated contact catalog")?
                    .boot_position(&input.id, input.stage.as_deref())?
            };
            poses.push((input.id.clone(), position));
        }
        for (id, p) in &poses {
            panel = Some(
                panel
                    .as_ref()
                    .ok_or("physical controls are not connected")?
                    .transition(id, p)?,
            );
        }
        // External RAF input is a physical scene, independent of a cold reset.
        // Preserve its immutable frame instead of retaining and reparsing the file.
        let mosaic = self.core.resources.images()?.mosaic_frames()?;
        // This image was captured before the first instruction or caller holds.
        // Reuse the same execution owner and RAM reservation across cold resets.
        self.core.restore(&reset)?;
        for (name, frame) in &mosaic {
            self.core.sensor_frame(name, frame)?;
        }
        if !mosaic.is_empty()
            && let Some(scene) = &metadata.scene
        {
            self.core.sensor_image(&scene.images)?;
            self.core
                .board
                .inputs_changed(self.core.node(&scene.node)?)?;
        }
        saved.media.apply(&mut self.core, &metadata.storage)?;
        for (id, p) in poses {
            self.core.set_position(&id, &p)?;
        }
        if let Some(input) = inputs.iter().find(|input| input.id == "usb") {
            self.core
                .logic_input("usb_cable", input.position.as_deref() == Some("on"))?;
        }
        if !saved.powered_on {
            for old in self.frames.values_mut() {
                let mut frame = (**old).clone();
                frame.rgba.fill(0);
                for p in frame.rgba.chunks_exact_mut(4) {
                    p[3] = 255;
                }
                frame.sequence = frame.sequence.saturating_add(1);
                *old = Arc::new(frame);
            }
        } else {
            self.frames.clear();
        }
        self.history = None;
        self.failed = false;
        self.powered_on = saved.powered_on;
        self.generation = generation;
        Ok(())
    }
    pub(super) fn power(&mut self, on: bool) -> Result<(), String> {
        if self.powered_on == on {
            return Ok(());
        }
        let mut saved = self.durable()?;
        saved.powered_on = on;
        self.cold(saved, &[], None)
    }
    pub(super) fn power_cycle(&mut self, inputs: &[BootInput]) -> Result<(), String> {
        let mut saved = self.durable()?;
        saved.powered_on = true;
        self.cold(saved, inputs, None)
    }
    pub(super) fn restart_card(
        &mut self,
        files: &[(String, Vec<u8>)],
        inputs: &[BootInput],
    ) -> Result<(), String> {
        let mut saved = self.durable()?;
        saved.powered_on = true;
        self.cold(saved, inputs, Some(files))
    }
}
