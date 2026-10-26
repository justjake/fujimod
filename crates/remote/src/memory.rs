//! Bounded live-memory observation. No reads advance execution.
use crate::{Control, InputState, ReadSet, Readiness, Result, State, backend::Backend};
use fujimod_catalog_controls::facts::{ConfigurationLocation, DecodedTouch};
use fujimod_catalog_remote::Facts;

pub struct Memory<'a, B> {
    pub backend: &'a mut B,
    pub facts: &'a Facts,
    pub reads: ReadSet,
}
impl<B: Backend> Memory<'_, B> {
    pub async fn bytes(&mut self, address: u64, size: usize) -> Result<Vec<u8>> {
        if address == 0
            || size == 0
            || size > 64 * 1024 * 1024
            || address.checked_add(size as u64).is_none()
        {
            return Err(format!("Invalid observation range {address:#x}+{size}"));
        }
        let bytes = self.backend.memory(address, size, &mut self.reads).await?;
        if bytes.len() != size {
            return Err(format!(
                "Truncated observation at {address:#x}: {}/{size}",
                bytes.len()
            ));
        }
        Ok(bytes)
    }
    pub async fn uint(&mut self, address: u64, size: usize) -> Result<u64> {
        if !matches!(size, 1 | 2 | 4 | 8) {
            return Err("Invalid integer observation width".into());
        }
        let bytes = self.bytes(address, size).await?;
        let mut word = [0; 8];
        word[..size].copy_from_slice(&bytes);
        Ok(u64::from_le_bytes(word))
    }
    pub async fn word(&mut self, address: u64) -> Result<u32> {
        Ok(self.uint(address, 4).await? as u32)
    }
    pub async fn location(&mut self, value: ConfigurationLocation) -> Result<u64> {
        match value {
            ConfigurationLocation::Direct { address } => Ok(address),
            ConfigurationLocation::Pointer { address } => Ok(self.word(address).await?.into()),
        }
    }
    pub async fn state(&mut self) -> Result<State> {
        let f = &self.facts.firmware.state;
        let mode = self.word(f.ui_mode).await?;
        let active = self.word(f.menu_active).await? != 0;
        let node = self.word(f.menu_node).await?.into();
        let cursor = self
            .word(
                self.facts.firmware.menus.cursor_address
                    + u64::from(self.facts.firmware.menus.cursor.fields.cursor),
            )
            .await?;
        let current_key = if active && node != 0 && cursor < 1024 {
            let l = &self.facts.firmware.menus.layout;
            let array = self.word(node + u64::from(l.fields.item_array)).await?;
            if array == 0 {
                None
            } else {
                Some(
                    self.word(
                        u64::from(array)
                            + u64::from(cursor) * u64::from(l.row_size)
                            + u64::from(l.row_key),
                    )
                    .await?,
                )
            }
        } else {
            None
        };
        Ok(State {
            mode,
            menu_active: active,
            node,
            cursor,
            current_key,
        })
    }
    /// Every check that must pass before the firmware accepts a new input.
    pub async fn readiness(&mut self, control: Option<&Control>) -> Result<Readiness> {
        let f = &self.facts.firmware.state;
        let mut pending = vec![];
        let mode_pending = self.word(f.pending_mode).await?;
        if mode_pending != 0 {
            pending.push(format!("pending UI mode={mode_pending}"));
        }
        let mut parameter_independent = false;
        if let Some(control) = control {
            for input in &self.facts.parameter_independent_inputs.clone() {
                if input.control != control.id() {
                    continue;
                }
                let mut matches = true;
                for state in &input.conditions {
                    matches &= state
                        .values
                        .contains(&self.uint(state.address, state.width).await?);
                }
                parameter_independent |= matches;
            }
        }
        let f = &f.readiness;
        let one = self.uint(f.category_one, 8).await?;
        let two = self.uint(f.category_two, 8).await?;
        let input = if f.input_pending == 0 {
            0
        } else {
            self.word(f.input_pending).await?
        };
        let timer = if f.operation_alert_timer == 0 {
            0
        } else {
            self.word(f.operation_alert_timer).await?
        };
        let override_value = self.word(f.override_address).await?;
        // REC parameter readiness observes category-one bit 9. Other
        // category-one operations may remain active while inputs are accepted.
        if !parameter_independent && one & (1 << 9) != 0 {
            pending.push(format!("UI category one={one:#x}"));
        }
        if !parameter_independent
            && (two & !u64::from(f.excluded_mask) != 0
                || override_value != 1 && two & u64::from(f.excluded_mask) != 0)
        {
            pending.push(format!(
                "UI category two={two:#x}, override={override_value}"
            ));
        }
        if !parameter_independent && input != 0 {
            pending.push(format!("input pending={input}"));
        }
        if timer != 0 {
            pending.push(format!("operation alert timer={timer}"));
        }
        // Every observation resolves task identity again. All descriptor and
        // receive-state reads remain in the predicate's continuation read set.
        if let Some(tasks) = self.facts.input_tasks.clone() {
            let mode = if tasks.busy.is_empty() {
                0
            } else {
                self.word(self.facts.firmware.state.ui_mode).await?
            };
            for gate in &tasks.busy {
                if mode == gate.mode && self.uint(gate.address, gate.width).await? != 0 {
                    pending.push(format!("Input operation busy at {:#x}", gate.address));
                }
            }
            let mut found = std::collections::BTreeMap::new();
            for header in &tasks.headers {
                let table = u64::from(self.word(*header).await?) & tasks.address_mask;
                let count = self.word(*header + 4).await?;
                let size = u64::from(count)
                    .checked_mul(tasks.stride)
                    .ok_or("Task table size overflow")?;
                if count == 0
                    || count > tasks.max_count
                    || table == 0
                    || table
                        .checked_add(size)
                        .is_none_or(|end| end >= tasks.address_limit)
                {
                    return Err("Input task table is unavailable".into());
                }
                for index in 0..count {
                    let tcb = table + u64::from(index) * tasks.stride;
                    let descriptor = u64::from(self.word(tcb + tasks.descriptor_offset).await?)
                        & tasks.address_mask;
                    if descriptor == 0
                        || descriptor
                            .checked_add(tasks.descriptor_size)
                            .is_none_or(|end| end >= tasks.address_limit)
                    {
                        continue;
                    }
                    let entry = self.word(descriptor + tasks.entry_offset).await?;
                    if tasks.entries.contains(&entry) && found.insert(entry, tcb).is_some() {
                        return Err(format!("Input task is ambiguous: {entry:#x}"));
                    }
                }
            }
            for entry in &tasks.entries {
                let tcb = found
                    .get(entry)
                    .ok_or_else(|| format!("Input task is missing: {entry:#x}"))?;
                if self.uint(tcb + tasks.state_offset, 1).await? != u64::from(tasks.receive_state) {
                    pending.push(format!("Input task {entry:#x} is not receiving"));
                }
            }
        }
        Ok(Readiness {
            ready: pending.is_empty(),
            pending,
            reads: self.reads.clone(),
        })
    }
    pub async fn text(&mut self, resource: u32) -> Result<String> {
        let r = &self.facts.firmware.resources;
        if resource == 0 || resource == 0x7ffffffe {
            return Ok(String::new());
        }
        if resource > r.last_caption {
            // Literal captions are bounded UTF-8 strings, read from live memory.
            let mut bytes = vec![];
            for i in 0..256 {
                let b = self.uint(u64::from(resource) + i, 1).await? as u8;
                if b == 0 {
                    return String::from_utf8(bytes).map_err(|e| e.to_string());
                }
                bytes.push(b);
            }
            return Err(format!("Unterminated literal caption {resource:#x}"));
        }
        let (pointer, limit) = if resource >= r.supplementary_first {
            (
                r.supplementary_address
                    + u64::from(resource - r.supplementary_first)
                        * u64::from(r.supplementary_stride),
                r.supplementary_max_units,
            )
        } else {
            let base = self.location(r.language).await?;
            if base == 0 {
                return Err("Language catalog has not been selected".into());
            }
            let record = base + u64::from(resource) * u64::from(r.text_record_size);
            let variants = self
                .uint(record + u64::from(r.text_variants_offset), 1)
                .await?;
            let metrics = self.word(record + u64::from(r.text_metrics_offset)).await?;
            if variants == 0 || metrics == 0 {
                return Err(format!("Caption {resource:#x} is not loaded"));
            }
            (
                u64::from(
                    self.word(u64::from(metrics) + u64::from(r.text_pointer_offset))
                        .await?,
                ),
                r.text_max_units,
            )
        };
        if limit > 4096 || r.text_unit_bytes != 2 {
            return Err("Unsupported text layout".into());
        }
        let mut units = vec![];
        for i in 0..limit {
            let mut code = self
                .uint(pointer + u64::from(i * r.text_unit_bytes), 2)
                .await? as u16;
            if code == 0 {
                return String::from_utf16(&units).map_err(|e| e.to_string());
            }
            if u32::from(code) >= r.glyph_first && u32::from(code) <= r.glyph_last {
                continue;
            }
            if u32::from(code) >= r.private_text_first && u32::from(code) <= r.private_text_last {
                code = (u32::from(code) - r.private_text_bias) as u16;
            }
            units.push(code);
        }
        Err(format!("Unterminated caption {resource:#x}"))
    }
}
pub fn word(bytes: &[u8], offset: u32) -> Result<u32> {
    let start = offset as usize;
    let raw = bytes
        .get(start..start.checked_add(4).ok_or("Layout offset overflow")?)
        .ok_or("Word exceeds supplied layout")?;
    Ok(u32::from_le_bytes(
        raw.try_into().map_err(|_| "Invalid word")?,
    ))
}
/// Bytes spanning every field of a decoded touch report.
pub fn touch_report_size(layout: &DecodedTouch) -> usize {
    layout
        .contacts
        .iter()
        .flat_map(|c| [c.active + 1, c.x + 2, c.y + 2])
        .chain([layout.count + 1])
        .max()
        .unwrap_or(1) as usize
}
/// Whether the firmware's latest decoded touch report shows this touch input.
/// A finger down or moved is the one decoded contact, at its coordinates; a
/// lifted finger leaves a report with no contacts.
pub fn touch_decoded(layout: &DecodedTouch, report: &[u8], state: &InputState) -> Result<bool> {
    let byte = |offset: u32| {
        report
            .get(offset as usize)
            .copied()
            .ok_or_else(|| "Decoded touch report is truncated".to_string())
    };
    let half = |offset: u32| -> Result<u32> {
        Ok(u32::from(u16::from_le_bytes([
            byte(offset)?,
            byte(offset + 1)?,
        ])))
    };
    let count = byte(layout.count)?;
    match state {
        InputState::TouchStart { x, y } | InputState::TouchMove { x, y } => {
            if count != 1 {
                return Ok(false);
            }
            for contact in &layout.contacts {
                if byte(contact.active)? != 0 && half(contact.x)? == *x && half(contact.y)? == *y {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        InputState::TouchEnd => Ok(count == 0),
        _ => Err("Decoded touch reports accept only touch inputs".into()),
    }
}
