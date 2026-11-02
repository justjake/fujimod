use super::*;

// Synthetic setup grammar, not a camera program. Keep the legitimate SETEND
// opcode in the full flash-address range to catch instruction/pointer confusion.
fn fixture() -> (Vec<u8>, Setup, FileApi) {
    let mut bytes = vec![0; 0x40000];
    put(&mut bytes, 0x800, 0xf1010000).unwrap();
    put(&mut bytes, 0x838, 0xe59fe060).unwrap(); // LDR lr,[pc,#0x60]
    put(&mut bytes, 0x8a0, 0xf00009c0).unwrap();
    put(&mut bytes, 0x8b0, immediate(0xe3003000, 0x900)).unwrap();
    put(&mut bytes, 0x8b4, immediate(0xe3403000, 0xf000)).unwrap();
    let setup = Setup {
        dat_offset: 0,
        active_size: 0x9c0,
        active_sha256: String::new(),
        slot_size: 0x20000,
        flash_aperture: 0xf0000000,
        code_offset: 0x800,
        code_size: 0x100,
        code_sha256: String::new(),
        continuation_offset: 0x9c0,
        continuation_address: 0xf00009c0,
        linked_sram_base: 0xfff10000,
    };
    let api = FileApi {
        mount: 0xfff02a2c,
        open: 0xfff02380,
        read: 0xfff02a64,
        close: 0xfff02ad0,
        load_address: setup.linked_sram_base,
        workspace: 0xfff2d800,
    };
    (bytes, setup, api)
}

fn build(bytes: &[u8], setup: &Setup, api: &FileApi) -> Result<Vec<u8>> {
    let mut setup = setup.clone();
    setup.active_sha256 = fujimod_firmware::sha256(&bytes[..setup.active_size]);
    setup.code_sha256 = fujimod_firmware::sha256(&bytes[0x800..0x900]);
    ffinit(bytes, &setup, api, &[1, 2, 3, 4], api.load_address, None)
}

#[test]
fn setup_relocates_its_data_and_continuation_without_misreading_opcodes() {
    let (bytes, setup, api) = fixture();
    let output = build(&bytes, &setup, &api).unwrap();
    let region = api.load_address + 0x4000;
    assert_eq!(
        pair(&output, 0xb0, 3).unwrap(),
        region + sd_loader::LAYOUT.ffinit.symbols["ffinit_setup_data"] as u32,
    );
    assert_eq!(
        word(&output, 0xa0).unwrap(),
        region + sd_loader::LAYOUT.ffinit.symbols["ffinit_dram_ready"] as u32,
    );
}

#[test]
fn references_beyond_the_setup_slot_cannot_escape_relocation_checks() {
    for address in [0xf00009c0, 0xf0020000, 0xf003fffc] {
        let (mut bytes, setup, api) = fixture();
        put(&mut bytes, 0x8b0, immediate(0xe3003000, address)).unwrap();
        put(&mut bytes, 0x8b4, immediate(0xe3403000, address >> 16)).unwrap();
        let error = build(&bytes, &setup, &api).unwrap_err().to_string();
        assert!(error.contains("outside extracted data"), "{error}");

        let (mut bytes, setup, api) = fixture();
        put(&mut bytes, 0x904, address).unwrap();
        let error = build(&bytes, &setup, &api).unwrap_err().to_string();
        assert!(error.contains("outside extracted data"), "{error}");

        let (mut bytes, setup, api) = fixture();
        put(&mut bytes, 0x83c, 0xe59f0064).unwrap(); // LDR r0,[pc,#0x64]
        put(&mut bytes, 0x8a8, address).unwrap();
        let error = build(&bytes, &setup, &api).unwrap_err().to_string();
        assert!(
            error.contains("literal load refers to internal storage"),
            "{error}"
        );
    }
}

#[test]
fn storage_beyond_the_update_envelope_is_preserved_as_a_separate_dependency() {
    let (mut bytes, setup, api) = fixture();
    // Setup may consult preserved per-unit storage outside the DAT payload.
    // Retaining it does not establish qualification with the entire chip zeroed.
    put(&mut bytes, 0x8c0, immediate(0xe3000000, 0xfff8)).unwrap();
    put(&mut bytes, 0x8c4, immediate(0xe3400000, 0xf3ff)).unwrap();
    let output = build(&bytes, &setup, &api).unwrap();
    assert_eq!(pair(&output, 0xc0, 0).unwrap(), 0xf3fffff8);
}

#[test]
fn replayed_header_operations_cannot_read_write_or_poll_internal_storage() {
    for operation in 0..3 {
        let (mut bytes, setup, api) = fixture();
        put(&mut bytes, 0x14, 1).unwrap();
        put(&mut bytes, 0x18, 0xf0020000 | operation).unwrap();
        let error = build(&bytes, &setup, &api).unwrap_err().to_string();
        assert!(
            error.contains("header operation accesses internal storage"),
            "{error}"
        );
    }
    // Delay rows encode a cycle count; their high address bits are unused.
    let (mut bytes, setup, api) = fixture();
    put(&mut bytes, 0x14, 1).unwrap();
    put(&mut bytes, 0x18, 0xf0020003).unwrap();
    build(&bytes, &setup, &api).unwrap();
}

#[test]
fn invalid_header_counts_and_literal_pool_extents_refuse() {
    let (mut bytes, setup, api) = fixture();
    put(&mut bytes, 0x14, 0xfe).unwrap();
    assert!(build(&bytes, &setup, &api).is_err());
    let (mut bytes, setup, api) = fixture();
    put(&mut bytes, 0x83c, 0xe59f0fff).unwrap();
    assert!(build(&bytes, &setup, &api).is_err());
}
