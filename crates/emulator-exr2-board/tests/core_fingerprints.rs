//! Complete fingerprints retain configuration that changes future hardware
//! behavior even when current registers and external levels are identical.
use fujimod_emulator_devices::{Device, exr2::irq, signals::Level};
use fujimod_emulator_exr2_system::boot;
use fujimod_emulator_exr2_i2c::{self as i2c};
use serde_json::json;
fn bus(connected: Option<bool>, bridge: bool) -> i2c::I2cController {
    let options = serde_json::from_value(
        json!({"base":0xfff85040u32,"irq":102,"modes":[9],"transfer_ticks":4,
        "profile":"declared-bus1-register-replies-unmeasured","parts":{"kind":"secondary",
        "register_address":10,"pwic_address":null,"register_modes":[9],"pwic_modes":[1],
        "register_values":[[2,0]],"register_masks":[],"pwic_values":[],"pwic_masks":[],
        "pwic_read_prefixes":[],"topology":[10],"connected":connected}}),
    )
    .unwrap();
    i2c::I2cController::new(
        options,
        i2c::Wiring {
            controllers: vec![(0, irq::Inputs::default()), (1, irq::Inputs::default())],
            cable: Some(Level::new(false)),
            bridge: bridge.then(Default::default),
        },
    )
    .unwrap()
}
#[test]
fn secondary_cable_policy_and_bridge_connection_are_not_erased_by_equal_registers() {
    let dynamic = bus(None, false);
    let fixed = bus(Some(false), false);
    let bridge = bus(None, true);
    assert_eq!(dynamic.save(), fixed.save());
    assert_eq!(dynamic.save(), bridge.save());
    let observed = dynamic.save();
    let a = dynamic.fingerprint().unwrap();
    assert_ne!(a, fixed.fingerprint().unwrap());
    assert_ne!(a, bridge.fingerprint().unwrap());
    assert_eq!(a, dynamic.fingerprint().unwrap());
    assert_eq!(observed, dynamic.save());
}
#[test]
fn boot_decoder_wiring_changes_the_fingerprint_without_a_register_write() {
    let model = |choice| {
        boot::Block98000::new(
            boot::Options {
                boot_source: 1,
                selection_reset: 0,
            },
            boot::Wiring {
                decoder: 0,
                selections: vec![
                    boot::Selection { word: 0, choice: 0 },
                    boot::Selection { word: 1, choice },
                ],
            },
        )
        .unwrap()
    };
    let a = model(1);
    let b = model(2);
    assert_eq!(a.save(), b.save());
    assert_ne!(a.fingerprint().unwrap(), b.fingerprint().unwrap());
}
#[test]
fn sdhci_clock_settling_input_is_complete_state_before_oscillation_starts() {
    let a = fujimod_emulator_ip::sdhci::Clocks::new(&[(0xfff5002c, 2)]).unwrap();
    let b = fujimod_emulator_ip::sdhci::Clocks::new(&[(0xfff5002c, 3)]).unwrap();
    assert_eq!(a.save()["clocks"], b.save()["clocks"]);
    assert_ne!(a.fingerprint().unwrap(), b.fingerprint().unwrap());
}

#[test]
fn native_audio_and_scanout_keep_bounds_and_timing_before_any_writes() {
    use fujimod_emulator_exr2_display::device as scanout;
    use fujimod_emulator_exr2_serial::dma::audio;
    let audio = |maximum| {
        audio::SerialDma::new(audio::Options {
            ram_bytes: 4096,
            max_transfer_bytes: maximum,
        })
        .unwrap()
    };
    let (a, b) = (audio(16), audio(32));
    assert_eq!(a.save(), b.save());
    assert_ne!(a.fingerprint().unwrap(), b.fingerprint().unwrap());
    let scanout = |packing: &str| {
        scanout::Scanout::new(scanout::Options {
            packing: packing.into(),
            matrix: "bt601-full".into(),
            graphics_profile: None,
            backdrop_profile: None,
            rectangle_profile: None,
            ram_bytes: 4096,
            max_pixels: 16,
            reset_ticks: 2,
        })
        .unwrap()
    };
    let (a, b) = (scanout("yycbcr"), scanout("yycrcb"));
    assert_eq!(a.save(), b.save());
    assert_ne!(a.fingerprint().unwrap(), b.fingerprint().unwrap());
}
