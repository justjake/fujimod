use fujimod_emulator_xp4_board::{Definition, Session};
use serde_json::{Value, json};

fn definition() -> Value {
    json!({"ram_size":65536,"cores":1,"discovery":false,"cbar":0xffff0000u32,
        "memory_regions":[
            {"base":0xfff00000u32,"size":4096,"permissions":5},
            {"base":0xfff10000u32,"size":4096,"permissions":7}],
        "initial_registers":[[[{"Arm32":{"R":15}},0xfff00000u32],
            [{"Arm32":{"R":0}},0xfff10000u32]]]})
}

#[test]
fn reset_executes_read_only_rom_and_writes_separate_sram() {
    let definition: Definition = serde_json::from_value(definition()).unwrap();
    let session = Session::new(definition, None).unwrap();
    let code = [0xe3a0102au32, 0xe5801000, 0xe320f003];
    let code: Vec<u8> = code.into_iter().flat_map(u32::to_le_bytes).collect();
    session.write(0xfff00000, &code).unwrap();
    session.run(0, 10).unwrap();
    let mut actual = [0; 4];
    session.read(0xfff10000, &mut actual).unwrap();
    assert_eq!(u32::from_le_bytes(actual), 42);
    assert!(session.read(0xfff20000, &mut actual).is_err());
    assert_eq!(session.read_virtual(0, 0xfff10000, 4).unwrap(), actual);
}

#[test]
fn physical_windows_and_reset_registers_refuse_ambiguous_extents() {
    let mut input = definition();
    input["memory_regions"][1]["base"] = json!(0xfff00000u32);
    let definition: Definition = serde_json::from_value(input).unwrap();
    assert!(Session::new(definition, None).is_err());
    let mut input = self::definition();
    input["initial_registers"] = json!([[], []]);
    let definition: Definition = serde_json::from_value(input).unwrap();
    assert!(Session::new(definition, None).is_err());
}

#[test]
fn cpu_cannot_write_read_only_rom() {
    let mut input = definition();
    input["initial_registers"][0][1][1] = json!(0xfff00000u32);
    let definition: Definition = serde_json::from_value(input).unwrap();
    let session = Session::new(definition, None).unwrap();
    let code = [0xe3a0102au32, 0xe5801000, 0xe320f003];
    let code: Vec<u8> = code.into_iter().flat_map(u32::to_le_bytes).collect();
    session.write(0xfff00000, &code).unwrap();
    let _ = session.run(0, 10);
    let mut actual = [0; 4];
    session.read(0xfff00000, &mut actual).unwrap();
    assert_eq!(u32::from_le_bytes(actual), 0xe3a0102a);
    assert!(session.state(false).failure.is_some());
}
