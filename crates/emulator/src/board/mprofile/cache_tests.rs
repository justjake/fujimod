use super::exclusive_tests::{board, board_with_bus};
use super::*;
use crate::board::bus::{Bus, Mode};
use fujimod_emulator_devices::{Cx, Device};

#[test]
fn peer_and_host_code_writes_survive_partial_quanta() {
    for quantum in [1, 64] {
        let mut b = board(&[0xbf00, 0x2601, 0xbf30], &[0xbf30]);
        b.run_for_instructions(1, quantum).unwrap();
        b.write_memory(1, 0x4102, &0x2609u16.to_le_bytes()).unwrap();
        b.run_until(20, quantum).unwrap();
        assert_eq!(get(b.cpu(0).unwrap(), reg(6)).unwrap(), 9);
    }
    // Populate core0's translation before a real STRH through core1's alias.
    let mut b = board(&[0xbf00, 0xbf00, 0x2601, 0xbf30], &[0xbf00, 0x802c, 0xbf30]);
    b.run_for_instructions(2, 1).unwrap();
    set(b.cpu(1).unwrap(), reg(4), 0x2609).unwrap();
    set(b.cpu(1).unwrap(), reg(5), 0x4104).unwrap();
    b.run_until(20, 1).unwrap();
    assert_eq!(get(b.cpu(0).unwrap(), reg(6)).unwrap(), 9);
}

#[test]
fn native_store_through_another_local_alias_replaces_cached_code() {
    let mut b = board(&[0xbf00, 0x802c, 0x2601, 0xbf30], &[0xbf30]);
    b.run_for_instructions(1, 64).unwrap();
    set(b.cpu(0).unwrap(), reg(4), 0x2609).unwrap();
    set(b.cpu(0).unwrap(), reg(5), 0x20104).unwrap();
    b.run_until(20, 64).unwrap();
    assert_eq!(get(b.cpu(0).unwrap(), reg(6)).unwrap(), 9);
    assert_eq!(b.state()["instructions"], json!([4, 1]));
}

#[test]
fn emulated_exclusive_store_through_another_alias_replaces_cached_code() {
    let mut b = board(
        &[0xbf00, 0xe8d0, 0x1f5f, 0xe8c0, 0x3f52, 0x2601, 0xbf30],
        &[0xbf30],
    );
    b.run_for_instructions(1, 64).unwrap();
    set(b.cpu(0).unwrap(), reg(0), 0x2010a).unwrap();
    set(b.cpu(0).unwrap(), reg(3), 0x2609).unwrap();
    b.run_until(20, 64).unwrap();
    assert_eq!(get(b.cpu(0).unwrap(), reg(2)).unwrap(), 0);
    assert_eq!(get(b.cpu(0).unwrap(), reg(6)).unwrap(), 9);
    assert_eq!(b.state()["instructions"], json!([5, 1]));
}

struct DmaCodeWrite(bool);
impl Device for DmaCodeWrite {
    fn name(&self) -> &'static str {
        "test-dma-code-write"
    }
    fn read(&mut self, cx: &mut Cx<'_>, _: u64, _: u8) -> u64 {
        cx.unmodeled();
        0
    }
    fn write(&mut self, cx: &mut Cx<'_>, _: u64, _: u8, _: u64) {
        cx.unmodeled();
    }
    fn next_deadline(&self, _: u64) -> Option<u64> {
        (!self.0).then_some(3)
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        if !self.0 && cx.now() >= 3 {
            self.0 = true;
            if let Err(e) = cx.ram_write(0x14106, &0x2609u16.to_le_bytes()) {
                cx.fail(e);
            }
        }
    }
}
#[test]
fn due_dma_code_write_restarts_the_current_peer_without_counting_stale_code() {
    for quantum in [1, 64] {
        let mut bus = Bus::new(Mode::Strict);
        bus.attach(Box::new(DmaCodeWrite(false)));
        let mut b = board_with_bus(&[0xbf30], &[0xbf00, 0xbf00, 0xbf00, 0x2601, 0xbf30], bus);
        b.run_until(20, quantum).unwrap();
        assert_eq!(get(b.cpu(1).unwrap(), reg(6)).unwrap(), 9);
        assert_eq!(b.state()["instructions"], json!([1, 5]));
    }
}
