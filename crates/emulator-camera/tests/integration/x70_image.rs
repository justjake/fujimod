//! X70 test for starting at the native loader with a verified image in RAM.
//!
//! Set `FUJIMOD_FIRMWARE_LIBRARY` to a directory holding the catalog's X70
//! update and boot ROM.
use crate::support;
use support::{Boot, library, state};

use fujimod_emulator_camera::{BootCache, Library, LoaderImage, OpenError, Opened, Request, Stage};

const IMAGE: &[u8] = b"fujimod loader image";
const IMAGE_SHA256: &str = "7bc8d50a5e07c084670132737321751a74a9f565ca0ee78b7b5bdcb2071b7339";

fn request(boot: Boot, image: Option<LoaderImage>) -> Request {
    let mut request = support::request(boot, image, BootCache::Bypass);
    request.run_to_rec = false;
    request
}

fn start(library: &Library, request: Request) -> Result<Opened, OpenError> {
    fujimod_emulator_camera::open(library, &state(), request)
}

#[test]
fn a_verified_image_starts_at_the_loader_before_any_instruction() {
    let Some(library) = library() else {
        eprintln!("skipped: set FUJIMOD_FIRMWARE_LIBRARY");
        return;
    };
    let opened = start(
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
    let error = match start(
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
fn a_loader_image_needs_the_catalog_flash() {
    let Some(library) = library() else {
        eprintln!("skipped: set FUJIMOD_FIRMWARE_LIBRARY");
        return;
    };
    let mut request = request(
        Boot::Stock,
        Some(LoaderImage {
            bytes: IMAGE.to_vec(),
            sha256: IMAGE_SHA256.into(),
        }),
    );
    request.flash = fujimod_emulator_camera::Flash::Image(vec![0xff; 16]);
    let error = match start(&library, request) {
        Err(error) => error,
        Ok(_) => panic!("an image opened over a saved flash image"),
    };
    assert!(
        error.detail.contains("catalog's flash"),
        "unexpected refusal: {}",
        error.detail
    );
}
