//! Instruction fetches from the serial-flash aperture execute the array in place, under the same gated read as loads, and
//! a copy is dropped when the array or the controller's read state changes.
use fujimod_emulator_xp4_board::{Definition, Session};
use serde_json::{json, Value};
const FLASH: u64 = 0xf0000000;
const CONTROLLER: u64 = 0xfffd8000;
const PAD: u64 = 0xfff4108c;
const GOOD: u64 = 9;
const BAD: u64 = 5;
const CODE_AT: u64 = 0x1100;
fn definition(sample: u64, control: u64, strict: bool) -> Value {
    definition_with(sample, control, strict, true)
}
fn definition_with(sample: u64, control: u64, strict: bool, write_protect: bool) -> Value {
    let registers = [
        (1, CONTROLLER), (2, PAD), (3, FLASH + CODE_AT), (4, control), (5, 0xffff5545u64), (6, 0x13),
        (7, 0x101003), (8, sample), (9, BAD), (11, 0x8000), (15, 0),
    ];
    let mut definition = json!({"ram_size":65536,"cores":1,"discovery":false,"cbar":0xffff0000u32,
        "storage_jedec":0x202018,"storage_dma_routes":(32..48).collect::<Vec<_>>(),
        "storage_geometry":[4096,256,10,2],"storage_irq":48,
        "gpio":vec![0u16; 16],
        "storage_mapped_read":{
            "states":[{"control":[0x780071,0x80041],"clock":0x13,"mode":0x101003,"samples":[GOOD, 11],"pad":true}],
            "miss_byte":0xa5,"pad":[PAD,0xffff,0x5545]},
        "storage_write_protect":write_protect,
        "initial_registers":[registers.iter().map(|&(r, v)| json!([{"Arm32":{"R":r}}, v])).collect::<Vec<_>>()]});
    if strict {
        definition["storage_strict_backing"] = json!(true);
    }
    definition
}
const CONFIGURE: [u32; 5] = [
    0xe5814000, // str r4, [r1]          control
    0xe5825000, // str r5, [r2]          pad function
    0xe5816004, // str r6, [r1, #4]      clock
    0xe5817014, // str r7, [r1, #0x14]   mode
    0xe5818010, // str r8, [r1, #0x10]   sample point
];
const CALL: u32 = 0xe12fff33; // blx r3: into the aperture
const ARRAY_CODE: [u32; 4] = [
    0xe3a00055, // mov r0, #0x55
    0xe58b0000, // str r0, [r11]
    0xe58b0004, // str r0, [r11, #4]
    0xe12fff1e, // bx lr
];
fn nor(length: usize) -> Vec<u8> {
    let mut nor = vec![0xff; length];
    for (i, word) in ARRAY_CODE.iter().enumerate() {
        nor[CODE_AT as usize + 4 * i..CODE_AT as usize + 4 * i + 4].copy_from_slice(&word.to_le_bytes());
    }
    nor
}
fn run(definition: Value, nor: Vec<u8>, program: &[u32]) -> (Session, [u32; 2]) {
    let s = Session::new(serde_json::from_value::<Definition>(definition).unwrap(), Some(nor), vec![]).unwrap();
    let bytes: Vec<u8> = program.iter().flat_map(|w| w.to_le_bytes()).collect();
    s.write(0, &bytes).unwrap();
    let _ = s.run(0, 200_000);
    let mut out = [0; 8];
    s.read(0x8000, &mut out).unwrap();
    (s, [u32::from_le_bytes(out[..4].try_into().unwrap()), u32::from_le_bytes(out[4..].try_into().unwrap())])
}
fn once() -> Vec<u32> {
    [CONFIGURE.as_slice(), &[CALL, 0xe320f003]].concat() // wfi
}
#[test]
fn code_in_the_aperture_runs_where_the_gate_reads_data() {
    let (s, marker) = run(definition(GOOD, 0x80041, false), nor(8192), &once());
    assert!(s.state(false).failure.is_none(), "{:?}", s.state(false).failure);
    assert_eq!(marker, [0x55, 0x55]);
}
#[test]
fn a_fetch_outside_the_sampling_window_executes_the_miss_pattern_not_the_array() {
    let (_, marker) = run(definition(BAD, 0x80041, false), nor(8192), &once());
    assert_eq!(marker, [0, 0]);
}
#[test]
fn a_controller_state_no_gate_state_declares_fails_the_fetch_instead_of_mapping_the_page() {
    let (s, marker) = run(definition(GOOD, 0x7, false), nor(8192), &once());
    let failure = s.state(false).failure.expect("the fetch must fail");
    assert!(failure.contains("no declared controller state"), "{failure}");
    assert_eq!(marker, [0, 0]);
}
#[test]
fn a_page_the_image_does_not_wholly_back_is_refused_not_executed() {
    // 6 KiB image: the page at 0x1000 reaches 0x2000, past the end.
    let (s, marker) = run(definition(GOOD, 0x80041, true), nor(6144), &once());
    assert!(s.state(false).failure.is_some());
    assert_eq!(marker, [0, 0]);
    // The same code with the whole page inside the image runs.
    let (s, marker) = run(definition(GOOD, 0x80041, true), nor(8192), &once());
    assert!(s.state(false).failure.is_none(), "{:?}", s.state(false).failure);
    assert_eq!(marker, [0x55, 0x55]);
}
#[test]
fn a_controller_read_state_change_drops_the_copy_and_the_next_fetch_reads_again() {
    let program = [
        CONFIGURE.as_slice(),
        &[
            CALL,
            0xe5819010, // str r9, [r1, #0x10]  a sample point outside the window
            0xe3a00000, // mov r0, #0
            0xe58b0000, // str r0, [r11]        clear the first marker
            CALL,       // the page is read again, now as the miss pattern
            0xe320f003,
        ],
    ]
    .concat();
    let (_, marker) = run(definition(GOOD, 0x80041, false), nor(8192), &program);
    // The first call ran the array; the second did not run it, so the cleared word stays clear and the other stays set.
    assert_eq!(marker, [0, 0x55]);
}
#[test]
fn fetching_a_page_the_cpu_already_read_as_data_replaces_its_mapping() {
    let program = [
        CONFIGURE.as_slice(),
        &[
            0xe5930000, // ldr r0, [r3]   a load maps the page as aperture MMIO (a fetch then faults as protected)
            CALL,
            0xe320f003,
        ],
    ]
    .concat();
    let (s, marker) = run(definition(GOOD, 0x80041, false), nor(8192), &program);
    assert!(s.state(false).failure.is_none(), "{:?}", s.state(false).failure);
    assert_eq!(marker, [0x55, 0x55]);
}
/// `ldr r0, [pc, #offset]` at word `at`, reading the literal word at `literal`.
fn ldr_r0(at: usize, literal: usize) -> u32 {
    0xe59f0000 | ((literal - at) as u32 * 4 - 8)
}
#[test]
fn an_erase_that_completes_makes_the_next_fetch_read_the_erased_array() {
    // mov r2, #200; add r2, r2, #0x400; subs r2, r2, #1; bne: long enough for each command and the erase deadline.
    let wait = [0xe3a020c8u32, 0xe2822b01, 0xe2522001, 0x1afffffd];
    let mut program: Vec<u32> = CONFIGURE.to_vec();
    program.push(CALL);
    program.push(0xe58b0008); // str r0, [r11, #8]  proof that the first call ran the array
    let literals = 100;
    program.push(ldr_r0(program.len(), literals)); // write enable
    program.push(0xe5810090); // str r0, [r1, #0x90]
    program.extend(wait);
    program.push(ldr_r0(program.len(), literals + 1)); // erase address
    program.push(0xe5810094); // str r0, [r1, #0x94]
    program.push(ldr_r0(program.len(), literals + 2)); // four-kib erase
    program.push(0xe5810090); // str r0, [r1, #0x90]
    program.extend(wait);
    program.extend([
        0xe3a00000, // mov r0, #0
        0xe58b0000, // str r0, [r11]
        0xe58b0004, // str r0, [r11, #4]
        CALL,       // the page now reads as erased: 0xffffffff is no instruction that sets the markers
        0xe320f003, // wfi
    ]);
    assert!(program.len() < literals);
    program.resize(literals, 0xe320f003);
    program.extend([0x06000001, 0xf0001000, 0x210b0001]);
    let (s, marker) = run(definition_with(GOOD, 0x80041, false, false), nor(8192), &program);
    let mut first = [0; 4];
    s.read(0x8008, &mut first).unwrap();
    assert_eq!(u32::from_le_bytes(first), 0x55);
    // Had the copy of the page survived the erase, the second call would have set both words again.
    assert_eq!(marker, [0, 0]);
}
