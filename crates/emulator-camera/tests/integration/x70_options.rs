//! X70 tests for opening before REC and for the SD card's model options.
//!
//! Set `FUJIMOD_FIRMWARE_LIBRARY` to a directory holding the catalog's X70
//! update and boot ROM. Without it, the tests return without opening a camera.
use crate::support;
use support::{Boot, CardSource, library};

use fujimod_emulator_camera::{
    BootCache, CardLayout, Library, OpenError, Opened, Request, RunFaceResultProfile, Stage,
    open_unbooted,
};

fn state() -> fujimod_emulator_camera::StateDir {
    support::state()
}

fn open(library: &Library, request: Request) -> Result<Opened, OpenError> {
    fujimod_emulator_camera::open(library, &state(), request)
}

fn request(boot: Boot) -> Request {
    support::request(boot, None, BootCache::Bypass)
}

fn sd_node(opened: &Opened) -> serde_json::Value {
    opened.session().definition["devices"]["nodes"]
        .as_array()
        .expect("device node list")
        .iter()
        .find(|node| node["name"] == "sd")
        .expect("SD card node")
        .clone()
}

fn face_node(opened: &Opened) -> serde_json::Value {
    opened.session().definition["devices"]["nodes"]
        .as_array()
        .expect("device node list")
        .iter()
        .find(|node| node["name"] == "face_detector")
        .expect("face node")
        .clone()
}

#[test]
fn a_declared_face_result_profile_replaces_the_passive_face_bank() {
    let Some(library) = library() else {
        eprintln!("skipped: set FUJIMOD_FIRMWARE_LIBRARY");
        return;
    };
    let mut request = request(Boot::Stock);
    request.face_result_profile = Some(RunFaceResultProfile::Box172x120);
    let opened = open_unbooted(&library, request).expect("construct with declared face results");
    let node = face_node(&opened);
    assert_eq!(node["model"]["kind"], "face-results");
    assert_eq!(node["model"]["profile"], "box172x120");
}

#[test]
fn an_absent_sd_card_boots_stock_and_is_reported_absent() {
    let Some(library) = library() else {
        eprintln!("skipped: set FUJIMOD_FIRMWARE_LIBRARY");
        return;
    };
    let mut request = request(Boot::Stock);
    request.card.present = false;
    let opened = open(&library, request).expect("stock boot without a card");
    assert_eq!(opened.stage, Stage::Rec);
    let state = opened.session().card_state("sd").expect("card state");
    assert!(!state.present);
    assert!(!state.protected);
}

#[test]
fn a_card_boot_refuses_an_absent_card() {
    let Some(library) = library() else {
        eprintln!("skipped: set FUJIMOD_FIRMWARE_LIBRARY");
        return;
    };
    let mut request = request(Boot::SdCard(CardSource::Files(Vec::new())));
    request.card.present = false;
    let error = match open(&library, request) {
        Err(error) => error,
        Ok(_) => panic!("a card boot opened without its card"),
    };
    assert_eq!(error.stage, Stage::BootRom);
    assert!(
        error.detail.contains("card present"),
        "unexpected refusal: {}",
        error.detail
    );
}

#[test]
fn a_write_protected_sd_card_boots_stock_and_is_reported_protected() {
    let Some(library) = library() else {
        eprintln!("skipped: set FUJIMOD_FIRMWARE_LIBRARY");
        return;
    };
    let mut request = request(Boot::Stock);
    request.card.protected = true;
    let opened = open(&library, request).expect("stock boot with a protected card");
    assert_eq!(opened.stage, Stage::Rec);
    let state = opened.session().card_state("sd").expect("card state");
    assert!(state.present);
    assert!(state.protected);
}

#[test]
fn sd_card_transfer_ticks_replace_the_catalog_value() {
    let Some(library) = library() else {
        eprintln!("skipped: set FUJIMOD_FIRMWARE_LIBRARY");
        return;
    };
    let catalog = open_unbooted(&library, request(Boot::Stock)).expect("unbooted stock camera");
    assert_eq!(sd_node(&catalog)["model"]["options"]["transfer_ticks"], 100);

    let mut request = request(Boot::Stock);
    request.card.transfer_ticks = Some(17);
    let opened = open(&library, request).expect("stock boot with 17 transfer ticks");
    assert_eq!(opened.stage, Stage::Rec);
    assert_eq!(sd_node(&opened)["model"]["options"]["transfer_ticks"], 17);
}

#[test]
fn transfer_ticks_must_be_positive() {
    let Some(library) = library() else {
        eprintln!("skipped: set FUJIMOD_FIRMWARE_LIBRARY");
        return;
    };
    let mut request = request(Boot::Stock);
    request.card.transfer_ticks = Some(0);
    let error = match open_unbooted(&library, request) {
        Err(error) => error,
        Ok(_) => panic!("zero transfer ticks opened"),
    };
    assert_eq!(error.stage, Stage::BootRom);
    assert!(
        error.detail.contains("transfer ticks"),
        "unexpected refusal: {}",
        error.detail
    );
}

fn layout() -> CardLayout {
    CardLayout {
        sectors: 8_388_608,
        label: "LAYOUT".into(),
        partition_start: 16_384,
        sectors_per_cluster: 32,
    }
}

#[test]
fn a_declared_card_layout_formats_the_stock_card() {
    let Some(library) = library() else {
        eprintln!("skipped: set FUJIMOD_FIRMWARE_LIBRARY");
        return;
    };
    let layout = layout();
    let directory =
        std::env::temp_dir().join(format!("fujimod-card-layout-{}", std::process::id()));
    let file = directory.join("LAYOUT.TXT");
    std::fs::create_dir_all(&directory).expect("card layout fixture directory");
    std::fs::write(&file, b"declared layout").expect("card layout fixture file");
    let mut request = request(Boot::Stock);
    request.card.layout = Some(layout.clone());
    request.card.files = vec![("LAYOUT.TXT".into(), b"declared layout".to_vec())];
    let opened = open(&library, request);
    let _ = std::fs::remove_dir_all(&directory);
    let opened = opened.expect("stock boot with a declared layout");
    assert_eq!(opened.stage, Stage::Rec);
    let state = opened.session().card_state("sd").expect("card state");
    assert_eq!(state.capacity_bytes, u64::from(layout.sectors) * 512);
    let node = sd_node(&opened);
    let options = &node["model"]["card"]["options"];
    assert_eq!(options["sectors"], layout.sectors);
    assert_eq!(options["label"], layout.label);
    assert_eq!(options["partition_start"], layout.partition_start);
    assert_eq!(options["sectors_per_cluster"], layout.sectors_per_cluster);
    assert_eq!(
        opened
            .session()
            .card_read("sd", "/LAYOUT.TXT", None, 64)
            .expect("card file on the declared layout"),
        b"declared layout"
    );
}

#[test]
fn a_declared_card_layout_formats_a_held_card_boot() {
    let Some(library) = library() else {
        eprintln!("skipped: set FUJIMOD_FIRMWARE_LIBRARY");
        return;
    };
    let layout = layout();
    let mut request = request(Boot::SdCard(CardSource::Files(Vec::new())));
    request.card.layout = Some(layout.clone());
    request.run_to_rec = false;
    let opened = open(&library, request).expect("card boot with a declared layout");
    assert_eq!(opened.stage, Stage::BootRom);
    let state = opened.session().card_state("sd").expect("card state");
    assert_eq!(state.capacity_bytes, u64::from(layout.sectors) * 512);
    let node = sd_node(&opened);
    assert_eq!(node["model"]["card"]["options"]["sectors"], layout.sectors);
}

#[test]
fn a_declared_card_layout_is_validated_and_needs_the_card() {
    let Some(library) = library() else {
        eprintln!("skipped: set FUJIMOD_FIRMWARE_LIBRARY");
        return;
    };
    let mut bad_cluster = request(Boot::Stock);
    bad_cluster.card.layout = Some(CardLayout {
        sectors_per_cluster: 3,
        ..layout()
    });
    let error = match open(&library, bad_cluster) {
        Err(error) => error,
        Ok(_) => panic!("a card layout with three sectors per cluster opened"),
    };
    assert_eq!(error.stage, Stage::BootRom);
    assert!(
        error.detail.contains("cluster size"),
        "unexpected refusal: {}",
        error.detail
    );

    let mut absent = request(Boot::Stock);
    absent.card.present = false;
    absent.card.layout = Some(layout());
    let error = match open(&library, absent) {
        Err(error) => error,
        Ok(_) => panic!("a card layout opened without its card"),
    };
    assert_eq!(error.stage, Stage::BootRom);
    assert!(
        error.detail.contains("card present"),
        "unexpected refusal: {}",
        error.detail
    );
}
