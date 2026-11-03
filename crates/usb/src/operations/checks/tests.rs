use super::*;
use crate::{
    ff80::framing::RequestKind,
    operations::card::BufferIo,
    testing::{Peer, peer_session},
};
use futures_lite::future::block_on;
use std::sync::{Arc, Mutex};
fn bytes(address: u32, size: u32) -> Vec<u8> {
    (0..size)
        .map(|n| ((address + n) ^ ((address + n) >> 8)) as u8)
        .collect()
}
#[derive(Default)]
struct State {
    reply: Vec<u8>,
    reads: Vec<(u32, u32)>,
}
struct Device(Arc<Mutex<State>>);
impl Peer for Device {
    fn control(&mut self, kind: RequestKind, _: [u8; 12]) -> Result<()> {
        let mut state = self.0.lock().unwrap();
        match kind {
            RequestKind::Open => state.reply = vec![0x34, 0x12, 0, 0, 0, 0, 1, 0],
            RequestKind::Close => state.reply = vec![0, 0],
            _ => {}
        }
        Ok(())
    }
    fn write(&mut self, raw: &[u8]) -> Result<()> {
        let w = |at| u32::from_le_bytes(raw[at..at + 4].try_into().unwrap());
        assert_eq!(w(0), 0x200001);
        assert_eq!(w(16), 2);
        let mut state = self.0.lock().unwrap();
        state.reads.push((w(28), w(32)));
        state.reply = raw[..36].to_vec();
        state.reply.extend(bytes(w(28), w(32)));
        Ok(())
    }
    fn read(&mut self, maximum: usize) -> Result<Vec<u8>> {
        let state = self.0.lock().unwrap();
        assert!(state.reply.len() <= maximum);
        Ok(state.reply.clone())
    }
}
struct Io {
    reads: Vec<(u32, u32)>,
    end: u64,
    corrupt: Option<u32>,
    short: bool,
}
impl BufferIo for Io {
    fn admits(&self, at: u32, n: u32) -> bool {
        at >= 0x1000 && u64::from(at) + u64::from(n) <= self.end && n >= 1024
    }
    fn read<'a>(
        &'a mut self,
        _: &'a mut Session,
        at: u32,
        n: u32,
    ) -> crate::emulator::EndpointFuture<'a, Result<Vec<u8>>> {
        Box::pin(async move {
            self.reads.push((at, n));
            let mut raw = bytes(at, n);
            if let Some(address) = self
                .corrupt
                .filter(|a| *a >= at && u64::from(*a) < u64::from(at) + u64::from(n))
            {
                raw[(address - at) as usize] ^= 1;
            }
            if self.short {
                raw.pop();
            }
            Ok(raw)
        })
    }
}
fn check(at: u32, n: u32) -> Sha256Check {
    Sha256Check {
        range: MemoryRange::new(Address(at), NonZeroU32::new(n).unwrap()).unwrap(),
        expected: sha256_bytes(&bytes(at, n)),
    }
}
fn limits() -> DebugMemoryLimits {
    DebugMemoryLimits {
        read: NonZeroU32::new(256).unwrap(),
        write: NonZeroU32::new(256).unwrap(),
    }
}
#[test]
fn coalesced_bulk_checks_still_verify_every_original_digest() {
    block_on(async {
        for corrupt in [None, Some(0x3003)] {
            let state = Arc::new(Mutex::new(State::default()));
            let mut session = peer_session(Device(state.clone()), 1 << 20, None)
                .await
                .unwrap();
            let mut io = Io {
                reads: vec![],
                end: 0x10000,
                corrupt,
                short: false,
            };
            let result = verify_sha256(
                &mut session,
                limits(),
                &[check(0x1000, 512), check(0x3000, 512)],
                Some(&mut io),
            )
            .await;
            assert_eq!(result.is_ok(), corrupt.is_none());
            assert_eq!(io.reads, [(0x1000, 0x2200)]);
            assert!(state.lock().unwrap().reads.is_empty());
            session.close().await.unwrap();
        }
    });
}
#[test]
fn cheap_or_unadmitted_groups_use_only_the_original_cpu_spans() {
    block_on(async {
        for (size, end) in [(16, 0x10000), (512, 0x2000)] {
            let state = Arc::new(Mutex::new(State::default()));
            let mut session = peer_session(Device(state.clone()), 1 << 20, None)
                .await
                .unwrap();
            let mut io = Io {
                reads: vec![],
                end,
                corrupt: None,
                short: false,
            };
            let before = session.round_trips();
            verify_sha256(
                &mut session,
                limits(),
                &[check(0x1000, size), check(0x3000, size)],
                Some(&mut io),
            )
            .await
            .unwrap();
            assert_eq!(
                session.round_trips() - before,
                1,
                "all bounded disjoint CPU reads share one host wait"
            );
            assert!(io.reads.is_empty());
            assert!(
                state
                    .lock()
                    .unwrap()
                    .reads
                    .iter()
                    .all(|&(at, n)| (at >= 0x1000 && at + n <= 0x1000 + size)
                        || (at >= 0x3000 && at + n <= 0x3000 + size))
            );
            session.close().await.unwrap();
        }
    });
}
#[test]
fn short_bulk_results_are_refused_without_a_cpu_retry() {
    block_on(async {
        let state = Arc::new(Mutex::new(State::default()));
        let mut session = peer_session(Device(state.clone()), 1 << 20, None)
            .await
            .unwrap();
        let mut io = Io {
            reads: vec![],
            end: 0x10000,
            corrupt: None,
            short: true,
        };
        assert!(matches!(
            verify_sha256(
                &mut session,
                limits(),
                &[check(0x1000, 2048)],
                Some(&mut io)
            )
            .await,
            Err(Error::InvalidLength)
        ));
        assert!(state.lock().unwrap().reads.is_empty());
        session.close().await.unwrap();
    });
}
#[test]
fn malformed_or_overflowing_checks_fail_before_io() {
    for (at, n, digest) in [
        (0, 0, "0".repeat(64)),
        (u32::MAX, 2, "0".repeat(64)),
        (0, 1, "z".repeat(64)),
        (0, 1, "a".repeat(63)),
    ] {
        assert!(Sha256Check::from_hex(at, n, &digest).is_err());
    }
}
