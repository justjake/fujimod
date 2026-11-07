//! X-Processor 4 USB condition register: bit 0 reports the USB cable present.
//! The stock service entry and the boot's USB task both read it
//! (`FUN_01d8a330`/`FUN_01d8a34c`/`FUN_011655a8` read `0xfff4a300`). It is a
//! board strap, not a writable register.
use fujimod_emulator_devices::{Cx, Device};
use serde_json::Value;

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsbCondition {
    base: u64,
    present: bool,
}
impl UsbCondition {
    pub fn new(base: u64, present: bool) -> Result<Self, String> {
        if base & 3 != 0 || base > u32::MAX as u64 - 4 {
            return Err("Invalid XP4 USB condition register".into());
        }
        Ok(Self { base, present })
    }
}
impl Device for UsbCondition {
    fn name(&self) -> &'static str {
        "xp4-usb-condition"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        if address != self.base || size != 4 {
            cx.unmodeled();
            return 0;
        }
        u64::from(self.present as u32)
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, _value: u64) {
        // A strap: the value is the cable's, not the writer's.
        if address != self.base || size != 4 {
            cx.unmodeled();
        }
    }
    fn save(&self) -> Value {
        serde_json::json!({ "base": self.base })
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let base = value
            .get("base")
            .and_then(Value::as_u64)
            .ok_or("Invalid XP4 USB condition checkpoint")?;
        if base != self.base {
            return Err("Invalid XP4 USB condition checkpoint".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::Host;

    #[derive(Default)]
    struct NoOp;
    impl Host for NoOp {
        fn now(&self) -> u64 {
            0
        }
        fn schedule(&mut self, _: usize, _: u64, _: u64) {}
        fn set_irq(&mut self, _: u32, _: bool) {}
        fn raise_irq(&mut self, _: usize, _: u32) -> Result<(), String> {
            Ok(())
        }
        fn ram_read(&self, _: u64, _: &mut [u8]) -> Result<(), String> {
            Err("no ram".into())
        }
        fn ram_write(&mut self, _: u64, _: &[u8]) -> Result<(), String> {
            Err("no ram".into())
        }
        fn record(&mut self, _: fujimod_emulator_devices::Record) {}
    }

    #[test]
    fn reports_presence_and_refuses_other_words() {
        let mut absent = UsbCondition::new(0xfff4a300, false).unwrap();
        let mut host = NoOp;
        assert_eq!(absent.read(&mut Cx::new(&mut host, 0, 0, 0), 0xfff4a300, 4), 0);
        let mut present = UsbCondition::new(0xfff4a300, true).unwrap();
        assert_eq!(present.read(&mut Cx::new(&mut host, 0, 0, 0), 0xfff4a300, 4), 1);
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        present.read(&mut cx, 0xfff4a304, 4);
        assert!(cx.refusal().is_some());
    }
}
