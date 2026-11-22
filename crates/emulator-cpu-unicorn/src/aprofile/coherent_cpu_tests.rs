use super::*;

const BUFFER: u64 = 0x4000;
const LENGTH: usize = 0x100;
const OLD: u8 = 0xaa;
const NEW: u8 = 0x11;

fn filler(clean: Option<u32>) -> (Vec<u32>, u64) {
    let mut code = vec![
        0xe3a0_0901, // mov r0, #0x4000
        0xe3a0_1011, // mov r1, #0x11
        0xe181_1401, // orr r1, r1, r1, lsl #8
        0xe181_1801, // orr r1, r1, r1, lsl #16
        0xe3a0_2c41, // mov r2, #0x4100
        0xe480_1004, // str r1, [r0], #4
        0xe150_0002, // cmp r0, r2
        0x1aff_fffc, // bne str
    ];
    if let Some(span) = clean {
        code.extend([
            0xe3a0_3901,        // mov r3, #0x4000
            0xe3a0_4901,        // mov r4, #0x4000
            0xe284_4000 | span, // add r4, r4, #span
            0xee07_3f3a,        // mcr p15, 0, r3, c7, c10, 1
            0xe283_3020,        // add r3, r3, #32
            0xe153_0004,        // cmp r3, r4
            0x1aff_fffb,        // bne mcr
        ]);
    }
    let end = 0x1000 + code.len() as u64 * 4;
    code.push(0xeaff_fffe); // b .
    (code, end)
}

/// Runs the filler over a buffer of old bytes; returns the device's view of
/// the first and second half.
fn run_filler(options: Option<CoherenceOptions>, clean: Option<u32>) -> (Vec<u8>, Vec<u8>) {
    let raw = crate::test_support::library()
        .open(Model::CortexA7, 1 << 20)
        .unwrap();
    let cpu = Core::new(raw, Memory::default()).unwrap();
    cpu.map_zeroed(0, 0x8000, 7).unwrap();
    cpu.write_register(arm::CPSR, &0xd3u64.to_le_bytes())
        .unwrap();
    cpu.write_memory(BUFFER, &[OLD; LENGTH]).unwrap();
    let (code, end) = filler(clean);
    let bytes: Vec<u8> = code.iter().flat_map(|w| w.to_le_bytes()).collect();
    cpu.write_memory(0x1000, &bytes).unwrap();
    let domain = options.map(|o| CoherenceDomain::new(&o).unwrap());
    cpu.set_coherence(domain.as_ref()).unwrap();
    cpu.run(Run {
        begin: 0x1000,
        until: Some(end),
        instructions: Some(10_000),
        watchdog_us: 0,
    })
    .unwrap();
    let mut first = vec![0; LENGTH / 2];
    let mut second = vec![0; LENGTH / 2];
    cpu.coherent_device_read(BUFFER, &mut first).unwrap();
    cpu.coherent_device_read(BUFFER + LENGTH as u64 / 2, &mut second)
        .unwrap();
    (first, second)
}

fn declared() -> Option<CoherenceOptions> {
    Some(CoherenceOptions {
        line_bytes: 32,
        ranges: vec![(BUFFER, BUFFER + LENGTH as u64)],
    })
}

#[test]
fn stores_without_a_clean_are_invisible_to_devices() {
    let (first, second) = run_filler(declared(), None);
    assert_eq!((first, second), (vec![OLD; 128], vec![OLD; 128]));
}

#[test]
fn cleaning_the_first_half_publishes_only_the_first_half() {
    let (first, second) = run_filler(declared(), Some(0x80));
    assert_eq!((first, second), (vec![NEW; 128], vec![OLD; 128]));
}

#[test]
fn cleaning_the_whole_buffer_publishes_all_of_it() {
    let (first, second) = run_filler(declared(), Some(0xc01));
    assert_eq!((first, second), (vec![NEW; 128], vec![NEW; 128]));
}

#[test]
fn without_the_option_devices_see_every_store() {
    let (first, second) = run_filler(None, None);
    assert_eq!((first, second), (vec![NEW; 128], vec![NEW; 128]));
}

const SECTION: u32 = 0x10_0000;
const TABLE: u64 = 0x8000;
const CACHEABLE: u64 = 0x10_0000;
const UNCACHED: u64 = 0x20_0000;

/// Enables the MMU over identity-mapped 1 MiB sections (0 and 1 write-back,
/// 2 normal non-cacheable), then fills 32 bytes at `buffer`.
fn mmu_store(buffer: u64) -> Vec<u8> {
    let raw = crate::test_support::library()
        .open(Model::CortexA7, 1 << 20)
        .unwrap();
    let cpu = Core::new(raw, Memory::default()).unwrap();
    cpu.map_zeroed(0, 0x30_0000, 7).unwrap();
    cpu.write_register(arm::CPSR, &0xd3u64.to_le_bytes())
        .unwrap();
    for (index, descriptor) in [0x0c0e, SECTION | 0x0c0e, (2 * SECTION) | 0x1c02]
        .into_iter()
        .enumerate()
    {
        let descriptor: u32 = descriptor;
        cpu.write_memory(TABLE + index as u64 * 4, &descriptor.to_le_bytes())
            .unwrap();
    }
    cpu.write_memory(buffer, &[OLD; 32]).unwrap();
    let code = [
        0xe3a0_0902u32, // mov r0, #0x8000
        0xee02_0f10,    // mcr p15, 0, r0, c2, c0, 0 (TTBR0)
        0xe3e0_0000,    // mvn r0, #0
        0xee03_0f10,    // mcr p15, 0, r0, c3, c0, 0 (DACR: manager)
        0xee11_0f10,    // mrc p15, 0, r0, c1, c0, 0
        0xe380_0001,    // orr r0, r0, #1
        0xee01_0f10,    // mcr p15, 0, r0, c1, c0, 0 (MMU on)
        0xe3a0_1011,    // mov r1, #0x11
        0xe181_1401,    // orr r1, r1, r1, lsl #8
        0xe181_1801,    // orr r1, r1, r1, lsl #16
    ];
    let mut code = code.to_vec();
    // movw/movt r0, buffer; 8 words of str r1, [r0], #4
    let immediate = |value: u32| ((value & 0xf000) << 4) | (value & 0xfff);
    code.push(0xe300_0000 | immediate(buffer as u32 & 0xffff));
    code.push(0xe340_0000 | immediate((buffer >> 16) as u32));
    code.extend([0xe480_1004; 8]);
    let end = 0x1000 + code.len() as u64 * 4;
    code.push(0xeaff_fffe);
    let bytes: Vec<u8> = code.iter().flat_map(|w| w.to_le_bytes()).collect();
    cpu.write_memory(0x1000, &bytes).unwrap();
    let domain = CoherenceDomain::new(&CoherenceOptions {
        line_bytes: 32,
        ranges: vec![
            (CACHEABLE, CACHEABLE + 0x1000),
            (UNCACHED, UNCACHED + 0x1000),
        ],
    })
    .unwrap();
    cpu.set_coherence(Some(&domain)).unwrap();
    cpu.run(Run {
        begin: 0x1000,
        until: Some(end),
        instructions: Some(10_000),
        watchdog_us: 0,
    })
    .unwrap();
    let mut seen = vec![0; 32];
    cpu.coherent_device_read(buffer, &mut seen).unwrap();
    seen
}

#[test]
fn a_cacheable_mapping_stays_dirty_until_cleaned() {
    assert_eq!(mmu_store(CACHEABLE), vec![OLD; 32]);
}

#[test]
fn a_non_cacheable_mapping_reaches_devices_without_a_clean() {
    assert_eq!(mmu_store(UNCACHED), vec![NEW; 32]);
}
