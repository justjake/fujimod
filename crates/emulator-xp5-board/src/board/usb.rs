//! XP5 USB device controller (DesignWare USB3, device side) and its indexed PHY port.
use fujimod_emulator_devices::graph::DeviceWindow;
use fujimod_emulator_ip::usb::dwc3::{Config, Dwc3};
use fujimod_emulator_xp5_io::usb_phy;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ControllerOptions {
    interrupt: u32,
    /// DSTS negotiated speed: 0 high speed, 4 SuperSpeed.
    speed: u8,
    command_ticks: u64,
    transaction_ticks: u64,
}

/// The shared DWC3 model checks every register, TRB and event it handles and
/// refuses the rest, so no XP5-specific register behavior is added here.
pub fn controller(window: &DeviceWindow) -> Result<Dwc3, String> {
    let options: ControllerOptions = toml::Value::Table(window.options.clone())
        .try_into()
        .map_err(|e| format!("USB device controller options: {e}"))?;
    if window.size < 0xcc00 {
        return Err("USB device controller window does not cover the device registers".into());
    }
    Dwc3::new(Config {
        base: window.base,
        irq: options.interrupt,
        command_ticks: options.command_ticks,
        transaction_ticks: options.transaction_ticks,
        speed: options.speed,
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PhyOptions {
    #[serde(default)]
    initial: Option<Vec<u8>>,
    #[serde(default)]
    parallel: u32,
}

pub fn phy(window: &DeviceWindow) -> Result<usb_phy::Phy, String> {
    let options: PhyOptions = toml::Value::Table(window.options.clone())
        .try_into()
        .map_err(|e| format!("USB PHY options: {e}"))?;
    let mut initial = [0u8; 32];
    if let Some(values) = options.initial {
        if values.len() != 32 {
            return Err("USB PHY initial registers must list 32 bytes".into());
        }
        initial.copy_from_slice(&values);
    }
    if window.size != 0x404 {
        return Err("USB PHY window must be 0x404 bytes".into());
    }
    usb_phy::Phy::new(usb_phy::Config {
        base: window.base,
        initial,
        parallel: options.parallel,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::{Cx, Device, Host, Record};
    use std::collections::BTreeMap;

    const BASE: u64 = 0xffc00000;
    const RING: u64 = 0x37a7000;

    /// Sparse guest RAM, the board clock and the one interrupt line.
    #[derive(Default)]
    struct Board {
        now: u64,
        ram: BTreeMap<u64, u8>,
        irq: Option<(u32, bool)>,
    }
    impl Host for Board {
        fn now(&self) -> u64 {
            self.now
        }
        fn schedule(&mut self, _: usize, _: u64, _: u64) {}
        fn set_irq(&mut self, line: u32, high: bool) {
            self.irq = Some((line, high));
        }
        fn raise_irq(&mut self, _: usize, _: u32) -> Result<(), String> {
            Err("not wired".into())
        }
        fn ram_read(&self, address: u64, out: &mut [u8]) -> Result<(), String> {
            for (n, byte) in out.iter_mut().enumerate() {
                *byte = self.ram.get(&(address + n as u64)).copied().unwrap_or(0);
            }
            Ok(())
        }
        fn ram_write(&mut self, address: u64, bytes: &[u8]) -> Result<(), String> {
            for (n, byte) in bytes.iter().enumerate() {
                self.ram.insert(address + n as u64, *byte);
            }
            Ok(())
        }
        fn record(&mut self, _: Record) {}
    }

    /// The X-T5 4.31 catalog fragment that declares these devices.
    fn xt5_windows() -> Vec<DeviceWindow> {
        #[derive(Deserialize)]
        struct Fragment {
            device: Vec<DeviceWindow>,
        }
        let text = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../catalog/firmware/xt5/4.31/xp5-boot/manifest.d/64-usb.toml"
        ));
        toml::from_str::<Fragment>(text).unwrap().device
    }

    fn write(d: &mut Dwc3, b: &mut Board, offset: u64, value: u32) {
        let mut cx = Cx::new(b, 0, 0, 0);
        d.write(&mut cx, BASE + offset, 4, u64::from(value));
        assert_eq!(cx.refusal(), None, "write {offset:#x} = {value:#x}");
    }
    fn read(d: &mut Dwc3, b: &mut Board, offset: u64) -> u32 {
        let mut cx = Cx::new(b, 0, 0, 0);
        let value = d.read(&mut cx, BASE + offset, 4) as u32;
        assert_eq!(cx.refusal(), None, "read {offset:#x}");
        value
    }
    fn at(d: &mut Dwc3, b: &mut Board, now: u64) {
        b.now = now;
        let mut cx = Cx::new(b, 0, 0, 0);
        d.event(&mut cx, 0);
        assert_eq!(cx.refusal(), None);
    }
    fn command(d: &mut Dwc3, b: &mut Board, ep: u64, code: u32, parameters: [u32; 3]) {
        for (n, value) in parameters.iter().enumerate() {
            write(d, b, 0xc800 + ep * 16 + n as u64 * 4, *value);
        }
        write(d, b, 0xc80c + ep * 16, 0x400 | code);
        let now = b.now + 1000;
        at(d, b, now);
        assert_eq!(read(d, b, 0xc80c + ep * 16) & 0x400, 0);
    }
    fn ring_word(b: &Board, index: u64) -> u32 {
        let mut bytes = [0; 4];
        b.ram_read(RING + index * 4, &mut bytes).unwrap();
        u32::from_le_bytes(bytes)
    }

    #[test]
    fn the_catalog_declares_the_controller_and_phy_from_the_static_decode() {
        let windows = xt5_windows();
        let controller = windows
            .iter()
            .find(|w| w.kind == "usb-device-controller")
            .expect("X-T5 4.31 declares a USB device controller");
        assert_eq!((controller.base, controller.size), (BASE, 0x10000));
        let phy_window = windows.iter().find(|w| w.kind == "usb-phy").unwrap();
        assert_eq!((phy_window.base, phy_window.size), (0xffc23800, 0x404));
        super::controller(controller).unwrap();
        super::phy(phy_window).unwrap();
    }

    /// The ff80 driver's device init (0x2bfbb68), then a cable and a host bus
    /// reset: connect-done and USB-reset events reach the ring at 0x37a7000 on
    /// INTID 283, and the handler's GEVNTSIZ mask (0x2bfb390) holds the line low.
    #[test]
    fn stock_init_then_cable_and_bus_reset_post_events_on_intid_283() {
        let windows = xt5_windows();
        let window = windows
            .iter()
            .find(|w| w.kind == "usb-device-controller")
            .unwrap();
        let mut d = controller(window).unwrap();
        let mut b = Board::default();
        // GUSB2PHYCFG &= ~0x141, GUSB3PIPECTL bit 17 cleared, DCTL.CSFTRST and its poll.
        write(&mut d, &mut b, 0xc200, 0);
        write(&mut d, &mut b, 0xc2c0, 0);
        write(&mut d, &mut b, 0xc704, 1 << 30);
        at(&mut d, &mut b, 1000);
        assert_eq!(read(&mut d, &mut b, 0xc704) & (1 << 30), 0);
        // Event buffer, GCTL device role, DCFG, DEVTEN.
        write(&mut d, &mut b, 0xc400, RING as u32);
        write(&mut d, &mut b, 0xc404, 0);
        write(&mut d, &mut b, 0xc408, 0x1000);
        write(&mut d, &mut b, 0xc40c, 0);
        write(&mut d, &mut b, 0xc110, 0x2000);
        write(&mut d, &mut b, 0xc700, 0x420005);
        write(&mut d, &mut b, 0xc708, 7);
        // EP0 OUT/IN: start configuration, 64-byte control endpoints, resources.
        command(&mut d, &mut b, 0, 9, [0, 0, 0]);
        command(&mut d, &mut b, 0, 1, [0, 0x500, 64 << 3]);
        command(&mut d, &mut b, 1, 1, [0, 0x500, 64 << 3]);
        command(&mut d, &mut b, 0, 2, [0, 0, 1]);
        command(&mut d, &mut b, 1, 2, [0, 0, 1]);
        write(&mut d, &mut b, 0xc720, 3);
        write(&mut d, &mut b, 0xc704, 1 << 31);
        let now = b.now + 1000;
        at(&mut d, &mut b, now);
        let state = d.save();
        assert_eq!(state["state"]["running"], true);
        // No cable: DSTS reports the disconnected link state.
        assert_eq!(read(&mut d, &mut b, 0xc70c) >> 18 & 15, 4);
        assert_eq!(read(&mut d, &mut b, 0xc40c), 0);

        d.link(b.now, true).unwrap();
        let now = b.now + 125_000;
        at(&mut d, &mut b, now);
        // High speed, link up.
        assert_eq!(read(&mut d, &mut b, 0xc70c) & 0x3c0007, 0);
        d.bus_reset(b.now).unwrap();
        let now = b.now + 125_000;
        at(&mut d, &mut b, now);
        assert_eq!(read(&mut d, &mut b, 0xc40c), 8);
        assert_eq!([ring_word(&b, 0), ring_word(&b, 1)], [0x201, 0x101]);
        assert_eq!(b.irq, Some((283, true)));
        // Hard handler masks the buffer; the task drains and acknowledges.
        write(&mut d, &mut b, 0xc408, 0x80001000);
        assert_eq!(b.irq, Some((283, false)));
        write(&mut d, &mut b, 0xc40c, 8);
        write(&mut d, &mut b, 0xc408, 0x1000);
        assert_eq!(b.irq, Some((283, false)));
    }
}
