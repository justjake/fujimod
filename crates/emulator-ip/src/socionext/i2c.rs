//! Socionext byte-oriented I2C master (eight byte-wide registers at a four-byte
//! stride; the bit names follow Linux's `i2c-synquacer` driver), in master mode
//! only. The attached targets are a separate [`Bus`].
//!
//! | Offset | Register | Model |
//! | --- | --- | --- |
//! | `0x00` | BSR, bus status (read only) | BB 7, LRB 4, TRX 3, FBT 0 |
//! | `0x04` | BCR, bus control | INT 0, INTE 1, ACK 3, MSS 4, SCC 5, BEIE 6 |
//! | `0x08` | CCR, clock control | EN 5 gates START; other bits are kept |
//! | `0x10` | DAR, data | staged transmit byte, last received byte |
//! | `0x14`, `0x18` | CSR, FSR, clock dividers | kept, no timing effect |
//! | `0x1c` | BC2R, line control | SCL/SDA low drive 0/1; line levels 4/5 |
//!
//! Setting MSS from idle sends a START and the staged DAR byte as the address;
//! SCC with MSS sends a repeated START and the staged address. With a byte
//! complete (INT set), writing BCR with MSS kept and INT clear moves the next
//! byte: the staged DAR byte while transmitting, or one received byte,
//! acknowledged when ACK is set, while receiving. Each byte completes after the
//! declared `byte_ns`, a functional latency rather than a bus rate derived
//! from CCR/CSR/FSR. Its completion sets INT and the status of that byte: BB,
//! FBT for an address byte, TRX while transmitting and LRB for the
//! acknowledge bit seen on the wire (a target's NACK of an address, or the
//! master's NACK of a received byte). The interrupt line follows INT and INTE.
//! Clearing MSS sends a STOP, which takes effect when written.
//!
//! An address with no attached target is not acknowledged. Targets acknowledge
//! every data byte; a write or read the target does not model refuses. Slave
//! mode, the slave address register (`0x0c`), general calls, arbitration loss,
//! bus errors and clock stretching are not modeled, and the commands that
//! would need them refuse, as does any command while a byte is in flight.
use fujimod_emulator_devices::parts::i2c::Bus;
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const BSR: u64 = 0x00;
const BCR: u64 = 0x04;
const CCR: u64 = 0x08;
const ADR: u64 = 0x0c;
const DAR: u64 = 0x10;
const CSR: u64 = 0x14;
const FSR: u64 = 0x18;
const BC2R: u64 = 0x1c;

const BSR_FBT: u8 = 1;
const BSR_TRX: u8 = 8;
const BSR_LRB: u8 = 0x10;
const BSR_BB: u8 = 0x80;

const BCR_INT: u8 = 1;
const BCR_INTE: u8 = 2;
const BCR_GCAA: u8 = 4;
const BCR_ACK: u8 = 8;
const BCR_MSS: u8 = 0x10;
const BCR_SCC: u8 = 0x20;
const BCR_BER: u8 = 0x80;

const CCR_EN: u8 = 0x20;
const CCR_RESERVED: u8 = 0x80;

const BC2R_SCLL: u8 = 1;
const BC2R_SDAL: u8 = 2;
const BC2R_SCLS: u8 = 0x10;
const BC2R_SDAS: u8 = 0x20;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub base: u64,
    pub irq: u32,
    /// Virtual time from a byte command to its completion.
    pub byte_ns: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Phase {
    Idle,
    /// After a START, until the address byte completes.
    Addressing,
    Transmitting,
    Receiving,
    /// The target did not acknowledge its address; only a STOP or repeated
    /// START follows.
    NotAcknowledged,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Operation {
    Address(u8),
    Write(u8),
    Read { ack: bool },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    bsr: u8,
    /// BCR as written, without INT.
    bcr: u8,
    int: bool,
    ccr: u8,
    csr: u8,
    fsr: u8,
    dar: u8,
    /// DAR holds a byte written since the last byte command.
    staged: bool,
    /// DAR holds a received byte not yet read.
    received: bool,
    low_drives: u8,
    phase: Phase,
    pending: Option<Operation>,
    generation: u64,
}

pub struct Controller<B: Bus> {
    options: Options,
    bus: B,
    state: State,
}

impl<B: Bus> Controller<B> {
    pub fn new(options: Options, bus: B) -> Result<Self, String> {
        if options.base & 0xfff != 0 || options.byte_ns == 0 {
            return Err("I2C controller needs a 4 KiB aligned base and a positive byte_ns".into());
        }
        Ok(Self {
            options,
            bus,
            state: State {
                bsr: 0,
                bcr: 0,
                int: false,
                ccr: 0,
                csr: 0,
                fsr: 0,
                dar: 0,
                staged: false,
                received: false,
                low_drives: 0,
                phase: Phase::Idle,
                pending: None,
                generation: 0,
            },
        })
    }
    pub fn bus(&self) -> &B {
        &self.bus
    }
    fn offset(&self, address: u64, size: u8) -> Result<u64, String> {
        let offset = address.wrapping_sub(self.options.base);
        if size != 1 || offset > BC2R || offset & 3 != 0 {
            return Err(format!(
                "I2C controller access {size} bytes at offset {offset:#x} is unmodeled"
            ));
        }
        if offset == ADR {
            return Err("I2C slave address register is unmodeled".into());
        }
        Ok(offset)
    }
    fn line(&self) -> bool {
        self.state.int && self.state.bcr & BCR_INTE != 0
    }
    fn control(&mut self, cx: &mut Cx<'_>, value: u8) -> Result<(), String> {
        if value & (BCR_INT | BCR_GCAA | BCR_BER) != 0 {
            return Err(format!("I2C control {value:#04x} sets status or general-call bits"));
        }
        if self.state.pending.is_some() {
            return Err("I2C control written while a byte is in flight".into());
        }
        let completed = self.state.int;
        let s = &mut self.state;
        if value & BCR_MSS == 0 {
            if value & (BCR_ACK | BCR_SCC) != 0 {
                return Err(format!("I2C control {value:#04x} without MSS is unmodeled"));
            }
            s.bcr = value;
            s.int = false;
            if s.phase != Phase::Idle {
                s.phase = Phase::Idle;
                s.bsr = 0;
                s.received = false;
                self.bus.stop();
            }
            return Ok(());
        }
        let operation = if s.phase == Phase::Idle || value & BCR_SCC != 0 {
            if s.ccr & CCR_EN == 0 || s.low_drives != 0 {
                return Err("I2C START while the controller is disabled or driving a line".into());
            }
            if s.phase != Phase::Idle && !completed {
                return Err("I2C repeated START before the last byte completed".into());
            }
            if !s.staged || value & BCR_ACK != 0 {
                return Err("I2C START needs a staged address byte and ACK clear".into());
            }
            Operation::Address(s.dar)
        } else {
            if !completed {
                return Err("I2C byte command before the last byte completed".into());
            }
            match s.phase {
                Phase::Transmitting if s.staged && value & BCR_ACK == 0 => Operation::Write(s.dar),
                Phase::Receiving if !s.staged && !s.received => Operation::Read {
                    ack: value & BCR_ACK != 0,
                },
                phase => {
                    return Err(format!(
                        "I2C control {value:#04x} in phase {phase:?} is unmodeled"
                    ));
                }
            }
        };
        s.bcr = value;
        s.int = false;
        s.staged = false;
        s.bsr |= BSR_BB;
        if matches!(operation, Operation::Address(_)) {
            s.phase = Phase::Addressing;
        }
        s.pending = Some(operation);
        s.generation += 1;
        cx.schedule(cx.now() + self.options.byte_ns, s.generation);
        Ok(())
    }
    fn write_register(&mut self, cx: &mut Cx<'_>, offset: u64, value: u8) -> Result<(), String> {
        let active = self.state.phase != Phase::Idle;
        match offset {
            BSR => return Err("I2C bus status is read only".into()),
            BCR => self.control(cx, value)?,
            CCR => {
                if value & CCR_RESERVED != 0 {
                    return Err("I2C clock control reserved bit".into());
                }
                // A driver may rewrite the unchanged value around a STOP.
                if active && value != self.state.ccr {
                    return Err("I2C clock control changed during a transfer".into());
                }
                self.state.ccr = value;
            }
            DAR => {
                if self.state.pending.is_some() || self.state.received {
                    return Err("I2C data written while a byte is in flight or unread".into());
                }
                self.state.dar = value;
                self.state.staged = true;
            }
            CSR | FSR => {
                if active {
                    return Err("I2C clock divider changed during a transfer".into());
                }
                if offset == CSR {
                    self.state.csr = value;
                } else {
                    self.state.fsr = value;
                }
            }
            BC2R => {
                if value & !(BC2R_SCLL | BC2R_SDAL) != 0 || active {
                    return Err("I2C line drive outside idle bus recovery".into());
                }
                self.state.low_drives = value;
            }
            _ => unreachable!(),
        }
        cx.set_irq(self.options.irq, self.line());
        Ok(())
    }
    fn complete(&mut self, cx: &mut Cx<'_>, operation: Operation) -> Result<(), String> {
        let s = &mut self.state;
        match operation {
            Operation::Address(byte) => {
                let read = byte & 1 != 0;
                let ack = self.bus.start(u16::from(byte >> 1), read);
                s.phase = match (ack, read) {
                    (false, _) => Phase::NotAcknowledged,
                    (true, false) => Phase::Transmitting,
                    (true, true) => Phase::Receiving,
                };
                s.bsr = BSR_BB
                    | BSR_FBT
                    | if read { 0 } else { BSR_TRX }
                    | if ack { 0 } else { BSR_LRB };
            }
            Operation::Write(byte) => {
                self.bus.write(byte)?;
                s.bsr = BSR_BB | BSR_TRX;
            }
            Operation::Read { ack } => {
                s.dar = self.bus.read()?;
                s.received = true;
                s.bsr = BSR_BB | if ack { 0 } else { BSR_LRB };
            }
        }
        s.int = true;
        cx.set_irq(self.options.irq, self.line());
        Ok(())
    }
}

impl<B: Bus> Device for Controller<B> {
    fn name(&self) -> &'static str {
        "i2c-controller"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        let offset = match self.offset(address, size) {
            Ok(offset) => offset,
            Err(reason) => {
                cx.unsupported(reason);
                return 0;
            }
        };
        let s = &mut self.state;
        let value = match offset {
            BSR => s.bsr,
            BCR => s.bcr | if s.int { BCR_INT } else { 0 },
            CCR => s.ccr,
            DAR => {
                if s.phase == Phase::Receiving {
                    if !s.received {
                        cx.unsupported("I2C receive data read before a byte arrived");
                        return 0;
                    }
                    s.received = false;
                }
                s.dar
            }
            CSR => s.csr,
            FSR => s.fsr,
            BC2R => {
                s.low_drives
                    | if s.low_drives & BC2R_SCLL == 0 { BC2R_SCLS } else { 0 }
                    | if s.low_drives & BC2R_SDAL == 0 { BC2R_SDAS } else { 0 }
            }
            _ => unreachable!(),
        };
        u64::from(value)
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let result = self.offset(address, size).and_then(|offset| {
            u8::try_from(value)
                .map_err(|_| "I2C register value exceeds a byte".to_owned())
                .and_then(|value| self.write_register(cx, offset, value))
        });
        if let Err(reason) = result {
            cx.unsupported(reason);
        }
    }
    fn pollable(&self, address: u64, size: u8) -> bool {
        matches!(self.offset(address, size), Ok(BSR | BCR | BC2R))
    }
    fn event(&mut self, cx: &mut Cx<'_>, token: u64) {
        if token != self.state.generation {
            return;
        }
        let Some(operation) = self.state.pending.take() else {
            return;
        };
        if let Err(reason) = self.complete(cx, operation) {
            cx.unsupported(reason);
        }
    }
    fn save(&self) -> Value {
        serde_json::json!({ "controller": self.state, "bus": self.bus.save() })
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let state: State = serde_json::from_value(value["controller"].clone())
            .map_err(|e| format!("invalid I2C controller checkpoint: {e}"))?;
        if state.low_drives & !(BC2R_SCLL | BC2R_SDAL) != 0
            || state.ccr & CCR_RESERVED != 0
            || state.bcr & (BCR_INT | BCR_GCAA | BCR_BER) != 0
            || (state.phase == Phase::Idle) != (state.bsr & BSR_BB == 0)
            || state.pending.is_some() && state.int
        {
            return Err("invalid I2C controller checkpoint state".into());
        }
        self.bus.restore(&value["bus"])?;
        self.state = state;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
