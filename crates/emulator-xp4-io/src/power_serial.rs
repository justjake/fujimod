//! Bounded physical power-controller SPI register protocol, not a CPU harness.
//! 10a09fc clocks a 24-bit frame; 10a1d40 supplies the six-bit register address.
//!
//! Selector 23 carries one observed command, `0x0400`, and selector 24 returns
//! the raw accessory status word. Its consumers test bits 2-4 for the headphone
//! state and bits 0-1 for the power-source class, so the pair is modeled as a
//! command plus one declared status input instead of a retained register.
//!
//! Selector 27 is the battery monitor's raw sample. X-T30 2.01 reads it once per
//! five-phase poll (application `0x0109ad3c`) and stores the word at
//! `0x0150b692`; no main-image function reads that back, so the sample is a
//! declared read-only input like the ADC and status words.
use crate::{rtc::Counter, spi::Input};
use fujimod_emulator_ip::socionext::gpio::SharedPins;
use fujimod_emulator_devices::checkpoint::Checkpoint;
use serde_json::{Value, json};
use std::collections::BTreeMap;
/// Accessory status command selector; the only observed payload is below.
const ACCESSORY_COMMAND_REGISTER: u8 = 23;
/// Accessory status word selector; read-only and declared by the profile.
const ACCESSORY_STATUS_REGISTER: u8 = 24;
/// Battery monitor raw sample selector; read-only and declared by the profile.
const BATTERY_SAMPLE_REGISTER: u8 = 27;
/// The one observed accessory status command: request a status sample.
const ACCESSORY_STATUS_REQUEST: u16 = 0x0400;
/// Direct stop/load/start law over the shared counter.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
struct Rtc(Counter);
impl Rtc {
    fn counter(&self, tick: u64) -> Result<u32, String> {
        self.0.counter(tick)
    }
    fn read(&self, register: u8, tick: u64) -> Result<u16, String> {
        Ok(match register {
            32 => {
                if self.0.running {
                    0x8000
                } else {
                    0
                }
            }
            34 => self.counter(tick)? as u16,
            35 => (self.counter(tick)? >> 16) as u16,
            _ => unreachable!(),
        })
    }
    fn write(&mut self, register: u8, value: u16, tick: u64) -> Result<(), String> {
        if register == 32 {
            if !matches!(value, 0 | 0x8000) {
                return Err("Unsupported power RTC control bits".into());
            }
            self.0.set_running(value != 0, tick)?;
        } else {
            if self.0.running {
                return Err("Power RTC counter write while running".into());
            }
            let seconds = if register == 34 {
                (self.0.seconds & 0xffff0000) | u32::from(value)
            } else {
                (self.0.seconds & 0xffff) | u32::from(value) << 16
            };
            self.0.load(seconds, tick);
        }
        Ok(())
    }
}
/// One explicitly declared uninterpreted external command output.
/// Acceptance supplies no register readback or device-state effect.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExactCommandOutput { pub bank: bool, pub register: u8, pub value: u16 }
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct AcceptedOutput { command: ExactCommandOutput, count: u64 }
pub struct PowerSerial {
    pins: SharedPins,
    port: usize,
    pin: usize,
    active_high: bool,
    idle: bool,
    /// Flat bank-zero declarations. Kept separate so every existing profile
    /// keeps its bank-blind meaning.
    inputs: BTreeMap<u8, u16>,
    /// Declarations for one explicit bank. A bank without an entry here still
    /// reads the flat map, so an undeclared bank stays bank-blind.
    bank_inputs: BTreeMap<(bool, u8), u16>,
    /// Opt-in to the eye-sensor commit word at register 38 bit 15, the same
    /// word X-T3 treats as the RTC alarm interrupt.
    eye_commit: bool,
    registers: BTreeMap<(bool, u8), u16>,
    edge: u64,
    phase: u8,
    header: u8,
    high: u8,
    response: u16,
    rtc: Rtc,
    command_output: Option<ExactCommandOutput>,
    accepted_output: Option<AcceptedOutput>,
}
impl PowerSerial {
    pub fn new(
        pins: SharedPins,
        port: usize,
        pin: usize,
        active_high: bool,
        idle: bool,
        inputs: BTreeMap<u8, u16>,
        rtc_ticks_per_second: u64,
        command_output: Option<ExactCommandOutput>,
    ) -> Result<Self, String> {
        if inputs.iter().any(|(k, v)| (3..=6).contains(k) && *v > 1023)
            || port >= 16
            || pin >= 16
            || inputs
                .keys()
                // 24: declared accessory status word
                // 27: declared battery monitor raw sample
                // 48..=55: initial contents of the retained profile registers
                .any(|k| !matches!(*k, 0..=6 | 16 | 21 | 24 | BATTERY_SAMPLE_REGISTER | 32 | 34 | 35 | 48..=55))
            || [0, 1, 2, 3, 32, 34, 35]
                .iter()
                .any(|n| !inputs.contains_key(n))
            || inputs.get(&32).is_some_and(|v| !matches!(*v, 0 | 0x8000))
            || !(1..=1_000_000_000).contains(&rtc_ticks_per_second)
        {
            return Err(
                "Power SPI needs explicit ADC/status inputs and a physical chip select".into(),
            );
        }
        if command_output.as_ref().is_some_and(|c| c.register >= 64
            || Self::writable(c.register) || inputs.contains_key(&c.register)
            || matches!(c.register, 23 | 24)) {
            return Err("Command output overlaps a register protocol or input".into());
        }
        let rtc = Rtc(Counter::new(
            u32::from(inputs[&34]) | u32::from(inputs[&35]) << 16,
            inputs[&32] != 0,
            rtc_ticks_per_second,
        )?);
        Ok(Self {
            command_output,
            accepted_output: None,
            pins,
            port,
            pin,
            active_high,
            idle,
            inputs,
            bank_inputs: BTreeMap::new(),
            eye_commit: false,
            registers: BTreeMap::new(),
            edge: 0,
            phase: 0,
            header: 0,
            high: 0,
            response: 0,
            rtc,
        })
    }
    /// Declare an input for an explicit bank. The eye-sensor status word is
    /// register 16 in bank one; the flat map keeps the bank-zero boot reason,
    /// so both meanings survive. Undeclared banks keep reading the flat map.
    pub fn with_bank_inputs(mut self, inputs: Vec<(bool, u8, u16)>) -> Result<Self, String> {
        let mut declared = BTreeMap::new();
        for (bank, register, value) in inputs {
            if !matches!(register, 0..=6 | 16 | 21 | 27)
                || ((3..=6).contains(&register) && value > 1023)
                || declared.insert((bank, register), value).is_some()
            {
                return Err("Power SPI bank input is not declarable".into());
            }
        }
        self.bank_inputs = declared;
        Ok(self)
    }
    /// Admit the eye-sensor commit word at register 38 bit 15. The X-T3
    /// meaning of the same bit is the unmodeled RTC alarm interrupt.
    pub fn with_eye_commit(mut self, enabled: bool) -> Self {
        self.eye_commit = enabled;
        self
    }
    fn declared_input(&self, bank: bool, register: u8) -> Option<u16> {
        self.bank_inputs
            .get(&(bank, register))
            .copied()
            .or_else(|| self.inputs.get(&register).copied())
    }
    fn output_header(&self, header: u8) -> bool {
        header & 1 != 0 && self.command_output.as_ref().is_some_and(|c|
            c.register == header >> 2 && c.bank == (header & 2 != 0))
    }
    fn accessory_status(&self) -> Option<u16> {
        self.inputs.get(&ACCESSORY_STATUS_REGISTER).copied()
    }
    fn admitted_header(&self, header: u8) -> bool {
        let register = header >> 2;
        let bank = header & 2 != 0;
        let status = self.accessory_status().is_some();
        if bank && (matches!(register, 32 | 34 | 35) || status && matches!(register, 23 | 24)) {
            return false;
        }
        if header & 1 != 0 {
            Self::writable(register)
                || status && register == ACCESSORY_COMMAND_REGISTER
                || self.output_header(header)
        } else {
            Self::writable(register) || self.declared_input(bank, register).is_some()
        }
    }
    fn writable(register: u8) -> bool {
        matches!(register, 2 | 13 | 14 | 18 | 20 | 32 | 34 | 35 | 37 | 38 | 48..=55)
    }
}
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct PowerState {
    #[serde(default)]
    command_output: Option<ExactCommandOutput>,
    #[serde(default)]
    accepted_output: Option<AcceptedOutput>,
    kind: String,
    port: usize,
    pin: usize,
    active_high: bool,
    idle: bool,
    inputs: BTreeMap<u8, u16>,
    /// Non-default bank declarations as triples: a JSON object cannot carry a
    /// `(bank, register)` key without spelling it.
    #[serde(default)]
    bank_inputs: Vec<(bool, u8, u16)>,
    #[serde(default)]
    eye_commit: bool,
    registers: Vec<(bool, u8, u16)>,
    phase: u8,
    header: u8,
    high: u8,
    response: u16,
    edge: u64,
    rtc: Rtc,
}

impl PowerSerial {
    fn capture(&self) -> PowerState {
        PowerState {
            kind: "power-registers".into(), port: self.port, pin: self.pin, active_high: self.active_high,
            idle: self.idle, inputs: self.inputs.clone(),
            bank_inputs: self.bank_inputs.iter().map(|(&(bank, register), &value)| (bank, register, value)).collect(),
            eye_commit: self.eye_commit,
            registers: self.registers.iter().map(|(&(bank, register), &value)| (bank, register, value)).collect(),
            command_output: self.command_output.clone(), accepted_output: self.accepted_output.clone(),
            phase: self.phase, header: self.header, high: self.high, response: self.response,
            edge: self.edge, rtc: self.rtc.clone(),
        }
    }

    fn validate(&self, state: &PowerState) -> Result<BTreeMap<(bool, u8), u16>, String> {
        let registers: BTreeMap<_, _> = state
            .registers
            .iter()
            .map(|(b, r, v)| ((*b, *r), *v))
            .collect();
        let bank_inputs: BTreeMap<_, _> = state
            .bank_inputs
            .iter()
            .map(|(b, r, v)| ((*b, *r), *v))
            .collect();
        if state.phase != 0 && (!self.admitted_header(state.header)
                || state.header & 1 != 0 && state.response != 0)
            || state.command_output != self.command_output
            || state.accepted_output.as_ref().is_some_and(|o| o.count == 0 || Some(&o.command) != self.command_output.as_ref())
            || state.kind != "power-registers"
            || state.port != self.port
            || state.pin != self.pin
            || state.active_high != self.active_high
            || state.idle != self.idle
            || state.inputs != self.inputs
            || state.bank_inputs.len() != bank_inputs.len()
            || bank_inputs != self.bank_inputs
            || state.eye_commit != self.eye_commit
            || state.rtc.0.ticks_per_second != self.rtc.0.ticks_per_second
            || state.phase > 2
            || registers.len() != state.registers.len()
            || registers
                .keys()
                .any(|(_, r)| !Self::writable(*r) || [32, 34, 35].contains(r))
        {
            return Err("Invalid power SPI checkpoint or physical input".into());
        }
        Ok(registers)
    }

    fn apply(&mut self, state: PowerState) -> Result<(), String> {
        let registers = self.validate(&state)?;
        self.accepted_output = state.accepted_output;
        self.registers = registers;
        self.phase = state.phase;
        self.header = state.header;
        self.high = state.high;
        self.response = state.response;
        self.edge = state.edge;
        self.rtc = state.rtc;
        Ok(())
    }
}

impl Input for PowerSerial {
    fn exchange(&mut self, bits: u32, tx: u16, tick: u64) -> Result<u16, String> {
        let (level, edge) = self.pins.lock().unwrap().output(self.port, self.pin);
        if edge != self.edge {
            self.phase = 0;
            self.edge = edge;
        }
        if level != self.active_high {
            return Ok(if self.idle {
                u16::MAX >> (16 - bits)
            } else {
                0
            });
        }
        if bits != 8 {
            return Err("Power SPI requires eight-bit serial words".into());
        }
        let tx = tx as u8;
        match self.phase {
            0 => {
                self.header = tx;
                let register = tx >> 2;
                let bank = tx & 2 != 0;
                if bank && matches!(register, 32 | 34 | 35) {
                    return Err("Power RTC second bank is not modeled".into());
                }
                // Selector 23 is admitted only alongside the declared status word,
                // because the pair has no meaning without a response.
                let status = self.accessory_status().is_some();
                if status && bank && matches!(register, 23 | 24) {
                    return Err("Accessory status pair supports bank zero only".into());
                }
                if tx & 1 != 0 {
                    if !Self::writable(register)
                        && !(status && register == ACCESSORY_COMMAND_REGISTER)
                        && !self.output_header(tx)
                    {
                        return Err(format!("Unsupported power SPI write register{register}"));
                    }
                    self.response = 0;
                } else if matches!(register, 32 | 34 | 35) {
                    self.response = self.rtc.read(register, tick)?;
                } else if Self::writable(register) {
                    self.response = self
                        .registers
                        .get(&(bank, register))
                        .copied()
                        .or_else(|| self.declared_input(bank, register))
                        .unwrap_or(0);
                } else if let Some(v) = self.declared_input(bank, register) {
                    // Declared read-only words: the accessory status, the battery
                    // sample and the ADC/status inputs. Nothing is retained.
                    self.response = v;
                } else {
                    return Err(format!("Unsupported power SPI read register{register}"));
                }
                self.phase = 1;
                Ok(0)
            }
            1 => {
                self.high = tx;
                self.phase = 2;
                Ok(self.response >> 8)
            }
            _ => {
                if self.header & 1 != 0 {
                    let register = self.header >> 2;
                    let value = u16::from(self.high) << 8 | u16::from(tx);
                    if self.output_header(self.header) {
                        let command = self.command_output.as_ref().unwrap();
                        if value != command.value { return Err("Unsupported command output payload".into()); }
                        let count = self.accepted_output.as_ref().map_or(0, |o| o.count)
                            .checked_add(1).ok_or("Command output count overflow")?;
                        self.accepted_output = Some(AcceptedOutput { command: command.clone(), count });
                    } else if register == ACCESSORY_COMMAND_REGISTER {
                        // Both observed callers send this one command and then read
                        // the status word. Nothing in the firmware retains a register
                        // value, ready bit, or completion latency for the command.
                        if self.accessory_status().is_none()
                            || self.header & 2 != 0
                            || value != ACCESSORY_STATUS_REQUEST
                        {
                            return Err("Unsupported power accessory status command".into());
                        }
                    } else if matches!(register, 32 | 34 | 35) {
                        self.rtc.write(register, value, tick)?;
                    } else {
                        if !Self::writable(register) {
                            return Err("Unsupported power SPI write completion".into());
                        }
                        if register == 38 && value & 0x8000 != 0 && !self.eye_commit {
                            return Err("Power RTC alarm interrupt is not modeled".into());
                        }
                        self.registers
                            .insert((self.header & 2 != 0, register), value);
                    }
                }
                self.phase = 0;
                Ok(self.response & 255)
            }
        }
    }
    fn save(&self) -> Value {
        json!({"kind":"power-registers","port":self.port,"pin":self.pin,"active_high":self.active_high,
        "idle":self.idle,"inputs":self.inputs,
        "bank_inputs":self.bank_inputs.iter().map(|(&(b,r),&v)|(b,r,v)).collect::<Vec<_>>(),
        "eye_commit":self.eye_commit,
        "registers":self.registers.iter().map(|(&(b,r),&v)|(b,r,v)).collect::<Vec<_>>(),
        "command_output":self.command_output,"accepted_output":self.accepted_output,
        "phase":self.phase,"header":self.header,"high":self.high,"response":self.response,"edge":self.edge,"rtc":self.rtc})
    }
    fn checkpoint(&self) -> Result<Checkpoint, String> {
        Checkpoint::encode("xp4-spi-power", &self.capture())
    }
    fn validate_checkpoint(&self, data: &Checkpoint) -> Result<(), String> {
        let state: PowerState = data.decode("xp4-spi-power")?;
        self.validate(&state).map(|_| ())
    }
    fn restore_checkpoint(&mut self, data: &Checkpoint) -> Result<(), String> {
        self.apply(data.decode("xp4-spi-power")?)
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        self.apply(serde_json::from_value(value.clone()).map_err(|error| error.to_string())?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_ip::socionext::gpio::{Gpio, Pins};
    const BASE: u64 = 0x10000;
    use fujimod_emulator_devices::{Cx, Device, Host, Record};
    struct Clock;
    impl Host for Clock {
        fn now(&self) -> u64 {
            0
        }
        fn schedule(&mut self, _: usize, _: u64, _: u64) {
            unreachable!()
        }
        fn set_irq(&mut self, _: u32, _: bool) {
            unreachable!()
        }
        fn raise_irq(&mut self, _: usize, _: u32) -> Result<(), String> {
            unreachable!()
        }
        fn ram_read(&self, _: u64, _: &mut [u8]) -> Result<(), String> {
            unreachable!()
        }
        fn ram_write(&mut self, _: u64, _: &[u8]) -> Result<(), String> {
            unreachable!()
        }
        fn record(&mut self, _: Record) {}
    }
    #[test]
    fn exact_output_is_opt_in_full_frame_only_and_has_no_register_effects() {
        let pins = Pins::new([0; 16]);
        let mut gpio = Gpio::new(BASE, pins.clone()).unwrap();
        gpio.write(&mut Cx::new(&mut Clock, 3, 0x1234, 0), BASE + 12 * 64, 4, (1 << 21) | (1 << 5));
        let policy = ExactCommandOutput { bank: false, register: 17, value: 0x1000 };
        let make = |policy| PowerSerial::new(pins.clone(), 12, 5, true, false,
            [(0,0),(1,0),(2,0),(3,0),(32,0),(34,0),(35,0)].into(), 54_000_000, policy).unwrap();
        assert!(make(None).exchange(8, 0x45, 0).is_err());
        let mut peer = make(Some(policy.clone()));
        let before = peer.save();
        assert!(peer.exchange(8, 0x44, 0).is_err()); // No readback.
        assert!(peer.exchange(8, 0x47, 0).is_err()); // Other bank.
        assert!(peer.exchange(16, 0x45, 0).is_err());
        peer.exchange(8, 0x45, 0).unwrap(); peer.exchange(8, 0x10, 1).unwrap();
        let partial = peer.save();
        assert!(peer.exchange(8, 1, 2).is_err());
        assert_eq!(peer.save()["accepted_output"], Value::Null);
        for field in ["inputs", "rtc", "registers"] { assert_eq!(peer.save()[field], before[field]); }
        peer.exchange(8, 0, 2).unwrap();
        assert_eq!(peer.save()["accepted_output"], json!({"command":policy,"count":1}));
        assert_eq!(peer.save()["registers"], json!([]));
        let mut resumed = make(Some(policy.clone())); resumed.restore(&partial).unwrap();
        resumed.exchange(8, 0, 2).unwrap(); assert_eq!(resumed.save(), peer.save());
        let good = peer.save();
        for changed in [None, Some(ExactCommandOutput { value: 7, ..policy.clone() })] {
            assert!(peer.restore(&make(changed).save()).is_err()); assert_eq!(peer.save(), good);
        }
        for count in [0, u64::MAX] {
            let mut edited = good.clone(); edited["accepted_output"]["count"] = json!(count);
            if count == 0 { assert!(peer.restore(&edited).is_err()); assert_eq!(peer.save(), good); }
            else {
                peer.restore(&edited).unwrap(); peer.exchange(8, 0x45, 0).unwrap(); peer.exchange(8, 0x10, 1).unwrap();
                let before = peer.save(); assert!(peer.exchange(8, 0, 2).is_err()); assert_eq!(peer.save(), before);
            }
        }
        let mut edited = good.clone(); edited["accepted_output"]["command"]["value"] = json!(7);
        let before = peer.save(); assert!(peer.restore(&edited).is_err()); assert_eq!(peer.save(), before);
        for register in [2, 18, 23, 24, 32, 38, 64] {
            assert!(PowerSerial::new(pins.clone(),12,5,true,false,
                [(0,0),(1,0),(2,0),(3,0),(32,0),(34,0),(35,0)].into(),54_000_000,
                Some(ExactCommandOutput { register, ..policy.clone() })).is_err());
        }
    }
    #[test]
    fn pending_output_restore_rechecks_header_and_write_response_atomically() {
        let pins=Pins::new([0;16]);let mut gpio=Gpio::new(BASE,pins.clone()).unwrap();
        gpio.write(&mut Cx::new(&mut Clock,0,0,0),BASE+12*64,4,(1<<21)|(1<<5));
        let policy=ExactCommandOutput {bank:false,register:17,value:4096};
        let make=|policy| PowerSerial::new(pins.clone(),12,5,true,false,
            [(0,0),(1,0),(2,0),(3,0),(32,0),(34,0),(35,0)].into(),54_000_000,policy).unwrap();
        let mut peer=make(Some(policy.clone()));
        peer.exchange(8,0x45,0).unwrap();peer.exchange(8,0x10,1).unwrap();
        let partial=peer.save();
        for phase in [1,2] {
            for header in [0x47,0x44,0xfd] {
                let mut changed=partial.clone();changed["phase"]=json!(phase);changed["header"]=json!(header);
                assert!(peer.restore(&changed).is_err());assert_eq!(peer.save(),partial);
            }
            let mut changed=partial.clone();changed["phase"]=json!(phase);changed["response"]=json!(1);
            assert!(peer.restore(&changed).is_err());assert_eq!(peer.save(),partial);
            let mut disabled=make(None);let before=disabled.save();
            let mut changed=partial.clone();changed["phase"]=json!(phase);changed["command_output"]=Value::Null;
            assert!(disabled.restore(&changed).is_err());assert_eq!(disabled.save(),before);
        }
        peer.exchange(8,0,2).unwrap();
        assert_eq!(peer.accepted_output.as_ref().unwrap().count,1);
        assert!(peer.registers.is_empty());assert_eq!(peer.inputs,make(Some(policy)).inputs);
        assert_eq!(peer.save()["rtc"],partial["rtc"]);
    }
    #[test]
    fn output_partial_frame_is_aborted_by_selection_and_repeat_count_is_exact() {
        for (core,pc) in [(0,0),(3,0x123456)] {
            let pins=Pins::new([0;16]); let mut gpio=Gpio::new(BASE,pins.clone()).unwrap();
            let select=|gpio: &mut Gpio, high: bool| gpio.write(&mut Cx::new(&mut Clock,core,pc,0),
                BASE+12*64,4,(1<<21)|if high {1<<5} else {0});
            select(&mut gpio,true);
            let mut peer=PowerSerial::new(pins,12,5,true,false,
                [(0,0),(1,0),(2,0),(3,0),(32,0),(34,0),(35,0)].into(),54_000_000,
                Some(ExactCommandOutput {bank:false,register:17,value:4096})).unwrap();
            peer.exchange(8,0x45,0).unwrap();peer.exchange(8,0x10,1).unwrap();
            select(&mut gpio,false);peer.exchange(8,0,2).unwrap();
            assert!(peer.accepted_output.is_none());select(&mut gpio,true);
            for count in 1..=2 {
                for byte in [0x45,0x10,0] { peer.exchange(8,byte,3).unwrap(); }
                assert_eq!(peer.accepted_output.as_ref().unwrap().count,count);
            }
            assert!(peer.registers.is_empty());assert_eq!(peer.rtc.counter(100).unwrap(),0);
            assert!(!peer.rtc.0.running);
        }
    }
    #[test]
    fn rtc_counter_stop_load_start_wrap_and_observation() {
        let mut rtc = Rtc(Counter::new(0, false, 10).unwrap());
        rtc.write(34, 0xfffe, 4).unwrap();
        rtc.write(35, 0xffff, 5).unwrap();
        rtc.write(32, 0x8000, 6).unwrap();
        assert_eq!(rtc.read(34, 15).unwrap(), 0xfffe);
        assert_eq!(rtc.read(34, 16).unwrap(), 0xffff);
        assert_eq!(rtc.counter(26).unwrap(), 0);
        let saved = serde_json::to_value(&rtc).unwrap();
        for _ in 0..10 {
            assert_eq!(rtc.counter(36).unwrap(), 1);
        }
        assert_eq!(serde_json::to_value(&rtc).unwrap(), saved);
        assert!(rtc.write(34, 12, 36).is_err());
        assert!(rtc.write(32, 1, 36).is_err());
        rtc.write(32, 0, 36).unwrap();
        assert_eq!(rtc.counter(100).unwrap(), 1);
    }
    #[test]
    fn checkpoint_preserves_partial_frame_running_rtc_and_gpio_connection() {
        let pins = Pins::new([0; 16]);
        let inputs = [(0, 0), (1, 0), (2, 0), (3, 512), (32, 0), (34, 0), (35, 0)].into();
        let mut source =
            PowerSerial::new(pins.clone(), 12, 5, true, false, inputs, 54_000_000, None).unwrap();
        let mut resumed = PowerSerial::new(
            pins.clone(),
            12,
            5,
            true,
            false,
            source.inputs.clone(),
            54_000_000,
            None,
        )
        .unwrap();
        let mut gpio = Gpio::new(BASE, pins).unwrap();
        gpio.write(
            &mut Cx::new(&mut Clock, 0, 0, 0),
            BASE + 12 * 64,
            4,
            (1 << 21) | (1 << 5),
        );
        source.rtc.write(32, 0x8000, 7).unwrap();
        assert_eq!(
            source.save()["rtc"],
            json!({"seconds":0,"anchor":7,"running":true,"ticks_per_second":54_000_000})
        );
        source.exchange(8, 18 * 4 + 1, 8).unwrap();
        source.exchange(8, 0x12, 9).unwrap();
        resumed.restore(&source.save()).unwrap();
        for byte in [0x34, 18 * 4, 0, 0] {
            assert_eq!(
                source.exchange(8, byte, 10).unwrap(),
                resumed.exchange(8, byte, 10).unwrap()
            );
        }
        assert_eq!(source.save(), resumed.save());
        assert_eq!(resumed.rtc.counter(54_000_007).unwrap(), 1);
        gpio.write(
            &mut Cx::new(&mut Clock, 0, 0, 0),
            BASE + 12 * 64,
            4,
            1 << 21,
        );
        assert_eq!(resumed.exchange(8, 0, 11).unwrap(), 0);
    }
    #[test]
    fn physical_selection_frames_latches_and_unknown_commands() {
        let pins = Pins::new([0; 16]);
        let mut gpio = Gpio::new(BASE, pins.clone()).unwrap();
        let mut peer = PowerSerial::new(
            pins,
            12,
            5,
            true,
            false,
            [(0, 0), (1, 0), (2, 0), (3, 512), (32, 0), (34, 0), (35, 0)].into(),
            54_000_000,
            None,
        )
        .unwrap();
        assert_eq!(peer.exchange(8, 12, 0).unwrap(), 0); // Unselected wire is low.
        let mut host = Clock;
        let mut cx = Cx::new(&mut host, 3, 0x765432, 0);
        gpio.write(&mut cx, BASE + 12 * 64, 4, (1 << 21) | (1 << 5));
        assert_eq!(
            [
                peer.exchange(8, 12, 0).unwrap(),
                peer.exchange(8, 0, 0).unwrap(),
                peer.exchange(8, 0, 0).unwrap()
            ],
            [0, 2, 0]
        );
        assert_eq!(
            [
                peer.exchange(8, 18 * 4 + 1, 0).unwrap(),
                peer.exchange(8, 0x12, 0).unwrap(),
                peer.exchange(8, 0x34, 0).unwrap()
            ],
            [0, 0, 0]
        );
        assert_eq!(
            [
                peer.exchange(8, 18 * 4, 0).unwrap(),
                peer.exchange(8, 0, 0).unwrap(),
                peer.exchange(8, 0, 0).unwrap()
            ],
            [0, 0x12, 0x34]
        );
        let saved = peer.save();
        assert_eq!(peer.save(), saved);
        peer.exchange(8, 12, 0).unwrap();
        gpio.write(&mut cx, BASE + 12 * 64, 4, 1 << 21);
        gpio.write(&mut cx, BASE + 12 * 64, 4, (1 << 21) | (1 << 5));
        assert_eq!(peer.exchange(8, 12, 0).unwrap(), 0);
        gpio.write(&mut cx, BASE + 12 * 64, 4, 1 << 21);
        gpio.write(&mut cx, BASE + 12 * 64, 4, (1 << 21) | (1 << 5));
        assert!(peer.exchange(8, 63 * 4, 0).is_err());
        peer.exchange(8, 38 * 4 + 1, 0).unwrap();
        peer.exchange(8, 0x80, 0).unwrap();
        assert!(peer.exchange(8, 0, 0).is_err());
        assert!(cx.refusal().is_none());
    }

    #[test]
    fn declared_battery_sample_is_read_only_and_unretained() {
        let pins = Pins::new([0; 16]);
        let mut gpio = Gpio::new(BASE, pins.clone()).unwrap();
        let make = |sample: Option<u16>| {
            let mut inputs: BTreeMap<u8, u16> =
                [(0, 0), (1, 0), (2, 0), (3, 512), (24, 0xa5), (32, 0), (34, 0), (35, 0)].into();
            if let Some(sample) = sample {
                inputs.insert(BATTERY_SAMPLE_REGISTER, sample);
            }
            PowerSerial::new(pins.clone(), 12, 5, true, false, inputs, 54_000_000, None).unwrap()
        };
        let mut host = Clock;
        let mut cx = Cx::new(&mut host, 3, 0, 0);
        gpio.write(&mut cx, BASE + 12 * 64, 4, (1 << 21) | (1 << 5));
        assert!(make(None).exchange(8, 0x6c, 0).is_err());
        for sample in [0u16, 0x1234, 0xffff] {
            let mut peer = make(Some(sample));
            let declared = peer.save();
            assert_eq!(peer.exchange(8, 0x6c, 0).unwrap(), 0);
            let partial = peer.save();
            assert_eq!(peer.exchange(8, 0x10, 1).unwrap(), sample >> 8);
            assert_eq!(peer.exchange(8, 0x34, 2).unwrap(), sample & 255);
            assert_eq!(peer.save()["registers"], json!([]));
            assert_eq!(peer.save()["inputs"], declared["inputs"]);
            let mut resumed = make(Some(sample));
            resumed.restore(&partial).unwrap();
            assert_eq!(resumed.exchange(8, 0x10, 1).unwrap(), sample >> 8);
            assert_eq!(resumed.exchange(8, 0x34, 2).unwrap(), sample & 255);
            assert_eq!(resumed.save(), peer.save());
            assert!(make(Some(sample)).exchange(8, 0x6d, 0).is_err());
            let mut absent = make(None);
            assert!(absent.exchange(8, 0x6c, 0).is_err());
            let before = absent.save();
            assert!(absent.restore(&declared).is_err());
            assert_eq!(absent.save(), before);
        }
    }

    #[test]
    fn accessory_status_command_is_exact_and_preserves_the_raw_word() {
        let pins = Pins::new([0; 16]);
        let mut gpio = Gpio::new(BASE, pins.clone()).unwrap();
        let make = |status: Option<u16>| {
            let mut inputs: BTreeMap<u8, u16> =
                [(0, 0), (1, 0), (2, 0), (3, 512), (32, 0), (34, 0), (35, 0)].into();
            if let Some(status) = status {
                inputs.insert(24, status);
            }
            PowerSerial::new(pins.clone(), 12, 5, true, false, inputs, 54_000_000, None).unwrap()
        };
        let mut host = Clock;
        let mut cx = Cx::new(&mut host, 3, 0, 0);
        gpio.write(&mut cx, BASE + 12 * 64, 4, (1 << 21) | (1 << 5));
        assert!(make(None).exchange(8, 0x5d, 0).is_err());
        assert!(make(None).exchange(8, 0x60, 0).is_err());
        for status in [0x00a5, 0xff0c] {
            let mut peer = make(Some(status));
            assert_eq!(peer.exchange(8, 0x5d, 0).unwrap(), 0);
            assert_eq!(peer.exchange(8, 4, 0).unwrap(), 0);
            let saved = peer.save();
            let mut restored = make(Some(status));
            restored.restore(&saved).unwrap();
            assert_eq!(restored.exchange(8, 0, 0).unwrap(), 0);
            assert_eq!(restored.save()["registers"], json!([]));
            assert_eq!(restored.exchange(8, 0x60, 0).unwrap(), 0);
            assert_eq!(restored.exchange(8, 0, 0).unwrap(), status >> 8);
            assert_eq!(restored.exchange(8, 0, 0).unwrap(), status & 255);
            assert!(make(Some(status ^ 1)).restore(&saved).is_err());
        }
        // The two observed consumers read disjoint bitfields of one raw word.
        let observed = 0x00a5u16;
        assert_ne!(observed & 0x1c, 0x0c);
        assert!(matches!(observed & 3, 1 | 2));
        let mut repeat = make(Some(observed));
        for _ in 0..2 {
            for byte in [0x5d, 0x04, 0x00] {
                repeat.exchange(8, byte, 0).unwrap();
            }
        }
        assert_eq!(repeat.save()["registers"], json!([]));
        assert_eq!(repeat.save()["inputs"], make(Some(observed)).save()["inputs"]);
        let mut wrong = make(Some(0xa5));
        wrong.exchange(8, 0x5d, 0).unwrap();
        wrong.exchange(8, 4, 0).unwrap();
        assert!(wrong.exchange(8, 1, 0).is_err());
        assert!(make(Some(0xa5)).exchange(8, 0x5f, 0).is_err());
        assert!(make(Some(0xa5)).exchange(8, 0x62, 0).is_err());
        let mut cancelled = make(Some(0xa5));
        cancelled.exchange(8, 0x5d, 0).unwrap();
        gpio.write(&mut cx, BASE + 12 * 64, 4, 1 << 21);
        gpio.write(&mut cx, BASE + 12 * 64, 4, (1 << 21) | (1 << 5));
        assert_eq!(cancelled.exchange(8, 0x60, 0).unwrap(), 0);
        assert_eq!(cancelled.exchange(8, 0, 0).unwrap(), 0);
        assert_eq!(cancelled.exchange(8, 0, 0).unwrap(), 0xa5);
    }

    #[test]
    fn bank_declared_inputs_keep_eye_status_apart_from_the_boot_reason() {
        let pins = Pins::new([0; 16]);
        let mut gpio = Gpio::new(BASE, pins.clone()).unwrap();
        gpio.write(&mut Cx::new(&mut Clock, 0, 0, 0), BASE + 12 * 64, 4, (1 << 21) | (1 << 5));
        // Bank-zero register 16 is the X-T3 cold-boot reason; bank one register
        // 16 is the X-T30 eye status word.
        let make = || PowerSerial::new(
            pins.clone(), 12, 5, true, false,
            [(0, 0), (1, 0), (2, 0), (3, 0), (16, 4), (32, 0), (34, 0), (35, 0)].into(),
            54_000_000, None,
        ).unwrap().with_bank_inputs(vec![(true, 16, 2)]).unwrap();
        let mut peer = make();
        assert_eq!([
            peer.exchange(8, 16 << 2, 0).unwrap(),
            peer.exchange(8, 0, 0).unwrap(),
            peer.exchange(8, 0, 0).unwrap(),
        ], [0, 0, 4]);
        assert_eq!([
            peer.exchange(8, 16 << 2 | 2, 0).unwrap(),
            peer.exchange(8, 0, 0).unwrap(),
            peer.exchange(8, 0, 0).unwrap(),
        ], [0, 0, 2]);
        // Undeclared banks keep the flat bank-blind value.
        assert_eq!([
            peer.exchange(8, 0, 0).unwrap(),
            peer.exchange(8, 0, 0).unwrap(),
            peer.exchange(8, 0, 0).unwrap(),
        ], [0, 0, 0]);
        assert_eq!([
            peer.exchange(8, 2, 0).unwrap(),
            peer.exchange(8, 0, 0).unwrap(),
            peer.exchange(8, 0, 0).unwrap(),
        ], [0, 0, 0]);
        assert!(peer.exchange(8, 21 << 2, 0).is_err());
        let saved = peer.save();
        assert_eq!(saved["bank_inputs"], json!([[true, 16, 2]]));
        let mut restored = make();
        restored.restore(&saved).unwrap();
        assert_eq!(restored.save(), saved);
        assert!(make().with_bank_inputs(vec![(true, 32, 0)]).is_err());
        assert!(make().with_bank_inputs(vec![(true, 16, 1), (true, 16, 2)]).is_err());
        assert!(make().with_bank_inputs(vec![(true, 3, 1024)]).is_err());
    }

    #[test]
    fn eye_commit_admits_register_38_bit_15_only_behind_its_flag() {
        let pins = Pins::new([0; 16]);
        let mut gpio = Gpio::new(BASE, pins.clone()).unwrap();
        gpio.write(&mut Cx::new(&mut Clock, 0, 0, 0), BASE + 12 * 64, 4, (1 << 21) | (1 << 5));
        let make = || PowerSerial::new(
            pins.clone(), 12, 5, true, false,
            [(0, 0), (1, 0), (2, 0), (3, 0), (32, 0), (34, 0), (35, 0)].into(),
            54_000_000, None,
        ).unwrap();
        let commit = 38 << 2 | 1;
        let mut alarm = make();
        alarm.exchange(8, commit, 0).unwrap();
        alarm.exchange(8, 0x80, 0).unwrap();
        assert!(alarm.exchange(8, 0, 0).is_err());
        let mut eye = make().with_eye_commit(true);
        assert_eq!(eye.exchange(8, commit, 0).unwrap(), 0);
        eye.exchange(8, 0x80, 0).unwrap();
        assert_eq!(eye.exchange(8, 0, 0).unwrap(), 0);
        assert_eq!(eye.save()["registers"], json!([[false, 38, 0x8000]]));
        assert_eq!([
            eye.exchange(8, 38 << 2, 0).unwrap(),
            eye.exchange(8, 0, 0).unwrap(),
            eye.exchange(8, 0, 0).unwrap(),
        ], [0, 0x80, 0]);
        let saved = eye.save();
        let mut resumed = make().with_eye_commit(true);
        resumed.restore(&saved).unwrap();
        assert_eq!(resumed.save(), saved);
        assert!(make().restore(&saved).is_err());
    }
}
