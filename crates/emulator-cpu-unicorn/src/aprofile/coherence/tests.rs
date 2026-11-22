use super::*;

const BASE: u64 = 0x1000;

fn ranges() -> CoherentRanges {
    CoherentRanges::new(32, &[(BASE, BASE + 0x100)]).unwrap()
}

/// Processor-view memory beside the model, as a caller would drive it.
struct Rig {
    model: CoherentRanges,
    memory: Vec<u8>,
}
impl Rig {
    fn new() -> Self {
        Self {
            model: ranges(),
            memory: vec![0xaa; 0x2000],
        }
    }
    fn store(&mut self, address: u64, byte: u8, len: usize) {
        let a = address as usize;
        let before = self.memory[a..a + len].to_vec();
        self.model.cpu_store(address, len, &before);
        self.memory[a..a + len].fill(byte);
    }
    fn device_read(&self, address: u64, len: usize) -> Vec<u8> {
        let a = address as usize;
        let mut buffer = self.memory[a..a + len].to_vec();
        self.model.device_read(address, &mut buffer);
        buffer
    }
    fn device_write(&mut self, address: u64, bytes: &[u8]) {
        let a = address as usize;
        let before = self.memory[a..a + bytes.len()].to_vec();
        self.model.device_write(address, bytes, &before);
        self.memory[a..a + bytes.len()].copy_from_slice(bytes);
    }
    fn cpu_load(&mut self, address: u64, len: usize) -> Vec<u8> {
        let a = address as usize;
        let mut buffer = self.memory[a..a + len].to_vec();
        if let Some(stale) = self.model.cpu_load(address, len) {
            stale.apply(address, &mut buffer);
        }
        buffer
    }
    fn patch(&mut self, patches: Vec<Patch>) {
        for p in patches {
            let a = p.address as usize;
            self.memory[a..a + p.bytes.len()].copy_from_slice(&p.bytes);
        }
    }
}

#[test]
fn store_is_invisible_to_device_until_clean() {
    let mut rig = Rig::new();
    rig.store(BASE, 0x11, 64);
    assert_eq!(rig.device_read(BASE, 64), vec![0xaa; 64]);
    assert!(rig.model.clean_line(BASE).is_empty());
    assert!(rig.model.clean_line(BASE + 32).is_empty());
    assert_eq!(rig.device_read(BASE, 64), vec![0x11; 64]);
}

#[test]
fn cleaning_half_a_buffer_exposes_half() {
    let mut rig = Rig::new();
    rig.store(BASE, 0x22, 128);
    for line in 0..2 {
        rig.model.clean_line(BASE + line * 32);
    }
    let seen = rig.device_read(BASE, 128);
    assert_eq!(seen[..64], [0x22; 64]);
    assert_eq!(seen[64..], [0xaa; 64]);
}

#[test]
fn device_write_stays_stale_for_cpu_until_invalidate() {
    let mut rig = Rig::new();
    assert_eq!(rig.cpu_load(BASE, 32), vec![0xaa; 32]);
    rig.device_write(BASE + 4, &[1, 2, 3, 4]);
    let seen = rig.cpu_load(BASE, 32);
    assert_eq!(seen, vec![0xaa; 32]);
    assert_eq!(rig.device_read(BASE + 4, 4), vec![1, 2, 3, 4]);
    rig.model.invalidate_line(BASE);
    assert_eq!(rig.cpu_load(BASE + 4, 4), vec![1, 2, 3, 4]);
}

#[test]
fn invalidating_a_dirty_line_reverts_memory() {
    let mut rig = Rig::new();
    rig.store(BASE, 0x33, 32);
    let patches = rig.model.invalidate_line(BASE);
    rig.patch(patches);
    assert_eq!(rig.cpu_load(BASE, 32), vec![0xaa; 32]);
}

#[test]
fn device_write_over_dirty_bytes_keeps_cpu_value_until_clean() {
    let mut rig = Rig::new();
    rig.store(BASE, 0x44, 32);
    rig.device_write(BASE, &[9; 4]);
    assert_eq!(rig.cpu_load(BASE, 4), vec![0x44; 4]);
    assert_eq!(rig.device_read(BASE, 4), vec![9; 4]);
    let patches = rig.model.clean_line(BASE);
    rig.patch(patches);
    assert_eq!(rig.device_read(BASE, 4), vec![0x44; 4]);
}

#[test]
fn outside_ranges_is_untouched() {
    let mut rig = Rig::new();
    rig.store(BASE - 32, 0x55, 32);
    rig.store(BASE + 0x100, 0x55, 32);
    assert_eq!(rig.device_read(BASE - 32, 32), vec![0x55; 32]);
    assert_eq!(rig.device_read(BASE + 0x100, 32), vec![0x55; 32]);
    rig.device_write(BASE - 32, &[1; 4]);
    assert_eq!(rig.cpu_load(BASE - 32, 4), vec![1; 4]);
    assert!(rig.model.clean_all().is_empty());
}

#[test]
fn empty_ranges_track_nothing() {
    let mut model = CoherentRanges::new(32, &[]).unwrap();
    model.cpu_store(0, 64, &[0; 64]);
    assert!(model.cpu_load(0, 64).is_none());
    assert!(model.lines.is_empty());
}
