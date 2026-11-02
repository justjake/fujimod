use super::*;
use crate::{
    ff80::{SessionState, framing::RequestKind},
    testing::{Peer, peer_session},
};
use fujimod_catalog_card::StockSdk;
use futures_lite::future::block_on;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

const TABLE: u32 = 0x2000;
const SLOT: u32 = TABLE + 12;
const REQUEST: u32 = 0x3000;
const OUTPUT: u32 = 0x4000;
const STATUS: u32 = 0x5000;
const VALUE: u32 = 0x5100;
const ERROR: u32 = 0x5200;
const BASE: u32 = 0x10004;

#[derive(Default)]
struct Camera {
    memory: BTreeMap<u32, u8>,
    files: BTreeMap<String, Vec<u8>>,
    reply: Vec<u8>,
    opened: Option<(String, usize)>,
    allocated: bool,
    scan: Vec<(String, Vec<u8>, u8)>,
    allocations: usize,
    failure: Option<&'static str>,
    mutations: usize,
}
impl Camera {
    fn read(&self, a: u32, n: usize) -> Vec<u8> {
        (0..n)
            .map(|i| *self.memory.get(&(a + i as u32)).unwrap_or(&0))
            .collect()
    }
    fn write(&mut self, a: u32, b: &[u8]) {
        for (i, &v) in b.iter().enumerate() {
            self.memory.insert(a + i as u32, v);
        }
    }
    fn word(&self, a: u32) -> u32 {
        u32_at(&self.read(a, 4), 0)
    }
    fn put(&mut self, a: u32, v: u32) {
        self.write(a, &v.to_le_bytes())
    }
    fn text(&self, a: u32) -> String {
        String::from_utf8(
            (0..48)
                .map(|i| self.read(a + i, 1)[0])
                .take_while(|&b| b != 0)
                .collect(),
        )
        .unwrap()
    }
    fn done(&mut self, status: u32, value: u32) {
        self.put(STATUS, status);
        self.put(VALUE, value);
    }
    fn mailbox(&mut self) {
        let request = self.word(0x40104);
        let op = request & 255;
        let size = request >> 8;
        let path = self.text(0x40200);
        let mut count = 0;
        match op {
            1 | 5 => {
                assert!(self.files.contains_key(&path));
                self.opened = Some((path, 0));
                self.put(0x4011c, 16)
            }
            2 => {
                let (name, offset) = self.opened.clone().unwrap();
                let data = &self.files[&name];
                let n = (size as usize).min(data.len().saturating_sub(offset));
                let raw = data[offset..offset + n].to_vec();
                self.write(0x41000, &raw);
                self.opened = Some((name, offset + n));
                count = n as u32;
            }
            3 | 4 => {
                self.opened = None;
                self.put(0x4011c, u32::MAX)
            }
            6 => {
                assert_eq!(self.word(0x40120), 0);
                let (name, offset) = self.opened.clone().unwrap();
                let raw = self.read(0x41000, size as usize);
                let data = self.files.get_mut(&name).unwrap();
                data[offset..offset + size as usize].copy_from_slice(&raw);
                self.opened = Some((name, offset + size as usize));
                count = size;
            }
            7 => {
                assert!(self.opened.is_none());
                self.files.remove(&path);
            }
            8 => {
                let entries: Vec<_> = self
                    .files
                    .iter()
                    .filter(|(name, _)| path == "A:\\*.*" || **name == path)
                    .skip(size as usize)
                    .take(2)
                    .map(|(name, data)| (name.clone(), data.len()))
                    .collect();
                let mut raw = vec![];
                for (name, n) in entries {
                    let mut row = vec![0; 24];
                    row[..4].copy_from_slice(&(n as u32).to_le_bytes());
                    let name = name.rsplit('\\').next().unwrap().as_bytes();
                    row[4..4 + name.len()].copy_from_slice(name);
                    raw.extend(row)
                }
                count = raw.len() as u32;
                self.write(0x41000, &raw);
            }
            _ => panic!("Unexpected mailbox operation {op}"),
        }
        self.put(0x40104, 0);
        self.put(0x40108, 2);
        self.put(0x4010c, 0);
        self.put(0x40110, count);
        self.put(0x40114, self.word(0x40114).wrapping_add(1));
    }
    fn query(&mut self) -> Result<()> {
        assert_eq!(self.word(SLOT), 0x9000);
        if self.failure == Some("lost") {
            return Err(fail("lost during callback"));
        }
        let target = self.word(REQUEST);
        let a: Vec<_> = (1..7).map(|i| self.word(REQUEST + i * 4)).collect();
        match target {
            1 => {
                assert_eq!(a[0], 9);
                assert_eq!(a[2], OUTPUT);
                self.allocated = true;
                self.allocations += 1;
                self.put(OUTPUT, BASE);
            }
            2 => {
                assert_eq!(&a[..2], &[9, BASE]);
                assert!(self.opened.is_none());
                self.allocated = false;
            }
            3 => (),
            4 => {
                assert_eq!(a[0], 0x9100);
                let name = self.text(a[1]);
                if a[2] == 1 {
                    self.files.insert(name.clone(), Vec::new());
                }
                if self.files.contains_key(&name) {
                    self.opened = Some((name, 0));
                    self.done(0, 16);
                } else {
                    self.done(6, 0);
                }
            }
            5 => {
                assert_eq!(a[1], 16);
                let (name, offset) = self.opened.clone().unwrap();
                let data = &self.files[&name];
                let n = (a[2] as usize).min(data.len().saturating_sub(offset));
                let raw = data[offset..offset + n].to_vec();
                self.write(a[3], &raw);
                self.opened = Some((name, offset + n));
                self.done(0, n as u32);
            }
            6 => {
                assert_eq!(a[1], 16);
                let (name, offset) = self.opened.clone().unwrap();
                let mut n = a[2];
                if self.failure == Some("short") {
                    n = n.saturating_sub(1)
                }
                let raw = self.read(a[3], n as usize);
                let data = self.files.get_mut(&name).unwrap();
                data.resize(data.len().max(offset + n as usize), 0);
                data[offset..offset + n as usize].copy_from_slice(&raw);
                self.opened = Some((name, offset + n as usize));
                self.done(0, n);
            }
            7 => {
                assert_eq!(a[1], 16);
                if a[2] != 0 {
                    let (name, _) = self.opened.as_ref().unwrap();
                    self.files.get_mut(name).unwrap().truncate(a[2] as usize);
                }
                self.opened = None;
                self.done(0, 0);
            }
            8 => {
                let name = self.text(a[1]);
                self.files.remove(&name);
                self.done(0, 0);
            }
            9 => {
                let packet = a[0];
                let code = self.word(packet) & 0xffff;
                let payload = self.word(packet + 8);
                let details = self.word(payload + 4);
                if code == 100 {
                    let pattern = self.text(payload + 40);
                    self.scan = self
                        .files
                        .iter()
                        .filter(|(n, _)| {
                            pattern == "*.*" || n.rsplit('\\').next().unwrap() == pattern
                        })
                        .map(|(n, d)| (n.clone(), d.clone(), 0x20))
                        .collect();
                    self.scan.reverse();
                } else if code == 102 {
                    self.scan = vec![("A:\\DCIM".into(), Vec::new(), 0x10)];
                } else {
                    assert!(code == 101 || code == 103)
                }
                if let Some((name, data, attributes)) = self.scan.pop() {
                    let mut raw = vec![0; 120];
                    let n = name.rsplit('\\').next().unwrap().as_bytes();
                    raw[..n.len()].copy_from_slice(n);
                    raw[59] = attributes;
                    raw[80..84].copy_from_slice(&(data.len() as u32).to_le_bytes());
                    self.write(details, &raw);
                    self.done(0, 0);
                } else {
                    self.done(6, 0)
                }
            }
            10 => {
                assert_eq!(&a[..2], &[0x9100, 16]);
                let (name, _) = self.opened.clone().unwrap();
                self.opened = Some((name, a[2] as usize));
                self.done(0, 0);
            }
            _ => panic!("Unexpected stock call {target}"),
        }
        self.put(REQUEST, 0);
        Ok(())
    }
}
struct Endpoint(Arc<Mutex<Camera>>);
impl Peer for Endpoint {
    fn control(&mut self, kind: RequestKind, envelope: [u8; 12]) -> Result<()> {
        if kind == RequestKind::Open {
            self.0.lock().unwrap().reply = vec![42, 0, 0, 0, 0, 0, 1, 0];
        }
        if kind == RequestKind::ReadData {
            let mut c = self.0.lock().unwrap();
            let offset = u32::from_be_bytes(envelope[2..6].try_into().unwrap());
            let n = u32::from_be_bytes(envelope[6..10].try_into().unwrap());
            assert!(offset + n <= c.word(OUTPUT + 4));
            let address = c.word(OUTPUT) + offset;
            c.reply = c.read(address, n as usize);
        }
        Ok(())
    }
    fn write(&mut self, raw: &[u8]) -> Result<()> {
        let mut c = self.0.lock().unwrap();
        let command = u32_at(raw, 0);
        match command {
            0x150000 => {
                let text = if u32_at(raw, 16) == 4 { "Model" } else { "1" };
                c.reply = raw[..16].to_vec();
                c.reply.extend(
                    text.encode_utf16()
                        .chain(Some(0))
                        .flat_map(u16::to_le_bytes),
                );
            }
            0x200001 => {
                let function = u32_at(raw, 16);
                let a = u32_at(raw, 28);
                let n = u32_at(raw, 32) as usize;
                c.reply = raw[..36].to_vec();
                match function {
                    2 => {
                        let data = c.read(a, n);
                        c.reply.extend(data);
                    }
                    3 => {
                        c.mutations += 1;
                        c.write(a, &raw[36..36 + n]);
                        if a == 0x40104 {
                            c.mailbox();
                        }
                    }
                    _ => panic!("Unexpected function {function}"),
                }
            }
            0x450006 => {
                c.query()?;
                c.reply = raw.to_vec();
            }
            _ => panic!("Unexpected command {command:#x}"),
        }
        Ok(())
    }
    fn read(&mut self, maximum: usize) -> Result<Vec<u8>> {
        let c = self.0.lock().unwrap();
        assert!(c.reply.len() <= maximum);
        Ok(c.reply.clone())
    }
}
fn profile() -> FirmwareCard {
    FirmwareCard {
        schema_uri: fujimod_catalog_card::SCHEMA_URI.into(),
        schema: 1,
        firmware_id: "model-1".into(),
        product: "Model".into(),
        internal_version: "1".into(),
        code_checks: vec![],
        mechanism: Mechanism::StockSdk(Box::new(StockSdk {
            callback_table_pointer: 0x1000,
            callback_offset: 12,
            stock_callback: 0x8000,
            table_bytes: 16,
            table_sha256: format!("{:x}", Sha256::digest(words(&[0, 0, 0, 0x8000]))),
            query_output: OUTPUT,
            bulk_source: Some(OUTPUT),
            dispatcher: 0x9000,
            request: REQUEST,
            completion: 0x9100,
            status: STATUS,
            value: VALUE,
            error: ERROR,
            pool_id: 0x6000,
            pool_begin: 0x10000,
            pool_bytes: 0x10000,
            allocate: 1,
            release: 2,
            cache: 3,
            sdk_table_cell: 0x6100,
            sdk_table: 0x7000,
            drive_map: 0x6200,
            ui_address: 0x6300,
            normal_ui: 3,
            open: 4,
            read: 5,
            write: 6,
            close: 7,
            seek: 10,
            unlink: 8,
            send: 9,
            list_first_code: 100,
            list_next_code: 101,
            directory_first_code: 102,
            directory_next_code: 103,
            receiver_cell: 0x6400,
            directory_context_bytes: 120,
            directory_attribute_offset: 59,
            directory_size_offset: 80,
            chunk_bytes: 64,
            end_of_directory_status: 6,
        })),
    }
}
fn camera() -> Arc<Mutex<Camera>> {
    let mut c = Camera::default();
    for (a, v) in [
        (0x1000, TABLE),
        (SLOT, 0x8000),
        (0x6000, 9),
        (0x6100, 0x7000),
        (0x6300, 3),
        (0x6400, 20),
        (STATUS, 12),
        (VALUE, 34),
        (ERROR, 56),
    ] {
        c.put(a, v)
    }
    c.files.insert("A:\\OLD.BIN".into(), vec![1, 2]);
    Arc::new(Mutex::new(c))
}
fn limits() -> DebugMemoryLimits {
    DebugMemoryLimits {
        read: NonZeroU32::new(256).unwrap(),
        write: NonZeroU32::new(256).unwrap(),
    }
}

#[test]
fn installed_report_service_uses_the_same_file_operations_and_preserves_source_pointer() {
    block_on(async {
        let camera = camera();
        {
            let mut c = camera.lock().unwrap();
            let descriptor = words(&[
                0x314c4946, 2, 123, 0x40100, 0x40104, 0x40108, 0x4010c, 0x40110, 0x40200, 0x41000,
                0x40114, 0x40118, 0x4011c, 0x40120, 0, 0,
            ]);
            c.write(0x40000, &descriptor);
            c.put(0x40100, 64);
            c.put(0x4011c, u32::MAX);
            c.put(0x40120, 0x1234);
        }
        let mut p = profile();
        p.mechanism = Mechanism::Mailbox {
            search_address: 0x40000,
            search_bytes: 0x4000,
            address_alias: 0,
            build: Some(123),
        };
        let mut session = peer_session(Endpoint(camera.clone()), 8192, None)
            .await
            .unwrap();
        let Response::Entries(entries) = execute(
            &mut session,
            limits(),
            &p,
            Request::List {
                pattern: "A:\\*.*".into(),
            },
        )
        .await
        .unwrap() else {
            panic!()
        };
        assert_eq!(entries.len(), 1);
        execute(
            &mut session,
            limits(),
            &p,
            Request::Write {
                path: "A:\\OLD.BIN".into(),
                data: vec![8, 9],
            },
        )
        .await
        .unwrap();
        assert_eq!(camera.lock().unwrap().word(0x40120), 0x1234);
        let Response::Bytes(data) = execute(
            &mut session,
            limits(),
            &p,
            Request::Read {
                path: "A:\\OLD.BIN".into(),
                max_bytes: 2,
            },
        )
        .await
        .unwrap() else {
            panic!()
        };
        assert_eq!(data, vec![8, 9]);
        execute(
            &mut session,
            limits(),
            &p,
            Request::Delete {
                path: "A:\\OLD.BIN".into(),
            },
        )
        .await
        .unwrap();
        assert!(camera.lock().unwrap().files.is_empty());
    })
}
fn restored(c: &Camera) {
    assert_eq!(c.word(SLOT), 0x8000);
    assert_eq!(c.read(REQUEST, 36), vec![0; 36]);
    assert_eq!(c.word(STATUS), 12);
    assert_eq!(c.word(VALUE), 34);
    assert_eq!(c.word(ERROR), 56);
    assert!(!c.allocated);
    assert!(c.opened.is_none())
}

#[test]
fn stock_sdk_replaces_reads_lists_and_deletes_with_restoration() {
    block_on(async {
        let camera = camera();
        let mut session = peer_session(Endpoint(camera.clone()), 8192, None)
            .await
            .unwrap();
        let profile = profile();
        let data: Vec<_> = (0..513).map(|n| (n * 37) as u8).collect();
        execute(
            &mut session,
            limits(),
            &profile,
            Request::Write {
                path: "A:\\NEW.BIN".into(),
                data: data.clone(),
            },
        )
        .await
        .unwrap();
        restored(&camera.lock().unwrap());
        let Response::Bytes(got) = execute(
            &mut session,
            limits(),
            &profile,
            Request::Read {
                path: "A:\\NEW.BIN".into(),
                max_bytes: 513,
            },
        )
        .await
        .unwrap() else {
            panic!()
        };
        assert_eq!(got, data);
        restored(&camera.lock().unwrap());
        let Response::Entries(entries) = execute(
            &mut session,
            limits(),
            &profile,
            Request::List {
                pattern: "A:\\*.*".into(),
            },
        )
        .await
        .unwrap() else {
            panic!()
        };
        assert_eq!(entries.len(), 2);
        assert!(entries.contains(&Entry {
            name: "NEW.BIN".into(),
            bytes: 513
        }));
        restored(&camera.lock().unwrap());
        execute(
            &mut session,
            limits(),
            &profile,
            Request::Delete {
                path: "A:\\OLD.BIN".into(),
            },
        )
        .await
        .unwrap();
        assert!(!camera.lock().unwrap().files.contains_key("A:\\OLD.BIN"));
        restored(&camera.lock().unwrap());
        execute(
            &mut session,
            limits(),
            &profile,
            Request::Delete {
                path: "A:\\MISSING.BIN".into(),
            },
        )
        .await
        .unwrap();
        restored(&camera.lock().unwrap());
    })
}

#[test]
fn short_write_closes_and_releases_but_uncertain_callback_fences() {
    block_on(async {
        for failure in ["short", "lost"] {
            let camera = camera();
            camera.lock().unwrap().failure = Some(failure);
            let mut session = peer_session(Endpoint(camera.clone()), 8192, None)
                .await
                .unwrap();
            assert!(
                execute(
                    &mut session,
                    limits(),
                    &profile(),
                    Request::Write {
                        path: "A:\\NEW.BIN".into(),
                        data: vec![3; 9]
                    }
                )
                .await
                .is_err()
            );
            if failure == "short" {
                restored(&camera.lock().unwrap());
                assert_eq!(session.state(), SessionState::Ready)
            } else {
                assert_eq!(session.state(), SessionState::CompletionUnknown);
                assert_eq!(camera.lock().unwrap().word(SLOT), 0x9000)
            }
        }
    })
}

#[test]
fn wrong_mode_and_oversize_read_fail_without_success_receipts() {
    block_on(async {
        let camera = camera();
        camera.lock().unwrap().put(0x6300, 9);
        let mut session = peer_session(Endpoint(camera.clone()), 8192, None)
            .await
            .unwrap();
        assert!(
            execute(
                &mut session,
                limits(),
                &profile(),
                Request::Delete {
                    path: "A:\\OLD.BIN".into()
                }
            )
            .await
            .is_err()
        );
        assert_eq!(camera.lock().unwrap().mutations, 0);
        camera.lock().unwrap().put(0x6300, 3);
        assert!(
            execute(
                &mut session,
                limits(),
                &profile(),
                Request::Read {
                    path: "A:\\OLD.BIN".into(),
                    max_bytes: 1
                }
            )
            .await
            .is_err()
        );
        restored(&camera.lock().unwrap());
    })
}

#[test]
fn mounted_sdk_amortizes_reservation_and_random_io_preserves_file_tail() {
    block_on(async {
        let camera = camera();
        camera
            .lock()
            .unwrap()
            .files
            .insert("A:\\OLD.BIN".into(), (0..=255).cycle().take(4096).collect());
        let mut session = peer_session(Endpoint(camera.clone()), 8192, None)
            .await
            .unwrap();
        let profile = profile();
        let mut volume = Volume::attach(&mut session, limits(), &profile, None)
            .await
            .unwrap();
        assert_eq!(camera.lock().unwrap().allocations, 1);
        for offset in [77, 203, 12] {
            let Response::Bytes(data) = volume
                .execute(Request::ReadAt {
                    path: "A:\\OLD.BIN".into(),
                    offset,
                    size: 3,
                })
                .await
                .unwrap()
            else {
                panic!()
            };
            assert_eq!(data, [offset as u8, (offset + 1) as u8, (offset + 2) as u8]);
        }
        volume
            .execute(Request::WriteAt {
                path: "A:\\OLD.BIN".into(),
                offset: 19,
                data: b"new".to_vec(),
            })
            .await
            .unwrap();
        {
            let c = camera.lock().unwrap();
            assert_eq!(&c.files["A:\\OLD.BIN"][19..22], b"new");
            assert_eq!(c.files["A:\\OLD.BIN"].len(), 4096);
            assert_eq!(c.files["A:\\OLD.BIN"][22], 22);
            assert_eq!(c.allocations, 1);
            assert!(c.allocated);
        }
        let Response::Directory(entries) = volume
            .execute(Request::ReadDirectory {
                path: "A:\\".into(),
            })
            .await
            .unwrap()
        else {
            panic!()
        };
        assert!(entries.iter().any(|e| e.name == "DCIM" && e.directory));
        assert!(entries.iter().any(|e| e.name == "OLD.BIN" && !e.directory));
        volume
            .execute(Request::Shrink {
                path: "A:\\OLD.BIN".into(),
                size: 23,
            })
            .await
            .unwrap();
        assert_eq!(camera.lock().unwrap().files["A:\\OLD.BIN"].len(), 23);
        volume.close().await.unwrap();
        restored(&camera.lock().unwrap());
    });
}

#[test]
fn mounted_sdk_retains_uncertain_allocation_and_refuses_further_work() {
    block_on(async {
        let camera = camera();
        let mut session = peer_session(Endpoint(camera.clone()), 8192, None)
            .await
            .unwrap();
        let profile = profile();
        let mut volume = Volume::attach(&mut session, limits(), &profile, None)
            .await
            .unwrap();
        camera.lock().unwrap().failure = Some("lost");
        assert!(
            volume
                .execute(Request::ReadAt {
                    path: "A:\\OLD.BIN".into(),
                    offset: 0,
                    size: 1
                })
                .await
                .is_err()
        );
        assert!(volume.read_memory(0x1000, 4).await.is_err());
        assert!(volume.close().await.is_err());
        assert!(camera.lock().unwrap().allocated);
        assert_eq!(session.state(), SessionState::CompletionUnknown);
    });
}
