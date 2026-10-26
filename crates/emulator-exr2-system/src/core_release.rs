//! One-shot processor release from a declared software-interrupt output.
//! This models the retained boot handshake, not a measured reset circuit.
//! The target's reset state and entry PC belong to the execution graph; neither
//! CPU registers nor uploaded code are inspected by this model.
use fujimod_emulator_devices::{Cx, Device, exr2::irq::SoftwarePulse};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub released: bool,
}
pub struct Wiring {
    pub core: usize,
    pub request: SoftwarePulse,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    released: bool,
}
pub struct CoreRelease {
    wiring: Wiring,
    state: State,
}
impl CoreRelease {
    pub fn new(options: Options, wiring: Wiring) -> Result<Self, String> {
        if wiring.core >= 2 {
            return Err("EXR II release targets an absent processor".into());
        }
        Ok(Self {
            wiring,
            state: State {
                released: options.released,
            },
        })
    }
}
impl Device for CoreRelease {
    fujimod_emulator_devices::checkpoint_fields!(state: State);

    fn fingerprint(&self) -> std::result::Result<serde_json::Value, String> {
        Ok(self.save())
    }

    fn name(&self) -> &'static str {
        "core-release"
    }
    fn read(&mut self, cx: &mut Cx<'_>, _: u64, _: u8) -> u64 {
        cx.unmodeled();
        0
    }
    fn write(&mut self, cx: &mut Cx<'_>, _: u64, _: u8, _: u64) {
        cx.unmodeled();
    }
    fn next_deadline(&self, now: u64) -> Option<u64> {
        (!self.state.released && self.wiring.request.asserted()).then_some(now)
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        if self.next_deadline(cx.now()).is_some() {
            match cx.reset_core(self.wiring.core, false) {
                Ok(()) => self.state.released = true,
                Err(error) => cx.fail(error),
            }
        }
    }
    fn save(&self) -> serde_json::Value {
        serde_json::to_value(&self.state).expect("core release state")
    }
    fn restore(&mut self, value: &serde_json::Value) -> Result<(), String> {
        self.state = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::{Host, Record, exr2::irq};
    #[derive(Default)]
    struct Pins {
        now: u64,
        resets: Vec<(usize, bool)>,
        refuse: bool,
    }
    impl Host for Pins {
        fn now(&self) -> u64 {
            self.now
        }
        fn schedule(&mut self, _: usize, _: u64, _: u64) {
            panic!("no scheduled releases")
        }
        fn set_irq(&mut self, _: u32, _: bool) {
            panic!("no interrupt drive")
        }
        fn raise_irq(&mut self, _: usize, _: u32) -> Result<(), String> {
            panic!("no interrupt drive")
        }
        fn reset_core(&mut self, core: usize, asserted: bool) -> Result<(), String> {
            if self.refuse {
                return Err("unconnected reset".into());
            }
            self.resets.push((core, asserted));
            Ok(())
        }
        fn ram_read(&self, _: u64, _: &mut [u8]) -> Result<(), String> {
            panic!("release cannot inspect RAM")
        }
        fn ram_write(&mut self, _: u64, _: &[u8]) -> Result<(), String> {
            panic!("release cannot patch RAM")
        }
        fn record(&mut self, _: Record) {
            panic!("no release records")
        }
    }
    fn controller(pulse: irq::SoftwarePulse) -> (irq::InterruptController, irq::Inputs) {
        let inputs = irq::Inputs::default();
        (
            irq::InterruptController::new(
                irq::Options {
                    arbitration: irq::Arbitration::Unique,
                },
                irq::Wiring {
                    base: 0x1000,
                    inputs: inputs.clone(),
                    enables: None,
                    software: vec![irq::SoftwareOutput { line: 120, pulse }],
                },
            )
            .unwrap(),
            inputs,
        )
    }
    fn write(d: &mut irq::InterruptController, host: &mut Pins, word: u32) {
        let mut cx = Cx::new(host, 0, 0, 0);
        d.write(&mut cx, 0x10cc, 4, word as u64);
        assert_eq!(cx.refusal(), None);
    }
    #[test]
    fn only_the_declared_software_command_releases_once_and_observation_is_pure() {
        let request = irq::SoftwarePulse::default();
        let (mut irq, inputs) = controller(request.clone());
        let mut release = CoreRelease::new(
            Options { released: false },
            Wiring {
                core: 1,
                request: request.clone(),
            },
        )
        .unwrap();
        let mut host = Pins::default();
        inputs.pulse(120).unwrap();
        irq.advance(&mut Cx::new(&mut host, 0, 0, 0));
        assert!(!request.asserted());
        assert_eq!(release.next_deadline(100), None);
        write(&mut irq, &mut host, 1 << 23);
        assert_eq!(release.next_deadline(100), None);
        let controller_before = irq.save();
        let release_before = release.save();
        write(&mut irq, &mut host, 1 << 24);
        assert!(request.asserted());
        assert_eq!(release.next_deadline(100), Some(100));
        assert_eq!(release.save(), release_before);
        assert!(host.resets.is_empty());
        host.now = 100;
        release.advance(&mut Cx::new(&mut host, 0, 0, 1));
        assert_eq!(host.resets, [(1, false)]);
        assert_eq!(release.next_deadline(100), None);
        host.now = 200;
        release.advance(&mut Cx::new(&mut host, 0, 0, 1));
        write(&mut irq, &mut host, 1 << 24);
        release.advance(&mut Cx::new(&mut host, 0, 0, 1));
        assert_eq!(host.resets, [(1, false)]);
        irq.restore(&controller_before).unwrap();
        release.restore(&release_before).unwrap();
        assert!(!request.asserted());
        assert_eq!(release.next_deadline(200), None);
        assert_eq!(host.resets, [(1, false)]);
    }
    #[test]
    fn initial_release_refusal_invalid_core_and_bad_restore_have_no_hidden_effects() {
        let request = irq::SoftwarePulse::default();
        let (mut irq, _) = controller(request.clone());
        let mut host = Pins {
            refuse: true,
            ..Default::default()
        };
        write(&mut irq, &mut host, 1 << 24);
        let mut r = CoreRelease::new(
            Options { released: false },
            Wiring {
                core: 1,
                request: request.clone(),
            },
        )
        .unwrap();
        let before = r.save();
        let mut cx = Cx::new(&mut host, 0, 0, 0);
        r.advance(&mut cx);
        assert_eq!(cx.refusal(), Some("unconnected reset"));
        assert_eq!(r.save(), before);
        assert_eq!(r.next_deadline(0), Some(0));
        assert!(host.resets.is_empty());
        assert!(
            r.restore(&serde_json::json!({"released":false,"extra":1}))
                .is_err()
        );
        assert_eq!(r.save(), before);
        let mut r = CoreRelease::new(
            Options { released: true },
            Wiring {
                core: 1,
                request: request.clone(),
            },
        )
        .unwrap();
        assert_eq!(r.next_deadline(0), None);
        r.advance(&mut Cx::new(&mut host, 0, 0, 0));
        assert!(host.resets.is_empty());
        assert!(
            CoreRelease::new(Options { released: false }, Wiring { core: 2, request }).is_err()
        );
    }
}
