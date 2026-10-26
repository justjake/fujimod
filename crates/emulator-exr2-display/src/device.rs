//! Native scanout registers and reset deadlines. Host rendering is observational.
use super::scanout::{self, BackdropProfile, Compositor, GraphicsProfile, RectangleComposition, ScanoutHost};
use fujimod_emulator_devices::exr2::scanout_frame::Frame;
use fujimod_emulator_devices::{Action, Cx, Device, Record};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub packing: String,
    pub matrix: String,
    pub graphics_profile: Option<String>,
    pub backdrop_profile: Option<String>,
    pub rectangle_profile: Option<String>,
    pub ram_bytes: u64,
    pub max_pixels: u64,
    pub reset_ticks: u64,
}

pub struct Scanout {
    compositor: Compositor,
    reset_ticks: u64,
    deadlines: BTreeMap<u64, u64>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    pipes: [scanout::Pipe; 2],
    deadlines: BTreeMap<u64, u64>,
}
impl Scanout {
    pub fn new(options: Options) -> Result<Self, String> {
        if !matches!(
            options.packing.as_str(),
            "yycbcr" | "yycrcb" | "yuyv" | "uyvy" | "rgb565"
        ) || !matches!(
            options.matrix.as_str(),
            "bt601-full" | "bt601-limited" | "bt709-full" | "bt709-limited"
        ) || options.ram_bytes == 0
            || options.ram_bytes > 0x40000000
            || options.max_pixels == 0
            || options.max_pixels > 32 * 1024 * 1024
            || options.reset_ticks == 0
            || options.reset_ticks > i64::MAX as u64
        {
            return Err("invalid scanout color, bounds or reset timing options".into());
        }
        let rectangle_composition = options
            .rectangle_profile
            .as_deref()
            .map(|v| RectangleComposition::from_name(v).ok_or("unknown rectangle interpretation"))
            .transpose()?;
        Ok(Self {
            compositor: Compositor {
                pipes: Default::default(),
                packing: options.packing,
                matrix: options.matrix,
                graphics_profile: options
                    .graphics_profile
                    .as_deref()
                    .map(GraphicsProfile::parse)
                    .transpose()?,
                backdrop_profile: options
                    .backdrop_profile
                    .as_deref()
                    .map(BackdropProfile::parse)
                    .transpose()?,
                rectangle_composition,
                ram_bytes: options.ram_bytes as i64,
                max_pixels: options.max_pixels as i64,
            },
            reset_ticks: options.reset_ticks,
            deadlines: BTreeMap::new(),
        })
    }
    pub fn metadata(&self) -> Value {
        json!({"device":"display-scanout", "packing":self.compositor.packing,"matrix":self.compositor.matrix,
            "graphics_profile":self.compositor.graphics_profile.as_ref().map(|p|p.as_str()),
            "backdrop_profile":self.compositor.backdrop_profile.as_ref().map(|p|p.as_str()),
            "instances":self.compositor.pipes.iter().enumerate().map(|(i,p)|json!({
                "instance":i,"image_base":format!("{:#x}",scanout::BASES[i]),
                "registers":p.regs.items().map(|(o,v)|(format!("{:#x}",scanout::BASES[i]+o as u64),format!("{v:#x}"))).collect::<BTreeMap<_,_>>(),
                "written_offsets":p.written.iter().map(|o|format!("{o:#x}")).collect::<Vec<_>>(),
                "graphics":self.compositor.graphics(i).json(),"reset_completions":p.reset_completions,
            })).collect::<Vec<_>>()})
    }
    /// Read current registers and addressed pixels; no counters or modeled state change.
    pub fn frame(
        &self,
        instance: usize,
        pixels: bool,
        host: &mut dyn ScanoutHost,
    ) -> Result<Option<Frame>, String> {
        if instance >= 2 {
            return Err("scanout instance must be 0 or 1".into());
        }
        self.compositor
            .frame(instance, 0, pixels, host)
            .map_err(|e| e.0)
    }
    /// Observe the main video plane before graphics and rectangle composition.
    pub fn main_frame(
        &self,
        instance: usize,
        pixels: bool,
        host: &mut dyn ScanoutHost,
    ) -> Result<Option<Frame>, String> {
        if instance >= 2 {
            return Err("scanout instance must be 0 or 1".into());
        }
        self.compositor
            .main_frame(instance, 0, pixels, host)
            .map_err(|e| e.0)
    }
    fn actions(&mut self, cx: &mut Cx<'_>, actions: Vec<Action>) {
        for action in actions {
            match action {
                Action::Log(record) => cx.record(record),
                Action::Schedule { token, .. } => {
                    let Some(due) = cx
                        .now()
                        .checked_add(self.reset_ticks)
                        .filter(|v| *v <= i64::MAX as u64)
                    else {
                        cx.fail("scanout reset deadline overflow");
                        return;
                    };
                    let pipe = scanout::reset_fields(token).0;
                    self.deadlines
                        .retain(|t, _| scanout::reset_fields(*t).0 != pipe);
                    self.deadlines.insert(token, due);
                }
                _ => cx.fail("unsupported scanout device effect"),
            }
        }
    }
}
impl Scanout {
    fn validate_state(&self, saved: Saved) -> Result<Saved, String> {
        if saved.deadlines.len() > 2
            || saved.deadlines.iter().any(|(token, due)| {
                *due > i64::MAX as u64
                    || scanout::reset_fields(*token).1
                        != saved.pipes[scanout::reset_fields(*token).0].generation
            })
            || saved.pipes.iter().enumerate().any(|(i, p)| {
                p.generation < 0
                    || p.generation > (u64::MAX >> 2) as i64
                    || p.reset_completions < 0
                    || p.regs
                        .items()
                        .any(|(o, _)| scanout::address(scanout::BASES[i] + o as u64, 4).is_err())
                    || p.written
                        .iter()
                        .any(|o| scanout::address(scanout::BASES[i] + *o as u64, 4).is_err())
            })
        {
            return Err("invalid scanout register/deadline state".into());
        }

        Ok(saved)
    }
}
impl Device for Scanout {
    fn scanout_source(&self) -> Option<&(dyn fujimod_emulator_devices::capability::ScanoutSource + 'static)> {
        Some(self)
    }
    fn scanout_source_mut(&mut self) -> Option<&mut (dyn fujimod_emulator_devices::capability::ScanoutSource + 'static)> {
        Some(self)
    }
    fn checkpoint(&self) -> Result<fujimod_emulator_devices::checkpoint::Checkpoint, String> {
        fujimod_emulator_devices::checkpoint::Checkpoint::encode(
            self.name(),
            &Saved {
                pipes: self.compositor.pipes.clone(),
                deadlines: self.deadlines.clone(),
            },
        )
    }
    fn validate_checkpoint(&self, saved: &fujimod_emulator_devices::checkpoint::Checkpoint) -> Result<(), String> {
        self.validate_state(saved.decode(self.name())?).map(|_| ())
    }
    fn restore_checkpoint(&mut self, saved: &fujimod_emulator_devices::checkpoint::Checkpoint) -> Result<(), String> {
        let state = self.validate_state(saved.decode(self.name())?)?;
        self.compositor.pipes = state.pipes;
        self.deadlines = state.deadlines;
        Ok(())
    }
    fn fingerprint(&self) -> Result<Value, String> {
        Ok(
            json!({"compositor":self.compositor,"reset_ticks":self.reset_ticks,"deadlines":self.deadlines}),
        )
    }

    fn name(&self) -> &'static str {
        "display-scanout"
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        match self.compositor.read(address, size.into()) {
            Ok(v) => v.into(),
            Err(_) => {
                cx.unmodeled();
                0
            }
        }
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        if scanout::address(address, size.into()).is_err() {
            cx.unmodeled();
            return;
        }
        let Ok(value) = u32::try_from(value) else {
            cx.fail("scanout value exceeds 32 bits");
            return;
        };
        if self
            .compositor
            .pipes
            .iter()
            .any(|p| p.generation >= (u64::MAX >> 2) as i64)
        {
            cx.fail("scanout reset generation exhausted");
            return;
        }
        let mut actions = Vec::new();
        if let Err(e) =
            self.compositor
                .write(cx.cpu as i64, address, size.into(), value, &mut actions)
        {
            cx.fail(e.0);
            return;
        }
        self.actions(cx, actions);
    }
    fn next_deadline(&self, _now: u64) -> Option<u64> {
        self.deadlines.values().min().copied()
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        while let Some((token, due)) = self
            .deadlines
            .iter()
            .map(|(t, d)| (*t, *d))
            .min_by_key(|(t, d)| (*d, *t))
        {
            if due > cx.now() {
                break;
            }
            self.deadlines.remove(&token);
            let mut actions = Vec::new();
            self.compositor
                .complete(token, cx.now() as i64, &mut actions);
            self.actions(cx, actions);
        }
    }
    fn pollable(&self, address: u64, size: u8) -> bool {
        scanout::address(address, size.into()).is_ok()
    }
    fn save(&self) -> Value {
        json!(Saved {
            pipes: self.compositor.pipes.clone(),
            deadlines: self.deadlines.clone()
        })
    }
    fn restore(&mut self, value: &Value) -> Result<(), String> {
        let saved: Saved = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        let saved = self.validate_state(saved)?;
        self.compositor.pipes = saved.pipes;
        self.deadlines = saved.deadlines;
        Ok(())
    }
}

/// The host supplies only addressed memory. Color conversion stays in Rust.
pub struct Observation<'a> {
    pub read: &'a mut dyn FnMut(u64, usize) -> Result<Vec<u8>, String>,
    pub diagnostic_hashes: bool,
}
impl ScanoutHost for Observation<'_> {
    fn diagnostic_hashes(&self) -> bool {
        self.diagnostic_hashes
    }
    fn read_bytes(
        &mut self,
        address: u64,
        count: usize,
        _cpu: i64,
    ) -> fujimod_emulator_devices::record::Result<Vec<u8>> {
        (self.read)(address, count).map_err(Into::into)
    }
    fn render_rgb(
        &mut self,
        raw: &[u8],
        width: usize,
        height: usize,
        stride: usize,
        packing: &str,
        matrix: &str,
    ) -> fujimod_emulator_devices::record::Result<Vec<u8>> {
        fujimod_emulator_imaging::packed_color::decode(
            raw,
            width as u64,
            height as u64,
            stride as u64,
            packing,
            matrix,
        )
        .map_err(Into::into)
    }
    fn encode_png(
        &mut self,
        _rgb: &[u8],
        _width: usize,
        _height: usize,
    ) -> fujimod_emulator_devices::record::Result<Option<Vec<u8>>> {
        Ok(None)
    }
    fn record(&mut self, _record: Record) -> fujimod_emulator_devices::record::Result<()> {
        Err("host scanout observation cannot emit device effects".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator_devices::test_host::{TestHost, advance, write};
    fn model() -> Scanout {
        model_with_ram(0x100000)
    }
    fn model_with_ram(ram_bytes: u64) -> Scanout {
        Scanout::new(Options {
            packing: "yycbcr".into(),
            matrix: "bt601-full".into(),
            graphics_profile: None,
            backdrop_profile: None,
            rectangle_profile: None,
            ram_bytes,
            max_pixels: 1000,
            reset_ticks: 100,
        })
        .unwrap()
    }
    fn graphics_model(profile: &str) -> Scanout {
        let mut d = model();
        d.compositor.graphics_profile = Some(GraphicsProfile::parse(profile).unwrap());
        d.compositor.backdrop_profile = Some(BackdropProfile::Black);
        d
    }
    fn graphics_window(
        d: &mut Scanout,
        host: &mut TestHost,
        layer: u64,
        source: u32,
        width: u32,
        height: u32,
        origin: u32,
    ) {
        let base = scanout::BASES[0];
        let control = d.compositor.read(base, 4).unwrap() & !0x0ff00000;
        // Group 0 format 3 and group 1 format 11, both mode 1.
        write(d, host, base, control | 11 << 24 | 3 << 20);
        for (offset, value) in [
            (0x34 + layer * 0x50, 1),
            (0x40 + layer * 0x50, 1),
            (0x44 + layer * 0x50, origin),
            (0x54 + layer * 0x50, (width - 1) << 16 | (height - 1)),
            (0x64 + layer * 0x50, source),
            (0x74 + layer * 0x50, 0),
            (0xe0 + layer * 4, 128),
        ] {
            write(d, host, base + offset, value);
        }
    }
    fn graphics_rgb(d: &Scanout, memory: &[u8]) -> Vec<u8> {
        let mut read = |address: u64, size: usize| {
            let start = (address & 0x3fffffff) as usize;
            Ok(memory[start..start + size].to_vec())
        };
        let mut observer = Observation {
            read: &mut read,
            diagnostic_hashes: true,
        };
        d.frame(0, true, &mut observer)
            .unwrap()
            .unwrap()
            .rgb
            .unwrap()
    }
    #[test]
    fn group1_format11_abgr_uses_byte_zero_alpha() {
        let mut d = graphics_model("mode1-abgr8888-straight");
        let mut host = TestHost::new();
        let mut memory = vec![0; 0x4000];
        memory[0x2000..0x2010].copy_from_slice(&[
            255, 30, 60, 210, 0, 20, 40, 60, 255, 100, 150, 200, 64, 40, 80, 120,
        ]);
        write(&mut d, &mut host, scanout::BASES[0] + 0x10, 0x10001);
        graphics_window(&mut d, &mut host, 1, 0x2000, 2, 2, 0);
        let before = d.fingerprint().unwrap();
        assert_eq!(
            graphics_rgb(&d, &memory),
            [210, 60, 30, 0, 0, 0, 200, 150, 100, 30, 20, 10]
        );
        assert_eq!(d.fingerprint().unwrap(), before);
    }
    #[test]
    fn group1_over_group0_overlap_and_window_alpha() {
        let profile = "mode1-abgr8888-straight";
        for x in [1, 2, 0] {
            let mut d = graphics_model(profile);
            let mut host = TestHost::new();
            let mut memory = vec![0; 0x4000];
            memory[0x2000..0x2020].copy_from_slice(&[20, 80, 160, 255].repeat(8));
            memory[0x3000..0x3004].copy_from_slice(&[255, 40, 80, 200]);
            write(&mut d, &mut host, scanout::BASES[0] + 0x10, 0x30001);
            graphics_window(&mut d, &mut host, 0, 0x2000, 4, 2, 0);
            graphics_window(&mut d, &mut host, 1, 0x3000, 1, 1, x << 16);
            for (alpha, color) in [(128, [200, 80, 40]), (64, [180, 80, 30])] {
                write(&mut d, &mut host, scanout::BASES[0] + 0xe4, alpha);
                let mut expected = [160, 80, 20].repeat(8);
                expected[x as usize * 3..x as usize * 3 + 3].copy_from_slice(&color);
                let before = d.fingerprint().unwrap();
                assert_eq!(
                    graphics_rgb(&d, &memory),
                    expected,
                    "{profile}, x={x}, alpha={alpha}"
                );
                assert_eq!(d.fingerprint().unwrap(), before);
            }
        }
    }
    #[test]
    fn scanout_observes_30_bit_sources_in_the_declared_aperture_above_512_mib() {
        let mut host = TestHost::new();
        for ram_bytes in [0x20000000, 0x3fff0000] {
            let mut d = model_with_ram(ram_bytes);
            for (offset, value) in [
                (0, 1),
                (0x10, 0x10000),
                (0x24, 0),
                (0x28, 0x10000),
                (0x2c, 0x3e100000),
                (0x30, 0),
                (0xd4, 0x10001000),
                (0xd8, 0),
                (0xdc, 0),
            ] {
                write(&mut d, &mut host, scanout::BASES[0] + offset, value);
            }
            let mut reads = vec![];
            let mut read = |address: u64, size: usize| {
                reads.push((address, size));
                Ok(vec![128u8; size])
            };
            let mut observer = Observation {
                read: &mut read,
                diagnostic_hashes: true,
            };
            let result = d.frame(0, true, &mut observer);
            if ram_bytes == 0x20000000 {
                assert!(result.is_err());
                assert!(reads.is_empty());
            } else {
                assert_eq!(result.unwrap().unwrap().rgb, Some(vec![128; 6]));
                assert!(!reads.is_empty());
                assert!(
                    reads
                        .iter()
                        .all(|&(address, size)| address == 0x7e100000 && size == 4)
                );
            }
        }
    }
    #[test]
    fn simultaneous_pipe_resets_survive_snapshot_and_settle_once_at_their_tick() {
        let mut host = TestHost::new();
        let mut d = model();
        for base in scanout::BASES {
            write(&mut d, &mut host, base, 0x10000);
            write(&mut d, &mut host, base, 0);
        }
        let saved = d.save();
        let checkpoint = d.checkpoint().unwrap();
        assert_eq!(d.deadlines.len(), 2);
        assert_eq!(d.next_deadline(0), Some(100));
        advance(&mut d, &mut host, 99);
        assert_eq!(d.save(), saved);
        let mut restored = model();
        restored.restore_checkpoint(&checkpoint).unwrap();
        advance(&mut d, &mut host, 100);
        advance(&mut restored, &mut host, 100);
        assert_eq!(d.save(), restored.save());
        assert_eq!(
            d.compositor.pipes.each_ref().map(|p| p.reset_completions),
            [1, 1]
        );
        assert_eq!(d.next_deadline(100), None);
        let done = d.save();
        advance(&mut d, &mut host, 1000);
        assert_eq!(d.save(), done);
        assert!(host.effects.is_empty());
    }
    #[test]
    fn host_pixels_and_metadata_queries_are_observational() {
        let mut host = TestHost::new();
        let mut d = model();
        for (o, v) in [
            (0, 1),
            (0x10, 0x10000),
            (0x24, 0),
            (0x28, 0x10000),
            (0x2c, 0x1000),
            (0x30, 0),
            (0xd4, 0x10001000),
            (0xd8, 0),
            (0xdc, 0),
        ] {
            write(&mut d, &mut host, scanout::BASES[0] + o, v);
        }
        let before = d.save();
        let meta = d.metadata();
        let mut read =
            |a: u64, n: usize| Ok(vec![128u8; (a.checked_add(n as u64).unwrap() - a) as usize]);
        let mut observer = Observation {
            read: &mut read,
            diagnostic_hashes: true,
        };
        let first = d.frame(0, true, &mut observer).unwrap().unwrap();
        let second = d.frame(0, true, &mut observer).unwrap().unwrap();
        assert_eq!(first.rgb, Some(vec![128; 6]));
        assert_eq!(first.state, second.state);
        assert_eq!(d.save(), before);
        assert_eq!(d.metadata(), meta);
    }
    #[test]
    fn scanout_reads_the_declared_high_ram_span_and_refuses_an_overrun_without_effects() {
        // Exercise scanout with the maximum declared RAM aperture.
        let options = |ram_bytes| Options {
            packing: "yycbcr".into(),
            matrix: "bt601-full".into(),
            graphics_profile: None,
            backdrop_profile: None,
            rectangle_profile: None,
            ram_bytes,
            max_pixels: 1000,
            reset_ticks: 100,
        };
        assert!(Scanout::new(options(0x40000001)).is_err());
        let mut d = Scanout::new(options(0x3fff0000)).unwrap();
        let mut host = TestHost::new();
        let source = 0x3ffefff8;
        for (o, v) in [
            (0, 1),
            (0x10, 0x10001),
            (0x24, 0),
            (0x28, 0x10001),
            (0x2c, source),
            (0x30, 0),
            (0xd4, 0x10001000),
            (0xd8, 0),
            (0xdc, 0),
        ] {
            write(&mut d, &mut host, scanout::BASES[0] + o, v);
        }
        let raw = [40, 60, 128, 128, 80, 100, 128, 128];
        let reads = std::cell::Cell::new(0);
        let mut read = |address, count| {
            assert_eq!((address, count), (u64::from(source) | 0x40000000, 8));
            reads.set(reads.get() + 1);
            Ok(raw.to_vec())
        };
        let mut observer = Observation {
            read: &mut read,
            diagnostic_hashes: true,
        };
        let before = d.fingerprint().unwrap();
        for _ in 0..2 {
            let frame = d.frame(0, true, &mut observer).unwrap().unwrap();
            assert_eq!((frame.width, frame.height), (2, 2));
            assert_eq!(frame.raw, raw);
            assert_eq!(
                frame.rgb,
                Some(vec![40, 40, 40, 60, 60, 60, 80, 80, 80, 100, 100, 100])
            );
            assert_eq!(d.fingerprint().unwrap(), before);
        }
        assert_eq!(reads.get(), 2);
        write(&mut d, &mut host, scanout::BASES[0] + 0x2c, source + 4);
        let before_refusal = d.fingerprint().unwrap();
        assert!(
            d.frame(0, true, &mut observer)
                .err()
                .unwrap()
                .contains("outside modeled RAM")
        );
        assert_eq!(d.fingerprint().unwrap(), before_refusal);
        assert_eq!(reads.get(), 2);
        assert_eq!(d.next_deadline(0), None);
        assert!(host.effects.is_empty());
    }
}

impl fujimod_emulator_devices::capability::ScanoutSource for Scanout {
    fn metadata(&self) -> serde_json::Value {
        Scanout::metadata(self)
    }
    fn frame(
        &self,
        instance: usize,
        pixels: bool,
        read: &mut dyn FnMut(u64, usize) -> std::result::Result<Vec<u8>, String>,
        diagnostic_hashes: bool,
    ) -> std::result::Result<Option<fujimod_emulator_devices::exr2::scanout_frame::Frame>, String> {
        Scanout::frame(self, instance, pixels, &mut Observation { read, diagnostic_hashes })
    }
    fn main_frame(
        &self,
        instance: usize,
        pixels: bool,
        read: &mut dyn FnMut(u64, usize) -> std::result::Result<Vec<u8>, String>,
        diagnostic_hashes: bool,
    ) -> std::result::Result<Option<fujimod_emulator_devices::exr2::scanout_frame::Frame>, String> {
        Scanout::main_frame(self, instance, pixels, &mut Observation { read, diagnostic_hashes })
    }
}
