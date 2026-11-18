//! Connect the XP5 JPEG encoder and decoder windows to the shared
//! X-Processor blocks under their wide register contracts
//! (`fujimod_emulator_ip::jpeg`).
//!
//! With `XT5_SCENE` set, every packed 4:2:2 run encodes the host scene
//! (`fujimod_emulator_imaging::frame::host_still_422`) at its programmed size,
//! written into the programmed source window first. This is a functional
//! shortcut: sensor readout and the develop blocks (PPFP, IIFP, RIFP, RESZ,
//! RZL2, MIX, RTCH) are completion stubs that write no pixels, so without it
//! the JPEG source buffer stays zero. The scene is a host input outside the
//! run definition and the checkpoint.
use fujimod_emulator_devices::graph::{DeviceGraph, DeviceWindow};
use fujimod_emulator_ip::jpeg::{Decoder, Encoder, Registers, WINDOW};

/// The window's `irq` and `run_ticks` options; no other option is accepted.
fn options(window: &DeviceWindow, block: &str) -> Result<(u32, u64), String> {
    if window.size != WINDOW
        || window
            .options
            .keys()
            .any(|key| !matches!(key.as_str(), "irq" | "run_ticks"))
    {
        return Err(format!("invalid JPEG {block} window or options"));
    }
    let number = |key: &str| {
        window
            .options
            .get(key)
            .and_then(toml::Value::as_integer)
            .and_then(|n| u64::try_from(n).ok())
            .ok_or_else(|| format!("JPEG {block} requires {key}"))
    };
    let irq = u32::try_from(number("irq")?).map_err(|e| e.to_string())?;
    Ok((irq, number("run_ticks")?))
}

/// The XP5 JPEG decoder (kind `jpeg-decoder`): the shared decoder under its
/// X-Processor 5 contract (`Decoder::wide`).
pub fn register_decoder(bus: &mut DeviceGraph, window: &DeviceWindow) -> Result<(), String> {
    let (irq, run_ticks) = options(window, "decoder")?;
    bus.register(
        window.base,
        window.size,
        Box::new(Decoder::wide("xp5-jpeg-decoder", window.base, irq, run_ticks)?),
    )?;
    Ok(())
}

pub fn register(bus: &mut DeviceGraph, window: &DeviceWindow) -> Result<(), String> {
    let (irq, run_ticks) = options(window, "encoder")?;
    bus.register(
        window.base,
        window.size,
        Box::new(Encoder::new(
            "xp5-jpeg-encoder",
            window.base,
            irq,
            run_ticks,
            Registers::Wide,
        )?
        .with_scene(fujimod_emulator_imaging::frame::host_still_422)),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    /// The X-T5 4.31 playback fragment declares a decoder this board opens,
    /// and the board refuses options it does not know.
    #[test]
    fn the_xt5_decoder_window_opens() {
        use fujimod_emulator_devices::graph::{DeviceGraph, DeviceWindow};
        #[derive(serde::Deserialize)]
        struct Fragment {
            device: Vec<DeviceWindow>,
        }
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../catalog/firmware/xt5/4.31/xp5-boot/manifest.d/58-playback.toml"
        );
        let fragment: Fragment = toml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let window = fragment
            .device
            .iter()
            .find(|window| window.kind == "jpeg-decoder")
            .expect("a jpeg-decoder window");
        assert_eq!((window.base, window.size), (0xffd1_1000, 0x1000));
        let mut graph = DeviceGraph::new();
        super::register_decoder(&mut graph, window).unwrap();
        assert!(graph.at(0xffd1_1000).is_ok());
        let mut extra = window.clone();
        extra.base = 0xffd1_2000;
        extra.options.insert("format".into(), toml::Value::Integer(1));
        assert!(super::register_decoder(&mut DeviceGraph::new(), &extra).is_err());
    }

    use fujimod_emulator_devices::test_host::recording::RecordingHost;
    use fujimod_emulator_devices::{Cx, Device};
    use fujimod_emulator_imaging::frame::{BARS, Scene, render_422, ycbcr};
    use fujimod_emulator_ip::jpeg::*;

    const BASE: u64 = 0xffd1_0000;
    const RAM: u64 = 0x1_0000_0000;
    const SOURCE_AT: u64 = RAM + 0x1000;
    const STREAM_AT: u64 = RAM + 0x8_0000;
    const PITCH_PIXELS: usize = 288;

    fn bars(width: usize, height: usize) -> Option<Vec<u8>> {
        render_422(&Scene::Bars, width, height)
    }
    fn write(e: &mut Encoder, h: &mut RecordingHost, offset: u64, value: u32) {
        let mut cx = Cx::new(h, 0, 0, 0);
        e.write(&mut cx, BASE + offset, 4, value.into());
        assert!(cx.refusal().is_none(), "{offset:#x}");
    }
    fn read(e: &mut Encoder, h: &mut RecordingHost, offset: u64) -> u32 {
        let mut cx = Cx::new(h, 0, 0, 0);
        e.read(&mut cx, BASE + offset, 4) as u32
    }

    /// A run of a 256x96 window at (16, 8) of a zero (flat green) buffer
    /// encodes the colour bars, and leaves them in the source window.
    #[test]
    fn a_run_with_the_host_scene_encodes_the_colour_bars() {
        let (width, height) = (256usize, 96usize);
        let mut e = Encoder::new("xp5-jpeg-encoder", BASE, 207, 10, Registers::Wide)
            .unwrap()
            .with_scene(bars);
        let mut h = RecordingHost {
            ram: vec![0; 1 << 20],
            ram_base: RAM,
            ..RecordingHost::default()
        };
        write(&mut e, &mut h, FORMAT, PACKED_422);
        write(&mut e, &mut h, PITCH, PITCH_PIXELS as u32);
        write(&mut e, &mut h, WINDOW_ORIGIN, 8 << 16 | 16);
        write(&mut e, &mut h, SIZE, (height as u32) << 16 | width as u32);
        write(&mut e, &mut h, SOURCE_HIGH, (SOURCE_AT >> 32) as u32);
        write(&mut e, &mut h, SOURCE, SOURCE_AT as u32);
        write(&mut e, &mut h, DESTINATION_HIGH, (STREAM_AT >> 32) as u32);
        write(&mut e, &mut h, DESTINATION, STREAM_AT as u32);
        write(&mut e, &mut h, LIMIT, 0x4_0000);
        for _ in 0..32 {
            write(&mut e, &mut h, TABLES, 0x0202_0202);
        }
        write(&mut e, &mut h, CONTROL, START);
        h.now = e.next_deadline(h.now).unwrap();
        let mut cx = Cx::new(&mut h, 0, 0, 0);
        e.advance(&mut cx);
        assert_eq!(cx.refusal(), None);
        assert_eq!(read(&mut e, &mut h, STATUS) & DONE, DONE);
        let size = read(&mut e, &mut h, STREAM) as usize;
        let stream = h.bytes(STREAM_AT, size).unwrap();
        let (ycc, w, hh) =
            fujimod_emulator_image_codecs::jpeg::decode(&stream, 1024, 1024).unwrap();
        assert_eq!((w, hh), (width, height));
        // Sample the middle of each bar in the top half.
        for (bar, rgb) in BARS.iter().enumerate() {
            let (x, y) = (bar * width / 8 + width / 16, height / 4);
            let got = &ycc[(y * width + x) * 3..][..3];
            let want = ycbcr(*rgb);
            for c in 0..3 {
                assert!(
                    (i32::from(got[c]) - want[c]).abs() <= 6,
                    "bar {bar} channel {c}: {got:?} vs {want:?}"
                );
            }
        }
        // The bottom quarter is a grey ramp: neutral chroma, rising luma.
        let y = height - 4;
        let left = &ycc[(y * width + 8) * 3..][..3];
        let right = &ycc[(y * width + width - 8) * 3..][..3];
        assert!(left[0] + 100 < right[0], "{left:?} {right:?}");
        assert!(left[1].abs_diff(128) <= 6 && right[2].abs_diff(128) <= 6);
        // The scene sits in the programmed window of the source buffer.
        let line = h.bytes(SOURCE_AT + ((8 * PITCH_PIXELS + 16) * 2) as u64, width * 2).unwrap();
        assert_eq!(line, bars(width, height).unwrap()[..width * 2]);
        assert!(h.bytes(SOURCE_AT, 32).unwrap().iter().all(|&b| b == 0));
    }
}
