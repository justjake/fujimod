use fujimod_emulator_devices::parts::nand::{Nand, Options, Overlay};
use std::{collections::BTreeMap, sync::Arc};

fn medium(page_bytes: usize, pages_per_block: u32, spare_bytes: usize) -> Nand {
    Nand::new(
        Options {
            bytes: page_bytes as u64 * u64::from(pages_per_block) * 3,
            page_bytes,
            pages_per_block,
            spare_bytes,
        },
        Arc::from(vec![0x77; page_bytes]),
        vec![Overlay {
            offset: page_bytes as u64,
            bytes: Arc::from(vec![0x35; page_bytes]),
        }],
        BTreeMap::from([(2, vec![0x42; spare_bytes])]),
    )
    .unwrap()
}

#[test]
fn committed_state_cold_import_preserves_inputs_programs_and_erases_for_board_geometries() {
    for (page_bytes, pages_per_block, spare_bytes) in [(512, 32, 16), (2048, 64, 512)] {
        let mut written = medium(page_bytes, pages_per_block, spare_bytes);
        written.erase(pages_per_block).unwrap();
        written
            .program(pages_per_block + 1, &vec![0xa5; page_bytes], &[0x12, 0x34])
            .unwrap();
        let export = written.committed_image().unwrap();
        let fresh = Nand::new(
            written.options().clone(),
            Arc::from(vec![0x77; page_bytes]),
            vec![],
            BTreeMap::new(),
        )
        .unwrap();
        let cold = fresh.restore_committed(&export).unwrap();
        let mut data = vec![0; page_bytes];
        cold.read(page_bytes as u64, &mut data).unwrap();
        assert_eq!(data, vec![0x35; page_bytes]);
        cold.read_spare(2, &mut data[..spare_bytes]).unwrap();
        assert_eq!(&data[..spare_bytes], vec![0x42; spare_bytes]);
        cold.read(u64::from(pages_per_block) * page_bytes as u64, &mut data)
            .unwrap();
        assert_eq!(data, vec![255; page_bytes]);
        cold.read(
            u64::from(pages_per_block + 1) * page_bytes as u64,
            &mut data,
        )
        .unwrap();
        assert_eq!(data, vec![0xa5; page_bytes]);
        let mut spare = vec![0; spare_bytes];
        cold.read_spare(pages_per_block + 1, &mut spare).unwrap();
        assert_eq!(&spare[..2], &[0x12, 0x34]);
        assert!(spare[2..].iter().all(|&byte| byte == 255));
        fresh
            .read(
                u64::from(pages_per_block + 1) * page_bytes as u64,
                &mut data,
            )
            .unwrap();
        assert_eq!(data, vec![255; page_bytes]);
        assert_eq!(cold.committed_image().unwrap(), export);
    }
}

#[test]
fn committed_output_is_deterministic_across_equivalent_program_order() {
    let mut first = medium(512, 32, 16);
    let mut second = medium(512, 32, 16);
    for (nand, rows) in [(&mut first, [32, 33]), (&mut second, [33, 32])] {
        nand.erase(32).unwrap();
        for row in rows {
            nand.program(row, &[0x69; 512], &[0x24; 16]).unwrap();
        }
    }
    let export = first.committed_image().unwrap();
    assert_eq!(export, first.committed_image().unwrap());
    assert_eq!(export, second.committed_image().unwrap());
}

#[test]
fn committed_import_rejects_other_base_and_geometry_without_mutating_receiver() {
    let source = medium(512, 32, 16);
    let export = source.committed_image().unwrap();
    let changed_base = Nand::new(
        source.options().clone(),
        Arc::from([0x76; 512]),
        vec![],
        BTreeMap::new(),
    )
    .unwrap();
    let changed_geometry = medium(2048, 64, 64);
    for receiver in [changed_base, changed_geometry] {
        let before = receiver.committed_image().unwrap();
        assert!(receiver.restore_committed(&export).is_err());
        assert_eq!(receiver.committed_image().unwrap(), before);
    }
}
