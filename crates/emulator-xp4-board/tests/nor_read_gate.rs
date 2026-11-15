//! The declared mapped-read gate on a whole board: the pad word the gate
//! owns is carved out of the GPIO bank, and a read outside every declared
//! state fails the run.
use fujimod_emulator_xp4_board::{Definition, Session};
use serde_json::{json, Value};
const FLASH: u64 = 0xf0000000;
const CONTROLLER: u64 = 0xfffd8000;
const PAD: u64 = 0xfff4108c;
fn definition(pad: u32, sample: u32) -> Value {
    let registers = [
        (1, CONTROLLER), (2, PAD), (3, FLASH + 0x100), (4, 0x80041), (5, pad as u64), (6, 0x13),
        (7, 0x101003), (8, sample as u64), (9, 0xfff41088), (10, 0x00010001), (11, 0x8000), (15, 0),
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
const PROGRAM: [u32; 11] = [
    0xe5814000, // str r4, [r1]          control
    0xe5825000, // str r5, [r2]          pad function
    0xe5816004, // str r6, [r1, #4]      clock
    0xe5817014, // str r7, [r1, #0x14]   mode
    0xe5818010, // str r8, [r1, #0x10]   sample point
    0xe5930000, // ldr r0, [r3]          mapped read
    0xe58b0000, // str r0, [r11]
    0xe589a000, // str r10, [r9]         neighboring GPIO word
    0xe592c000, // ldr r12, [r2]
    0xe58bc004, // str r12, [r11, #4]
    0xe320f003, // wfi
];
fn run(pad: u32, sample: u32) -> (Session, [u32; 2]) {
    let mut nor = vec![0xff; 8192];
    nor[0x100..0x104].copy_from_slice(&0x12345678u32.to_le_bytes());
    let s = Session::new(serde_json::from_value::<Definition>(definition(pad, sample)).unwrap(), Some(nor), vec![]).unwrap();
    let program: Vec<u8> = PROGRAM.iter().flat_map(|w| w.to_le_bytes()).collect();
    s.write(0, &program).unwrap();
    let _ = s.run(0, PROGRAM.len());
    let mut out = [0; 8];
    s.read(0x8000, &mut out).unwrap();
    (s, [u32::from_le_bytes(out[..4].try_into().unwrap()), u32::from_le_bytes(out[4..].try_into().unwrap())])
}
#[test]
fn initialized_controller_reads_array_data_through_the_carved_pad_word() {
    let (s, [word, pad]) = run(0xffff5545, 9);
    assert!(s.state(false).failure.is_none());
    assert_eq!((word, pad), (0x12345678, 0x5545));
}
#[test]
fn a_sample_outside_the_window_reads_the_miss_pattern() {
    let (s, [word, _]) = run(0xffff5545, 5);
    assert!(s.state(false).failure.is_none());
    assert_eq!(word, 0xa5a5a5a5);
}
#[test]
fn a_missing_pad_function_fails_the_run() {
    let (s, _) = run(0, 9);
    assert!(s.state(false).failure.is_some());
}
#[test]
fn a_gate_with_an_immutable_prefix_or_without_a_nor_is_refused() {
    let mut d = definition(0, 9);
    d["storage_immutable_prefix"] = json!({"bytes":4096});
    assert!(Session::new(serde_json::from_value::<Definition>(d).unwrap(), Some(vec![0xff; 8192]), vec![]).is_err());
    let mut d = definition(0, 9);
    for key in ["storage_jedec", "storage_dma_routes", "storage_geometry", "storage_irq"] {
        d.as_object_mut().unwrap().remove(key);
    }
    assert!(Session::new(serde_json::from_value::<Definition>(d).unwrap(), None, vec![]).is_err());
}
