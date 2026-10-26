//! XP5 interprocessor register mailbox. Payload words are opaque transport.
//! The two CPU aliases dispatch to one instance; only requester identity and
//! register accesses select interrupt outputs. There are no deadlines, memory
//! writes, command handlers or firmware execution state in this device.
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

const CAUSES: u32 = 0x79;
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    words: [u32; 8],
    causes: u32,
    traffic: Vec<Value>,
}
/// Observational state shared with runtime status reporting. CPU checkpoints
/// and substitution policy are deliberately outside this channel.
#[derive(Clone, Default)]
pub struct Channel(Arc<Mutex<State>>);
impl Channel {
    pub fn status(&self) -> Value {
        serde_json::to_value(&*self.0.lock().unwrap()).unwrap()
    }
}
#[derive(Clone, Copy)]
pub struct Pins {
    pub companion: usize,
    pub request: u32,
    pub control: u32,
    pub reply: u32,
    pub status: u32,
    pub fault: u32,
}
pub struct Mailbox {
    base: u64,
    pins: Pins,
    channel: Channel,
}
impl Mailbox {
    pub fn new(base: u64, pins: Pins, channel: Channel) -> Self {
        Self {
            base,
            pins,
            channel,
        }
    }
    fn offset(&self, address: u64, size: u8) -> Option<u64> {
        let o = address.wrapping_sub(self.base);
        (size == 4 && o & 3 == 0 && o <= 0x20).then_some(o)
    }
}
impl Device for Mailbox {
    fn name(&self) -> &'static str {
        "interprocessor-mailbox"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let Some(o) = self.offset(address, size) else {
            cx.unmodeled();
            return 0;
        };
        let s = self.channel.0.lock().unwrap();
        if o == 0x20 {
            s.causes as u64
        } else {
            s.words[(o / 4) as usize] as u64
        }
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let Some(o) = self.offset(address, size) else {
            cx.unmodeled();
            return;
        };
        let companion = cx.cpu == self.pins.companion;
        let bit = match (o, companion) {
            (0, false) => 1,
            (0xc, false) => 8,
            (0x10, true) => 0x10,
            (0x14, true) => 0x20,
            (0x18, true) => 0x40,
            (0x20, _) => 0,
            _ if value == 0 => 0,
            _ => {
                cx.unmodeled();
                return;
            }
        };
        let mut s = self.channel.0.lock().unwrap();
        let old = s.causes;
        if o == 0x20 {
            s.causes &= !(value as u32);
        } else {
            s.words[(o / 4) as usize] = value as u32;
            s.causes |= bit;
        }
        let causes = s.causes;
        s.traffic
            .push(json!({"cpu":cx.cpu,"offset":o,"word":value as u32}));
        if s.traffic.len() > 64 {
            s.traffic.remove(0);
        }
        drop(s);
        for (mask, line) in [(1, self.pins.request), (8, self.pins.control)] {
            if (old ^ causes) & mask != 0 {
                if let Err(e) = cx.set_core_irq(self.pins.companion, line, causes & mask != 0) {
                    cx.fail(e);
                }
            }
        }
        for (mask, line) in [
            (0x10, self.pins.reply),
            (0x20, self.pins.status),
            (0x40, self.pins.fault),
        ] {
            if (old ^ causes) & mask != 0 {
                cx.set_irq(line, causes & mask != 0);
            }
        }
    }
    fn pollable(&self, address: u64, size: u8) -> bool {
        self.offset(address, size).is_some()
    }
    fn save(&self) -> Value {
        self.channel.status()
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let s: State = serde_json::from_value(value.clone())
            .map_err(|e| format!("mailbox checkpoint: {e}"))?;
        if s.causes & !CAUSES != 0 || s.traffic.len() > 64 {
            return Err("unsupported mailbox checkpoint state".into());
        }
        *self.channel.0.lock().unwrap() = s;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::{Host, Record};
    #[derive(Default)]
    struct TestHost {
        core: Vec<(usize, u32, bool)>,
        irqs: Vec<(u32, bool)>,
    }
    impl Host for TestHost {
        fn now(&self) -> u64 {
            0
        }
        fn schedule(&mut self, _: usize, _: u64, _: u64) {
            panic!("a mailbox never schedules completions")
        }
        fn set_irq(&mut self, line: u32, level: bool) {
            self.irqs.push((line, level));
        }
        fn raise_irq(&mut self, _: usize, _: u32) -> Result<(), String> {
            panic!("mailbox IRQs are level inputs")
        }
        fn set_core_irq(&mut self, cpu: usize, line: u32, level: bool) -> Result<(), String> {
            self.core.push((cpu, line, level));
            Ok(())
        }
        fn ram_read(&self, _: u64, _: &mut [u8]) -> Result<(), String> {
            panic!("mailbox payloads are opaque")
        }
        fn ram_write(&mut self, _: u64, _: &[u8]) -> Result<(), String> {
            panic!("mailbox never writes private RAM")
        }
        fn record(&mut self, _: Record) {}
    }
    fn device() -> Mailbox {
        Mailbox::new(
            0xff90e100,
            Pins {
                companion: 4,
                request: 55,
                control: 58,
                reply: 141,
                status: 142,
                fault: 143,
            },
            Channel::default(),
        )
    }
    fn write(m: &mut Mailbox, h: &mut TestHost, cpu: usize, offset: u64, value: u64) {
        let mut cx = Cx::new(h, cpu, 0, 0);
        m.write(&mut cx, 0xff90e100 + offset, 4, value);
        assert_eq!(cx.refusal(), None);
    }
    #[test]
    fn every_opaque_request_uses_the_same_register_and_level_contract() {
        for word in [0, 0x2a << 6, 0x2b << 6, 0x80 << 6, 0xa9 << 6, 0xffffffff] {
            let mut m = device();
            let mut h = TestHost::default();
            write(&mut m, &mut h, 0, 0, word);
            assert_eq!(h.core, vec![(4, 55, true)]);
            assert!(h.irqs.is_empty());
            let saved = m.save();
            let mut cx = Cx::new(&mut h, 4, 0xffff, 0);
            assert_eq!(m.read(&mut cx, 0xff90e100, 4), word);
            assert_eq!(m.read(&mut cx, 0xff90e120, 4), 1);
            m.advance(&mut cx);
            m.event(&mut cx, 1);
            assert_eq!(m.next_deadline(u64::MAX), None);
            assert_eq!(m.save(), saved);
            write(&mut m, &mut h, 4, 0x20, 1);
            assert_eq!(h.core, vec![(4, 55, true), (4, 55, false)]);
            assert!(h.irqs.is_empty());
        }
    }
    #[test]
    fn independent_causes_acknowledge_without_affecting_other_outputs() {
        let mut m = device();
        let mut h = TestHost::default();
        write(&mut m, &mut h, 3, 0xc, 0x1234);
        write(&mut m, &mut h, 4, 0x10, 0xfeed);
        write(&mut m, &mut h, 4, 0x14, 0x22);
        write(&mut m, &mut h, 4, 0x18, 0x33);
        assert_eq!(h.core, vec![(4, 58, true)]);
        assert_eq!(h.irqs, vec![(141, true), (142, true), (143, true)]);
        let saved = m.save();
        let mut restored = device();
        restored.restore(&saved).unwrap();
        assert_eq!(restored.save(), saved);
        write(&mut restored, &mut h, 0, 0x20, 0x20);
        assert_eq!(h.irqs.last(), Some(&(142, false)));
        assert_eq!(restored.save()["causes"], 0x58);
        assert_eq!(m.save(), saved);
        write(&mut m, &mut h, 0, 0x20, 0xffffffff);
        assert_eq!(m.save()["causes"], 0);
    }
    #[test]
    fn invalid_accesses_and_execution_metadata_are_refused_without_mutation() {
        let mut m = device();
        let mut h = TestHost::default();
        let initial = m.save();
        for (cpu, offset, size, value) in [
            (0, 0x10, 4, 1),
            (4, 0, 4, 1),
            (0, 1, 4, 1),
            (0, 0, 2, 1),
            (0, 0x24, 4, 1),
        ] {
            let mut cx = Cx::new(&mut h, cpu, 0, 0);
            m.write(&mut cx, 0xff90e100 + offset, size, value);
            assert!(cx.is_unmodeled());
            assert_eq!(m.save(), initial);
        }
        let mut invalid = initial.clone();
        invalid["snapshot"] = json!({"pc":0x3780});
        assert!(m.restore(&invalid).is_err());
        invalid = initial.clone();
        invalid["causes"] = json!(2);
        assert!(m.restore(&invalid).is_err());
        assert!(h.core.is_empty() && h.irqs.is_empty());
    }
}
