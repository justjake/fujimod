//! An SDHCI 4 UHS-II host port and the PHY control block in front of it, for a
//! slot whose card has no UHS-II lanes.
//!
//! The port is a second SDHCI register file for the same slot as a UHS-I host.
//! Its card-detect bits follow the slot's contacts; its UHS-II interface
//! detection and lane synchronization bits (present state 31 and 30) stay
//! clear, because no modeled card has UHS-II lanes. The driver then stops the
//! PHY and uses the UHS-I host. A UHS-II packet, tuning, 1.8 V signaling, preset
//! values and the PHY settings area refuse.
//!
//! Register file, relative to `host`: present state `+0x24`, host and power
//! control `+0x28/+0x29`, clock control `+0x2c` (the shared settling model),
//! timeout and software reset `+0x2e/+0x2f`, status and enables
//! `+0x30..+0x3b`, host control 2 `+0x3e`, capabilities `+0x40..+0x47` (a
//! declared input), UHS-II software reset `+0xc0`, timer control `+0xc2` and
//! UHS-II error status and enables `+0xc4..+0xcf`.
//!
//! PHY block, relative to `phy`. Configuration words `+0x20`, `+0x24`,
//! `+0x80`, `+0x94` and `+0xec` are stored. Control `+0x78` bit 0 enables the
//! PHY. Status `+0x7c` bit 0 is set while that enable is clear. Status `+0x60`
//! bit 1 (sequence busy) stays clear: a sequence started through `+0x94` bit 0
//! completes immediately. These three meanings are inferred from the order of
//! a stock driver's power-down sequence, not from a datasheet or a measured
//! part: start `+0x94` bit 0, wait for `+0x60` bit 1 clear, clear `+0x78`
//! bit 0, stop the clocks, then wait for `+0x7c` bit 0 set.
use super::Clocks;
use fujimod_emulator_devices::parts::card_contacts::Contacts;
use fujimod_emulator_devices::{Cx, Device};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Physical inputs of one UHS-II port.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Options {
    /// Base of the PHY control block (0x200 bytes).
    pub phy: u64,
    /// Base of the SDHCI register file (0x100 bytes).
    pub host: u64,
    /// Capabilities register value, `+0x40` low word and `+0x44` high word.
    pub capabilities: u64,
    /// Internal clock settling time in ticks.
    pub clock_ticks: u64,
}

const PHY_SIZE: u64 = 0x200;
const HOST_SIZE: u64 = 0x100;
/// Host control 2 bits stored: UHS mode select, UHS-II interface enable, host
/// version 4 enable, ADMA2 26-bit length and asynchronous interrupt enable.
const HOST_CONTROL2: u16 = 0x7107;
const UHS2_MODE: u16 = 7;

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Host {
    host_control: u16,
    timeout: u8,
    status: [u16; 2],
    status_enable: [u16; 2],
    signal_enable: [u16; 2],
    host_control2: u16,
    uhs2_timer: u16,
    uhs2_error_status: u32,
    uhs2_error_enable: u32,
    uhs2_error_signal: u32,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Phy {
    configuration: [u32; 2],
    control: u32,
    lanes: u32,
    sequence: u32,
    trailer: u32,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct State {
    host: Host,
    phy: Phy,
}

pub struct Port {
    options: Options,
    contacts: Contacts,
    clocks: Clocks,
    state: State,
}

enum Area {
    Phy(u64),
    Host(u64),
}

impl Port {
    pub fn new(options: Options, contacts: Contacts) -> Result<Self, String> {
        let overlaps = options.phy < options.host + HOST_SIZE && options.host < options.phy + PHY_SIZE;
        if options.phy & 0xff != 0
            || options.host & 0xff != 0
            || options.phy.checked_add(PHY_SIZE).is_none_or(|end| end > 1 << 32)
            || options.host.checked_add(HOST_SIZE).is_none_or(|end| end > 1 << 32)
            || overlaps
        {
            return Err("Invalid UHS-II port PHY or host base".into());
        }
        let clocks = Clocks::new(&[(options.host + 0x2c, options.clock_ticks)])?;
        Ok(Self {
            options,
            contacts,
            clocks,
            state: State::default(),
        })
    }

    fn area(&self, address: u64) -> Option<Area> {
        if let Some(offset) = address.checked_sub(self.options.phy).filter(|o| *o < PHY_SIZE) {
            Some(Area::Phy(offset))
        } else {
            address
                .checked_sub(self.options.host)
                .filter(|o| *o < HOST_SIZE)
                .map(Area::Host)
        }
    }

    fn present_state(&self) -> u32 {
        // Card inserted (16), card state stable (17), card-detect pin level (18)
        // and write-protect pin level (19, set while writes are enabled).
        let present = self.contacts.present();
        0x20000
            | if present { 0x50000 } else { 0 }
            | if present && !self.contacts.protected() { 0x80000 } else { 0 }
    }

    fn reset_all(&mut self, cx: &mut Cx<'_>) {
        self.state.host = Host::default();
        self.clocks.write(cx, self.options.host + 0x2c, 2, 0);
    }

    fn read_phy(&mut self, cx: &mut Cx<'_>, offset: u64, size: u8) -> u64 {
        let phy = &self.state.phy;
        let value = match (offset, size) {
            (0x20, 4) => phy.configuration[0],
            (0x24, 4) => phy.configuration[1],
            (0x60, 4) => 0,
            (0x78, 2 | 4) => phy.control,
            (0x7c, 4) => u32::from(phy.control & 1 == 0),
            (0x80, 4) => phy.lanes,
            (0x94, 4) => phy.sequence,
            (0xec, 4) => phy.trailer,
            _ => {
                cx.unmodeled();
                return 0;
            }
        };
        u64::from(value)
    }

    fn write_phy(&mut self, cx: &mut Cx<'_>, offset: u64, size: u8, value: u32) {
        let phy = &mut self.state.phy;
        match (offset, size) {
            (0x20, 4) => phy.configuration[0] = value,
            (0x24, 4) => phy.configuration[1] = value,
            (0x78, 2) => phy.control = phy.control & !0xffff | value,
            (0x78, 4) => phy.control = value,
            (0x80, 4) => phy.lanes = value,
            (0x94, 4) => phy.sequence = value,
            (0xec, 4) => phy.trailer = value,
            _ => cx.unmodeled(),
        }
    }

    fn read_host(&mut self, cx: &mut Cx<'_>, offset: u64, size: u8) -> u64 {
        let host = &self.state.host;
        let value = match (offset, size) {
            (0x24, 4) => self.present_state(),
            (0x28, 1) => u32::from(host.host_control & 0xff),
            (0x29, 1) => u32::from(host.host_control >> 8),
            (0x28, 2) => u32::from(host.host_control),
            (0x2c, 2) => self.clocks.read(cx, self.options.host + 0x2c, 2) as u32,
            (0x2c, 4) => {
                self.clocks.read(cx, self.options.host + 0x2c, 2) as u32
                    | u32::from(host.timeout) << 16
            }
            (0x2e, 1) => u32::from(host.timeout),
            (0x2f, 1) => 0,
            (0x30 | 0x32, 2) => u32::from(host.status[(offset as usize - 0x30) / 2]),
            (0x34 | 0x36, 2) => u32::from(host.status_enable[(offset as usize - 0x34) / 2]),
            (0x38 | 0x3a, 2) => u32::from(host.signal_enable[(offset as usize - 0x38) / 2]),
            (0x30, 4) => u32::from(host.status[0]) | u32::from(host.status[1]) << 16,
            (0x34, 4) => u32::from(host.status_enable[0]) | u32::from(host.status_enable[1]) << 16,
            (0x38, 4) => u32::from(host.signal_enable[0]) | u32::from(host.signal_enable[1]) << 16,
            (0x3c, 2) => 0,
            (0x3e, 2) => u32::from(host.host_control2),
            (0x3c, 4) => u32::from(host.host_control2) << 16,
            (0x40, 4) => self.options.capabilities as u32,
            (0x44, 4) => (self.options.capabilities >> 32) as u32,
            (0xc0, 2) => 0,
            (0xc2, 2) => u32::from(host.uhs2_timer),
            (0xc4, 4) => host.uhs2_error_status,
            (0xc8, 4) => host.uhs2_error_enable,
            (0xcc, 4) => host.uhs2_error_signal,
            (0x80..0x100, _) => {
                cx.unsupported(format!(
                    "UHS-II register {offset:#x} needs a card with UHS-II lanes"
                ));
                return 0;
            }
            _ => {
                cx.unmodeled();
                return 0;
            }
        };
        u64::from(value)
    }

    fn write_host(&mut self, cx: &mut Cx<'_>, offset: u64, size: u8, value: u32) {
        match (offset, size) {
            (0x2c, 2) => return self.clocks.write(cx, self.options.host + 0x2c, 2, u64::from(value)),
            (0x2f, 1) if value & 1 != 0 && value & !7 == 0 => return self.reset_all(cx),
            _ => {}
        }
        let host = &mut self.state.host;
        match (offset, size) {
            (0x28, 1) => host.host_control = host.host_control & 0xff00 | value as u16,
            (0x29, 1) => host.host_control = host.host_control & 0xff | (value as u16) << 8,
            (0x28, 2) => host.host_control = value as u16,
            (0x2e, 1) if value <= 15 => host.timeout = value as u8,
            // Command or data line reset: no command or transfer exists to abort.
            (0x2f, 1) if value & !6 == 0 => host.status = [0; 2],
            (0x30 | 0x32, 2) => host.status[(offset as usize - 0x30) / 2] &= !(value as u16),
            (0x30, 4) => {
                host.status[0] &= !(value as u16);
                host.status[1] &= !((value >> 16) as u16);
            }
            (0x34 | 0x36, 2) => host.status_enable[(offset as usize - 0x34) / 2] = value as u16,
            (0x38 | 0x3a, 2) => host.signal_enable[(offset as usize - 0x38) / 2] = value as u16,
            (0x34, 4) => host.status_enable = [value as u16, (value >> 16) as u16],
            (0x38, 4) => host.signal_enable = [value as u16, (value >> 16) as u16],
            (0x3e, 2) if value as u16 & !HOST_CONTROL2 == 0
                && matches!(value as u16 & 7, 0 | UHS2_MODE) =>
            {
                host.host_control2 = value as u16
            }
            (0x3e, 2) => cx.unsupported(format!(
                "UHS-II port host control 2 {value:#x}: only UHS-II mode and interface enable are modeled"
            )),
            (0xc0, 2) if value & !3 == 0 => {
                if value & 1 != 0 {
                    host.uhs2_timer = 0;
                    host.uhs2_error_status = 0;
                    host.uhs2_error_enable = 0;
                    host.uhs2_error_signal = 0;
                }
            }
            (0xc2, 2) => host.uhs2_timer = value as u16,
            (0xc4, 4) => host.uhs2_error_status &= !value,
            (0xc8, 4) => host.uhs2_error_enable = value,
            (0xcc, 4) => host.uhs2_error_signal = value,
            (0x80..0x100, _) => cx.unsupported(format!(
                "UHS-II register {offset:#x} needs a card with UHS-II lanes"
            )),
            _ => cx.unmodeled(),
        }
    }

    fn validate(&self, state: &State) -> Result<(), String> {
        if state.host.host_control2 & !HOST_CONTROL2 != 0 || state.host.timeout > 15 {
            return Err("Invalid UHS-II port checkpoint".into());
        }
        Ok(())
    }
}

impl Device for Port {
    fujimod_emulator_devices::checkpoint_fields!(state: State, clocks: Clocks);
    fn name(&self) -> &'static str {
        "sdhci-uhs2-port"
    }
    fn fingerprint(&self) -> Result<Value, String> {
        Ok(json!({"options": self.options, "state": self.state, "clocks": self.clocks.save()}))
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        match self.area(address) {
            Some(Area::Phy(offset)) => self.read_phy(cx, offset, size),
            Some(Area::Host(offset)) => self.read_host(cx, offset, size),
            None => {
                cx.unmodeled();
                0
            }
        }
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        let Ok(value) = u32::try_from(value) else {
            cx.unmodeled();
            return;
        };
        if size < 4 && value >> (size * 8) != 0 {
            cx.unmodeled();
            return;
        }
        match self.area(address) {
            Some(Area::Phy(offset)) => self.write_phy(cx, offset, size, value),
            Some(Area::Host(offset)) => self.write_host(cx, offset, size, value),
            None => cx.unmodeled(),
        }
    }
    fn pollable(&self, address: u64, size: u8) -> bool {
        matches!(
            (self.area(address), size),
            (Some(Area::Phy(0x60 | 0x7c)), 4)
                | (Some(Area::Host(0x24)), 4)
                | (Some(Area::Host(0x2c)), 2)
        )
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        self.clocks.advance(cx);
    }
    fn event(&mut self, cx: &mut Cx<'_>, token: u64) {
        self.clocks.event(cx, token);
    }
    fn next_deadline(&self, now: u64) -> Option<u64> {
        self.clocks.next_deadline(now)
    }
    fn save(&self) -> Value {
        json!({"state": self.state, "clocks": self.clocks.save()})
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Saved {
            state: State,
            clocks: Value,
        }
        let saved: Saved = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        self.validate(&saved.state)?;
        self.clocks.restore(&saved.clocks)?;
        self.state = saved.state;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::test_host::recording::RecordingHost;

    const PHY: u64 = 0xff9b2000;
    const HOST: u64 = 0xff9b2200;
    const UHS2_SUPPORT: u64 = 1 << 35;

    fn port(contacts: Contacts) -> Port {
        Port::new(
            Options {
                phy: PHY,
                host: HOST,
                capabilities: UHS2_SUPPORT,
                clock_ticks: 50,
            },
            contacts,
        )
        .unwrap()
    }
    fn write(port: &mut Port, host: &mut RecordingHost, address: u64, size: u8, value: u64) {
        let mut cx = Cx::new(host, 0, 0, 0);
        port.write(&mut cx, address, size, value);
        assert_eq!(cx.refusal(), None, "write {address:#x}");
    }
    fn read(port: &mut Port, host: &mut RecordingHost, address: u64, size: u8) -> u64 {
        let mut cx = Cx::new(host, 0, 0, 0);
        let value = port.read(&mut cx, address, size);
        assert_eq!(cx.refusal(), None, "read {address:#x}");
        value
    }

    /// The stock UHS-II bring-up and power-down order for an empty slot: PHY
    /// enable, internal clock, capabilities, power, UHS-II mode, SD clock, no
    /// interface detection, then the PHY power-down sequence.
    #[test]
    fn empty_slot_reports_no_uhs2_interface_and_powers_the_phy_down() {
        let contacts = Contacts::new(false, false);
        let mut port = port(contacts.clone());
        let mut host = RecordingHost::default();
        write(&mut port, &mut host, PHY + 0x80, 4, 0xf00);
        write(&mut port, &mut host, HOST + 0x3e, 2, 0x1100);
        write(&mut port, &mut host, PHY + 0x78, 2, 1);
        write(&mut port, &mut host, PHY + 0xec, 4, 1);
        assert_eq!(read(&mut port, &mut host, PHY + 0x7c, 4) & 1, 0);
        write(&mut port, &mut host, HOST + 0x2c, 2, 1);
        assert_eq!(read(&mut port, &mut host, HOST + 0x2c, 2) & 2, 0);
        host.now = 50;
        assert_eq!(read(&mut port, &mut host, HOST + 0x2c, 2), 3);
        assert_eq!(read(&mut port, &mut host, HOST + 0x44, 4) >> 3 & 1, 1);
        write(&mut port, &mut host, HOST + 0x28, 2, 0xbf00);
        write(&mut port, &mut host, HOST + 0x3e, 2, 0x1107);
        write(&mut port, &mut host, HOST + 0x2c, 2, 7);
        assert_eq!(read(&mut port, &mut host, HOST + 0x24, 4), 0x20000);
        write(&mut port, &mut host, PHY + 0x94, 4, 0x4050);
        write(&mut port, &mut host, PHY + 0x94, 4, 0x4051);
        assert_eq!(read(&mut port, &mut host, PHY + 0x60, 4) & 2, 0);
        write(&mut port, &mut host, PHY + 0x78, 4, 0);
        write(&mut port, &mut host, HOST + 0x2c, 2, 2);
        assert_eq!(read(&mut port, &mut host, HOST + 0x2c, 2), 0);
        assert_eq!(read(&mut port, &mut host, PHY + 0x7c, 4) & 1, 1);
        write(&mut port, &mut host, HOST + 0x3e, 2, 0x1007);
        let saved = port.save();
        let mut restored = self::port(contacts.clone());
        restored.restore(&saved).unwrap();
        assert_eq!(restored.save(), saved);
        contacts.set(true, false);
        assert_eq!(read(&mut port, &mut host, HOST + 0x24, 4), 0xf0000);
        contacts.set(true, true);
        assert_eq!(read(&mut port, &mut host, HOST + 0x24, 4), 0x70000);
    }

    #[test]
    fn uhs2_transfers_tuning_and_unknown_registers_refuse() {
        let mut port = port(Contacts::new(true, false));
        let mut host = RecordingHost::default();
        for (address, size, value) in [
            (HOST + 0x9c, 4, 0),
            (HOST + 0x3e, 2, 0x40),
            (HOST + 0x3e, 2, 0x1103),
            (HOST + 0x2f, 1, 8),
            (PHY + 0x7c, 4, 1),
            (PHY + 0x100, 4, 0),
        ] {
            let mut cx = Cx::new(&mut host, 0, 0, 0);
            port.write(&mut cx, address, size, value);
            assert!(cx.refusal().is_some(), "write {address:#x} {value:#x}");
        }
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        port.read(&mut cx, HOST + 0xa0, 4);
        assert!(cx.refusal().is_some());
        assert!(
            Port::new(
                Options { phy: PHY, host: PHY + 0x100, capabilities: 0, clock_ticks: 1 },
                Contacts::new(false, false),
            )
            .is_err()
        );
    }

    #[test]
    fn software_reset_all_clears_registers_and_stops_the_clock() {
        let mut port = port(Contacts::new(false, false));
        let mut host = RecordingHost::default();
        write(&mut port, &mut host, HOST + 0x2c, 2, 1);
        write(&mut port, &mut host, HOST + 0x34, 4, 0xffff_ffff);
        write(&mut port, &mut host, HOST + 0x3e, 2, 0x1100);
        write(&mut port, &mut host, HOST + 0x2f, 1, 1);
        assert_eq!(read(&mut port, &mut host, HOST + 0x2c, 2), 0);
        assert_eq!(read(&mut port, &mut host, HOST + 0x34, 4), 0);
        assert_eq!(read(&mut port, &mut host, HOST + 0x3e, 2), 0);
        assert_eq!(port.next_deadline(0), None);
    }
}
