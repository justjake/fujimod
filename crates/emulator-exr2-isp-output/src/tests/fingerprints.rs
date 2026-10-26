use crate::combination;
use fujimod_emulator_devices::Device;
use fujimod_emulator_imaging::exr2_isp::dma;
use fujimod_emulator_devices::exr2::irq_routes::Irq;

#[test]
fn combination_fingerprint_keeps_reset_timing_before_any_writes() {
    let combination = |delay| {
        combination::Combination::new(
            fujimod_emulator_imaging::exr2_isp::options::combination::Options {
                reset_ticks: delay,
                clock: dma::Clock::Fixed { ticks: 10 },
                arithmetic: fujimod_emulator_imaging::exr2_isp::options::combination::Arithmetic::Unmodeled,
            },
            Irq { core: 0, line: 68 },
        )
        .unwrap()
    };
    let (a, b) = (combination(2), combination(3));
    assert_eq!(a.save(), b.save());
    assert_ne!(a.fingerprint().unwrap(), b.fingerprint().unwrap());
}
