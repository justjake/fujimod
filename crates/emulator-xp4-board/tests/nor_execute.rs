use fujimod_emulator_xp4_board::{Definition, Session};
use serde_json::{json, Value};
const FLASH: usize = 0xf0000000;
fn definition(cores: usize, entry: usize) -> Value {
    json!({"ram_size":65536,"cores":cores,"discovery":false,"cbar":0xffff0000u32,
        "storage_jedec":0x202018,"storage_dma_routes":(32..48).collect::<Vec<_>>(),
        "storage_geometry":[4096,256,10,2],"storage_irq":48,
        "storage_immutable_prefix":{"bytes":65536},
        "initial_registers":(0..cores).map(|_| json!([
            [{"Arm32":{"R":15}},entry], [{"Arm32":{"R":0}},0x8000],
            [{"Arm32":{"R":2}},FLASH+0x100]])).collect::<Vec<_>>()})
}
fn session(input: Value, image: Vec<u8>) -> Result<Session, String> {
    Session::new(serde_json::from_value::<Definition>(input).unwrap(), Some(image), vec![])
}
fn image(entry: usize, words: &[u32]) -> Vec<u8> {
    let mut bytes = vec![0xff; 131072];
    bytes[0x100..0x104].copy_from_slice(&0x12345678u32.to_le_bytes());
    for (i, word) in words.iter().enumerate() {
        bytes[entry-FLASH+i*4..entry-FLASH+i*4+4].copy_from_slice(&word.to_le_bytes());
    }
    bytes
}
#[test]
fn retained_prefix_executes_and_reads_data_on_each_core() {
    for (cores, entry) in [(1, FLASH+0x7000),(2, FLASH+0x9000)] {
        let s = session(definition(cores,entry), image(entry,&[0xe5921000,0xe5801000,0xe320f003])).unwrap();
        for core in 0..cores { s.run(core, 8).unwrap(); }
        let mut out = [0;4]; s.read(0x8000,&mut out).unwrap();
        assert_eq!(u32::from_le_bytes(out),0x12345678);
        s.read(FLASH+0x100,&mut out).unwrap();
        assert_eq!(u32::from_le_bytes(out),0x12345678);
        assert!(s.state(false).failure.is_none());
    }
}
#[test]
fn immutable_host_writes_and_cross_boundary_writes_refuse_atomically() {
    let entry=FLASH+0x7000;
    let s=session(definition(1,entry),image(entry,&[0xe320f003])).unwrap();
    for (address, bytes) in [(FLASH,vec![0;4]),(FLASH+65534,vec![0;4]),(FLASH-2,vec![0;4])] {
        assert!(s.write(address,&bytes).is_err());
    }
    let mut out=[0;4];s.read(FLASH+65532,&mut out).unwrap();assert_eq!(out,[0xff;4]);
    assert!(s.state(false).failure.is_none());
}
#[test]
fn guest_store_cannot_modify_prefix() {
    let entry=FLASH+0x7000;
    let s=session(definition(1,entry),image(entry,&[0xe3a01000,0xe5821000,0xe320f003])).unwrap();
    let _=s.run(0,8);
    let mut out=[0;4];s.read(FLASH+0x100,&mut out).unwrap();
    assert_eq!(u32::from_le_bytes(out),0x12345678);
    assert!(s.state(false).failure.is_some());
}
#[test]
fn prefix_policy_and_owner_overlaps_are_checked() {
    let entry=FLASH+0x7000;
    for count in [0,1,4097,196608,usize::MAX] {
        let mut d=definition(1,entry);d["storage_immutable_prefix"]["bytes"]=json!(count);
        assert!(session(d, image(entry,&[])).is_err());
    }
    let mut d=definition(1,entry);d.as_object_mut().unwrap().remove("storage_jedec");
    assert!(session(d,image(entry,&[])).is_err());
    for base in [FLASH,FLASH+65536,0xfffd8000,0xfffe0000,0xffff1000] {
        let mut d=definition(1,entry);d["memory_regions"]=json!([{"base":base,"size":4096,"permissions":5}]);
        assert!(session(d,image(entry,&[])).is_err(),"{base:x}");
    }
    let mut d=definition(1,entry);d["memory_regions"]=json!([{"base":0xeffff000u32,"size":4096,"permissions":5}]);
    assert!(session(d,image(entry,&[])).is_ok());
}
#[test]
fn omitted_policy_preserves_non_executable_aperture() {
    let entry=FLASH+0x7000;let mut d=definition(1,entry);
    d.as_object_mut().unwrap().remove("storage_immutable_prefix");
    let s=session(d,image(entry,&[0xe320f003])).unwrap();
    assert!(s.read(FLASH,&mut [0;4]).is_err());
    let _=s.run(0,8);assert!(s.state(false).failure.is_some());
}

#[test]
fn dma_reads_the_same_retained_prefix_as_cpu_fetch_and_data() {
    let entry=FLASH+0x7000;
    let writes=[(3,0x20,0x606u32),(3,0x68,(FLASH+0x100) as u32),(3,0x6c,4),
        (3,0x60,1),(5,0,0x1002a),(4,0x10,3),(4,0x14,(FLASH+0x100) as u32),
        (4,0x18,0x9000),(4,0x28,0x10000001)];
    let mut words=Vec::new();
    for &(register,offset,_) in &writes {
        words.push(0xe59f1000); // LDR r1, literal, fixed up below.
        words.push(0xe5801000 | register<<16 | offset);
    }
    words.extend([0xe1a00000;64]); words.push(0xe320f003);
    let literals=words.len();
    for (index,&(_,_,value)) in writes.iter().enumerate() {
        let offset=(literals+index)*4-(index*2*4+8);
        words[index*2] |= offset as u32; words.push(value);
    }
    let mut d=definition(1,entry);
    d["initial_registers"][0].as_array_mut().unwrap().extend([
        json!([{ "Arm32":{"R":3}},0xfffd8000u32]),
        json!([{ "Arm32":{"R":4}},0xfffe0000u32]),
        json!([{ "Arm32":{"R":5}},0xfff4a500u32])]);
    let s=session(d,image(entry,&words)).unwrap(); s.run(0,150).unwrap();
    assert!(s.state(false).failure.is_none(),"{:?}",s.state(false).failure);
    let mut direct=[0;4];let mut dma=[0;4];
    s.read(FLASH+0x100,&mut direct).unwrap();s.read(0x9000,&mut dma).unwrap();
    assert_eq!(direct,0x12345678u32.to_le_bytes());assert_eq!(direct,dma);
}
