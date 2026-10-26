//! The EXR II boot serial register blocks: the SPI0 byte shifter and UART/DMA
//! with physical GPIO selection.
//! Uploaded UART bytes are opaque transport; no recipient effect is inferred.
pub mod construct;
use fujimod_emulator_exr2_serial::{dma::uart::PeripheralDma, uart};
use fujimod_emulator_devices::{Cx, Device, Record, Value, WordMap, exr2::gpio::Pads, graph::Mapping, parts::pins::Pins};
use fujimod_emulator_parts_io::spi::{Endpoint, FixedInput, FixedOptions, Shift};
use fujimod_emulator_exr2_plan::serial::Options;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
const SPI: u64 = 0xfff84000;
const UART: u64 = 0xfff46080;
const DMA: u64 = 0xfff02080;
pub struct Wiring {
    pub gpio: Pads,
}
#[derive(Clone, Default, Serialize, Deserialize)]
struct Shifter {
    busy: bool,
    rx: Option<u8>,
    started: u64,
    completed: u64,
    consumed: u64,
}
#[derive(Clone, Serialize, Deserialize)]
struct ByteTransfer {
    due: u64,
    order: u64,
    command: u32,
    transport_only: bool,
}
#[derive(Clone, Serialize, Deserialize)]
struct Program {
    due: u64,
    order: u64,
    generation: u64,
    bytes: Vec<u8>,
}
pub(crate) struct SerialState {
    options: Options,
    gpio: Pads,
    input: Option<FixedInput>,
    registers: WordMap,
    shifter: Shifter,
    dma: PeripheralDma,
    byte: Option<ByteTransfer>,
    program: Option<Program>,
    order: u64,
    transmitted: Vec<Vec<u8>>,
}
impl SerialState {
    fn new(options: Options, wiring: Wiring) -> Result<Self, String> {
        if options.transfer_ticks == 0 || !(1..=0x20000000).contains(&options.max_program_bytes) {
            return Err("power peripherals require bounded DMA and positive serial timing".into());
        }
        let input = if let Some(input) = options.spi0 {
            input.selection.pin.validate()?;
            input.presence.pin.validate()?;
            if input.presence.pin.port >= 16 || input.required[0] & 1 != 0 || input.required[2] != 0
            {
                return Err(
                    "fixed SPI0 input requires a physical input pad and polled transport".into(),
                );
            }
            Some(FixedInput::new(FixedOptions { bits: 8, word: 0 })?)
        } else {
            None
        };
        Ok(Self {
            options,
            gpio: wiring.gpio,
            input,
            registers: WordMap::default(),
            shifter: Shifter::default(),
            dma: PeripheralDma::default(),
            byte: None,
            program: None,
            order: 0,
            transmitted: vec![],
        })
    }
    fn reg(&self, address: u64) -> u32 {
        self.registers.get_or_zero(address as u32)
    }
    fn put(&mut self, address: u64, value: u32) {
        self.registers.insert(address as u32, value);
    }
    fn order(&mut self) -> Result<u64, String> {
        let order = self.order;
        self.order = self
            .order
            .checked_add(1)
            .ok_or("serial transaction order overflow")?;
        Ok(order)
    }
    fn offset(address: u64, size: u8) -> Option<(u8, u32)> {
        if size != 4 || address & 3 != 0 {
            return None;
        }
        if (SPI..SPI + 0x20).contains(&address) {
            return Some((0, (address - SPI) as u32));
        }
        if address >= UART && matches!(address - UART, 0 | 4 | 12) {
            return Some((1, (address - UART) as u32));
        }
        if address >= DMA
            && matches!(
                address - DMA,
                0 | 4 | 8 | 12 | 16 | 20 | 0x24 | 0x28 | 0x2c | 0x30 | 0x34
            )
        {
            return Some((2, (address - DMA) as u32));
        }
        matches!(address, 0xfff98500 | 0xfff98504).then_some((3, 0))
    }
    fn admission(&self, command: u32) -> Result<(), String> {
        let input = self
            .options
            .spi0
            .as_ref()
            .ok_or("SPI0 input is not connected")?;
        if self.shifter.busy || self.shifter.rx.is_some() {
            return Err("SPI0 shift while busy or receive FIFO unread".into());
        }
        if [self.reg(SPI) & !1, self.reg(SPI + 4), self.reg(SPI + 8)] != input.required
            || self.reg(SPI + 12) & 0xffff0000 != 0
            || command > 255
        {
            return Err("unsupported SPI0 byte transport configuration".into());
        }
        if self
            .gpio
            .output_level(input.selection.pin)?
            .ok_or("GPIO output has no declared or guest-written level")?
            != input.selection.high
        {
            return Err("SPI0 endpoint requires its physical chip select".into());
        }
        if self.gpio.input_level(input.presence.pin)? != input.presence.high {
            return Err("SPI0 fixed input contradicts its physical presence pad".into());
        }
        self.input.as_ref().unwrap().validate(Shift {
            word: command as u16,
            bits: 8,
            receive: true,
        })
    }
    fn start_byte(
        &mut self,
        cx: &mut Cx<'_>,
        command: u32,
        transport_only: bool,
    ) -> Result<(), String> {
        if !transport_only {
            self.admission(command)?;
        }
        let due = cx
            .now()
            .checked_add(self.options.transfer_ticks)
            .ok_or("SPI0 deadline overflow")?;
        let order = self.order()?;
        self.put(SPI + 0x1c, command);
        self.shifter.busy = true;
        self.shifter.started += 1;
        self.byte = Some(ByteTransfer {
            due,
            order,
            command,
            transport_only,
        });
        cx.record(Record::new(
            "SPI0 byte shift start",
            [
                ("command", Value::hex(command.into())),
                ("due", Value::int(due)),
            ],
        ));
        Ok(())
    }
    fn start_program(&mut self, cx: &mut Cx<'_>) -> Result<(), String> {
        self.dma.preflight().map_err(|e| e.0)?;
        let count = self.reg(DMA + 8) as usize;
        if count == 0
            || count > self.options.max_program_bytes
            || self.reg(DMA + 0x2c) != 0x0022242a
            || self.reg(DMA + 4) != UART as u32 + 4
            || (self.reg(0xfff98500) >> 16) & 31 != 0x12
            || [12, 16, 20, 0x30, 0x34]
                .iter()
                .any(|&o| self.reg(DMA + o) != 0)
        {
            return Err("unsupported loader UART DMA configuration, route or extent".into());
        }
        let due = cx
            .now()
            .checked_add((count as u64).max(4))
            .ok_or("UART DMA deadline overflow")?;
        let bytes = cx.ram_bytes(self.reg(DMA) as u64, count)?;
        let order = self.order()?;
        let generation = self.dma.start(&mut self.registers);
        self.program = Some(Program {
            due,
            order,
            generation,
            bytes,
        });
        cx.record(Record::new(
            "Boot serial DMA start",
            [
                ("bytes", Value::int(count as u64)),
                ("due", Value::int(due)),
            ],
        ));
        Ok(())
    }
    fn load(
        &mut self,
        cx: &mut Cx<'_>,
        kind: u8,
        offset: u32,
        address: u64,
    ) -> Result<u32, String> {
        let value = self.reg(address);
        match (kind, offset) {
            (0, 0) => Ok((value & !1) | u32::from(self.shifter.busy)),
            (0, 0x10) => Ok((value & !31) | u32::from(self.shifter.rx.is_some())),
            (0, 0x14) => Ok(value & !31),
            (0, 0x18) => {
                let value = self
                    .shifter
                    .rx
                    .take()
                    .ok_or("SPI0 receive data has no completed exchange")?;
                self.shifter.consumed += 1;
                cx.record(Record::new(
                    "SPI0 receive consumed",
                    [("value", Value::int(value))],
                ));
                Ok(value.into())
            }
            (1, o) => uart::read(o, value).map_err(|e| e.0),
            _ => Ok(value),
        }
    }
    fn store(
        &mut self,
        cx: &mut Cx<'_>,
        kind: u8,
        offset: u32,
        address: u64,
        value: u32,
    ) -> Result<(), String> {
        match (kind, offset) {
            (0, 0x18) => Err("SPI0 receive data is read-only".into()),
            (0, 0x1c) => self.start_byte(cx, value, false),
            (0, 0xc) => {
                self.put(
                    address,
                    (value & 0xffff0000) | (self.reg(address) & 0xffff & !(value & 0xffff)),
                );
                Ok(())
            }
            (0, 8) => {
                self.put(address, value);
                if value & 0x10000000 != 0 {
                    self.byte = None;
                    self.shifter.busy = false;
                    self.shifter.rx = None;
                    self.put(SPI + 12, self.reg(SPI + 12) & !0x300);
                }
                Ok(())
            }
            (1, o) => uart::write(o, value, &mut self.registers).map_err(|e| e.0),
            (2, 0x24) => Err("loader serial DMA completion status is read-only".into()),
            (2, 0x28) => {
                self.put(address, value);
                if self
                    .dma
                    .control(value, &mut self.registers)
                    .map_err(|e| e.0)?
                {
                    self.start_program(cx)
                } else {
                    self.program = None;
                    Ok(())
                }
            }
            _ => {
                self.put(address, value);
                Ok(())
            }
        }
    }
    fn complete_byte(&mut self, cx: &mut Cx<'_>) -> Result<(), String> {
        let pending = self.byte.take().unwrap();
        let reply = if pending.transport_only {
            0
        } else {
            self.input
                .as_mut()
                .ok_or("SPI0 input disconnected during transfer")?
                .transfer(
                    cx,
                    Shift {
                        word: pending.command as u16,
                        bits: 8,
                        receive: true,
                    },
                    pending.due,
                )? as u8
        };
        self.shifter.busy = false;
        self.shifter.rx = Some(reply);
        self.shifter.completed += 1;
        self.put(SPI + 12, self.reg(SPI + 12) | 0x300);
        if pending.transport_only {
            let fields = vec![
                ("bus_kind", Value::str("spi")),
                ("bus", Value::hex(SPI.into())),
                ("chip", Value::None),
                ("configuration", Value::hex(self.reg(SPI + 4).into())),
                ("tx", Value::Bytes(pending.command.to_le_bytes().to_vec())),
            ];
            let mut begin = fields.clone();
            begin.extend([
                ("port", Value::hex((SPI + 0x18).into())),
                ("unit", Value::int(4)),
            ]);
            cx.record(Record::new("Wire reply begin", begin));
            let mut transaction = fields;
            transaction.extend([
                ("rx", Value::Bytes(vec![0; 4])),
                ("unsupported", Value::Bool(true)),
            ]);
            cx.record(Record::new("Wire transaction", transaction));
        }
        cx.record(Record::new(
            "SPI0 byte shift complete",
            [
                ("reply", Value::int(reply)),
                ("tick", Value::int(pending.due)),
            ],
        ));
        Ok(())
    }
}
impl SerialState {
    fn read(&mut self, cx: &mut Cx<'_>, block: Block, address: u64, size: u8) -> u64 {
        let Some((kind, offset)) =
            Self::offset(address, size).filter(|(kind, _)| *kind == block.index())
        else {
            cx.unmodeled();
            return 0;
        };
        match self.load(cx, kind, offset, address) {
            Ok(value) => value.into(),
            Err(error) => {
                cx.fail(error);
                0
            }
        }
    }
    fn write(&mut self, cx: &mut Cx<'_>, block: Block, address: u64, size: u8, value: u64) {
        let Some((kind, offset)) =
            Self::offset(address, size).filter(|(kind, _)| *kind == block.index())
        else {
            cx.unmodeled();
            return;
        };
        let Ok(value) = u32::try_from(value) else {
            cx.unmodeled();
            return;
        };
        if let Err(error) = self.store(cx, kind, offset, address, value) {
            cx.fail(error);
        }
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        loop {
            let byte = self
                .byte
                .as_ref()
                .map(|pending| (pending.due, pending.order));
            let program = self
                .program
                .as_ref()
                .map(|pending| (pending.due, pending.order));
            let choose_byte = match (byte, program) {
                (Some(byte), Some(program)) => byte <= program,
                (Some(_), None) => true,
                _ => false,
            };
            let next = if choose_byte { byte } else { program };
            if next.is_none_or(|(due, _)| due > cx.now()) {
                return;
            }
            if choose_byte {
                if let Err(error) = self.complete_byte(cx) {
                    cx.fail(error);
                    return;
                }
            } else {
                let pending = self.program.take().unwrap();
                if self.dma.complete(pending.generation, &mut self.registers) {
                    cx.record(Record::new(
                        "Boot serial DMA complete",
                        [
                            ("bytes", Value::int(pending.bytes.len() as u64)),
                            ("tick", Value::int(pending.due)),
                        ],
                    ));
                    self.transmitted.push(pending.bytes);
                }
            }
        }
    }
    fn next_deadline(&self) -> Option<u64> {
        self.byte
            .as_ref()
            .map(|pending| pending.due)
            .into_iter()
            .chain(self.program.as_ref().map(|pending| pending.due))
            .min()
    }
    fn save(&self) -> serde_json::Value {
        serde_json::json!({"options":self.options,"registers":self.registers,"shifter":self.shifter,
            "dma":self.dma,"byte":self.byte,"program":self.program,"order":self.order,"transmitted":self.transmitted})
    }
    fn snapshot(&self) -> SerialSnapshot {
        SerialSnapshot {
            registers: self.registers.clone(),
            shifter: self.shifter.clone(),
            dma: self.dma.clone(),
            byte: self.byte.clone(),
            program: self.program.clone(),
            order: self.order,
            transmitted: self.transmitted.clone(),
        }
    }
    fn restore(&mut self, snapshot: SerialSnapshot) {
        self.registers = snapshot.registers;
        self.shifter = snapshot.shifter;
        self.dma = snapshot.dma;
        self.byte = snapshot.byte;
        self.program = snapshot.program;
        self.order = snapshot.order;
        self.transmitted = snapshot.transmitted;
    }
}

#[derive(Clone, Copy)]
pub(crate) enum Block {
    Spi0,
    Uart,
    Dma,
    Routing,
    Coordinator,
}
impl Block {
    fn index(self) -> u8 {
        match self {
            Self::Spi0 => 0,
            Self::Uart => 1,
            Self::Dma => 2,
            Self::Routing => 3,
            Self::Coordinator => u8::MAX,
        }
    }
    fn name(self) -> &'static str {
        match self {
            Self::Spi0 => "spi0-byte-shifter",
            Self::Uart => "boot-uart",
            Self::Dma => "boot-peripheral-dma",
            Self::Routing => "boot-dma-request-selector",
            Self::Coordinator => "serial-event-coordinator",
        }
    }
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct SerialSnapshot {
    registers: WordMap,
    shifter: Shifter,
    dma: PeripheralDma,
    byte: Option<ByteTransfer>,
    program: Option<Program>,
    order: u64,
    transmitted: Vec<Vec<u8>>,
}
struct SerialDevice {
    core: Arc<Mutex<SerialState>>,
    block: Block,
}
pub struct BootSerialBlocks {
    core: Arc<Mutex<SerialState>>,
}
impl BootSerialBlocks {
    pub fn new(options: Options, wiring: Wiring) -> Result<Self, String> {
        Ok(Self {
            core: Arc::new(Mutex::new(SerialState::new(options, wiring)?)),
        })
    }
    /// Consumes the blocks into mapped register devices and an unmapped event coordinator.
    pub fn devices(self) -> Vec<(Option<Mapping>, Box<dyn Device>)> {
        [
            (
                Some(Mapping {
                    base: SPI,
                    size: 0x20,
                }),
                Block::Spi0,
            ),
            (
                Some(Mapping {
                    base: UART,
                    size: 0x10,
                }),
                Block::Uart,
            ),
            (
                Some(Mapping {
                    base: DMA,
                    size: 0x40,
                }),
                Block::Dma,
            ),
            (
                Some(Mapping {
                    base: 0xfff98500,
                    size: 8,
                }),
                Block::Routing,
            ),
            (None, Block::Coordinator),
        ]
        .into_iter()
        .map(|(mapping, block)| {
            (
                mapping,
                Box::new(SerialDevice {
                    core: self.core.clone(),
                    block,
                }) as Box<dyn Device>,
            )
        })
        .collect()
    }
}
impl Device for SerialDevice {
    fn name(&self) -> &'static str {
        self.block.name()
    }
    fn checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        fujimod_emulator_devices::checkpoint::Checkpoint::encode(
            self.name(),
            &self.core.lock().unwrap().snapshot(),
        )
    }
    fn validate_checkpoint(
        &self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<(), String> {
        let _: SerialSnapshot = saved.decode(self.name())?;
        Ok(())
    }
    fn restore_checkpoint(
        &mut self,
        saved: &fujimod_emulator_devices::checkpoint::Checkpoint,
    ) -> Result<(), String> {
        let snapshot = saved.decode(self.name())?;
        self.core.lock().unwrap().restore(snapshot);
        Ok(())
    }
    fn fingerprint(&self) -> Result<serde_json::Value, String> {
        Ok(self.core.lock().unwrap().save())
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        if matches!(self.block, Block::Coordinator) {
            cx.unmodeled();
            return 0;
        }
        self.core
            .lock()
            .unwrap()
            .read(cx, self.block, address, size)
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        if matches!(self.block, Block::Coordinator) {
            cx.unmodeled();
            return;
        }
        self.core
            .lock()
            .unwrap()
            .write(cx, self.block, address, size, value);
    }
    fn access_contract(&self, address: u64, size: u8, value: Option<u64>) -> Option<String> {
        if !matches!(self.block, Block::Spi0) || address != SPI + 0x1c || size != 4 {
            return None;
        }
        value.and_then(|value| {
            u32::try_from(value)
                .map_err(|_| "SPI0 word exceeds 32 bits".to_owned())
                .and_then(|value| self.core.lock().unwrap().admission(value))
                .err()
        })
    }
    fn discovery_access(
        &mut self,
        cx: &mut Cx<'_>,
        address: u64,
        size: u8,
        value: Option<u64>,
    ) -> Option<u64> {
        if !matches!(self.block, Block::Spi0) || address != SPI + 0x1c || size != 4 {
            return None;
        }
        let value = u32::try_from(value?).ok()?;
        match self.core.lock().unwrap().start_byte(cx, value, true) {
            Ok(()) => Some(0),
            Err(error) => {
                cx.fail(error);
                None
            }
        }
    }
    fn pollable(&self, address: u64, size: u8) -> bool {
        !matches!(self.block, Block::Coordinator)
            && SerialState::offset(address, size)
                .is_some_and(|(kind, _)| kind == self.block.index())
            && address != SPI + 0x18
            && address != UART + 4
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        if matches!(self.block, Block::Coordinator) {
            self.core.lock().unwrap().advance(cx);
        }
    }
    fn next_deadline(&self, _now: u64) -> Option<u64> {
        matches!(self.block, Block::Coordinator)
            .then(|| self.core.lock().unwrap().next_deadline())
            .flatten()
    }
    fn save(&self) -> serde_json::Value {
        self.core.lock().unwrap().save()
    }
    fn restore(&mut self, state: &serde_json::Value) -> Result<(), String> {
        let registers =
            serde_json::from_value(state["registers"].clone()).map_err(|e| e.to_string())?;
        let shifter =
            serde_json::from_value(state["shifter"].clone()).map_err(|e| e.to_string())?;
        let dma = serde_json::from_value(state["dma"].clone()).map_err(|e| e.to_string())?;
        let byte = serde_json::from_value(state["byte"].clone()).map_err(|e| e.to_string())?;
        let program =
            serde_json::from_value(state["program"].clone()).map_err(|e| e.to_string())?;
        let order = state["order"].as_u64().ok_or("invalid serial order")?;
        let transmitted =
            serde_json::from_value(state["transmitted"].clone()).map_err(|e| e.to_string())?;
        self.core.lock().unwrap().restore(SerialSnapshot {
            registers,
            shifter,
            dma,
            byte,
            program,
            order,
            transmitted,
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests;
