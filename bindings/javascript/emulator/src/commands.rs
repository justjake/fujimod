//! Host session operations. Unsupported lifecycle and input policies refuse.
use super::{
    lifecycle::BootInput,
    results::{
        CardFile, DeviceStates, DeviceWorkers, Preview, Profile, SessionState, TouchPhase,
        UsbStatus,
    },
    session::Session,
};
use fujimod_emulator_session::{
    card::{CardState, CardTree},
    history::Input,
};
use fujimod_emulator_boot::RecReport;

impl Session {
    fn card_node(&self) -> Result<&str, String> {
        self.camera
            .as_ref()
            .and_then(|m| m.card.as_ref())
            .map(|c| c.node.as_str())
            .ok_or("session has no declared SD card".into())
    }
    pub fn card_state(&self) -> Result<CardState, String> {
        match self.camera.as_ref().and_then(|m| m.card.as_ref()) {
            Some(c) => self.core.card_state(&c.node),
            None => Ok(CardState::unsupported()),
        }
    }
    pub fn fingerprint(&self) -> Result<super::results::Fingerprint, String> {
        super::fingerprint::session(&self.core, self.powered_on)
    }
    pub fn session_state(&self) -> Result<SessionState, String> {
        let (inputs, positions, input_queue) = if self.core.controls.is_some() {
            let state = self.core.control_state()?;
            (state.inputs, state.positions, state.input_queue)
        } else {
            Default::default()
        };
        Ok(SessionState {
            inputs,
            positions,
            input_queue,
            card: self.card_state()?,
            powered_on: self.powered_on,
        })
    }
    /// Replace the declared sensor scene with a RAF's mosaic.
    pub fn scene_raf(&mut self, raf: &[u8]) -> Result<(), String> {
        let scene = self
            .camera
            .as_ref()
            .and_then(|m| m.scene.as_ref())
            .ok_or("session has no declared scene input")?;
        let sensor = scene
            .sensor
            .clone()
            .ok_or("session has no declared sensor input")?;
        let images = scene.images.clone();
        let node = scene.node.clone();
        self.apply_marked_input(Input::SensorRaf {
            source: sensor,
            frame: self.core.decode_raf(raf)?,
        })?;
        self.apply_input(Input::SensorImage { source: images })?;
        self.apply_input(Input::Wake { node })
    }
    pub fn usb_status(&self, node: &str) -> Result<UsbStatus, String> {
        serde_json::from_value(self.core.usb_status(node)?).map_err(|e| e.to_string())
    }
    pub fn usb_reset(&mut self, node: &str) -> Result<(), String> {
        self.core.usb_reset(node)
    }
    pub fn usb_setup(&mut self, node: &str, packet: &[u8]) -> Result<(), String> {
        self.core.usb_setup(node, packet)
    }
    pub fn usb_out(&mut self, node: &str, endpoint: u8, packet: &[u8]) -> Result<(), String> {
        self.core.usb_out(node, endpoint, packet)
    }
    pub fn usb_in(&mut self, node: &str, endpoint: u8, size: usize) -> Result<Vec<u8>, String> {
        self.core.usb_in(node, endpoint, size)
    }
    pub fn usb_in_token(&mut self, node: &str, endpoint: u8) -> Result<(), String> {
        self.core.usb_in_token(node, endpoint)
    }
    pub fn card_tree(&self, revision: Option<u64>) -> Result<CardTree, String> {
        self.core.card_tree(self.card_node()?, revision)
    }
    pub fn card_read(
        &self,
        path: &str,
        revision: Option<u64>,
        max_bytes: Option<usize>,
    ) -> Result<Vec<u8>, String> {
        self.core.card_read(
            self.card_node()?,
            path,
            revision,
            max_bytes.unwrap_or(64 << 20),
        )
    }
    pub fn card_insert(&mut self, present: bool) -> Result<SessionState, String> {
        let node = self.card_node()?.to_owned();
        self.apply_marked_input(Input::CardContacts {
            node,
            present,
            protected: false,
        })?;
        self.session_state()
    }
    pub fn card_format(&mut self) -> Result<SessionState, String> {
        let card = self
            .camera
            .as_ref()
            .and_then(|m| m.card.as_ref())
            .ok_or("session has no declared SD card")?;
        let (node, format) = (card.node.clone(), card.format.clone());
        self.apply_marked_input(Input::CardFormat {
            node,
            options: format,
            files: vec![],
        })?;
        self.session_state()
    }
    pub fn set_position(&mut self, id: String, position: String) -> Result<SessionState, String> {
        // Validate the complete contact network before taking an Undo mark.
        self.core.admit_position(&id, &position)?;
        self.apply_marked_input(Input::Position {
            control: id,
            position,
        })?;
        self.session_state()
    }
    pub fn press(&mut self, id: String, stage: Option<String>) -> Result<SessionState, String> {
        self.core
            .admit_contact_input(&id, stage.as_deref(), false)?;
        self.apply_marked_input(Input::Press { control: id, stage })?;
        self.session_state()
    }
    pub fn release(&mut self, id: String) -> Result<SessionState, String> {
        self.core.admit_contact_input(&id, None, true)?;
        self.apply_marked_input(Input::Release { control: id })?;
        self.session_state()
    }
    pub fn rotate(&mut self, id: &str, steps: i32) -> Result<SessionState, String> {
        let (node, cycles) = self.core.rotation(id, steps)?;
        self.apply_marked_input(Input::Rotate { node, cycles })?;
        self.session_state()
    }
    pub fn touch(
        &mut self,
        id: &str,
        phase: TouchPhase,
        x: u16,
        y: u16,
    ) -> Result<SessionState, String> {
        let (node, script) = self.core.touch_input(id, phase.name(), x, y)?;
        self.apply_marked_input(Input::Touch { node, script })?;
        self.session_state()
    }
    /// Start the declared preview clock and report its frame state.
    pub fn preview(&mut self, start: bool) -> Result<Preview, String> {
        let preview = self
            .camera
            .as_ref()
            .and_then(|m| m.preview.clone())
            .ok_or("session has no declared preview policy")?;
        if start {
            self.apply_marked_input(Input::Frames {
                node: preview.node.clone(),
                period: preview.period_ticks,
                cadence: preview.cadence,
            })?;
        }
        let state = self.core.frame_state(&preview.node)?;
        Ok(Preview {
            ticks_per_second: preview.tick_hz.to_string(),
            frames: state["frames"]
                .as_u64()
                .ok_or("preview state has no frame count")?,
            state,
        })
    }
    pub fn device_workers(&mut self, count: Option<usize>) -> Result<DeviceWorkers, String> {
        if let Some(count) = count {
            self.core.board.set_device_workers(count)?;
        }
        Ok(DeviceWorkers {
            count: self.core.board.device_worker_preference(),
            effective: self.core.board.device_worker_count(),
        })
    }
    pub fn device_states(&self, kind: &str) -> Result<DeviceStates, String> {
        if kind.is_empty() {
            return Err("device kind must not be empty".into());
        }
        Ok(DeviceStates {
            states: self.core.board.device_states(kind)?,
        })
    }
    pub fn memory_read(&self, address: u64, size: usize) -> Result<Vec<u8>, String> {
        if size == 0 || size > 64 << 20 {
            return Err("memory read requires 1..67108864 bytes".into());
        }
        let mut bytes = vec![0; size];
        self.core.board.read_physical(address, &mut bytes)?;
        Ok(bytes)
    }
    pub fn watch_pcs(&mut self, addresses: &[u64]) -> Result<(), String> {
        self.core.board.watch_pcs(addresses)
    }
    pub fn profile(&self, limit: Option<usize>) -> Result<Profile, String> {
        let limit = limit.unwrap_or(1000);
        if limit > 10000 {
            return Err("profile limit exceeds 10000".into());
        }
        Ok(Profile {
            ticks: self.core.board.now(),
            instructions: self.core.board.instruction_counts(),
            pc_entries: self
                .core
                .board
                .pc_entries()
                .into_iter()
                .take(limit)
                .collect(),
            failed: self.failed,
        })
    }
    /// Native first-start and REC observation, bounded by an instruction budget.
    pub fn boot_to_rec(&mut self, limit: u64) -> Result<RecReport, String> {
        fujimod_emulator_boot::to_rec(&mut self.core, limit)
    }
    pub fn set_power(&mut self, on: bool) -> Result<SessionState, String> {
        self.power(on)?;
        self.session_state()
    }
    pub fn cycle_power(&mut self, inputs: &[BootInput]) -> Result<SessionState, String> {
        self.power_cycle(inputs)?;
        self.session_state()
    }
    /// Replace the card with `files`, packed contiguously into `upload`, and boot.
    pub fn replace_card(
        &mut self,
        files: Vec<CardFile>,
        upload: &[u8],
        inputs: &[BootInput],
    ) -> Result<SessionState, String> {
        let mut extents = Vec::new();
        let mut end = 0usize;
        for file in files {
            if file.offset != end {
                return Err("card file extents must be contiguous".into());
            }
            end = file
                .offset
                .checked_add(file.length)
                .ok_or("card file extent overflow")?;
            extents.push((
                file.path,
                upload
                    .get(file.offset..end)
                    .ok_or("card file extent exceeds upload")?
                    .to_vec(),
            ));
        }
        if end != upload.len() {
            return Err("unused card file upload bytes".into());
        }
        self.restart_card(&extents, inputs)?;
        self.session_state()
    }
    pub fn load_durable(&mut self, durable: &[u8]) -> Result<SessionState, String> {
        self.restore_durable(durable)?;
        self.session_state()
    }
    /// Show `rgb` (tightly packed, top row first) as the scene; `None` restores the analytic scene.
    pub fn set_scene(&mut self, scene: Option<(u32, u32, Vec<u8>)>) -> Result<(), String> {
        let scene_input = self
            .camera
            .as_ref()
            .and_then(|m| m.scene.as_ref())
            .ok_or("session has no declared scene input")?;
        let (images, sensor, node, white_luminance) = (
            scene_input.images.clone(),
            scene_input.sensor.clone(),
            scene_input.node.clone(),
            scene_input.white_luminance,
        );
        match scene {
            None => {
                if sensor.is_some() {
                    return Err(
                        "analytic sensor reset is not yet available in the Rust session".into(),
                    );
                }
                self.apply_marked_input(Input::Analytic { source: images })?;
            }
            Some((width, height, bytes)) => {
                if width == 0 || height == 0 || width > 8192 || height > 8192 {
                    return Err("invalid scene RGB dimensions".into());
                }
                let (width, height) = (width as usize, height as usize);
                if bytes.len() > 64 << 20 {
                    return Err("scene RGB exceeds 64 MiB".into());
                }
                if width * height * 3 != bytes.len() {
                    return Err("scene RGB length differs from its dimensions".into());
                }
                if let Some(sensor) = sensor {
                    self.apply_marked_input(Input::SensorRgb {
                        source: sensor,
                        width,
                        height,
                        bytes,
                        white_luminance: white_luminance
                            .ok_or("sensor scene has no declared white luminance")?,
                    })?;
                    self.apply_input(Input::SensorImage { source: images })?;
                } else {
                    self.apply_marked_input(Input::Rgb {
                        source: images,
                        width,
                        height,
                        bytes,
                    })?;
                }
            }
        }
        self.apply_input(Input::Wake { node })
    }
}
