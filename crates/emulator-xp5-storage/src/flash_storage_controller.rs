//! Serial NOR flash-storage PIO/status and aperture mode.
use crate::flash_storage::Shared;
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[derive(Serialize, Deserialize)]
struct State {
    regs: Vec<u32>,
    active: bool,
    complete: bool,
    cursor: u32,
    tx: Vec<u8>,
    #[serde(default)]
    rx_fifo: std::collections::VecDeque<u8>,
    configuration_complete: bool,
}
pub struct Controller {
    state: State,
    flash_storage: Shared,
    jedec: u32,
}
impl Controller {
    pub fn new(flash_storage: Shared, jedec: u32) -> Self {
        let mut regs = vec![0; 64];
        regs[0x10 / 4] = 0x10001; // Explicit inherited enabled/idle controller assumption.
        Self {
            state: State {
                regs,
                active: false,
                complete: false,
                cursor: 0,
                tx: Vec::new(),
                rx_fifo: Default::default(),
                configuration_complete: false,
            },
            flash_storage,
            jedec,
        }
    }
    fn known(o: u64) -> bool {
        matches!(
            o,
            0 | 4
                | 8
                | 0x10
                | 0x14
                | 0x18
                | 0x20
                | 0x24
                | 0x34
                | 0x40
                | 0x44
                | 0x48
                | 0x50
                | 0x74
                | 0x80
                | 0x94
                | 0x98
                | 0xa0
                | 0xa4
                | 0xc0
                | 0xc4
                | 0xc8
                | 0xd0
                | 0xe0
                | 0xf0
                | 0xf4
                | 0xf8
                | 0xfc
        )
    }
    fn cmd(&self) -> u8 {
        if self.state.regs[0] & 3 == 1 {
            self.state.regs[0x40 / 4] as u8
        } else {
            (self.state.regs[0x40 / 4] >> 8) as u8
        }
    }
    fn begin(&mut self) -> Result<(), String> {
        let cmd = self.cmd();
        self.state.cursor = 0;
        self.state.tx.clear();
        self.state.active = true;
        self.state.complete = false;
        let mut flash_storage = self.flash_storage.lock().unwrap();
        flash_storage.session.controller_complete = false;
        match cmd {
            0x06 => {
                flash_storage.session.write_enable = true;
                self.state.complete = true;
            }
            0x04 => {
                flash_storage.session.write_enable = false;
                self.state.complete = true;
            }
            0x05 | 0x9f | 0x03 | 0x13 | 0x0b | 0x0c | 0xeb | 0xec => {
                let count = (self.state.regs[0xa0 / 4] & 0x7ff) as usize + 1;
                self.state.rx_fifo = match cmd {
                    0x05 => vec![(flash_storage.session.write_enable as u8) << 1; count].into(),
                    0x9f => (0..count)
                        .map(|i| {
                            if i < 3 {
                                (self.jedec >> (16 - i * 8)) as u8
                            } else {
                                0xff
                            }
                        })
                        .collect(),
                    _ => flash_storage
                        .read_device(self.state.regs[0x50 / 4] as u64, count)?
                        .into(),
                };
                self.state.complete = true;
            }
            0x02 | 0x12 | 0x32 | 0x34 => {}
            0x20 | 0x21 | 0xd8 | 0xdc => {
                let size = if cmd == 0x20 || cmd == 0x21 {
                    0x1000
                } else {
                    0x10000
                };
                flash_storage.erase(self.state.regs[0x50 / 4] as u64, size)?;
                self.state.complete = true;
            }
            _ => return Err(format!("unsupported serial-flash storage command {cmd:#x}")),
        }
        Ok(())
    }
    fn stop(&mut self) -> Result<(), String> {
        if matches!(self.cmd(), 0x02 | 0x12 | 0x32 | 0x34) && !self.state.tx.is_empty() {
            self.flash_storage
                .lock()
                .unwrap()
                .program(self.state.regs[0x50 / 4] as u64, &self.state.tx)?;
        }
        self.state.active = false;
        self.state.complete = true;
        Ok(())
    }
    fn rx(&mut self) -> Result<u8, String> {
        // FIFO contents survive a later command/strobe; register configuration
        // is not the identity of data already clocked into the receive FIFO.
        if let Some(b) = self.state.rx_fifo.pop_front() {
            self.state.cursor += 1;
            return Ok(b);
        }
        if !self.state.active {
            return Err("flash storage FIFO read outside transaction".into());
        }
        let mut s = self.flash_storage.lock().unwrap();
        let b = match self.cmd() {
            0x05 => (s.session.write_enable as u8) << 1,
            0x9f => {
                if self.state.cursor < 3 {
                    (self.jedec >> (16 - self.state.cursor * 8)) as u8
                } else {
                    0xff
                }
            }
            0x03 | 0x13 | 0x0b | 0x0c | 0xeb | 0xec => s.read_device(
                self.state.regs[0x50 / 4] as u64 + self.state.cursor as u64,
                1,
            )?[0],
            cmd => return Err(format!("flash storage FIFO read for command {cmd:#x}")),
        };
        self.state.cursor += 1;
        self.state.complete = true;
        Ok(b)
    }
}
impl Device for Controller {
    fn pollable(&self, offset: u64, size: u8) -> bool {
        offset == 0xff90a0f0 && size == 4
    }
    fn name(&self) -> &'static str {
        "serial-flash storage"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, n: u8) -> u64 {
        let o = address.wrapping_sub(0xff90a000);
        if n != 4 || !Self::known(o) {
            cx.unmodeled();
            return 0;
        }
        match o {
            0x80 => match self.rx() {
                Ok(b) => b as u64,
                Err(e) => {
                    cx.fail(e);
                    0
                }
            },
            0xe0 => {
                let mode = if self.state.regs[0xfc / 4] & 1 != 0 {
                    0x88
                } else if self.state.regs[0x10 / 4] & 1 == 0 {
                    0x18
                } else {
                    8
                };
                mode | if self.state.configuration_complete {
                    1 << 23
                } else {
                    0
                }
            }
            0xf0 => {
                let receive = !self.state.rx_fifo.is_empty()
                    || (self.state.active
                        && matches!(
                            self.cmd(),
                            0x05 | 0x9f | 0x03 | 0x13 | 0x0b | 0x0c | 0xeb | 0xec
                        ));
                let complete = self.state.complete
                    || self
                        .flash_storage
                        .lock()
                        .unwrap()
                        .session
                        .controller_complete;
                4 | if receive { 2 } else { 0 } | complete as u64
            }
            0xd0 => 0,
            _ => self.state.regs[o as usize / 4] as u64,
        }
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, n: u8, v: u64) {
        let o = address.wrapping_sub(0xff90a000);
        if n != 4 || !Self::known(o) {
            cx.unmodeled();
            return;
        }
        let v = v as u32;
        let result = match o {
            0x80 => {
                if !self.state.active || !matches!(self.cmd(), 0x02 | 0x12 | 0x32 | 0x34) {
                    Err("unsupported flash storage FIFO write".into())
                } else {
                    self.state.tx.push(v as u8);
                    if self.state.tx.len() == (self.state.regs[0xa0 / 4] & 0x7ff) as usize + 1 {
                        self.stop()
                    } else {
                        Ok(())
                    }
                }
            }
            0xc0 => match v {
                1 => self.begin(),
                2 => self.stop(),
                _ => Err("unsupported flash storage transaction strobe".into()),
            },
            0xf0 => {
                if v & 1 != 0 {
                    self.state.complete = false;
                    self.flash_storage
                        .lock()
                        .unwrap()
                        .session
                        .controller_complete = false;
                }
                Ok(())
            }
            0xd0 => {
                if v & !0x101 != 0 {
                    Err("unsupported flash storage FIFO reset".into())
                } else {
                    Ok(())
                }
            }
            0xe0 => {
                cx.unmodeled();
                return;
            }
            _ => {
                self.state.regs[o as usize / 4] = v;
                if o == 0xc8 {
                    self.state.configuration_complete = true;
                }
                if o == 0x48 {
                    self.flash_storage.lock().unwrap().session.program_mode =
                        matches!(v as u8, 0x02 | 0x12 | 0x32 | 0x34);
                }
                Ok(())
            }
        };
        if let Err(e) = result {
            cx.fail(e);
        }
    }
    fn save(&self) -> Value {
        serde_json::to_value(&self.state).unwrap()
    }
    fn restore(&mut self, v: &Value) -> Result<(), String> {
        self.state = serde_json::from_value(v.clone())
            .map_err(|e| format!("invalid storage controller checkpoint: {e}"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flash_storage::Storage;
    use std::sync::{Arc, Mutex};

    #[test]
    fn pio_reads_count_transferred_bytes_once_and_exclude_identity_commands() {
        let storage = Arc::new(Mutex::new(Storage::new((0..32).collect()).unwrap()));
        let mut controller = Controller::new(storage.clone(), 0xef4018);
        controller.state.regs[0] = 1;
        controller.state.regs[0x40 / 4] = 0x03;
        controller.state.regs[0x50 / 4] = 8;
        controller.state.regs[0xa0 / 4] = 3;
        controller.begin().unwrap();
        assert_eq!(storage.lock().unwrap().session.reads.bytes, 4);
        for value in 8..12 {
            assert_eq!(controller.rx().unwrap(), value);
        }
        assert_eq!(storage.lock().unwrap().session.reads.bytes, 4);
        assert_eq!(controller.rx().unwrap(), 12);
        assert_eq!(storage.lock().unwrap().session.reads.ranges, [(8, 13)]);
        controller.state.regs[0x40 / 4] = 0x9f;
        controller.begin().unwrap();
        assert_eq!(controller.rx().unwrap(), 0xef);
        assert_eq!(storage.lock().unwrap().session.reads.bytes, 5);
    }
}
