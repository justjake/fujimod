//! Serial register endpoint connected to the power wake countdown.
use fujimod_emulator_devices::{Cx, checkpoint::Checkpoint, exr2::wake_alarm::WakeAlarm};
use fujimod_emulator_parts_io::spi::{Endpoint, RegisterFile, RegisterOptions, Shift};
use std::sync::{Arc, Mutex};

/// Shared by the serial controller and the SoC's power-hold output.
pub type Power = Arc<Mutex<WakeAlarm>>;

pub struct PowerEndpoint {
    registers: RegisterFile,
    power: Power,
}
impl PowerEndpoint {
    pub fn new(options: RegisterOptions, power: Power) -> Result<Self, String> {
        for register in [0x68, 0x69, 0x6a] {
            if !options
                .registers
                .iter()
                .any(|(address, _)| *address == register)
                || options.status_registers.contains(&register)
                || options
                    .clock
                    .as_ref()
                    .is_some_and(|clock| clock.registers.contains(&register))
            {
                return Err("wake registers must be writable command registers".into());
            }
        }
        Ok(Self {
            registers: RegisterFile::new(options)?,
            power,
        })
    }
}
impl Endpoint for PowerEndpoint {
    fn name(&self) -> &'static str {
        "exr2-power-endpoint"
    }
    fn checkpoint(&self) -> Result<Checkpoint, String> {
        let state = self.power.lock().map_err(|_| "power alarm poisoned")?;
        Ok(Checkpoint::encode(self.name(), &*state)?
            .with_children(vec![self.registers.checkpoint()?]))
    }
    fn validate_checkpoint(&self, saved: &Checkpoint) -> Result<(), String> {
        let _: WakeAlarm = saved.decode(self.name())?;
        if saved.children().len() != 1 {
            return Err("power endpoint register state missing".into());
        }
        self.registers.validate_checkpoint(&saved.children()[0])
    }
    fn restore_checkpoint(&mut self, saved: &Checkpoint) -> Result<(), String> {
        self.validate_checkpoint(saved)?;
        let state: WakeAlarm = saved.decode(self.name())?;
        self.registers.restore_checkpoint(&saved.children()[0])?;
        *self.power.lock().map_err(|_| "power alarm poisoned")? = state;
        Ok(())
    }
    fn status_value(&self, register: u8) -> Result<u8, String> {
        self.registers.status_value(register)
    }
    fn status_input(&mut self, register: u8, value: u8) -> Result<(), String> {
        self.registers.status_input(register, value)
    }
    fn fingerprint(&self) -> Result<serde_json::Value, String> {
        Ok(
            serde_json::json!({"registers":self.registers.fingerprint()?,
            "power":*self.power.lock().map_err(|_| "power alarm poisoned")?}),
        )
    }
    fn state(&self) -> serde_json::Value {
        self.registers.state()
    }
    fn validate(&self, shift: Shift) -> Result<(), String> {
        self.registers.validate(shift)
    }
    fn transfer(&mut self, cx: &mut Cx<'_>, shift: Shift, at: u64) -> Result<u16, String> {
        self.registers.validate(shift)?;
        let register = (shift.word >> 8 & 0x7f) as u8;
        if shift.word & 0x8000 != 0 && matches!(register, 0x68..=0x6a) {
            let mut power = self.power.lock().map_err(|_| "power alarm poisoned")?;
            // Validate the whole effect before changing either owner.
            let mut next = power.clone();
            next.write(at, register, shift.word as u8)?;
            let reply = self.registers.transfer(cx, shift, at)?;
            *power = next;
            Ok(reply)
        } else {
            self.registers.transfer(cx, shift, at)
        }
    }
    fn advance(&mut self, _: &mut Cx<'_>, at: u64) -> Result<(), String> {
        self.power
            .lock()
            .map_err(|_| "power alarm poisoned")?
            .advance(at)
    }
    fn next_deadline(&self, _: u64) -> Option<u64> {
        self.power.lock().ok()?.next_deadline()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::{exr2::wake_alarm::Supply, test_host::TestHost};
    #[test]
    fn serial_writes_arm_the_same_power_state_used_by_supply_hold() {
        let power = Arc::new(Mutex::new(WakeAlarm::new(100, 10).unwrap()));
        let mut part = PowerEndpoint::new(
            RegisterOptions {
                registers: vec![(0x68, 0), (0x69, 0), (0x6a, 0)],
                readable: true,
                status_registers: vec![],
                clock: None,
            },
            power.clone(),
        )
        .unwrap();
        let mut host = TestHost::new();
        let mut clock = fujimod_emulator_devices::exr2::clock::Clocks::new(
            fujimod_emulator_devices::exr2::clock::Options {
                output_settle_ticks: 2,
                reset_ready: true,
            },
            (),
        )
        .unwrap();
        clock.output().attach_power(power.clone()).unwrap();
        for (at, word) in [(1, 0xe901), (2, 0xea00), (3, 0xe801)] {
            let mut cx = Cx::new(&mut host, 0, 0, 0);
            part.transfer(
                &mut cx,
                Shift {
                    word,
                    bits: 16,
                    receive: true,
                },
                at,
            )
            .unwrap();
        }
        host.now = 4;
        fujimod_emulator_devices::test_host::write(
            &mut clock,
            &mut host,
            fujimod_emulator_devices::exr2::clock::BASE + 0x84,
            0,
        );
        {
            let mut state = power.lock().unwrap();
            state.advance(14).unwrap();
            assert_eq!(state.supply(), Supply::Off);
            state.advance(103).unwrap();
            assert_eq!(state.supply(), Supply::Wake);
        }
    }
}
