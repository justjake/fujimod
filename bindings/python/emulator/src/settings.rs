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

/// The user profile for `requested` settings: the header and each sealed
/// journal record with its flash block, or `None` when the layout locates no
/// profile. A layout with a configuration window places no bank, so the
/// profile is all a run supplies.
#[pyfunction]
pub(super) fn settings_profile(
    layout: &str,
    flash: &[u8],
    requested: BTreeMap<String, String>,
) -> PyResult<Option<(Vec<u8>, Vec<(u32, Vec<u8>)>)>> {
    let layout: fujimod_catalog_settings::FirmwareSettings =
        serde_json::from_str(layout).map_err(|failure| error(failure.to_string()))?;
    let settings = fujimod_settings::build(&layout, flash, &requested, None).map_err(error)?;
    Ok(settings.profile.map(|profile| {
        let records = profile
            .records
            .into_iter()
            .map(|record| (record.block, record.bytes))
            .collect();
        (profile.header.to_vec(), records)
    }))
}
