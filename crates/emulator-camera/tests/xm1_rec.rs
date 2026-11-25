//! Open the installed X-M1 firmware at REC from its stock boot.
//!
//! Set `FUJIMOD_FIRMWARE_LIBRARY` to a directory holding the catalog's X-M1
//! update and boot ROM. Without it, the test returns without opening a camera.
use fujimod_emulator_boot as boot;
use fujimod_emulator_camera::{
    Boot, BootCache, ExecutionPolicy, Library, LibraryError, Request, Stage, open,
};

const FIRMWARE: &str = "xm1-150";
/// The catalog declares REC as this UI mode for X-M1.
const REC_MODE: u32 = 3;

fn library() -> Option<Library> {
    match Library::from_env() {
        Ok(library) => Some(library),
        Err(LibraryError::Environment) => None,
        Err(error) => panic!("firmware library: {error}"),
    }
}

fn request() -> Request {
    Request {
        firmware: FIRMWARE.into(),
        boot: Boot::Stock,
        policy: ExecutionPolicy::Interactive,
        engine: None,
        card_files: Vec::new(),
        cache: BootCache::Bypass,
        run_to_rec: true,
        card: Default::default(),
        kept: Default::default(),
        card_source: None,
        image: None,
    }
}

#[test]
fn stock_opens_to_rec() {
    let Some(library) = library() else {
        eprintln!("skipped: set FUJIMOD_FIRMWARE_LIBRARY to a firmware library");
        return;
    };
    let started = std::time::Instant::now();
    let opened = open(&library, request()).unwrap_or_else(|error| panic!("stock: {error}"));
    assert_eq!(opened.stage, Stage::Rec, "stock did not report REC");
    assert_eq!(
        opened.resolved.configuration.id, "xm1-150-runtime",
        "the library opened another run configuration"
    );
    let report = boot::status(opened.session()).expect("status");
    assert_eq!(report.status, "rec", "stock did not settle at REC");
    assert_eq!(report.ui_mode, REC_MODE, "stock reported another UI mode");
    eprintln!(
        "stock: REC at tick {} after {} instructions in {:?}",
        report.virtual_ticks,
        report.instructions,
        started.elapsed()
    );
}
