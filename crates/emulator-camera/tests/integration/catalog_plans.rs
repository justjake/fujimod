//! Every firmware in the bundled catalog resolves through the shared open path
//! to the point just before execution, or stops at a named, expected gap.
//!
//! The plan reads no firmware bytes, so this runs without a firmware library.
//! A firmware that newly opens, or newly fails, changes the table below: a
//! body falling off the shared path fails here rather than at a camera.
use fujimod_emulator_camera::{FirmwareChoice, Library, Request, StartupPath, plan};

/// What the plan of one firmware reaches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Expected {
    /// It plans through this startup path, and the open observes REC.
    Rec(StartupPath),
    /// It plans through this startup path; the firmware declares no REC
    /// readiness facts, so the open returns the camera before its first
    /// instruction.
    Started(StartupPath),
    /// It stops at a gap whose refusal contains this text.
    Gap(&'static str),
}

use Expected::{Gap, Rec, Started};
use StartupPath::{Application, Exr2, Xp5};

const NO_DEVICE_LIST: &str = "has no device list yet";
const NO_XP5_INPUTS: &str = "declares no XP5 startup inputs yet";
const NO_DRAM: &str = "declares no DRAM address space";
const PLAN: &str = "starts from an application startup plan";

/// Every bundled firmware and what its plan reaches.
const EXPECTED: &[(&str, Expected)] = &[
    ("gfx100rf-112", Gap(NO_XP5_INPUTS)),
    ("gfx100rf-114", Gap(NO_XP5_INPUTS)),
    ("gfx100s-213", Gap(PLAN)),
    ("gfx50r-311", Gap(PLAN)),
    ("x100f-212", Gap(NO_DRAM)),
    ("x100s-121", Started(Exr2)),
    ("x100t-111", Started(Exr2)),
    ("x100v-301", Gap(PLAN)),
    ("x100vi-131", Started(Xp5)),
    ("x20-103", Started(Exr2)),
    ("x30-103", Started(Exr2)),
    ("x70-110", Rec(Exr2)),
    ("xa1-150", Started(Exr2)),
    ("xa2-130", Started(Exr2)),
    ("xa3-222", Gap(NO_DEVICE_LIST)),
    ("xe2-411", Rec(Exr2)),
    ("xe2s-111", Rec(Exr2)),
    ("xe4-201", Started(Application)),
    ("xe5-110", Gap(NO_DRAM)),
    ("xe5-111", Gap(NO_DRAM)),
    ("xe5-112", Rec(Xp5)),
    ("xh2-530", Rec(Xp5)),
    ("xh2-533", Started(Xp5)),
    ("xh2s-730", Rec(Xp5)),
    ("xm1-101", Started(Exr2)),
    ("xm1-120", Started(Exr2)),
    ("xm1-150", Rec(Exr2)),
    ("xm5-130", Started(Xp5)),
    ("xpro3-200", Gap(PLAN)),
    ("xq1-121", Started(Exr2)),
    ("xq2-101", Started(Exr2)),
    ("xs10-311", Started(Application)),
    ("xs20-330", Gap(NO_XP5_INPUTS)),
    ("xt1-552", Started(Exr2)),
    ("xt10-131", Rec(Exr2)),
    ("xt2-441", Started(Application)),
    ("xt20-211", Started(Application)),
    ("xt3-511", Started(Application)),
    ("xt30-201", Started(Application)),
    ("xt4-212", Started(Application)),
    ("xt5-431", Rec(Xp5)),
];

/// What one firmware's plan reaches, or its refusal.
fn reached(library: &Library, body: &str, firmware: &str) -> Result<Expected, String> {
    let request = Request::new(body, FirmwareChoice::Named(firmware.into()));
    let plan = plan(library, &request).map_err(|error| error.detail)?;
    assert_eq!(
        plan.firmware, firmware,
        "{body} {firmware} planned another firmware"
    );
    assert_eq!(
        plan.resolved.configuration.id, firmware,
        "{firmware} planned another run configuration"
    );
    Ok(match plan.rec_unobserved {
        None => Rec(plan.path),
        Some(_) => Started(plan.path),
    })
}

#[test]
fn every_catalog_firmware_plans_through_the_shared_path_or_names_its_gap() {
    let library = Library::without_directory().expect("bundled catalog");
    let mut seen = Vec::new();
    let mut wrong = Vec::new();
    for (id, expected) in EXPECTED {
        let firmware = library.firmware(id).expect("catalog firmware");
        seen.push(*id);
        let outcome = reached(&library, &firmware.body_id, id);
        let matches = match (expected, &outcome) {
            (Gap(text), Err(refusal)) => refusal.contains(text),
            (expected, Ok(reached)) => expected == reached,
            (_, Err(_)) => false,
        };
        eprintln!("{id}: {outcome:?}");
        if !matches {
            wrong.push(format!("{id}: expected {expected:?}, reached {outcome:?}"));
        }
    }
    assert!(wrong.is_empty(), "plans changed:\n{}", wrong.join("\n"));
    // Every catalog firmware has a row, so a new one must be placed.
    let mut catalog: Vec<String> = fujimod_catalog::ReferenceCatalog::bundled()
        .expect("bundled catalog")
        .firmware_ids()
        .map(str::to_owned)
        .collect();
    catalog.sort();
    let mut listed: Vec<String> = seen.iter().map(|id| (*id).to_owned()).collect();
    listed.sort();
    assert_eq!(listed, catalog, "the table lists every catalog firmware");
}

#[test]
fn a_body_opens_its_newest_firmware_and_names_refusals() {
    let library = Library::without_directory().expect("bundled catalog");
    let newest = plan(&library, &Request::new("xh2", FirmwareChoice::Newest)).unwrap();
    assert_eq!(newest.firmware, "xh2-533");
    let by_version = plan(
        &library,
        &Request::new("xh2s", FirmwareChoice::Named("7.30".into())),
    )
    .unwrap();
    assert_eq!(by_version.firmware, "xh2s-730");
    let refusal = |body: &str, firmware: FirmwareChoice| {
        plan(&library, &Request::new(body, firmware))
            .err()
            .expect("a refusal")
            .detail
    };
    assert_eq!(
        refusal("xh2s", FirmwareChoice::Named("xh2-530".into())),
        "xh2-530 is X-H2 firmware, not X-H2S"
    );
    assert!(
        refusal("xh2s", FirmwareChoice::Named("9.99".into()))
            .contains("body xh2s has no firmware 9.99; the catalog has 7.30")
    );
    assert!(refusal("x70", FirmwareChoice::Empty).contains("cannot start from an empty flash"));
    assert!(refusal("xa3", FirmwareChoice::Newest).contains("body xa3 has no device list yet"));
    assert!(refusal("no-such-body", FirmwareChoice::Newest).contains("not in the catalog"));
}

#[test]
fn cards_are_accepted_by_the_slots_a_body_declares() {
    let library = Library::without_directory().expect("bundled catalog");
    let mut request = Request::new("x70", FirmwareChoice::Newest);
    request.cards.sd_card_2 = Some(Default::default());
    let refusal = plan(&library, &request).err().unwrap().detail;
    assert_eq!(
        refusal,
        "body x70 has no sd_card_2 slot; its slots: sd_card"
    );
    let mut request = Request::new("xh2s", FirmwareChoice::Newest);
    request.cards.cfexpress_card = Some(Default::default());
    assert_eq!(
        plan(&library, &request).err().unwrap().detail,
        "CFexpress is not modeled yet"
    );
    let mut request = Request::new("xt5", FirmwareChoice::Newest);
    request.cards.sd_card_2 = Some(Default::default());
    plan(&library, &request).expect("X-T5 has a second SD slot");
}
