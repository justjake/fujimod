//! FFF19000 parameter retention. The bank supplies uploaded pixel parameters
//! through 96 canonical word masks. Implemented silicon bits remain unmeasured.
use fujimod_emulator_devices::{Cx, Device};
use fujimod_emulator_imaging::exr2_isp::parameters::{Options, ParameterHandle, Parameters, mask};
use fujimod_emulator_imaging::exr2_isp::word;

pub struct ParameterBank {
    pub parameters: ParameterHandle,
}
impl ParameterBank {
    pub fn new(_: Options, parameters: ParameterHandle) -> Result<Self, String> {
        Ok(Self { parameters })
    }
}
fn valid(o: usize, v: u32) -> bool {
    if mask(o).is_none_or(|m| v & !m != 0) {
        return false;
    }
    match o {
        0 => v & 63 <= 0x2d,
        8 => v >> 12 & 3 <= 2,
        0x104 => v & 127 <= 0x51,
        0x110 => v & 511 <= 256 && v >> 16 & 511 <= 256,
        0x114 => v & 511 <= 256,
        0x300 => (0..3).all(|i| v >> (i * 8) & 63 <= 32),
        0x900 => v & 255 <= 128 && v >> 8 & 255 <= 128,
        _ => true,
    }
}
impl Device for ParameterBank {
    fn checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        fujimod_emulator_devices::checkpoint::Checkpoint::encode(
            self.name(),
            &*self
                .parameters
                .lock()
                .map_err(|_| "parameter state poisoned")?,
        )
    }
    fn validate_checkpoint(&self, saved: &fujimod_emulator_devices::checkpoint::Checkpoint) -> Result<(), String> {
        let _: Parameters = saved.decode(self.name())?;
        Ok(())
    }
    fn restore_checkpoint(&mut self, saved: &fujimod_emulator_devices::checkpoint::Checkpoint) -> Result<(), String> {
        *self
            .parameters
            .lock()
            .map_err(|_| "parameter state poisoned")? = saved.decode(self.name())?;
        Ok(())
    }

    fn fingerprint(&self) -> std::result::Result<serde_json::Value, String> {
        Ok(self.save())
    }

    fn name(&self) -> &'static str {
        "exr2-isp-parameters"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let Some(o) = word(cx, address, size, 0xfff19000, 0x1000) else {
            return 0;
        };
        if mask(o).is_none() {
            cx.unmodeled();
            return 0;
        }
        self.parameters.lock().unwrap().words[o / 4] as u64
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let Some(o) = word(cx, address, size, 0xfff19000, 0x1000) else {
            return;
        };
        if value > u32::MAX as u64 || !valid(o, value as u32) {
            cx.fail("image parameter word exceeds its mask/domain");
            return;
        }
        self.parameters.lock().unwrap().words[o / 4] = value as u32;
    }
    fn save(&self) -> serde_json::Value {
        serde_json::to_value(&*self.parameters.lock().unwrap()).unwrap()
    }
    fn restore(&mut self, v: &serde_json::Value) -> Result<(), String> {
        let p: Parameters = serde_json::from_value(v.clone()).map_err(|e| e.to_string())?;
        if p.words.len() != 1024
            || p.words.iter().enumerate().any(|(i, &v)| {
                if mask(i * 4).is_some() {
                    !valid(i * 4, v)
                } else {
                    v != 0
                }
            })
        {
            return Err("invalid parameter bank state".into());
        }
        *self.parameters.lock().unwrap() = p;
        Ok(())
    }
}
