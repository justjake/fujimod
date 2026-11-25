//! X70 test for starting at the native loader with a verified image in RAM.
//!
//! Set `FUJIMOD_FIRMWARE_LIBRARY` to a directory holding the catalog's X70
//! update and boot ROM.
use fujimod_emulator_camera::{
    Boot, BootCache, CardSource, ExecutionPolicy, Library, LibraryError, LoaderImage, Request,
    Stage, open,
};

const FIRMWARE: &str = "x70-110";
const IMAGE: &[u8] = b"fujimod loader image";
const IMAGE_SHA256: &str = "7bc8d50a5e07c084670132737321751a74a9f565ca0ee78b7b5bdcb2071b7339";

fn library() -> Option<Library> {
    match Library::from_env() {
        Ok(library) => Some(library),
        Err(LibraryError::Environment) => None,
        Err(error) => panic!("firmware library: {error}"),
    }
}

fn request(boot: Boot, image: Option<LoaderImage>) -> Request {
    Request {
        firmware: FIRMWARE.into(),
        boot,
        policy: ExecutionPolicy::Interactive,
        engine: None,
        card_files: Vec::new(),
        cache: BootCache::Bypass,
        run_to_rec: false,
        card: Default::default(),
        kept: Default::default(),
        card_source: None,
        image,
    }
}

#[test]
fn a_verified_image_starts_at_the_loader_before_any_instruction() {
    let Some(library) = library() else {
        eprintln!("skipped: set FUJIMOD_FIRMWARE_LIBRARY");
        return;
    };
    let opened = open(
        &library,
        request(
            Boot::Stock,
            Some(LoaderImage {
                bytes: IMAGE.to_vec(),
                sha256: IMAGE_SHA256.into(),
            }),
        ),
    )
    .expect("loader image open");
    assert_eq!(opened.stage, Stage::BootRom);
    let report = fujimod_emulator_boot::status(opened.session()).expect("status");
    assert_eq!(report.instructions, 0, "the guest has not run");
    let mut placed = vec![0u8; IMAGE.len()];
    opened
        .session()
        .board
        .read_physical(0, &mut placed)
        .expect("read the image");
    assert_eq!(placed, IMAGE);
    let pc = opened
        .session()
        .board
        .read_register(0, fujimod_emulator_cpu::arm::CORE[15])
        .expect("read the program counter");
    assert_eq!(pc, 0, "the first core starts at the loader");
}

#[test]
fn an_image_whose_hash_differs_is_refused() {
    let Some(library) = library() else {
        eprintln!("skipped: set FUJIMOD_FIRMWARE_LIBRARY");
        return;
    };
    let error = match open(
        &library,
        request(
            Boot::Stock,
            Some(LoaderImage {
                bytes: IMAGE.to_vec(),
                sha256: "0".repeat(64),
            }),
        ),
    ) {
        Err(error) => error,
        Ok(_) => panic!("an image with a different hash opened"),
    };
    assert!(
        error.detail.contains("sha256"),
        "unexpected refusal: {}",
        error.detail
    );
}

#[test]
fn an_image_requires_a_stock_boot() {
    let Some(library) = library() else {
        eprintln!("skipped: set FUJIMOD_FIRMWARE_LIBRARY");
        return;
    };
    let error = match open(
        &library,
        request(
            Boot::SdCard(CardSource::Files(Vec::new())),
            Some(LoaderImage {
                bytes: IMAGE.to_vec(),
                sha256: IMAGE_SHA256.into(),
            }),
        ),
    ) {
        Err(error) => error,
        Ok(_) => panic!("an image opened from a card boot"),
    };
    assert!(
        error.detail.contains("stock boot"),
        "unexpected refusal: {}",
        error.detail
    );
}
