//! Service mode: confirm the stock service session and arm the reset guard.
//!
//! Camera-validated on X70 1.10. The guard byte is set in configuration RAM so
//! the stock teardown does not restore defaults on shutdown. It is never
//! cleared here, including after a failure. Arming alone restores debug access
//! to 0; preparing a service session leaves it at 1. A failed check restores
//! the original debug value when completion is known.
use crate::{
    Limits,
    access::{Access, range},
};
use fujimod_catalog_camera::{CameraDefinition, CameraError, Guard};
use fujimod_usb::{
    ff80::{
        Session, SessionState,
        commands::{configuration::ConfigurationView, info::InfoSelector},
    },
    operations::checks,
};

#[derive(Debug, thiserror::Error)]
pub enum ServiceModeError {
    #[error(transparent)]
    Usb(#[from] fujimod_usb::Error),
    #[error(transparent)]
    Definition(#[from] CameraError),
    #[error("connected device {0:?} is not the selected camera")]
    OtherCamera(String),
    #[error(
        "configuration RAM is {capacity:#x} bytes in {page:#x}-byte pages; the definition differs"
    )]
    Configuration { capacity: u32, page: u32 },
    #[error("unexpected {name}: {value:#x}")]
    Unexpected { name: String, value: u32 },
    #[error("camera code differs from stock: {0}")]
    CodeDiffers(String),
    #[error("service callbacks are not stock")]
    Callbacks,
    #[error("the reset guard did not verify")]
    GuardUnverified,
    #[error("user settings changed while arming the guard")]
    SettingsChanged,
    #[error("USB output state changed while arming the guard")]
    OutputChanged,
}
pub type Result<T> = std::result::Result<T, ServiceModeError>;

/// What arming found and left.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GuardArmed {
    pub guard_was_set: bool,
    pub debug_flag_before: u8,
}

/// The connected device names the selected camera.
pub(crate) async fn check_product(session: &mut Session, camera: &CameraDefinition) -> Result<()> {
    let product = session.device_info(InfoSelector::Product).await?.text;
    let normalized = product.to_uppercase().replace('-', "");
    if camera
        .products
        .iter()
        .any(|name| normalized.contains(&name.to_uppercase().replace('-', "")))
    {
        Ok(())
    } else {
        Err(ServiceModeError::OtherCamera(product))
    }
}

/// Stock state words and callbacks; returns the USB output-state bytes.
pub(crate) async fn stock_state(access: &mut Access<'_>, guard: &Guard) -> Result<Vec<u8>> {
    for word in &guard.state_words {
        let value = access.uncached_word(word.address).await?;
        if value != word.value {
            return Err(ServiceModeError::Unexpected {
                name: word.name.clone(),
                value,
            });
        }
    }
    let (address, table) = &guard.callback_table;
    if access.uncached(*address, table.len() as u32).await? != *table {
        return Err(ServiceModeError::Callbacks);
    }
    Ok(access
        .uncached(guard.output_state.address, guard.output_state.size)
        .await?)
}

/// Confirm the connected camera is the selected one in stock service mode,
/// then arm the reset guard. `progress` receives one line per step as it runs.
pub async fn arm_reset_guard(
    session: &mut Session,
    camera: &CameraDefinition,
    limits: Limits,
    progress: &mut (dyn FnMut(&str) + Send),
) -> Result<GuardArmed> {
    prepare(session, camera, limits, progress, false).await
}

/// Confirm stock service mode and leave the reset guard and debug access in
/// configuration RAM for this boot. No stored setting is changed.
pub async fn prepare_service_session(
    session: &mut Session,
    camera: &CameraDefinition,
    limits: Limits,
    progress: &mut (dyn FnMut(&str) + Send),
) -> Result<GuardArmed> {
    prepare(session, camera, limits, progress, true).await
}

async fn prepare(
    session: &mut Session,
    camera: &CameraDefinition,
    limits: Limits,
    progress: &mut (dyn FnMut(&str) + Send),
    leave_debug_enabled: bool,
) -> Result<GuardArmed> {
    let guard = camera.guard()?.clone();
    let debug = camera.debug()?;
    progress("Checking the camera");
    check_product(session, camera).await?;
    let geometry = session
        .configuration_geometry(ConfigurationView::Ram)
        .await?;
    if geometry.capacity != camera.configuration.size
        || geometry.page_bytes.get() != camera.configuration.max_read
    {
        return Err(ServiceModeError::Configuration {
            capacity: geometry.capacity,
            page: geometry.page_bytes.get(),
        });
    }
    let mut access = Access {
        session,
        camera,
        limits,
    };
    let flag_before = access.byte(debug.flag_offset).await?;
    let guard_before = access.byte(guard.offset).await?;
    for (name, value) in [("debug flag", flag_before), ("reset guard", guard_before)] {
        if value > 1 {
            return Err(ServiceModeError::Unexpected {
                name: name.into(),
                value: value.into(),
            });
        }
    }
    let outcome = arm(
        &mut access,
        &guard,
        debug.flag_offset,
        flag_before,
        guard_before,
        progress,
    )
    .await;
    // A failed preparation restores the original gate; never undo the guard.
    let target = if outcome.is_ok() {
        u8::from(leave_debug_enabled)
    } else {
        flag_before
    };
    if access.session.state() == SessionState::Ready {
        let cleanup = async {
            if access.byte(debug.flag_offset).await? != target {
                access.set(debug.flag_offset, target).await?;
            }
            Ok::<_, ServiceModeError>(())
        }
        .await;
        if cleanup.is_err() {
            access.session.invalidate_completion();
        }
        outcome?;
        cleanup?;
    } else {
        outcome?;
    }
    progress(if leave_debug_enabled {
        "Service session prepared"
    } else {
        "Reset guard armed"
    });
    Ok(GuardArmed {
        guard_was_set: guard_before == 1,
        debug_flag_before: flag_before,
    })
}

async fn arm(
    access: &mut Access<'_>,
    guard: &Guard,
    flag_offset: u32,
    flag_before: u8,
    guard_before: u8,
    progress: &mut (dyn FnMut(&str) + Send),
) -> Result<()> {
    if flag_before == 0 {
        access.set(flag_offset, 1).await?;
    }
    progress("Verifying stock camera code");
    let alias = access.camera.ram_alias;
    for check in &guard.code_checks {
        let range = range(check.range.address | alias, check.range.size)?;
        if !checks::region_matches(
            access.session,
            range,
            access.limits.memory,
            check.sha256.bytes(),
        )
        .await?
        {
            return Err(ServiceModeError::CodeDiffers(check.name.clone()));
        }
    }
    progress("Confirming service mode");
    let output_before = stock_state(access, guard).await?;
    let mut payloads = Vec::new();
    for payload in &guard.payloads {
        payloads.push(
            access
                .uncached(payload.range.address, payload.range.size)
                .await?,
        );
    }
    progress("Arming the reset guard");
    if guard_before != guard.value {
        access.set(guard.offset, guard.value).await?;
    }
    for alias in &guard.aliases {
        if access.raw(*alias, 1).await? != [guard.value] {
            return Err(ServiceModeError::GuardUnverified);
        }
    }
    for (payload, before) in guard.payloads.iter().zip(&payloads) {
        if access
            .uncached(payload.range.address, payload.range.size)
            .await?
            != *before
        {
            return Err(ServiceModeError::SettingsChanged);
        }
    }
    if stock_state(access, guard).await? != output_before {
        return Err(ServiceModeError::OutputChanged);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
