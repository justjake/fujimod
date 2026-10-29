use super::*;
use futures_lite::future::block_on;
use relocation::{Destination as D, Kind as K, Relocation};
use std::collections::BTreeMap;

fn image() -> RuntimeImage {
    let mut bytes = vec![0; 256];
    bytes[64..68].copy_from_slice(&0x12345678u32.to_le_bytes());
    bytes[68..72].copy_from_slice(&7u32.to_le_bytes());
    RuntimeImage {
        version: 1,
        bytes,
        link_base: 0x1000,
        alignment: 32,
        capacity: 256,
        hooks: vec![HookWord {
            address: 0x3000,
            old: 1,
            new: 2,
        }],
        relocations: vec![],
        hook_table: None,
        control: ControlLayout {
            control_offset: 0,
            state_offset: 64,
            magic_offset: 0,
            nonce_offset: 4,
            token_offset: 8,
            operation_offset: None,
            inits_done_offset: 12,
            installed_offset: 16,
            magic: 0x12345678,
            nonce: 7,
            core: 3,
            init_count: 2,
            operations: Operations {
                install: 1,
                rollback: 2,
                status: 3,
                init: 4,
            },
            read_alias: 0,
            code_size: 64,
            mailbox: Mailbox {
                offset: 32,
                size: 36,
                nonce: 0,
                operation: 4,
                status: 8,
                result: 12,
                words_new: 16,
                words_old: 20,
                core: 24,
                token: 28,
                inits_done: Some(32),
            },
        },
    }
}
#[derive(Default)]
struct Memory {
    ram: BTreeMap<u64, u8>,
    events: Vec<String>,
    fail: Option<&'static str>,
    image: Option<RuntimeImage>,
    replay: bool,
}
impl Memory {
    fn put(&mut self, a: u64, bytes: &[u8]) {
        for (i, b) in bytes.iter().enumerate() {
            self.ram.insert(a + i as u64, *b);
        }
    }
    fn get(&self, a: u64, n: usize) -> Vec<u8> {
        (0..n)
            .map(|i| *self.ram.get(&(a + i as u64)).unwrap_or(&0))
            .collect()
    }
    fn event(&mut self, name: &'static str) -> Result<()> {
        self.events.push(name.into());
        if self.fail == Some(name) {
            Err(Error::Body(name.into()))
        } else {
            Ok(())
        }
    }
}
impl Body for Memory {
    fn allocate(&mut self, _: u64, _: u64) -> Pending<'_, u64> {
        Box::pin(async {
            self.event("allocate")?;
            Ok(0x2000)
        })
    }
    fn free(&mut self, _: u64) -> Pending<'_, ()> {
        Box::pin(async { self.event("free") })
    }
    fn read(&mut self, r: Range) -> Pending<'_, Vec<u8>> {
        Box::pin(async move {
            self.event("read")?;
            Ok(self.get(r.address, r.size as usize))
        })
    }
    fn write(&mut self, a: u64, b: Vec<u8>) -> Pending<'_, ()> {
        Box::pin(async move {
            self.event("write")?;
            self.put(a, &b);
            Ok(())
        })
    }
    fn synchronize(&mut self, _: Range) -> Pending<'_, ()> {
        Box::pin(async { self.event("synchronize") })
    }
    fn protect(&mut self, _: Range) -> Pending<'_, ()> {
        Box::pin(async { self.event("protect") })
    }
    fn placed(&mut self, i: &RuntimeImage, _: &Lease) -> Pending<'_, ()> {
        self.image = Some(i.clone());
        Box::pin(async { self.event("placed") })
    }
    fn control(&mut self, _: u64, op: u32) -> Pending<'_, ()> {
        Box::pin(async move {
            self.event("control")?;
            let image = self.image.clone().unwrap();
            let c = image.control;
            let state = image.link_base + u64::from(c.state_offset);
            let token = u32::from_le_bytes(
                self.get(state + u64::from(c.token_offset), 4)
                    .try_into()
                    .unwrap(),
            );
            if op == c.operations.install || op == c.operations.rollback {
                self.put(
                    state + u64::from(c.installed_offset),
                    &(if op == c.operations.install { 1u32 } else { 0 }).to_le_bytes(),
                );
                for hook in image.hooks {
                    self.put(
                        hook.address,
                        &if op == c.operations.install {
                            hook.new
                        } else {
                            hook.old
                        }
                        .to_le_bytes(),
                    );
                }
            }
            if op == c.operations.init {
                self.put(state + u64::from(c.inits_done_offset), &1u32.to_le_bytes());
            }
            let values = [
                c.nonce,
                op,
                0,
                if op == c.operations.init {
                    c.init_count
                } else {
                    0
                },
                0,
                0,
                c.core,
                if self.replay { token + 1 } else { token },
                1,
            ];
            for (index, value) in values.iter().enumerate() {
                self.put(
                    state + u64::from(c.mailbox.offset) + index as u64 * 4,
                    &value.to_le_bytes(),
                );
            }
            Ok(())
        })
    }
}
#[test]
fn complete_lifecycle_and_serializable_lease() {
    block_on(async {
        let mut installer = Installer::plan(image()).unwrap();
        let mut body = Memory::default();
        installer
            .install(&mut body, Placement::Relocate, 11)
            .await
            .unwrap();
        assert!(installer.lease.as_ref().unwrap().publication_attempted);
        assert_eq!(body.get(0x3000, 4), 2u32.to_le_bytes());
        let lease: Lease =
            serde_json::from_str(&serde_json::to_string(&installer.lease).unwrap()).unwrap();
        let mut installer = Installer::attach(installer.image, lease).unwrap();
        installer.status(&mut body, 13).await.unwrap();
        installer
            .set(&mut body, 0x20c0, vec![9, 8, 7, 6])
            .await
            .unwrap();
        assert_eq!(
            installer
                .read(
                    &mut body,
                    Range {
                        address: 0x20c0,
                        size: 4
                    }
                )
                .await
                .unwrap(),
            [9, 8, 7, 6]
        );
        installer.rollback(&mut body, 14).await.unwrap();
        assert_eq!(body.get(0x3000, 4), 1u32.to_le_bytes());
        assert!(!body.events.iter().any(|s| s == "free"));
    });
}
#[test]
fn pre_publication_failure_frees_and_publication_failure_retains() {
    for failure in [
        "placed",
        "write",
        "read",
        "synchronize",
        "protect",
        "control",
    ] {
        block_on(async {
            let mut installer = Installer::plan(image()).unwrap();
            let mut body = Memory {
                fail: Some(failure),
                ..Memory::default()
            };
            assert!(
                installer
                    .install(&mut body, Placement::Relocate, 1)
                    .await
                    .is_err()
            );
            assert_eq!(
                body.events.iter().any(|s| s == "free"),
                failure != "control",
                "{failure}"
            );
            assert_eq!(
                installer.lease.unwrap().publication_attempted,
                failure == "control"
            );
        });
    }
}
#[test]
fn relink_and_stale_mailbox() {
    block_on(async {
        let mut installer = Installer::plan(image()).unwrap();
        let mut body = Memory::default();
        let mut relink = |base| {
            let mut i = image();
            i.link_base = base;
            Ok(i)
        };
        installer
            .prepare(&mut body, Placement::Relink(&mut relink))
            .await
            .unwrap();
        body.replay = true;
        assert!(
            installer
                .publish(&mut body, 33)
                .await
                .unwrap_err()
                .to_string()
                .contains("mailbox")
        );
        assert!(installer.lease.as_ref().unwrap().publication_attempted);
        assert!(installer.set(&mut body, 0x3000, vec![0; 4]).await.is_err());
        assert!(!body.events.iter().any(|s| s == "free"));
    });
}
#[test]
fn placement_refuses_duplicate_overlapping_and_unknown_records() {
    let mut i = image();
    i.relocations = vec![
        Relocation {
            destination: D::Image(0),
            kind: K::Aarch64Abs64,
            value: 0x1000,
            moves: true,
            pair: None,
        },
        Relocation {
            destination: D::Image(4),
            kind: K::ArmAbs32,
            value: 0,
            moves: false,
            pair: None,
        },
    ];
    assert!(i.relocate(0x2000).is_err());
    i.relocations.truncate(1);
    i.relocations[0].destination = D::Image(256);
    assert!(i.relocate(0x2000).is_err());
    let metadata = serde_json::to_string(&image()).unwrap().replace(
        "\"relocations\":[]",
        "\"relocations\":[{\"kind\":\"unsupported\"}]",
    );
    assert!(serde_json::from_str::<RuntimeImage>(&metadata).is_err());
}
#[test]
fn arm_and_aarch64_branch_limits_and_opcodes() {
    for (kind, opcode, bits) in [
        (K::ArmCall, 0xeb000000, 26),
        (K::ArmJump24, 0xea000000, 26),
        (K::Aarch64Call26, 0x94000000, 28),
        (K::Aarch64Jump26, 0x14000000, 28),
    ] {
        let place = 1u64 << 30;
        for distance in [-(1i64 << (bits - 1)), (1i64 << (bits - 1)) - 4] {
            assert!(
                relocation::encode(kind, opcode, (place as i64 + distance) as u64, place).is_ok()
            );
        }
        for distance in [-(1i64 << (bits - 1)) - 4, 1i64 << (bits - 1), 2] {
            assert!(
                relocation::encode(kind, opcode, (place as i64 + distance) as u64, place).is_err()
            );
        }
        assert!(relocation::encode(kind, 0, place, place).is_err());
    }
}
#[test]
fn aarch64_pages_low_bits_and_scaled_loads() {
    assert_eq!(
        relocation::encode(K::Aarch64Adrp, 0x90000000, 0x5000, 0x1000).unwrap(),
        0x90000020
    );
    assert!(relocation::encode(K::Aarch64Adrp, 0x90000000, 1 << 32, 0).is_err());
    assert_eq!(
        relocation::encode(K::Aarch64AddLo12, 0x91000000, 0x1234, 0).unwrap(),
        0x9108d000
    );
    for (kind, word, scale) in [
        (K::Aarch64Ldst8Lo12, 0x39000000, 0),
        (K::Aarch64Ldst16Lo12, 0x79000000, 1),
        (K::Aarch64Ldst32Lo12, 0xb9000000, 2),
        (K::Aarch64Ldst64Lo12, 0xf9000000, 3),
        (K::Aarch64Ldst128Lo12, 0x3d800000, 4),
    ] {
        assert_eq!(
            relocation::encode(kind, word, 0x120, 0).unwrap(),
            word | ((0x120 >> scale) << 10)
        );
        if scale > 0 {
            assert!(relocation::encode(kind, word, 0x121, 0).is_err());
        }
    }
}
#[test]
fn mov_pair_carries_borrows_and_preserves_register_and_condition() {
    fn pair(value: u32) -> (u32, u32) {
        let w = |op: u32, half: u32| 0x80002000u32 | op | ((half & 0xf000) << 4) | (half & 0xfff);
        (w(0x03000000, value & 0xffff), w(0x03400000, value >> 16))
    }
    for (pointer, delta) in [(0x39fff0, 0x30), (0x390010, -0x30), (0xfffffff0, 0x30)] {
        let (low, high) = pair(pointer);
        assert_eq!(
            relocation::mov_pair(low, high, delta).unwrap(),
            pair((i128::from(pointer) + delta) as u32)
        );
        assert!(relocation::mov_pair(low, high ^ (1 << 12), delta).is_err());
    }
}

#[test]
fn failure_after_preparation_but_before_install_frees() {
    block_on(async {
        let mut installer = Installer::plan(image()).unwrap();
        let mut body = Memory::default();
        assert!(
            installer
                .install(&mut body, Placement::Relocate, 0)
                .await
                .is_err()
        );
        assert!(installer.lease.as_ref().unwrap().freed);
        assert!(!installer.lease.as_ref().unwrap().publication_attempted);
    });
}

#[test]
fn mov_hook_pair_moves_downward_and_round_trips() {
    let mut i = image();
    i.link_base = 0x4000;
    i.hooks = vec![
        HookWord {
            address: 0x3000,
            old: 0,
            new: 0xe3040010,
        },
        HookWord {
            address: 0x3004,
            old: 0,
            new: 0xe3400000,
        },
    ];
    i.relocations = vec![Relocation {
        destination: D::Hook(0),
        kind: K::ArmMov32,
        value: 0,
        moves: true,
        pair: Some(D::Hook(1)),
    }];
    let placed = i.relocate(0x2000).unwrap();
    assert_eq!(placed.hooks[0].new, 0xe3020010);
    let restored = placed.relocate(0x4000).unwrap();
    assert_eq!(restored.hooks, i.hooks);
}

#[test]
fn lp64_hook_table_and_pointer_relocation_round_trip() {
    let mut i = image();
    i.hooks = vec![
        HookWord {
            address: 0x1_0000_3000,
            old: 1,
            new: 0x1080,
        },
        HookWord {
            address: 0x1_0000_3004,
            old: 2,
            new: 0,
        },
    ];
    i.hook_table = Some(HookTable {
        offset: 160,
        address_bytes: 8,
    });
    i.relocations.push(Relocation {
        destination: D::Hook(0),
        kind: K::Aarch64Abs64,
        value: 0x1080,
        moves: true,
        pair: None,
    });
    let placed = i.relocate(0x1_0000_1000).unwrap();
    assert_eq!(placed.hooks[0].new, 0x1080);
    assert_eq!(placed.hooks[1].new, 1);
    assert_eq!(&placed.bytes[160..168], &0x1_0000_3000u64.to_le_bytes());
    assert_eq!(&placed.bytes[172..176], &0x1080u32.to_le_bytes());
    assert_eq!(&placed.bytes[188..192], &1u32.to_le_bytes());
    assert_eq!(placed.relocate(0x1000).unwrap().hooks, i.hooks);
    i.hooks[1].address += 4;
    assert!(i.validate().is_err());
}

#[test]
fn request_cell_is_written_and_cannot_overlap_ownership() {
    let mut i = image();
    i.control.operation_offset = Some(20);
    let mut memory = Memory::default();
    let mut installer = Installer::plan(i.clone()).unwrap();
    block_on(installer.install(&mut memory, Placement::Relocate, 9)).unwrap();
    let state = installer.image.link_base + 64;
    assert_eq!(memory.get(state + 20, 4), 4u32.to_le_bytes());
    i.control.operation_offset = Some(4);
    assert!(i.validate().is_err());
    i.control.operation_offset = Some(224);
    assert!(i.validate().is_err());
}
