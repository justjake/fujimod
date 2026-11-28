//! Write-only bank command words with independent command history.
//!
//! A word write of one arms that bank; a zero consumes its armed state.
//! Repeated ones are accepted. Reads and other accesses are unsupported.
use fujimod_emulator_devices::{Cx, Device, Record, Value, checkpoint::Checkpoint};

#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    addresses: Vec<u64>,
    armed: Vec<bool>,
}

/// Configurable write-only command words. Armed state is command history,
/// not readable bank status or a claim about physical reset values.
pub struct BankControl {
    state: State,
}

impl BankControl {
    /// Addresses must be distinct, aligned 32-bit words.
    pub fn new(addresses: Vec<u64>) -> Result<Self, String> {
        if addresses.is_empty() || addresses.iter().any(|address| address % 4 != 0) {
            return Err("bank addresses must contain aligned words".into());
        }
        let mut sorted = addresses.clone();
        sorted.sort_unstable();
        if sorted.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err("bank addresses must be distinct".into());
        }
        Ok(Self {
            state: State {
                armed: vec![false; addresses.len()],
                addresses,
            },
        })
    }

    fn decode(&self, saved: &Checkpoint) -> Result<State, String> {
        if !saved.children().is_empty() || !saved.blobs().is_empty() {
            return Err("bank control checkpoint has unexpected attachments".into());
        }
        let state: State = saved.decode(self.name())?;
        if state.addresses != self.state.addresses
            || state.armed.len() != self.state.addresses.len()
        {
            return Err("bank control checkpoint configuration differs".into());
        }
        Ok(state)
    }
}

impl Device for BankControl {
    fn name(&self) -> &'static str {
        "xp4-bank-control"
    }

    fn read(&mut self, cx: &mut Cx<'_>, _: u64, _: u8) -> u64 {
        cx.unsupported("bank command readback is not modeled");
        0
    }

    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let index = self
            .state
            .addresses
            .iter()
            .position(|word| *word == address);
        let Some(index) = index.filter(|_| size == 4) else {
            cx.unsupported("bank command requires a configured word address and width");
            return;
        };
        match value {
            1 => self.state.armed[index] = true,
            0 if self.state.armed[index] => self.state.armed[index] = false,
            _ => {
                cx.unsupported("bank command requires one or zero after one on the same word");
                return;
            }
        }
        cx.record(Record::new(
            "XP4 bank control write",
            [
                ("address", Value::int(address)),
                ("width", Value::int(size)),
                ("value", Value::int(value)),
                ("core", Value::int(cx.cpu as u64)),
                ("pc", Value::int(cx.pc)),
                ("tick", Value::int(cx.now())),
            ],
        ));
    }

    fn checkpoint(&self) -> Result<Checkpoint, String> {
        Checkpoint::encode(self.name(), &self.state)
    }
    fn validate_checkpoint(&self, saved: &Checkpoint) -> Result<(), String> {
        self.decode(saved).map(|_| ())
    }
    fn restore_checkpoint(&mut self, saved: &Checkpoint) -> Result<(), String> {
        self.state = self.decode(saved)?;
        Ok(())
    }
    fn fingerprint(&self) -> Result<serde_json::Value, String> {
        serde_json::to_value(&self.state).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::Host;

    #[derive(Default)]
    struct Capture {
        records: Vec<Record>,
    }
    impl Host for Capture {
        fn now(&self) -> u64 {
            9
        }
        fn schedule(&mut self, _: usize, _: u64, _: u64) {
            panic!("unexpected deadline")
        }
        fn set_irq(&mut self, _: u32, _: bool) {
            panic!("unexpected IRQ")
        }
        fn raise_irq(&mut self, _: usize, _: u32) -> Result<(), String> {
            panic!("unexpected IRQ")
        }
        fn ram_read(&self, _: u64, _: &mut [u8]) -> Result<(), String> {
            panic!("unexpected DMA")
        }
        fn ram_write(&mut self, _: u64, _: &[u8]) -> Result<(), String> {
            panic!("unexpected DMA")
        }
        fn record(&mut self, row: Record) {
            self.records.push(row);
        }
    }
    fn write(
        device: &mut BankControl,
        host: &mut Capture,
        addr: u64,
        size: u8,
        value: u64,
        accepted: bool,
    ) {
        let mut cx = Cx::new(host, 2, 0x100, 0);
        device.write(&mut cx, addr, size, value);
        assert_eq!(cx.refusal().is_none(), accepted);
    }
    #[test]
    fn independent_history_and_strict_accesses() {
        let mut device = BankControl::new(vec![0x1000, 0x1010, 0x1020]).unwrap();
        let mut host = Capture::default();
        write(&mut device, &mut host, 0x1000, 4, 0, false);
        write(&mut device, &mut host, 0x1000, 4, 1, true);
        write(&mut device, &mut host, 0x1000, 4, 1, true);
        write(&mut device, &mut host, 0x1010, 4, 0, false);
        let saved = device.checkpoint().unwrap();
        for (addr, size, value) in [
            (0x1001, 4, 1),
            (0x1000, 1, 1),
            (0x1000, 4, 2),
            (0x1000, 4, 0x1_0000_0001),
        ] {
            write(&mut device, &mut host, addr, size, value, false);
        }
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        device.read(&mut cx, 0x1000, 4);
        assert!(cx.refusal().is_some());
        assert_eq!(device.checkpoint().unwrap(), saved);
        write(&mut device, &mut host, 0x1000, 4, 0, true);
        write(&mut device, &mut host, 0x1000, 4, 0, false);
        assert_eq!(host.records.len(), 3);
    }
    #[test]
    fn construction_and_restore_validate_configuration() {
        assert!(BankControl::new(vec![]).is_err());
        assert!(BankControl::new(vec![1]).is_err());
        assert!(BankControl::new(vec![4, 4]).is_err());
        let mut host = Capture::default();
        let mut device = BankControl::new(vec![4, 8]).unwrap();
        write(&mut device, &mut host, 4, 4, 1, true);
        let saved = device.checkpoint().unwrap();
        let mut restored = BankControl::new(vec![4, 8]).unwrap();
        restored.restore_checkpoint(&saved).unwrap();
        assert_eq!(
            device.fingerprint().unwrap(),
            restored.fingerprint().unwrap()
        );
        write(&mut restored, &mut host, 8, 4, 0, false);
        write(&mut restored, &mut host, 4, 4, 0, true);
        let before = restored.checkpoint().unwrap();
        let wrong = BankControl::new(vec![8, 4]).unwrap().checkpoint().unwrap();
        assert!(restored.restore_checkpoint(&wrong).is_err());
        let malformed = Checkpoint::encode(
            device.name(),
            &State {
                addresses: vec![4, 8],
                armed: vec![true],
            },
        )
        .unwrap();
        assert!(restored.restore_checkpoint(&malformed).is_err());
        assert!(
            restored
                .restore_checkpoint(&saved.clone().with_children(vec![saved.clone()]))
                .is_err()
        );
        assert!(
            restored
                .restore_checkpoint(&saved.clone().with_blobs(vec![vec![0].into()]))
                .is_err()
        );
        assert_eq!(restored.checkpoint().unwrap(), before);
    }
}
