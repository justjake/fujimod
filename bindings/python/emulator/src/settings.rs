//! Requested camera settings that live outside flash.
use super::error;
use pyo3::prelude::*;
use std::collections::BTreeMap;

/// The power controller's profile header for `requested` settings, built from
/// the firmware's settings layout and its installed flash image, or `None` when
/// the layout locates no profile.
#[pyfunction]
pub(super) fn settings_header(
    layout: &str,
    flash: &[u8],
    requested: BTreeMap<String, String>,
) -> PyResult<Option<Vec<u8>>> {
    let layout: fujimod_catalog_settings::FirmwareSettings =
        serde_json::from_str(layout).map_err(|failure| error(failure.to_string()))?;
    let settings = fujimod_settings::build(&layout, flash, &requested, None).map_err(error)?;
    Ok(settings.profile.map(|profile| profile.header.to_vec()))
}
