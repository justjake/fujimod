//! Synthetic timing regression for the bank 1 scan-phase interrupt source.
use crate::display::{Config, Display};
use crate::panel_clock::PanelPll;
use fujimod_emulator_devices::xp5::panel_clocks::Clocks;
use fujimod_emulator_devices::{Cx, Device, Host, Record};

const BASE: u64 = 0xff9a0000;

#[derive(Default)]
struct TestHost {
    now: u64,
    events: Vec<(u64, u64)>,
    irqs: Vec<(u32, bool)>,
}
impl Host for TestHost {
    fn now(&self) -> u64 {
        self.now
    }
    fn schedule(&mut self, _: usize, at: u64, token: u64) {
        self.events.push((at, token));
    }
    fn set_irq(&mut self, line: u32, level: bool) {
        self.irqs.push((line, level));
    }
    fn raise_irq(&mut self, _: usize, _: u32) -> Result<(), String> {
        Err("unexpected per-core IRQ".into())
    }
    fn ram_read(&self, _: u64, _: &mut [u8]) -> Result<(), String> {
        Err("unexpected DMA read".into())
    }
    fn ram_write(&mut self, _: u64, _: &[u8]) -> Result<(), String> {
        Err("unexpected DMA write".into())
    }
    fn scanout(
        &mut self,
        _: fujimod_emulator_devices::graphics::scanout::Frame,
    ) -> Result<(), String> {
        Ok(())
    }
    fn record(&mut self, _: Record) {}
}
fn write(device: &mut dyn Device, host: &mut TestHost, address: u64, value: u64) {
    let mut cx = Cx::new(host, 0, 0, 0);
    device.write(&mut cx, address, 4, value);
    assert_eq!(cx.refusal(), None);
}
fn event(display: &mut Display, host: &mut TestHost, at: u64, token: u64) {
    host.now = at;
    let mut cx = Cx::new(host, 0, 0, 0);
    display.event(&mut cx, token);
    assert_eq!(cx.refusal(), None);
}

#[test]
fn bank1_phase_pulses_irq_186_and_disabled_bank_stays_quiet() {
    let clocks = Clocks::new();
    let mut host = TestHost::default();
    let mut pll = PanelPll::new(0xffa11000, clocks.clone()).unwrap();
    write(&mut pll, &mut host, 0xffa11000, 99);
    write(&mut pll, &mut host, 0xffa11040, 8);
    let mut display = Display::new(
        BASE,
        clocks,
        Config {
            period_ns: Some(30_000_000),
            ..Default::default()
        },
    )
    .unwrap();
    // Two active lines, one sync, one back porch, and two front porch.
    for (offset, value) in [
        (0x14, 2),
        (0x18, 0),
        (0x24, 2 | (1 << 16)),
        (0x28, 2 | (1 << 16)),
    ] {
        write(&mut display, &mut host, BASE + offset, value);
    }
    assert_eq!(display.next_deadline(0), None);
    event(&mut display, &mut host, 100_000_000, 0);
    assert!(host.irqs.is_empty());
    host.now = 0;
    write(&mut display, &mut host, BASE + 4, 1);
    let (first, token) = *host.events.last().unwrap();
    assert_eq!(first, 30_000_000);
    event(&mut display, &mut host, first - 1, token);
    assert!(host.irqs.is_empty());
    event(&mut display, &mut host, first, token);
    assert_eq!(host.irqs, [(192, true), (192, false)]);
    let (vsync, token) = *host.events.last().unwrap();
    assert_eq!(vsync, 40_000_000);
    event(&mut display, &mut host, vsync - 1, token);
    assert_eq!(host.irqs.len(), 2);
    event(&mut display, &mut host, vsync, token);
    // The device emits a pulse; it does not expose an IRQ acknowledgement register.
    assert_eq!(&host.irqs[2..], &[(186, true), (186, false)]);
    let (stale, token) = *host.events.last().unwrap();
    write(&mut display, &mut host, BASE + 4, 0);
    assert_eq!(display.next_deadline(host.now), None);
    event(&mut display, &mut host, stale, token);
    assert_eq!(host.irqs.len(), 4);
}
