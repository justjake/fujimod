use super::*;
use fujimod_catalog::ReferenceCatalog;
use fujimod_usb::{
    ff80::framing::RequestKind,
    testing::{Peer, peer_session},
};
use futures_lite::future::block_on;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

#[derive(Default)]
struct State {
    memory: BTreeMap<u32, u8>,
    reply: Vec<u8>,
    live: bool,
    opens: usize,
    closes: usize,
    allocations: usize,
    frees: usize,
    refuse_init: bool,
    maximum_allocation: Option<u32>,
}
impl State {
    fn put(&mut self, at: u32, raw: &[u8]) {
        for (i, b) in raw.iter().enumerate() {
            self.memory.insert((at & !ALIAS) + i as u32, *b);
        }
    }
    fn bytes(&self, at: u32, n: u32) -> Vec<u8> {
        (0..n)
            .map(|i| *self.memory.get(&((at & !ALIAS) + i)).unwrap_or(&0))
            .collect()
    }
    fn word(&self, at: u32) -> u32 {
        word(&self.bytes(at, 4))
    }
    fn number(&mut self, at: u32, n: u32) {
        self.put(at, &n.to_le_bytes());
    }
}
struct Device(Arc<Mutex<State>>);
impl Peer for Device {
    fn control(&mut self, kind: RequestKind, _: [u8; 12]) -> Result<()> {
        let mut s = self.0.lock().unwrap();
        match kind {
            RequestKind::Open => {
                assert!(!s.live);
                s.live = true;
                s.opens += 1;
                s.reply = vec![0x34, 0x12, 0, 0, 0, 0, 1, 0];
            }
            RequestKind::Close => {
                assert!(s.live);
                s.live = false;
                s.closes += 1;
                s.reply = vec![0, 0];
            }
            _ => assert!(s.live),
        }
        Ok(())
    }
    fn write(&mut self, raw: &[u8]) -> Result<()> {
        let mut s = self.0.lock().unwrap();
        let w = |n| word(&raw[n..]);
        match w(0) {
            0x200001 => {
                let at = w(28);
                let n = w(32);
                s.reply = raw[..36].to_vec();
                match w(16) {
                    2 => {
                        let bytes = s.bytes(at, n);
                        s.reply.extend(bytes);
                    }
                    3 => s.put(at, &raw[36..]),
                    _ => panic!("unplanned memory operation"),
                }
            }
            0x450006 => {
                let entry = s.word(0x300c);
                s.reply = raw.to_vec();
                match entry {
                    0x6080 => match s.word(0x4004) {
                        0x8000 => {
                            s.allocations += 1;
                            let address = if s.maximum_allocation.is_some_and(|max| w(16) > max) {
                                0
                            } else {
                                0x100000
                            };
                            s.number(0x4000, address);
                        }
                        0x8004 => {
                            assert_eq!(w(16), 0x100000);
                            s.frees += 1;
                            s.number(0x4000, 0);
                        }
                        _ => panic!("unplanned allocator callback"),
                    },
                    0x100000 => {
                        assert_eq!(w(16), 0x40102000);
                        assert_eq!(s.word(w(16) + 68), 13);
                        if s.refuse_init {
                            s.reply[4..8].copy_from_slice(&1u32.to_le_bytes());
                            s.reply[8..12].copy_from_slice(&0xffffu32.to_le_bytes());
                        } else {
                            s.number(w(16) + 68, 0);
                            s.number(w(16) + 72, 2);
                            s.number(w(16) + 76, 0);
                            s.number(w(16) + 148, 1);
                            s.number(0x100000 + IMAGE.len() as u32 - 4, w(16));
                        }
                    }
                    0x8008 | 0x800c | 0x8010 | 0x8014 => {}
                    _ => panic!("unplanned execution {entry:#x}"),
                }
            }
            _ => panic!("unplanned command {:#x}", w(0)),
        }
        Ok(())
    }
    fn read(&mut self, maximum: usize) -> Result<Vec<u8>> {
        let s = self.0.lock().unwrap();
        assert!(s.reply.len() <= maximum);
        Ok(s.reply.clone())
    }
}
fn fixture() -> (
    Arc<Mutex<State>>,
    QueryService,
    FirmwareBulk,
    DebugMemoryLimits,
) {
    let mut state = State::default();
    state.number(0x2000, 0x3000);
    state.number(0x300c, 0x9000);
    state.put(0x4000, &words(&[0x4e7283c0, 1 << 20]));
    state.number(0x5000, 0x40006000);
    state.put(0x6080, &[0xa5; 32]);
    state.number(0x7000, 0x77);
    state.number(0x7004, 1);
    let facts = QueryService {
        allocate: 0x8000,
        release: 0x8004,
        configuration_pointer: 0x5000,
        cache_clean: 0x8008,
        instruction_invalidate: 0x800c,
        barrier: 0x8010,
        instruction_barrier: 0x8014,
        worker_slots: 0x7000,
        functions: vec![0x8030; 20],
        checks: vec![fujimod_catalog_bulk::CodeCheck {
            name: "execution".into(),
            address: 0x8000,
            size: 128,
            sha256: format!("{:x}", sha2::Sha256::digest([0; 128])),
        }],
    };
    let mut bulk = ReferenceCatalog::bundled()
        .unwrap()
        .resolve_firmware("x70-110")
        .unwrap()
        .bulk
        .unwrap();
    bulk.source = BulkSource::Query {
        tag: 128,
        callback_table: 0x2000,
        callback_offset: 12,
        stock_callback: 0x9000,
        base: 0x4e7283c0,
        source_state: 0x4000,
        capacities: vec![1 << 20],
        wrapping: true,
    };
    (
        Arc::new(Mutex::new(state)),
        facts,
        bulk,
        DebugMemoryLimits {
            read: NonZeroU32::new(256).unwrap(),
            write: NonZeroU32::new(256).unwrap(),
        },
    )
}
fn restored(s: &State) {
    assert_eq!(s.word(0x300c), 0x9000);
    assert_eq!(s.bytes(0x4000, 8), words(&[0x4e7283c0, 1 << 20]));
    assert_eq!(s.bytes(0x6080, 32), [0xa5; 32]);
}
#[test]
fn setup_and_release_use_separate_sessions_and_restore_borrowed_words() {
    block_on(async {
        let (state, facts, bulk, limits) = fixture();
        let mut session = peer_session(Device(state.clone()), 1 << 20, None)
            .await
            .unwrap();
        let service = Service::install(&mut session, limits, &bulk, &facts)
            .await
            .unwrap();
        session.close().await.unwrap();
        {
            let s = state.lock().unwrap();
            restored(&s);
            assert!(!s.live);
            assert_eq!((s.allocations, s.frees), (1, 0));
        }
        let mut session = peer_session(Device(state.clone()), 1 << 20, None)
            .await
            .unwrap();
        service.check(&mut session, limits, None).await.unwrap();
        service.release(&mut session, limits).await.unwrap();
        session.close().await.unwrap();
        let s = state.lock().unwrap();
        restored(&s);
        assert_eq!((s.opens, s.closes, s.allocations, s.frees), (2, 2, 1, 1));
    });
}
#[test]
fn completed_setup_refusal_frees_its_allocation_and_restores_scratch() {
    block_on(async {
        let (state, facts, bulk, limits) = fixture();
        state.lock().unwrap().refuse_init = true;
        let mut session = peer_session(Device(state.clone()), 1 << 20, None)
            .await
            .unwrap();
        assert!(
            Service::install(&mut session, limits, &bulk, &facts)
                .await
                .is_err()
        );
        assert_eq!(session.state(), SessionState::Ready);
        session.close().await.unwrap();
        let s = state.lock().unwrap();
        restored(&s);
        assert_eq!((s.allocations, s.frees), (1, 1));
    });
}

#[test]
fn confirmed_null_allocations_reduce_capacity_and_bound_the_owned_reservation() {
    block_on(async {
        let (state, facts, bulk, limits) = fixture();
        state.lock().unwrap().maximum_allocation = Some(BUFFER + (1 << 20) + 32);
        let mut session = peer_session(Device(state.clone()), 1 << 20, None)
            .await
            .unwrap();
        let service = Service::install(&mut session, limits, &bulk, &facts)
            .await
            .unwrap();
        assert_eq!(service.bytes, BUFFER + (1 << 20) + 32);
        assert_eq!(
            state.lock().unwrap().word(service.base + SHARED + 64),
            1 << 20
        );
        assert_eq!(state.lock().unwrap().allocations, 4);
        service.release(&mut session, limits).await.unwrap();
        assert_eq!(state.lock().unwrap().frees, 1);
        restored(&state.lock().unwrap());
        session.close().await.unwrap();
    });
}
#[test]
fn changed_code_or_outstanding_storage_never_frees_the_helper() {
    block_on(async {
        for change_code in [false, true] {
            let (state, facts, bulk, limits) = fixture();
            let mut session = peer_session(Device(state.clone()), 1 << 20, None)
                .await
                .unwrap();
            let service = Service::install(&mut session, limits, &bulk, &facts)
                .await
                .unwrap();
            if change_code {
                state.lock().unwrap().put(service.base, &[0xff]);
            } else {
                state.lock().unwrap().number(service.base + SHARED + 72, 1);
            }
            assert!(service.release(&mut session, limits).await.is_err());
            assert_eq!(state.lock().unwrap().frees, 0);
            if change_code {
                session.close().await.unwrap();
            } else {
                assert_ne!(session.state(), SessionState::Ready);
            }
        }
    });
}
