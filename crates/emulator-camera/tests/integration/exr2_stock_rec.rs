//! Open the installed EXR II bodies' stock firmware from its ROM boot.
//!
//! Set `FUJIMOD_FIRMWARE_LIBRARY` to a firmware library. Without it
//! each test returns without opening a camera.
use crate::support;
use support::{library, state};

use fujimod_emulator_camera::{
    BootCache, CardInputs, ExecutionPolicy, FirmwareInput, Flash, Request, Stage, Start, open,
    open_unbooted,
};

/// The three EXR II bodies whose runtime profiles declare X70's hardware.
const BODIES: [&str; 3] = ["xe2-411", "xe2s-111", "xt10-131"];
/// The catalog declares REC as this UI mode for each body.
const REC_MODE: u32 = 3;
/// Enough stock instructions to reach the record UI and settle there.
const INSTRUCTIONS: u64 = 400_000_000;

fn request(firmware: &str) -> Request {
    Request {
        firmware: firmware.into(),
        files: FirmwareInput::Library,
        overlays: Vec::new(),
        start: Start::Rom,
        flash: Flash::Stock,
        card: CardInputs::default(),
        policy: ExecutionPolicy::Interactive,
        engine: None,
        cache: BootCache::Bypass,
        run_to_rec: true,
        nand_image: None,
        face_result_profile: None,
        camera_settings: Default::default(),
        kept: Default::default(),
    }
}

/// The stock firmware runs on the body's declared EXR II hardware into REC.
#[test]
fn stock_opens_boot_the_declared_hardware_to_rec_mode() {
    let Some(library) = library() else {
        eprintln!("skipped: set FUJIMOD_FIRMWARE_LIBRARY");
        return;
    };
    for firmware in BODIES {
        let mut opened = open_unbooted(&library, request(firmware))
            .unwrap_or_else(|error| panic!("{firmware}: {error}"));
        opened
            .release_holds()
            .unwrap_or_else(|error| panic!("{firmware}: {error}"));
        let report = fujimod_emulator_boot::to_rec(opened.session_mut(), INSTRUCTIONS)
            .unwrap_or_else(|error| panic!("{firmware}: {error}"));
        assert_eq!(
            report.ui_mode,
            REC_MODE,
            "{firmware} reported another UI mode: {}",
            report.failure.as_deref().unwrap_or("no failure recorded")
        );
        assert!(
            matches!(report.system_mode, 1 | 3),
            "{firmware} reported system mode {}",
            report.system_mode
        );
        assert_eq!(report.menu_active, 0, "{firmware} stopped with a menu open");
        // The strict REC gate requires a presented frame for every body. A run
        // that cannot present stops before REC with the observation that says
        // why, so the status and the scanout must agree.
        match report.status.as_str() {
            "rec" => assert!(
                report.scanout_instance.is_some(),
                "{firmware}: REC without a presented scanout"
            ),
            "instruction-limit" => {
                assert!(report.failure.is_none(), "{firmware}: {report:?}");
                assert!(report.scanout_instance.is_none(), "{firmware}: {report:?}");
            }
            other => panic!(
                "{firmware}: {other}: {}",
                report.failure.as_deref().unwrap_or("no failure recorded")
            ),
        }
        eprintln!(
            "{firmware}: {} at tick {} after {} instructions; pending {}, scanout {:?}",
            report.status,
            report.virtual_ticks,
            report.instructions,
            report.pending,
            report.scanout_instance
        );
    }
}

/// The camera API's stock open reaches REC on every sibling body.
#[test]
fn stock_open_through_the_camera_api_reaches_rec() {
    let Some(library) = library() else {
        eprintln!("skipped: set FUJIMOD_FIRMWARE_LIBRARY");
        return;
    };
    for firmware in BODIES {
        let opened = open(&library, &state(), request(firmware))
            .unwrap_or_else(|error| panic!("{firmware}: {error}"));
        assert_eq!(opened.stage, Stage::Rec, "{firmware} stopped short of REC");
    }
}
