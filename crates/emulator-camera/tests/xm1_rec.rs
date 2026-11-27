//! Open the installed X-M1 firmware from its stock boot.
//!
//! Set `FUJIMOD_FIRMWARE_LIBRARY` to a firmware library. Without it,
//! the test returns without opening a camera.
use fujimod_emulator_boot as boot;
use fujimod_emulator_camera::{
    BootCache, CardInputs, ExecutionPolicy, FirmwareInput, Flash, Library, Request, Stage, Start,
    open_unbooted,
};

const FIRMWARE: &str = "xm1-150";
/// The catalog declares REC as this UI mode for X-M1.
const REC_MODE: u32 = 3;
/// The catalog declares the normal shooting system mode for X-M1.
const REC_SYSTEM_MODE: u32 = 1;
/// The EXR II boot ROM's first instruction is `b 0x80000008`.
const ROM_HEAD: [u8; 4] = [0x08, 0x00, 0x00, 0xea];

fn library() -> Option<Library> {
    let library = Library::from_env().unwrap_or_else(|error| panic!("firmware library: {error}"));
    library
        .firmwares()
        .iter()
        .any(|firmware| firmware.id == FIRMWARE)
        .then_some(library)
}

fn request() -> Request {
    Request {
        firmware: FIRMWARE.into(),
        files: FirmwareInput::Library,
        overlays: Vec::new(),
        start: Start::Rom,
        flash: Flash::Stock,
        card: CardInputs::default(),
        policy: ExecutionPolicy::Interactive,
        engine: None,
        cache: BootCache::Bypass,
        run_to_rec: false,
        nand_image: None,
        face_result_profile: None,
        kept: Default::default(),
    }
}

#[test]
fn stock_starts_at_the_boot_rom() {
    let Some(library) = library() else {
        eprintln!("skipped: set FUJIMOD_FIRMWARE_LIBRARY to a firmware library");
        return;
    };
    let opened = open_unbooted(&library, request()).unwrap_or_else(|error| panic!("open: {error}"));
    assert_eq!(
        opened.resolved.configuration.id, "xm1-150-runtime",
        "the library opened another run configuration"
    );
    assert_eq!(opened.stage, Stage::BootRom);
    let session = opened.session();
    assert_eq!(
        session.definition["devices"]["boot_entry"], "rom",
        "the plan starts at the boot ROM"
    );
    let address = session.definition["inputs"]["boot_rom"]["address"]
        .as_u64()
        .expect("the run declares its boot ROM address");
    assert_eq!(address, 0x8000_0000, "the boot ROM is mapped high");
    let cpu = session.board.cpu(0).expect("core 0");
    let mut pc = [0u8; 8];
    cpu.read_register(fujimod_emulator_cpu::Register::Pc, &mut pc)
        .expect("core 0 PC");
    assert_eq!(
        u64::from_le_bytes(pc),
        0,
        "core 0 starts at address zero with the decoder on the boot ROM"
    );
    let mut head = [0u8; 4];
    session
        .board
        .read_physical(address, &mut head)
        .expect("boot ROM head");
    assert_eq!(
        head, ROM_HEAD,
        "the boot ROM's first instruction is its branch"
    );
    let report = boot::status(session).expect("status");
    assert_eq!(report.instructions, 0, "the guest has not run");
    assert_ne!(report.status, "rec");
}

#[test]
fn stock_reaches_rec_mode() {
    let Some(library) = library() else {
        eprintln!("skipped: set FUJIMOD_FIRMWARE_LIBRARY to a firmware library");
        return;
    };
    let mut opened =
        open_unbooted(&library, request()).unwrap_or_else(|error| panic!("open: {error}"));
    opened.release_holds().expect("no boot holds to release");
    let started = std::time::Instant::now();
    let report = boot::to_rec(&mut opened.camera.session, 400_000_000).expect("to_rec");
    assert_eq!(report.ui_mode, REC_MODE, "stock reported another UI mode");
    assert_eq!(
        report.system_mode, REC_SYSTEM_MODE,
        "stock reported another system mode"
    );
    // The strict gate accepts REC only with a presented frame. X-M1's firmware
    // leaves its LCD channel disabled at REC, so the observation and the gate
    // must agree about that.
    if report.status == "rec" {
        assert!(
            report.scanout_instance.is_some(),
            "REC status without a presented scanout: {report:?}"
        );
    }
    eprintln!(
        "stock: ui mode {} system mode {} at tick {} after {} instructions in {:?}; scanout {:?}",
        report.ui_mode,
        report.system_mode,
        report.virtual_ticks,
        report.instructions,
        started.elapsed(),
        report.scanout_instance,
    );
}
