//! X70 tests for powering a camera off and on again with its media kept.
//!
//! Set `FUJIMOD_FIRMWARE_LIBRARY` to a directory holding the catalog's X70
//! update and boot ROM. The card test also needs `FUJIMOD_TEST_SD_CARD`; the
//! automatic-boot test fetches the release card into the library.
use fujimod_emulator_camera::{
    Boot, BootCache, CardSource, ExecutionPolicy, Library, LibraryError, Opened, PowerOn, Request,
    Stage, open,
};
use std::path::PathBuf;

const FIRMWARE: &str = "x70-110";

fn library() -> Option<Library> {
    match Library::from_env() {
        Ok(library) => Some(library),
        Err(LibraryError::Environment) => None,
        Err(error) => panic!("firmware library: {error}"),
    }
}

fn request(boot: Boot) -> Request {
    Request {
        firmware: FIRMWARE.into(),
        boot,
        policy: ExecutionPolicy::Interactive,
        engine: None,
        card_files: Vec::new(),
        cache: BootCache::Bypass,
        run_to_rec: true,
        card: Default::default(),
        kept: Default::default(),
        card_source: None,
    }
}

fn release_card() -> Option<PathBuf> {
    std::env::var_os("FUJIMOD_TEST_SD_CARD").map(PathBuf::from)
}

fn assert_rec(opened: &Opened, name: &str) {
    assert_eq!(opened.stage, Stage::Rec, "{name} did not reach REC");
    let report = fujimod_emulator_boot::status(opened.session())
        .unwrap_or_else(|error| panic!("{name}: status: {error}"));
    assert_eq!(report.status, "rec", "{name} did not report REC");
}

/// Whether FujiMod's stage-2 loader is resident in the declared RAM range.
fn fujimod_resident(opened: &Opened, start: u64) -> bool {
    const MARKER: &[u8] = b"FUJIMOD SET-UP";
    const BYTES: u64 = 512 << 20;
    const CHUNK: u64 = 1 << 20;
    let mut chunk = vec![0u8; CHUNK as usize];
    for base in (start..start + BYTES).step_by(CHUNK as usize) {
        if opened
            .session()
            .board
            .read_physical(base, &mut chunk)
            .is_ok()
            && chunk.windows(MARKER.len()).any(|window| window == MARKER)
        {
            return true;
        }
    }
    false
}

#[test]
fn the_x70_keeps_its_flash_and_card_across_power_ons() {
    let (Some(library), Some(card)) = (library(), release_card()) else {
        eprintln!("skipped: set FUJIMOD_FIRMWARE_LIBRARY and FUJIMOD_TEST_SD_CARD");
        return;
    };
    let mut opened =
        open(&library, request(Boot::SdCard(CardSource::Directory(card)))).expect("SD card open");
    assert_rec(&opened, "SD card");
    opened = opened
        .power_on(&library, PowerOn::default())
        .expect("power on from internal flash");
    assert_rec(&opened, "internal flash with the kept card");
    opened = opened
        .power_on(
            &library,
            PowerOn {
                hold_controls: true,
                ..PowerOn::default()
            },
        )
        .expect("power on from the kept card");
    assert_rec(&opened, "SD card with the kept card");
    opened = opened
        .power_on(
            &library,
            PowerOn {
                hold_controls: true,
                fresh_card: true,
                ..PowerOn::default()
            },
        )
        .expect("power on from a fresh card");
    assert_rec(&opened, "SD card with a fresh card");
}

#[test]
fn a_stock_boot_cannot_hold_controls_it_has_no_card_source_for() {
    let Some(library) = library() else {
        eprintln!("skipped: set FUJIMOD_FIRMWARE_LIBRARY");
        return;
    };
    let opened = open(&library, request(Boot::Stock)).expect("stock open");
    let error = match opened.power_on(
        &library,
        PowerOn {
            hold_controls: true,
            ..PowerOn::default()
        },
    ) {
        Err(error) => error,
        Ok(_) => panic!("a stock boot held SD card controls"),
    };
    assert!(
        error.detail.contains("SD card source"),
        "unexpected refusal: {}",
        error.detail
    );
}

#[test]
fn the_x70_automatic_boot_survives_four_power_ons() {
    let (Some(library), Some(card)) = (library(), release_card()) else {
        eprintln!("skipped: set FUJIMOD_FIRMWARE_LIBRARY and FUJIMOD_TEST_SD_CARD");
        return;
    };
    let load = library
        .nand_boot(FIRMWARE)
        .unwrap_or_else(|error| panic!("NAND boot facts: {error}"))
        .load_range
        .address;
    let mut opened = open(
        &library,
        request(Boot::AutomaticBoot(CardSource::Directory(card))),
    )
    .expect("automatic boot install");
    assert_rec(&opened, "automatic boot install");
    assert!(
        fujimod_resident(&opened, load),
        "FujiMod is not resident after the install"
    );

    for (index, (hold_controls, fresh_card)) in
        [(false, false), (false, false), (true, false), (true, true)]
            .into_iter()
            .enumerate()
    {
        let name = format!("power-on {}", index + 1);
        opened = opened
            .power_on(
                &library,
                PowerOn {
                    hold_controls,
                    fresh_card,
                    ..PowerOn::default()
                },
            )
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_rec(&opened, &name);
        assert!(
            fujimod_resident(&opened, load),
            "{name} did not run FujiMod"
        );
    }
}
