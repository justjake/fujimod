//! Instruction fetches from the serial-flash aperture execute the array in place, under the same gated read as loads.
use fujimod_emulator_xp4_board::{Definition, Session};
use serde_json::{json, Value};
const FLASH: u64 = 0xf0000000;
const CONTROLLER: u64 = 0xfffd8000;
const PAD: u64 = 0xfff4108c;
fn definition(sample: u32) -> Value {
    let registers = [
        (1, CONTROLLER), (2, PAD), (3, FLASH + 0x1100), (4, 0x80041), (5, 0xffff5545u64), (6, 0x13),
        (7, 0x101003), (8, sample as u64), (11, 0x8000), (15, 0),
    ];
    json!({"ram_size":65536,"cores":1,"discovery":false,"cbar":0xffff0000u32,
        "storage_jedec":0x202018,"storage_dma_routes":(32..48).collect::<Vec<_>>(),
        "storage_geometry":[4096,256,10,2],"storage_irq":48,
        "gpio":vec![0u16; 16],
        "storage_mapped_read":{
            "states":[{"control":[0x780071,0x80041],"clock":0x13,"mode":0x101003,"samples":[9,11],"pad":true}],
            "miss_byte":0xa5,"pad":[PAD,0xffff,0x5545]},
        "storage_write_protect":true,
        "initial_registers":[registers.iter().map(|&(r, v)| json!([{"Arm32":{"R":r}}, v])).collect::<Vec<_>>()]})
}
const PROGRAM: [u32; 6] = [
    0xe5814000, // str r4, [r1]          control
    0xe5825000, // str r5, [r2]          pad function
    0xe5816004, // str r6, [r1, #4]      clock
    0xe5817014, // str r7, [r1, #0x14]   mode
    0xe5818010, // str r8, [r1, #0x10]   sample point
    0xe12fff13, // bx r3                 into the aperture
];
const ARRAY_CODE: [u32; 3] = [
    0xe3a00055, // mov r0, #0x55
    0xe58b0000, // str r0, [r11]
    0xe320f003, // wfi
];
fn run(sample: u32) -> (Session, u32) {
    let mut nor = vec![0xff; 8192];
    for (i, word) in ARRAY_CODE.iter().enumerate() {
        nor[0x1100 + 4 * i..0x1104 + 4 * i].copy_from_slice(&word.to_le_bytes());
    }
    let s = Session::new(serde_json::from_value::<Definition>(definition(sample)).unwrap(), Some(nor), vec![]).unwrap();
    let program: Vec<u8> = PROGRAM.iter().flat_map(|w| w.to_le_bytes()).collect();
    s.write(0, &program).unwrap();
    let _ = s.run(0, PROGRAM.len() + ARRAY_CODE.len());
    let mut out = [0; 4];
    s.read(0x8000, &mut out).unwrap();
    (s, u32::from_le_bytes(out))
}
#[test]
fn code_in_the_aperture_runs_where_the_gate_reads_data() {
    let (s, marker) = run(9);
    assert!(s.state(false).failure.is_none(), "{:?}", s.state(false).failure);
    assert_eq!(marker, 0x55);
}
#[test]
fn a_fetch_at_a_sample_outside_the_window_executes_the_miss_pattern_not_the_array() {
    let (_, marker) = run(5);
    assert_ne!(marker, 0x55);
}
