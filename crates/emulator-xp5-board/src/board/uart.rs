//! The SoC's Arm PL011 UARTs (four ports at `0xff920000 + n * 0x1000`).
//!
//! A declared `pl011` window takes two options: `irq`, the port's level interrupt line, and
//! `rl78`, a virtual RL78 programming target with its code-flash size. Without a peer the
//! receiver stays empty. See `fujimod_emulator_ip_arm::pl011`.
use fujimod_emulator_ip_arm::pl011::{Pl011, Rl78Config, WINDOW};
use fujimod_emulator_devices::graph::DeviceWindow;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Options {
    #[serde(default)]
    irq: Option<u32>,
    #[serde(default)]
    rl78: Option<Rl78Config>,
}

pub fn port(window: &DeviceWindow) -> Result<Pl011, String> {
    if window.size != WINDOW {
        return Err("a pl011 window is one 4 KiB page".into());
    }
    let options: Options = toml::Value::Table(window.options.clone())
        .try_into()
        .map_err(|e| format!("pl011 options: {e}"))?;
    Pl011::with_rl78(window.base, options.irq, options.rl78)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::test_host::recording::RecordingHost;
    use fujimod_emulator_devices::{Cx, Device};

    const BASE: u64 = 0xff920000;
    const DR: u64 = 0;
    const FR: u64 = 0x18;
    const IMSC: u64 = 0x38;
    const MIS: u64 = 0x40;
    const ICR: u64 = 0x44;

    /// The X-T5 4.31 window of port 0.
    fn xt5_window() -> DeviceWindow {
        let mut windows = crate::boot::xt5_windows(&["pl011"]);
        windows.retain(|window| window.base == BASE);
        assert_eq!(windows.len(), 1);
        windows.remove(0)
    }
    fn write(u: &mut Pl011, h: &mut RecordingHost, offset: u64, value: u64) {
        let mut cx = Cx::new(h, 0, 0, 0);
        u.write(&mut cx, BASE + offset, 4, value);
        assert_eq!(cx.refusal(), None, "write {offset:#x} = {value:#x}");
    }
    fn read(u: &mut Pl011, h: &mut RecordingHost, offset: u64) -> u64 {
        let mut cx = Cx::new(h, 0, 0, 0);
        let value = u.read(&mut cx, BASE + offset, 4);
        assert_eq!(cx.refusal(), None, "read {offset:#x}");
        value
    }

    /// The update program's first frame (0x13d4e8c: baud rate set, 115200 baud, 3.3 V) is
    /// transmitted on the transmit interrupt of INTID 293 and answered by the declared peer.
    #[test]
    fn xt5_port_0_sends_on_its_interrupt_and_the_declared_peer_answers() {
        let window = xt5_window();
        assert_eq!((window.base, window.size), (BASE, 0x1000));
        let mut uart = port(&window).unwrap();
        let mut h = RecordingHost::default();
        // Transmit, receive and receive-timeout interrupts.
        write(&mut uart, &mut h, IMSC, 0x70);
        for byte in [1, 3, 0x9a, 0, 0x21, 0x42, 3] {
            write(&mut uart, &mut h, DR, byte);
            assert_eq!(h.irqs.last(), Some(&(293, true)));
            assert_ne!(read(&mut uart, &mut h, MIS) & 0x20, 0);
            write(&mut uart, &mut h, ICR, 0x20);
        }
        assert_eq!(read(&mut uart, &mut h, FR) & 0x10, 0, "a reply is waiting");
        let reply: Vec<u8> = (0..7).map(|_| read(&mut uart, &mut h, DR) as u8).collect();
        assert_eq!(reply, [2, 3, 6, 32, 0, 0xd7, 3]);
        assert_eq!(read(&mut uart, &mut h, MIS), 0);
        assert_eq!(h.irqs.last(), Some(&(293, false)));
    }

    #[test]
    fn a_window_of_another_size_or_an_unknown_option_is_refused() {
        let mut window = xt5_window();
        window.size = 0x2000;
        assert!(port(&window).is_err());
        let mut window = xt5_window();
        window.options.insert("baud".into(), toml::Value::Integer(115200));
        assert!(port(&window).is_err());
    }
}
