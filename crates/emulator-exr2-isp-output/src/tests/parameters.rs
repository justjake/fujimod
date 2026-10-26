use crate::*;
use fujimod_emulator_devices::Device;
use fujimod_emulator_devices::exr2::irq_routes::Irq;
use fujimod_emulator_devices::test_host::*;
use fujimod_emulator_exr2_isp_input::*;
use fujimod_emulator_image_processing::parameters::ParameterBank;
use fujimod_emulator_imaging::exr2_isp::*;
#[test]
fn parameter_domains_and_raw_video_refusals() {
    let mut h = TestHost::new();
    let mut d = ParameterBank::new(parameters::Options {}, Default::default()).unwrap();
    write(&mut d, &mut h, 0xfff19200, 0x1000100);
    assert_eq!(read(&mut d, &mut h, 0xfff19200), 0x1000100);
    let mut cx = h.cx();
    d.write(&mut cx, 0xfff19000, 4, 0x3f);
    assert!(cx.refusal().is_some());
    let mut d = RawConversion::new(
        fujimod_emulator_imaging::exr2_isp::options::raw::RegisterOptions {},
        (),
    )
    .unwrap();
    write(&mut d, &mut h, 0xfff42000, 1);
    assert_eq!(d.next_deadline(0), None);
    let mut cx = h.cx();
    d.read(&mut cx, 0xfff42000, 4);
    assert!(cx.refusal().is_some());
    let mut d = AvcCodec::new(
        avc::Options {
            backend: fujimod_catalog_construction::VideoEncoderBackend::Software,
        },
        Irq { core: 0, line: 75 },
    )
    .unwrap();
    for (address, pointer) in [
        (0xfff60400, 0x45123450),
        (0xfff60404, 0x46234560),
        (0xfff60408, 0x47345670),
    ] {
        write(&mut d, &mut h, address, pointer);
        assert_eq!(read(&mut d, &mut h, address), pointer as u64);
    }
    assert_eq!(d.next_deadline(0), None);
    assert!(h.effects.is_empty());
    // A main-image submission must validate its mode and DMA configuration.
    let mut cx = h.cx();
    d.write(&mut cx, 0xfff60c00, 4, 0x8000);
    assert!(cx.refusal().unwrap().contains("H.264"));
    assert_eq!(d.next_deadline(0), None);
    assert!(h.effects.is_empty());
    write(&mut d, &mut h, 0xfff60400, 0);
    // Alternate decoding requires its own syntax and DMA configuration.
    let mut cx = h.cx();
    d.write(&mut cx, 0xfff60c00, 4, 0x4000);
    assert!(
        cx.refusal()
            .unwrap()
            .contains("alternate stream configuration")
    );
    assert_eq!(d.next_deadline(0), None);
    assert!(h.effects.is_empty());
}
