//! Persist the debug-access byte through the stock one-byte range writer.
//!
//! The reset guard stays set in RAM and clear in storage. A borrowed callback
//! runs a no-write probe, then the writer exactly once. Cleanup restores only
//! recognized callback/scratch values; uncertain completion withholds all I/O.
use crate::{
    Limits,
    access::{Access, range},
    service_mode::{self, ServiceModeError},
};
use fujimod_catalog_camera::{CameraDefinition, CameraError, DebugMode};
use fujimod_usb::{
    ff80::{
        Session, SessionState,
        commands::{
            configuration::ConfigurationView,
            diagnostic::{CommandStatus, DiagnosticTag},
            info::InfoSelector,
            memory::Address,
        },
    },
    operations::{checks, configuration, memory},
};

#[derive(Debug, thiserror::Error)]
pub enum DebugModeError {
    #[error(transparent)]
    Usb(#[from] fujimod_usb::Error),
    #[error(transparent)]
    Definition(#[from] CameraError),
    #[error(transparent)]
    Guard(#[from] ServiceModeError),
    #[error("{0}")]
    Check(String),
    #[error("{operation}; cleanup failed: {cleanup}")]
    Cleanup {
        operation: String,
        cleanup: Box<DebugModeError>,
    },
}
pub type Result<T> = std::result::Result<T, DebugModeError>;

/// Verified storage value and whether it was already present.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Persisted {
    pub value: u8,
    pub already_persisted: bool,
}

/// Store debug access enabled, keeping the stored reset guard clear.
pub async fn enter(
    session: &mut Session,
    camera: &CameraDefinition,
    limits: Limits,
    progress: &mut (dyn FnMut(&str) + Send),
) -> Result<Persisted> {
    persist(session, camera, limits, progress, 1).await
}

/// Store debug access disabled, keeping the stored reset guard clear.
pub async fn leave(
    session: &mut Session,
    camera: &CameraDefinition,
    limits: Limits,
    progress: &mut (dyn FnMut(&str) + Send),
) -> Result<Persisted> {
    persist(session, camera, limits, progress, 0).await
}

fn require(condition: bool, message: &str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(DebugModeError::Check(message.into()))
    }
}

struct Transaction<'a> {
    access: Access<'a>,
    mode: DebugMode,
    old_scratch: Option<Vec<u8>>,
    old_outputs: Option<Vec<u8>>,
    old_flag: Option<u8>,
    callback_touched: bool,
    scratch_touched: bool,
    writer_attempted: bool,
}

async fn persist(
    session: &mut Session,
    camera: &CameraDefinition,
    limits: Limits,
    progress: &mut (dyn FnMut(&str) + Send),
    target: u8,
) -> Result<Persisted> {
    let mut transaction = Transaction {
        mode: camera.debug_mode()?.clone(),
        access: Access {
            session,
            camera,
            limits,
        },
        old_scratch: None,
        old_outputs: None,
        old_flag: None,
        callback_touched: false,
        scratch_touched: false,
        writer_attempted: false,
    };
    let outcome = transaction.execute(target, progress).await;
    let cleanup = transaction.cleanup(outcome.is_ok()).await;
    if let Err(error) = cleanup {
        // Equivalent to abandoning a failed cleanup: never send session close
        // or any further command when restoration could not be confirmed.
        transaction.access.session.invalidate_completion();
        return Err(DebugModeError::Cleanup {
            operation: match outcome {
                Ok(_) => "persistence verified".into(),
                Err(error) => error.to_string(),
            },
            cleanup: Box::new(error),
        });
    }
    outcome
}

impl Transaction<'_> {
    fn allowed_callback(&self, value: u32) -> bool {
        let mode = &self.mode;
        [
            mode.stock_callback,
            mode.adapter.0,
            mode.cache_clean,
            mode.instruction_invalidate,
            mode.barrier,
            mode.instruction_barrier,
        ]
        .contains(&value)
    }

    async fn set_flag(&mut self, value: u8) -> Result<()> {
        require(value <= 1, "Write outside the debug byte allowlist")?;
        let offset = self.access.camera.debug()?.flag_offset;
        Ok(self.access.set(offset, value).await?)
    }

    fn barrier_output(&self) -> Vec<u8> {
        [
            self.mode.output_fields.wrapping_add(4).to_le_bytes(),
            [0; 4],
        ]
        .concat()
    }

    async fn write(&mut self, address: u32, data: &[u8]) -> Result<()> {
        let callback = address == self.mode.callback
            && data.len() == 4
            && self.allowed_callback(u32::from_le_bytes(data.try_into().expect("word")));
        let output = address == self.mode.output_fields
            && (self.old_outputs.as_deref() == Some(data)
                || data == [0; 8]
                || data == self.barrier_output());
        let scratch = address == self.mode.adapter.0
            && (data == self.mode.adapter.1 || self.old_scratch.as_deref() == Some(data));
        require(
            callback || output || scratch,
            "RAM write outside the fixed persistence plan",
        )?;
        memory::write_range(
            self.access.session,
            Address(address),
            data,
            self.access.limits.memory,
        )
        .await?;
        require(
            self.access.raw(address, data.len() as u32).await? == data,
            "RAM write did not verify",
        )?;
        if address != self.mode.adapter.0 {
            require(
                self.access.uncached(address, data.len() as u32).await? == data,
                "RAM write aliases disagree",
            )?;
        }
        Ok(())
    }

    async fn restore_outputs(&mut self) -> Result<()> {
        let outputs = self
            .old_outputs
            .clone()
            .ok_or_else(|| DebugModeError::Check("Missing original output fields".into()))?;
        self.write(self.mode.output_fields, &outputs).await
    }

    async fn query(
        &mut self,
        callback: u32,
        tag: u32,
        void_result: bool,
        writer: bool,
        preinstalled: bool,
    ) -> Result<()> {
        require(
            self.allowed_callback(callback) && callback != self.mode.stock_callback,
            "Unplanned callback",
        )?;
        require(
            callback != self.mode.adapter.0 || matches!(tag, 0 | 1),
            "Unplanned adapter tag",
        )?;
        require(
            writer == (callback == self.mode.adapter.0 && tag == 1),
            "Writer identity mismatch",
        )?;
        require(
            !preinstalled || writer,
            "Only the fixed writer may use a preinstalled callback",
        )?;
        self.callback_touched = true;
        if !preinstalled {
            self.write(self.mode.callback, &callback.to_le_bytes())
                .await?;
        }
        if writer {
            require(!self.writer_attempted, "A storage write cannot be retried")?;
            self.writer_attempted = true;
        }
        let response = self
            .access
            .session
            .diagnostic_query(DiagnosticTag::Research(tag))
            .await?;
        if writer {
            // Even target zero leaves the configuration interface available.
            self.set_flag(1).await?;
            let offset = self.access.camera.debug()?.flag_offset;
            require(
                self.access.byte(offset).await? == 1,
                "Cannot re-enable RAM access after the writer",
            )?;
        }
        require(
            response.status == CommandStatus::Success
                || (void_result
                    && response.status
                        == CommandStatus::DeviceError {
                            primary: 1,
                            secondary: 0xffff,
                        }),
            "Query returned an unexpected error",
        )?;
        self.write(self.mode.callback, &self.mode.stock_callback.to_le_bytes())
            .await?;
        self.restore_outputs().await
    }

    async fn dsb(&mut self) -> Result<()> {
        self.write(self.mode.output_fields, &self.barrier_output())
            .await?;
        self.query(
            self.mode.barrier,
            self.mode.output_fields,
            true,
            false,
            false,
        )
        .await
    }

    async fn sync_code(&mut self) -> Result<()> {
        self.query(self.mode.cache_clean, 0, true, false, false)
            .await?;
        self.dsb().await?;
        self.query(self.mode.instruction_invalidate, 0, false, false, false)
            .await?;
        self.dsb().await?;
        self.query(self.mode.instruction_barrier, 0, false, false, false)
            .await
    }

    async fn capture_storage(&mut self) -> Result<Vec<u8>> {
        let offset = self.access.camera.debug()?.flag_offset;
        // One stock loader call per capture, then read the loaded buffers.
        let flag = configuration::read_byte(
            self.access.session,
            ConfigurationView::Storage,
            offset,
            self.access.limits.configuration,
        )
        .await?;
        let (address, words) = &self.mode.storage_geometry;
        let geometry = self.access.raw(*address, (words.len() * 4) as u32).await?;
        for (bytes, expected) in geometry.as_chunks::<4>().0.iter().zip(words) {
            if let Some(expected) = expected {
                require(
                    u32::from_le_bytes(*bytes) == *expected,
                    "Unexpected storage geometry",
                )?;
            }
        }
        let mut combined = Vec::new();
        for bank in &self.mode.storage_banks {
            combined.extend(self.access.raw(bank.address, bank.size).await?);
        }
        require(
            combined.get(offset as usize) == Some(&flag),
            "Storage getter and capture disagree",
        )?;
        Ok(combined)
    }

    async fn guard(&mut self, progress: &mut (dyn FnMut(&str) + Send)) -> Result<()> {
        service_mode::arm_reset_guard(
            self.access.session,
            self.access.camera,
            self.access.limits,
            progress,
        )
        .await?;
        let guard = self.access.camera.guard()?;
        let flag = self.access.camera.debug()?.flag_offset;
        require(
            self.access.byte(guard.offset).await? == 1 && self.access.byte(flag).await? == 0,
            "Guard preparation or cleanup did not verify",
        )
    }

    async fn execute(
        &mut self,
        target: u8,
        progress: &mut (dyn FnMut(&str) + Send),
    ) -> Result<Persisted> {
        let product = self
            .access
            .session
            .device_info(InfoSelector::Product)
            .await?
            .text;
        let normalized = product.to_uppercase().replace('-', "");
        require(
            self.access
                .camera
                .products
                .iter()
                .any(|name| normalized == name.to_uppercase().replace('-', "")),
            "Device did not identify as the selected camera",
        )?;
        let flag = self.access.camera.debug()?.flag_offset;
        let guard = self.access.camera.guard()?.clone();
        self.old_flag = Some(self.access.byte(flag).await?);
        self.guard(progress).await?;
        self.set_flag(1).await?;
        require(
            self.access.byte(flag).await? == 1 && self.access.byte(guard.offset).await? == 1,
            "Require live debug access and reset guard",
        )?;
        progress("Verifying persistence code");
        for check in guard.code_checks.iter().chain(&self.mode.code_checks) {
            if !checks::region_matches(
                self.access.session,
                range(
                    check.range.address | self.access.camera.ram_alias,
                    check.range.size,
                )?,
                self.access.limits.memory,
                check.sha256.bytes(),
            )
            .await?
            {
                return Err(DebugModeError::Check(format!(
                    "Required camera instruction bytes differ: {}",
                    check.name
                )));
            }
        }
        let (adapter, bytes) = self.mode.adapter.clone();
        let scratch = self.access.raw(adapter, bytes.len() as u32).await?;
        self.old_scratch = Some(scratch.clone());
        let outputs = self.access.raw(self.mode.output_fields, 8).await?;
        self.old_outputs = Some(outputs.clone());
        require(
            self.access.uncached(adapter, scratch.len() as u32).await? == scratch,
            "Initial scratch aliases disagree",
        )?;
        require(outputs == [0; 8], "Require idle zero output fields")?;
        let configuration = self.access.camera.configuration;
        let before_cfg = self
            .access
            .raw(configuration.address, configuration.size)
            .await?;
        let preferences = self.mode.preferences;
        let before_preferences = self
            .access
            .raw(preferences.address, preferences.size)
            .await?;
        progress("Reloading and capturing configuration storage: before");
        let before = self.capture_storage().await?;
        require(
            before.get(guard.offset as usize) == Some(&0)
                && before.get(flag as usize).is_some_and(|value| *value <= 1),
            "Require stored reset guard clear and debug byte in {0,1}",
        )?;
        let already_persisted = before[flag as usize] == target;
        if !already_persisted {
            progress("Staging the adapter and checking the no-write probe");
            self.scratch_touched = true;
            self.write(adapter, &bytes).await?;
            self.sync_code().await?;
            require(
                self.access.uncached(adapter, bytes.len() as u32).await? == bytes,
                "Adapter uncached alias did not verify after synchronization",
            )?;
            self.query(adapter, 0, false, false, false).await?;
            require(
                self.access.raw(self.mode.output_fields, 8).await? == outputs
                    && self.access.byte(guard.offset).await? == 1,
                "No-write probe changed output fields or guard",
            )?;
            self.write(self.mode.callback, &adapter.to_le_bytes())
                .await?;
            self.set_flag(target).await?;
            require(
                self.access.byte(flag).await? == target,
                "Target RAM debug flag did not verify",
            )?;
            progress("Calling the stock one-byte range save once");
            self.query(adapter, 1, false, true, true).await?;
            self.write(adapter, &scratch).await?;
            self.sync_code().await?;
            self.scratch_touched = false;
        }
        progress("Reloading and capturing configuration storage: after");
        let after = self.capture_storage().await?;
        let mut expected = before;
        expected[flag as usize] = target;
        require(
            after == expected && after[guard.offset as usize] == 0,
            "Persistent readback differs outside the planned debug byte",
        )?;
        let after_cfg = self
            .access
            .raw(configuration.address, configuration.size)
            .await?;
        let after_preferences = self
            .access
            .raw(preferences.address, preferences.size)
            .await?;
        require(
            after_cfg == before_cfg,
            "Configuration RAM changed outside the restored scratch/flag plan",
        )?;
        for payload in &guard.payloads {
            let start = ((payload.range.address | self.access.camera.ram_alias)
                - preferences.address) as usize;
            let end = start + payload.range.size as usize;
            require(
                before_preferences.get(start..end) == after_preferences.get(start..end)
                    && before_preferences.get(start..end).is_some(),
                "Active settings/profile payload changed",
            )?;
        }
        require(
            self.access
                .raw(guard.callback_table.0, guard.callback_table.1.len() as u32)
                .await?
                == guard.callback_table.1,
            "Service callbacks are not fully restored",
        )?;
        self.guard(progress).await?;
        Ok(Persisted {
            value: target,
            already_persisted,
        })
    }

    async fn cleanup(&mut self, success: bool) -> Result<()> {
        require(
            self.access.session.state() == SessionState::Ready,
            "Session completion unknown; cleanup withheld",
        )?;
        if self.callback_touched || self.scratch_touched {
            self.set_flag(1).await?;
            let bytes = self.access.raw(self.mode.callback, 4).await?;
            let callback = u32::from_le_bytes(bytes.try_into().expect("word"));
            require(
                self.allowed_callback(callback),
                "Unexpected callback preserved; no automatic repair",
            )?;
            if callback != self.mode.stock_callback {
                self.write(self.mode.callback, &self.mode.stock_callback.to_le_bytes())
                    .await?;
            }
            self.restore_outputs().await?;
            if self.scratch_touched {
                let (address, adapter) = self.mode.adapter.clone();
                let current = self.access.raw(address, adapter.len() as u32).await?;
                if current == adapter {
                    let original = self
                        .old_scratch
                        .clone()
                        .ok_or_else(|| DebugModeError::Check("Missing original scratch".into()))?;
                    self.write(address, &original).await?;
                    self.sync_code().await?;
                } else {
                    require(
                        Some(current.as_slice()) == self.old_scratch.as_deref(),
                        "Unexpected scratch bytes preserved",
                    )?;
                }
            }
        }
        if let Some(flag) = self.old_flag {
            self.set_flag(if success { 0 } else { flag }).await?;
            let offset = self.access.camera.debug()?.flag_offset;
            self.access.byte(offset).await?;
        }
        let offset = self.access.camera.guard()?.offset;
        self.access.byte(offset).await?;
        Ok(())
    }
}
