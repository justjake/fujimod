//! I2C1 battery/power clients. Address36 is the gauge;66/68 retain
//! power-control writes. Address 39 is the X-T5 eye sensor (see
//! [`Proximity`]).
//! Other addresses NACK. No device identity or unsolicited ID is invented.
use fujimod_emulator_parts_io::power_controller::FuelGauge;
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// The viewfinder eye sensor: a TMD2772-class ambient-light/proximity
/// sensor at address 0x39 (the X-T5 firmware's I2C device table entry 2,
/// image0 0x18E9258; its poll task at 0x13DDB40 reads PDATA 0x18/0x19 and
/// STATUS 0x13 through command bytes 0xA0|register).
///
/// The model keeps the written configuration, answers the ID register with
/// the TMD27723 value the firmware expects (0x39) and reports a valid
/// proximity sample of zero counts with no interrupt: nothing is in front
/// of the finder, so the firmware keeps the rear LCD as the live display.
/// Without this client the bus NACKs every transfer; the firmware's zeroed
/// read buffer happens to give the same "finder empty" answer, but the
/// sensor's configuration writes and identity read fail on the bus.
#[derive(Serialize, Deserialize)]
pub struct Proximity {
    registers: Vec<u8>,
}
impl Default for Proximity {
    fn default() -> Self {
        Self {
            registers: vec![0; 0x20],
        }
    }
}
impl Proximity {
    pub const ADDRESS: u8 = 0x39;
    const ID: u8 = 0x12;
    const STATUS: u8 = 0x13;
    /// Command register: bit 7 set, bits 6:5 transaction type, bits 4:0
    /// the register address (auto-increment is the only type the firmware uses).
    fn select(&self, command: u8) -> u8 {
        command & 0x1f
    }
    fn write(&mut self, register: u8, byte: u8) {
        if let Some(slot) = self.registers.get_mut(register as usize) {
            *slot = byte;
        }
    }
    fn read(&self, register: u8) -> u8 {
        match register {
            Self::ID => 0x39,
            // AVALID | PVALID; no threshold interrupt pending.
            Self::STATUS => 0x03,
            // CDATA/IRDATA/PDATA: dark, nothing near the finder.
            0x14..=0x19 => 0,
            r => self.registers.get(r as usize).copied().unwrap_or(0),
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct BatteryI2c {
    regs: [u32; 8],
    gauge: FuelGauge,
    #[serde(default)]
    proximity: Proximity,
    power: BTreeMap<u16, u8>,
    address: u8,
    register: u8,
    active: bool,
    reading: bool,
    indexed: bool,
    generation: u64,
    pending: Option<(bool, u8, bool)>,
    transfers: u64,
}
impl Default for BatteryI2c {
    fn default() -> Self {
        Self {
            regs: [0; 8],
            gauge: FuelGauge::new(),
            proximity: Proximity::default(),
            power: BTreeMap::new(),
            address: 0,
            register: 0,
            active: false,
            reading: false,
            indexed: false,
            generation: 0,
            pending: None,
            transfers: 0,
        }
    }
}
impl BatteryI2c {
    fn known_address(&self) -> bool {
        matches!(self.address, 0x36 | 0x66 | 0x68 | Proximity::ADDRESS)
    }
    fn byte(&mut self, cx: &mut Cx<'_>, byte: u8) {
        if !self.indexed {
            self.register = if self.address == Proximity::ADDRESS {
                self.proximity.select(byte)
            } else {
                byte
            };
            self.indexed = true;
            return;
        }
        let result = if self.address == FuelGauge::ADDRESS {
            self.gauge.write(self.register, byte)
        } else if self.address == Proximity::ADDRESS {
            self.proximity.write(self.register, byte);
            Ok(())
        } else {
            self.power
                .insert((self.address as u16) << 8 | self.register as u16, byte);
            Ok(())
        };
        if let Err(error) = result {
            cx.fail(error);
        }
        self.register = self.register.wrapping_add(1);
    }
    fn receive(&mut self, cx: &mut Cx<'_>) -> u8 {
        let result = if self.address == FuelGauge::ADDRESS {
            self.gauge.read(self.register)
        } else if self.address == Proximity::ADDRESS {
            Ok(self.proximity.read(self.register))
        } else {
            self.power
                .get(&((self.address as u16) << 8 | self.register as u16))
                .copied()
                .ok_or_else(|| {
                    format!(
                        "unmodeled power I2C {:02x}:{:02x}",
                        self.address, self.register
                    )
                })
        };
        self.register = self.register.wrapping_add(1);
        match result {
            Ok(value) => value,
            Err(error) => {
                cx.fail(error);
                0
            }
        }
    }
}
impl Device for BatteryI2c {
    fn name(&self) -> &'static str {
        "i2c1-battery-power"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let offset = address.wrapping_sub(0xff925000);
        if !matches!(size, 1 | 4) || offset > 0x1c || offset & 3 != 0 {
            cx.unmodeled();
            return 0;
        }
        //1472880 recovery checks SDA/SCL at bits4/5, idle high.
        if offset == 0x1c {
            return (self.regs[7] | 0x30) as u64;
        }
        self.regs[offset as usize / 4] as u64
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let offset = address.wrapping_sub(0xff925000);
        if !matches!(size, 1 | 4) || offset > 0x1c || offset & 3 != 0 {
            cx.unmodeled();
            return;
        }
        self.regs[offset as usize / 4] = value as u32;
        if offset != 4 {
            return;
        }
        cx.set_irq(289, false);
        self.generation += 1;
        self.pending = None;
        let control = value as u32;
        if control & 0x12 != 0x12 {
            self.active = false;
            self.regs[0] = 0;
            return;
        }
        let address = !self.active || control & 0x20 != 0;
        self.pending = Some((address, self.regs[4] as u8, control & 8 != 0));
        // Functional byte budget, not a measured bus rate. At 20us, the
        // A 1ms interval completes the declared setup-key workload.
        cx.schedule(cx.now() + 1_000_000, self.generation);
    }
    fn event(&mut self, cx: &mut Cx<'_>, token: u64) {
        if token != self.generation {
            return;
        }
        let Some((address, byte, ack)) = self.pending.take() else {
            return;
        };
        if address {
            self.address = byte >> 1;
            self.reading = byte & 1 != 0;
            self.active = true;
            if !self.reading {
                self.indexed = false;
            }
            //Read-address ACK is not a data byte (ISR tests status bit0).
            self.regs[0] = if self.reading { 1 } else { 8 };
        } else if self.reading {
            self.regs[4] = self.receive(cx) as u32;
            self.regs[0] = if ack { 0 } else { 0x10 };
        } else {
            if self.known_address() {
                self.byte(cx, byte);
            }
            self.regs[0] = 8;
        }
        if !self.known_address() {
            self.regs[0] |= 0x10;
        }
        self.transfers += 1;
        self.regs[1] |= 0x10;
        if self.regs[1] & 0x40 != 0 {
            cx.set_irq(289, true);
        }
    }
    fn save(&self) -> Value {
        serde_json::to_value(self).unwrap()
    }
    fn restore(&mut self, state: &Value) -> Result<(), String> {
        *self = serde_json::from_value(state.clone())
            .map_err(|e| format!("invalid I2C battery checkpoint: {e}"))?;
        Ok(())
    }
}

#[cfg(test)]
mod proximity_tests {
    use super::Proximity;

    #[test]
    fn eye_sensor_reports_identity_and_an_empty_finder() {
        let mut sensor = Proximity::default();
        // Firmware command bytes are 0xA0 | register (auto-increment type).
        assert_eq!(sensor.select(0xa0 | 0x12), 0x12);
        assert_eq!(sensor.read(0x12), 0x39);
        assert_eq!(sensor.read(0x13) & 0x22, 0x02, "PVALID set, PINT clear");
        assert_eq!([sensor.read(0x18), sensor.read(0x19)], [0, 0]);
        // Configuration writes are retained and read back.
        sensor.write(0x0e, 0x0c);
        assert_eq!(sensor.read(0x0e), 0x0c);
    }
}
