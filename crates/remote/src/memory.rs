//! Bounded live-memory observation. No reads advance execution.
use crate::{Control, InputState, ReadSet, Readiness, Result, State, backend::Backend};
use fujimod_catalog_controls::facts::{ConfigurationLocation, DecodedTouch};
use fujimod_catalog_remote::Facts;

pub struct Memory<'a, B> {
    pub backend: &'a mut B,
    pub facts: &'a Facts,
    pub reads: ReadSet,
    /// One observation owns the backend's native read scope.
    pub scope: bool,
    pub(crate) depth: usize,
}

pub(crate) fn checked_address(base: u64, offset: u64) -> Result<u64> {
    base.checked_add(offset)
        .ok_or_else(|| "menu memory address overflow".into())
}

pub(crate) fn checked_indexed_address(
    base: u64,
    index: u64,
    stride: u64,
    offset: u64,
) -> Result<u64> {
    let indexed = index
        .checked_mul(stride)
        .ok_or("menu record offset overflow")?;
    checked_address(checked_address(base, indexed)?, offset)
}

impl<B: Backend> Memory<'_, B> {
    pub(crate) fn begin_observation(&mut self) -> Result<()> {
        if self.scope {
            if self.depth == 0 {
                self.backend.read_set_begin()?;
            }
            self.depth += 1;
        }
        Ok(())
    }
    pub(crate) fn end_observation(&mut self) -> Result<()> {
        if self.scope {
            self.depth = self
                .depth
                .checked_sub(1)
                .ok_or("Unbalanced memory observation")?;
            if self.depth == 0 {
                if let Some(reads) = self.backend.read_set_end()? {
                    self.reads = reads;
                }
            }
        }
        Ok(())
    }
    pub(crate) fn finish<T>(&mut self, result: Result<T>) -> Result<T> {
        let ended = self.end_observation();
        match (result, ended) {
            (Ok(value), Ok(())) => Ok(value),
            (Err(error), _) => Err(error),
            (_, Err(error)) => Err(error),
        }
    }
    pub async fn bytes(&mut self, address: u64, size: usize) -> Result<Vec<u8>> {
        let size_u64 = u64::try_from(size).map_err(|_| "Observation size exceeds 64 bits")?;
        if address == 0
            || size == 0
            || size > 64 * 1024 * 1024
            || address.checked_add(size_u64).is_none()
        {
            return Err(format!("Invalid observation range {address:#x}+{size}"));
        }
        self.begin_observation()?;
        let result = self.backend.memory(address, size, &mut self.reads).await;
        let bytes = self.finish(result)?;
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
    pub async fn uint32(&mut self, address: u64, width: u8) -> Result<u32> {
        u32::try_from(self.uint(address, usize::from(width)).await?)
            .map_err(|_| "Menu scalar exceeds 32 bits".into())
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
        self.begin_observation()?;
        let result = self.state_inner().await;
        self.finish(result)
    }
    async fn state_inner(&mut self) -> Result<State> {
        let menus = &self.facts.firmware.menus;
        menus.layout.validate()?;
        let f = &self.facts.firmware.state;
        let mode = u32::try_from(self.uint(f.ui_mode, f.widths.ui_mode).await?)
            .map_err(|_| "UI mode exceeds 32 bits")?;
        let active = self.uint(f.menu_active, f.widths.menu_active).await? != 0;
        let node = self.uint(f.menu_node, f.widths.menu_node).await?;
        let cursor_address = checked_address(
            menus.cursor_address,
            u64::from(menus.cursor.fields.cursor),
        )?;
        let cursor = self.word(cursor_address).await?;
        let current_key = if active && node != 0 && cursor < 1024 {
            let l = &menus.layout;
            let array_address = checked_address(node, u64::from(l.fields.item_array))?;
            let array = self
                .uint(array_address, usize::from(l.field_widths.item_array))
                .await?;
            if array == 0 {
                None
            } else {
                let key_address = checked_indexed_address(
                    array,
                    u64::from(cursor),
                    u64::from(l.row_size),
                    u64::from(l.row_key),
                )?;
                Some(self.uint32(key_address, l.row_key_width).await?)
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
        self.begin_observation()?;
        let result = self.readiness_inner(control).await;
        let mut readiness = self.finish(result)?;
        readiness.reads = self.reads.clone();
        Ok(readiness)
    }
    async fn readiness_inner(&mut self, control: Option<&Control>) -> Result<Readiness> {
        let f = &self.facts.firmware.state;
        let mut pending = vec![];
        let mode_pending = self.uint(f.pending_mode, f.widths.pending_mode).await?;
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
            self.uint(f.input_pending, f.input_pending_width).await?
        };
        let timer = if f.operation_alert_timer == 0 {
            0
        } else {
            self.word(f.operation_alert_timer).await?
        };
        let override_value = self.uint(f.override_address, f.override_width).await?;
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
                u32::try_from(
                    self.uint(
                        self.facts.firmware.state.ui_mode,
                        self.facts.firmware.state.widths.ui_mode,
                    )
                    .await?,
                )
                .map_err(|_| "UI mode exceeds 32 bits")?
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
        self.begin_observation()?;
        let result = self.text_inner(resource).await;
        self.finish(result)
    }
    async fn text_inner(&mut self, resource: u32) -> Result<String> {
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
            let glyph = (r.glyph_first..=r.glyph_last).contains(&u32::from(code));
            if !glyph
                && (r.private_text_first..=r.private_text_last).contains(&u32::from(code))
            {
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
