use super::*;
use fujimod_usb::{
    ff80::framing::RequestKind,
    operations::card::Mechanism,
    testing::{Exchange as E, Script},
};
use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

fn envelope(kind: RequestKind, size: u32, id: u16) -> [u8; 12] {
    let mut data = [0; 12];
    data[0] = kind as u8;
    data[6..10].copy_from_slice(&size.to_be_bytes());
    data[10..].copy_from_slice(&id.to_be_bytes());
    data
}
fn exchange(request: Vec<u8>, response: Vec<u8>, maximum: usize) -> Vec<E> {
    vec![
        E::Control(envelope(
            RequestKind::WriteCommand,
            request.len() as u32,
            0x1234,
        )),
        E::Write(request),
        E::Control(envelope(RequestKind::ReadCommand, maximum as u32, 0x1234)),
        E::Read {
            maximum,
            data: response,
        },
    ]
}
fn info(selector: u32, text: &str) -> Vec<E> {
    let mut request = vec![0; 32];
    request[..4].copy_from_slice(&0x150000u32.to_le_bytes());
    request[16..20].copy_from_slice(&selector.to_le_bytes());
    let mut response = request[..16].to_vec();
    response.extend(
        text.encode_utf16()
            .chain(Some(0))
            .flat_map(u16::to_le_bytes),
    );
    exchange(request, response, 128)
}
fn opened(mut commands: Vec<E>, close: bool) -> Script {
    let mut steps = vec![
        E::Control(envelope(RequestKind::Open, 8, 0)),
        E::Read {
            maximum: 8,
            data: vec![0x34, 0x12, 0, 0, 0, 0, 1, 0],
        },
    ];
    steps.append(&mut commands);
    if close {
        steps.extend([
            E::Control(envelope(RequestKind::Close, 2, 0x1234)),
            E::Read {
                maximum: 2,
                data: vec![0, 0],
            },
        ]);
    }
    Script::new(steps)
}
fn memory_script(value: u8, malformed: bool) -> Script {
    let mut steps = info(4, "Model");
    steps.extend(info(8, "1"));
    let request: Vec<u8> = [0x200001u32, 0, 0, 0, 2, 0, 0, 0x1000, 4]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect();
    let mut response = request.clone();
    response.extend([value; 4]);
    if malformed {
        response[0] ^= 1;
    }
    steps.extend(exchange(request, response, 40));
    opened(steps, !malformed)
}
struct TestSessions {
    scripts: VecDeque<Script>,
    opens: Arc<AtomicUsize>,
}
impl Sessions for TestSessions {
    async fn open(&mut self) -> fujimod_usb::Result<Session> {
        self.opens.fetch_add(1, Ordering::SeqCst);
        self.scripts
            .pop_front()
            .expect("no unplanned acquisition")
            .begin_session(65536, None)
            .await
    }
}
fn client(scripts: Vec<Script>) -> Client<TestSessions> {
    Client {
        sessions: TestSessions {
            scripts: scripts.into(),
            opens: Arc::new(AtomicUsize::new(0)),
        },
        product: "Model".into(),
        internal_version: "1".into(),
        capability: Some(FirmwareCard {
            schema_uri: "https://fujimod.com/schemas/card-files.generated.schema.json".into(),
            schema: 1,
            firmware_id: "model-1".into(),
            product: "Model".into(),
            internal_version: "1".into(),
            code_checks: vec![],
            service: None,
            mechanism: Mechanism::Mailbox {
                search_address: 0x1000,
                search_bytes: 64,
                address_alias: 0,
                build: None,
            },
        }),
        bulk: None,
        flash: None,
        service: None,
        limits: DebugMemoryLimits {
            read: NonZeroU32::new(256).unwrap(),
            write: NonZeroU32::new(256).unwrap(),
        },
        mailbox_hint: None,
        fenced: false,
        lease: None,
    }
}
#[test]
fn bounded_reads_release_ff80_before_reply_and_sync_or_idle_never_acquires() {
    block_on(async {
        let first = memory_script(1, false);
        let second = memory_script(2, false);
        let mut client = client(vec![first.clone(), second.clone()]);
        for (value, script) in [(1, first), (2, second)] {
            let Answer::Bytes(data) = client
                .transaction(Operation::Memory(0x1000, 4))
                .await
                .unwrap()
            else {
                panic!()
            };
            assert_eq!(data, vec![value; 4]);
            // The strict script requires C4 before the result is returned.
            assert_eq!(script.remaining(), 0);
            client.transaction(Operation::Sync).await.unwrap();
        }
        client.transaction(Operation::Close).await.unwrap();
        assert_eq!(client.sessions.opens.load(Ordering::SeqCst), 2);
    });
}
#[test]
fn completed_identity_refusal_closes_and_yields_before_another_client_transaction() {
    block_on(async {
        let refusal = opened(info(4, "Other"), true);
        let next = memory_script(3, false);
        let mut client = client(vec![refusal.clone(), next.clone()]);
        assert!(
            client
                .transaction(Operation::Memory(0x1000, 4))
                .await
                .is_err()
        );
        assert_eq!(refusal.remaining(), 0);
        assert!(!client.fenced);
        assert!(
            client
                .transaction(Operation::Memory(0x1000, 4))
                .await
                .is_ok()
        );
        assert_eq!(next.remaining(), 0);
    });
}
#[test]
fn uncertain_completion_never_closes_reacquires_or_reclaims() {
    block_on(async {
        let broken = memory_script(1, true);
        let mut client = client(vec![broken.clone()]);
        assert!(
            client
                .transaction(Operation::Memory(0x1000, 4))
                .await
                .is_err()
        );
        assert!(client.fenced);
        assert_eq!(broken.remaining(), 0);
        assert!(matches!(
            client.transaction(Operation::Memory(0x1000, 4)).await,
            Err(fujimod_usb::Error::CompletionUnknown)
        ));
        assert_eq!(client.sessions.opens.load(Ordering::SeqCst), 1);
    });
}

#[derive(Default)]
struct MailboxState {
    memory: std::collections::BTreeMap<u32, u8>,
    reply: Vec<u8>,
    live: bool,
    opens: usize,
    closes: usize,
    searches: usize,
}
impl MailboxState {
    fn put(&mut self, at: u32, value: u32) {
        for (n, b) in value.to_le_bytes().into_iter().enumerate() {
            self.memory.insert(at + n as u32, b);
        }
    }
    fn word(&self, at: u32) -> u32 {
        u32::from_le_bytes(std::array::from_fn(|n| {
            *self.memory.get(&(at + n as u32)).unwrap_or(&0)
        }))
    }
}
struct MailboxPeer(Arc<std::sync::Mutex<MailboxState>>);
impl fujimod_usb::testing::Peer for MailboxPeer {
    fn control(&mut self, kind: RequestKind, _: [u8; 12]) -> fujimod_usb::Result<()> {
        let mut state = self.0.lock().unwrap();
        match kind {
            RequestKind::Open => {
                assert!(!state.live);
                state.live = true;
                state.opens += 1;
                state.reply = vec![0x34, 0x12, 0, 0, 0, 0, 1, 0];
            }
            RequestKind::Close => {
                assert!(state.live);
                state.live = false;
                state.closes += 1;
                state.reply = vec![0, 0];
            }
            _ => assert!(state.live),
        }
        Ok(())
    }
    fn write(&mut self, raw: &[u8]) -> fujimod_usb::Result<()> {
        let mut state = self.0.lock().unwrap();
        let word = |n| u32::from_le_bytes(raw[n..n + 4].try_into().unwrap());
        match word(0) {
            0x150000 => {
                let text = if word(16) == 4 { "Model" } else { "1" };
                state.reply = raw[..16].to_vec();
                state.reply.extend(
                    text.encode_utf16()
                        .chain(Some(0))
                        .flat_map(u16::to_le_bytes),
                );
            }
            0x200001 => {
                let at = word(28);
                let size = word(32);
                state.reply = raw[..36].to_vec();
                match word(16) {
                    2 => {
                        // The last chunk of the complete 0x400-byte search;
                        // compact descriptor/status snapshots stay below it.
                        if at == 0x1300 && size == 256 {
                            state.searches += 1;
                        }
                        let bytes: Vec<_> = (0..size)
                            .map(|n| *state.memory.get(&(at + n)).unwrap_or(&0))
                            .collect();
                        state.reply.extend(bytes);
                    }
                    3 => {
                        for (n, b) in raw[36..].iter().enumerate() {
                            state.memory.insert(at + n as u32, *b);
                        }
                        if at == 0x1104 {
                            assert_eq!(state.word(at), 10, "only a directory transaction");
                            let ops = state.word(0x1114);
                            state.put(0x1114, ops + 1);
                            state.put(0x1104, 0);
                            state.put(0x1108, 2);
                            state.put(0x110c, 0);
                            state.put(0x1110, 0);
                        }
                    }
                    _ => panic!("unexpected memory operation"),
                }
            }
            _ => panic!("unplanned ff80 command"),
        }
        Ok(())
    }
    fn read(&mut self, maximum: usize) -> fujimod_usb::Result<Vec<u8>> {
        let state = self.0.lock().unwrap();
        assert!(state.reply.len() <= maximum);
        Ok(state.reply.clone())
    }
}
struct MailboxSessions(Arc<std::sync::Mutex<MailboxState>>);
impl Sessions for MailboxSessions {
    async fn open(&mut self) -> fujimod_usb::Result<Session> {
        fujimod_usb::testing::peer_session(MailboxPeer(self.0.clone()), 65536, None).await
    }
}
#[test]
fn large_read_bursts_reuse_a_session_but_idle_sync_and_deadlines_release_it() {
    block_on(async {
        let state = Arc::new(std::sync::Mutex::new(MailboxState::default()));
        let mut client = Client {
            sessions: MailboxSessions(state.clone()),
            capability: None,
            product: "Model".into(),
            internal_version: "1".into(),
            bulk: None,
            flash: None,
            service: None,
            limits: DebugMemoryLimits {
                read: NonZeroU32::new(256).unwrap(),
                write: NonZeroU32::new(256).unwrap(),
            },
            mailbox_hint: None,
            fenced: false,
            lease: None,
        };
        for _ in 0..2 {
            assert!(matches!(
                client
                    .transaction(Operation::Memory(0x2000, 65536))
                    .await
                    .unwrap(),
                Answer::Bytes(_)
            ));
        }
        {
            let s = state.lock().unwrap();
            assert_eq!((s.opens, s.closes), (1, 0));
        }
        client.lease.as_mut().unwrap().started = std::time::Instant::now() - Duration::from_secs(3);
        client
            .transaction(Operation::Memory(0x2000, 65536))
            .await
            .unwrap();
        {
            let s = state.lock().unwrap();
            assert_eq!((s.opens, s.closes), (2, 1));
        }
        client.transaction(Operation::Sync).await.unwrap();
        {
            let s = state.lock().unwrap();
            assert!(!s.live);
            assert_eq!((s.opens, s.closes), (2, 2));
        }
        let outside = fujimod_usb::testing::peer_session(MailboxPeer(state.clone()), 65536, None)
            .await
            .unwrap();
        outside.close().await.unwrap();
        client
            .transaction(Operation::Memory(0x2000, 65536))
            .await
            .unwrap();
        client.lease.as_mut().unwrap().bytes = (64 << 20) - 65536;
        client
            .transaction(Operation::Memory(0x2000, 65536))
            .await
            .unwrap();
        assert!(client.lease.is_none());
        let s = state.lock().unwrap();
        assert_eq!((s.opens, s.closes), (4, 4));
        assert!(!s.live);
    });
}
#[test]
fn sd_transactions_yield_to_another_ff80_client_and_revalidate_the_descriptor_hint() {
    block_on(async {
        let mut state = MailboxState::default();
        let words = [
            0x314c4946, 3, 0, 0x1100, 0x1104, 0x1108, 0x110c, 0x1110, 0x1180, 0x1200, 0x1114,
            0x1118, 0x111c, 0x1120, 0, 0,
        ];
        for (n, value) in words.into_iter().enumerate() {
            state.put(0x1000 + n as u32 * 4, value);
        }
        state.put(0x1100, 64);
        state.put(0x111c, u32::MAX);
        let state = Arc::new(std::sync::Mutex::new(state));
        let mut profile = client(vec![]).capability;
        profile.as_mut().unwrap().mechanism = Mechanism::Mailbox {
            search_address: 0x1000,
            search_bytes: 0x400,
            address_alias: 0,
            build: None,
        };
        let mut client = Client {
            sessions: MailboxSessions(state.clone()),
            capability: profile,
            product: "Model".into(),
            internal_version: "1".into(),
            bulk: None,
            flash: None,
            service: None,
            limits: DebugMemoryLimits {
                read: NonZeroU32::new(256).unwrap(),
                write: NonZeroU32::new(256).unwrap(),
            },
            mailbox_hint: None,
            fenced: false,
            lease: None,
        };
        for _ in 0..2 {
            assert!(matches!(
                client
                    .transaction(Operation::Card(Request::ReadDirectory {
                        path: "A:\\".into()
                    }))
                    .await
                    .unwrap(),
                Answer::Card(Response::Directory(_))
            ));
            assert!(
                !state.lock().unwrap().live,
                "C4 completed before the filesystem received its result"
            );
            let outside =
                fujimod_usb::testing::peer_session(MailboxPeer(state.clone()), 65536, None)
                    .await
                    .unwrap();
            outside.close().await.unwrap();
        }
        let state = state.lock().unwrap();
        assert_eq!((state.opens, state.closes), (4, 4));
        assert_eq!(
            state.searches, 1,
            "the next transaction verifies the hint instead of rescanning"
        );
        assert_eq!(client.mailbox_hint, Some(0x1000));
        assert_eq!(state.word(0x111c), u32::MAX);
    });
}

#[test]
fn memory_access_does_not_require_a_card_service() {
    block_on(async {
        let script = memory_script(7, false);
        let mut client = client(vec![script.clone()]);
        client.capability = None;
        let Answer::Bytes(bytes) = client
            .transaction(Operation::Memory(0x1000, 4))
            .await
            .unwrap()
        else {
            panic!("memory bytes")
        };
        assert_eq!(bytes, [7; 4]);
        assert_eq!(script.remaining(), 0);
    });
}
