use fujimod_emulator_storage::fat::{Options, Reader, format, merge};
#[test]
fn merge_preserves_valid_preallocation_and_releases_it_on_replacement() {
    let options = Options {
        sectors: 0x400000,
        max_dirty_sectors: 65536,
        partition_start: 2048,
        sectors_per_cluster: 1,
        label: "MERGE".into(),
        read_only_files: Vec::new(),
    };
    let mut card = format(&options, &[("A.BIN".into(), vec![37; 1536])]).unwrap();
    let boot = card.sector(2048).unwrap();
    let reserved = u16::from_le_bytes(boot[14..16].try_into().unwrap()) as u32;
    let fat_size = u32::from_le_bytes(boot[36..40].try_into().unwrap());
    let fat = 2048 + reserved;
    let data = fat + boot[16] as u32 * fat_size;
    let root = card.sector(data).unwrap();
    let slot = root
        .chunks_exact(32)
        .position(|row| &row[..11] == b"A       BIN")
        .unwrap();
    let first = u16::from_le_bytes(root[slot * 32 + 26..slot * 32 + 28].try_into().unwrap()) as u32;
    card.write_bytes(
        data as u64 * 512 + slot as u64 * 32 + 28,
        &512u32.to_le_bytes(),
    )
    .unwrap();
    let before = Reader::new(&card).unwrap().read("A.BIN", 65536).unwrap();
    assert_eq!(before, vec![37; 512]);
    let merged = merge(&card, &[("NEW.TXT".into(), b"inserted".to_vec())]).unwrap();
    assert_eq!(
        Reader::new(&merged).unwrap().read("A.BIN", 65536).unwrap(),
        before
    );
    let replaced = merge(&merged, &[("A.BIN".into(), vec![5])]).unwrap();
    assert_eq!(
        Reader::new(&replaced)
            .unwrap()
            .read("A.BIN", 65536)
            .unwrap(),
        vec![5]
    );
    for cluster in first + 1..first + 3 {
        let table = replaced.sector(fat + cluster / 128).unwrap();
        let at = (cluster % 128 * 4) as usize;
        assert_eq!(
            u32::from_le_bytes(table[at..at + 4].try_into().unwrap()) & 0x0fffffff,
            0
        );
    }
}
