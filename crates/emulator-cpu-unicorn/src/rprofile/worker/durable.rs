//! Encoding and decoding run on the CPU's owning thread. Only checkpoint bytes
//! and target-owned tokens cross the worker boundary.
use super::*;

impl RemoteSnapshot {
    pub(super) fn export_bytes(&self) -> Result<Vec<u8>, String> {
        let (reply, response) = mpsc::channel();
        self.commands
            .send(Message::Request {
                operation: Operation::ExportSnapshot(self.token),
                reply,
            })
            .map_err(|_| "R-profile snapshot worker disconnected")?;
        match response
            .recv()
            .map_err(|_| "R-profile snapshot worker reply disconnected")??
        {
            Value::Bytes(bytes) => Ok(bytes),
            _ => Err("R-profile snapshot worker returned unexpected archive data".into()),
        }
    }
}
impl CpuWorker {
    pub(super) fn import_bytes(&self, bytes: &[u8]) -> Result<Box<dyn Snapshot>, String> {
        match self.rpc(Operation::ImportSnapshot(bytes.to_vec()))? {
            Value::Snapshot(token) => Ok(Box::new(RemoteSnapshot {
                owner: self.identity,
                token,
                commands: self.commands.clone(),
            })),
            _ => Err("R-profile snapshot worker returned an unexpected imported token".into()),
        }
    }
}
impl WorkerLoop {
    pub(super) fn retain_snapshot(&self, snapshot: Box<dyn Snapshot>) -> Result<Value, String> {
        let token = self.next_snapshot.get();
        self.next_snapshot.set(
            token
                .checked_add(1)
                .ok_or("R-profile worker snapshot token space exhausted")?,
        );
        self.snapshots.borrow_mut().insert(token, snapshot);
        Ok(Value::Snapshot(token))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn processor() -> CpuWorker {
        let cpu = spawn(Config {
            translation_cache_size_bytes: 1 << 20,
            options: core::Options {
                cp14_identity: 7,
                ..Default::default()
            },
            memory: SharedMemory::default(),
        })
        .unwrap();
        cpu.map_zeroed(0x1000, 0x1000, 7).unwrap();
        cpu.write_memory(0x1000, &[1, 0, 0x80, 0xe2, 0xfd, 0xff, 0xff, 0xea])
            .unwrap();
        cpu.write_register(arm::CPSR, &0xd3u32.to_le_bytes())
            .unwrap();
        cpu.write_register(arm::PC, &0x1000u32.to_le_bytes())
            .unwrap();
        cpu
    }
    fn run(cpu: &CpuWorker, count: usize) {
        let mut bytes = [0; 4];
        cpu.read_register(arm::PC, &mut bytes).unwrap();
        cpu.run(Run {
            begin: u64::from(u32::from_le_bytes(bytes)),
            until: None,
            watchdog_us: 0,
            instructions: Some(count),
        })
        .unwrap();
    }
    #[test]
    fn archive_from_a_destroyed_worker_resumes_on_a_new_owning_thread() {
        let (bytes, expected, before_resume) = {
            let source = processor();
            source
                .set_guest_clock(Clock {
                    now: 100,
                    phase: 7,
                    ratio: timeline::Ratio::new(24, 1).unwrap(),
                })
                .unwrap();
            source.rprofile_identity(Some(42)).unwrap();
            run(&source, 35);
            let saved = source.snapshot(None).unwrap();
            let bytes = saved.export().unwrap();
            let before_resume = source.fingerprint().unwrap();
            run(&source, 2113);
            assert_eq!(saved.export().unwrap(), bytes);
            (bytes, source.fingerprint().unwrap(), before_resume)
        };
        let target = processor();
        let before = target.fingerprint().unwrap();
        let saved = target.import_snapshot(&bytes).unwrap();
        assert_eq!(saved.owner(), target.identity());
        assert_eq!(target.fingerprint().unwrap(), before);
        target.restore(saved.as_ref()).unwrap();
        assert_eq!(target.fingerprint().unwrap(), before_resume);
        target.snapshot(Some(saved.as_ref())).unwrap();
        run(&target, 2113);
        assert_eq!(target.fingerprint().unwrap(), expected);
    }
    #[test]
    fn invalid_worker_archive_does_not_change_live_processor_or_retained_points() {
        let cpu = processor();
        let point = cpu.snapshot(None).unwrap();
        let bytes = point.export().unwrap();
        let mut bad = bytes.clone();
        bad.push(0);
        let before = cpu.fingerprint().unwrap();
        assert!(cpu.import_snapshot(&bad).is_err());
        assert!(cpu.import_snapshot(&[]).is_err());
        assert_eq!(cpu.fingerprint().unwrap(), before);
        assert_eq!(point.export().unwrap(), bytes);
        cpu.restore(point.as_ref()).unwrap();
    }
}
