//! The XP5 device windows built from the catalog, compared with each firmware's
//! boot-manifest windows. Each manifest window is matched with the built window
//! at the same base; every difference is listed. The list only shrinks: when it
//! is empty the manifests' device tables are deleted and the boot builds from
//! the catalog (tools/bringup/DESIGN.md, "XP5 device windows from the catalog").

use super::manifest::BootState;
use fujimod_catalog_resolution::ResolvedConfiguration;
use fujimod_emulator_devices::graph::DeviceWindow;

const FIRMWARES: [&str; 6] = ["x100vi-131", "xe5-112", "xh2-530", "xh2s-730", "xm5-130", "xt5-431"];

/// Every difference between the manifest windows of `firmware` and the catalog's.
pub(crate) fn differences(firmware: &str) -> Result<Vec<String>, String> {
    let state = BootState::for_firmware(firmware, None)?;
    let mut resolved = ResolvedConfiguration::bundled(firmware)?;
    // A manifest DRAM map moves the body's one bus drain past it, as the boot does.
    let dram_limit = match &state.dram {
        Some(dram) => {
            let drains = resolved.reference.body.hardware.as_mut().map(|hardware| &mut hardware.bus_drain);
            let Some([drain]) = drains.map(|drains| drains.as_mut_slice()) else {
                return Err(format!("{firmware}: the manifest moves a bus drain the body does not declare once"));
            };
            drain.address = dram.bus_drain;
            dram.limit
        }
        None => resolved.dram_address_space_limit_bytes()?,
    };
    let mut built = fujimod_emulator_xp5_board::boot::windows_from_blocks(&resolved, dram_limit)?;
    // The boot applies the run's scene on top of its windows; compare what each side boots with.
    let mut manifest = state.device.clone();
    if let Some(fujimod_catalog_configuration::RunScene::Xp5(scene)) = resolved.configuration.scene.as_ref() {
        let ticks = match resolved.configuration.inputs.as_ref() {
            Some(fujimod_catalog_configuration::RunInputs::Xp5(inputs)) => inputs.ticks_per_second,
            _ => return Err(format!("{firmware}: run has no XP5 inputs")),
        };
        fujimod_emulator_xp5_board::boot::apply_run_scene(&resolved, &mut manifest, scene, ticks)?;
        fujimod_emulator_xp5_board::boot::apply_run_scene(&resolved, &mut built, scene, ticks)?;
    }
    Ok(compare(firmware, &manifest, &built))
}

fn compare(firmware: &str, manifest: &[DeviceWindow], built: &[DeviceWindow]) -> Vec<String> {
    let mut differences = Vec::new();
    for window in manifest {
        let at: Vec<_> = built.iter().filter(|candidate| candidate.base == window.base).collect();
        let label = format!("{firmware} {:#x} {}", window.base, window.kind);
        // A manifest window that describes part of a catalog window, with the same kind and
        // settings, is the same hardware at a finer granularity; the catalog only adds the gaps.
        let contains = |candidate: &&DeviceWindow| {
            candidate.base <= window.base
                && window.base + window.size <= candidate.base + candidate.size
                && candidate.kind == window.kind
                && candidate.options == window.options
        };
        if built.iter().any(|candidate| contains(&candidate)) {
            continue;
        }
        match at.as_slice() {
            [] => differences.push(format!("{label}: no catalog window (size {:#x}, options {})", window.size, toml::Value::Table(window.options.clone()))),
            [one] => {
                if one.size != window.size {
                    differences.push(format!("{label}: size {:#x} != {:#x}", one.size, window.size));
                }
                if one.kind != window.kind {
                    differences.push(format!("{label}: kind {}", one.kind));
                }
                let mut keys: Vec<&String> = window.options.keys().chain(one.options.keys()).collect();
                keys.sort();
                keys.dedup();
                for key in keys {
                    let (left, right) = (one.options.get(key), window.options.get(key));
                    if left != right {
                        differences.push(format!(
                            "{label}: option {key}: catalog {} != manifest {}",
                            left.map_or("absent".into(), ToString::to_string),
                            right.map_or("absent".into(), ToString::to_string),
                        ));
                    }
                }
            }
            _ => differences.push(format!("{label}: {} catalog windows", at.len())),
        }
    }
    differences
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Differences left before the switch; lower it as facts move, never raise it.
    const REMAINING: usize = 0;

    #[test]
    fn catalog_windows_match_the_boot_manifests() {
        let mut all = Vec::new();
        for firmware in FIRMWARES {
            all.extend(differences(firmware).unwrap_or_else(|error| panic!("{firmware}: {error}")));
        }
        let report = std::env::temp_dir().join("xp5-catalog-parity.txt");
        std::fs::write(&report, all.join("\n") + "\n").unwrap();
        eprintln!("{} differences, listed in {}", all.len(), report.display());
        assert!(all.len() <= REMAINING, "{} differences (limit {REMAINING})", all.len());
    }
}
