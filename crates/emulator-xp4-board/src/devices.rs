//! Common Device/Host adapter owned by the Unicorn application CPU profile.
//! Devices receive physical RAM, guest time and IRQ lines, never CPU state.
use super::Cluster;
use fujimod_emulator_devices::graphics::scanout::Frame;
use fujimod_emulator_ip::usb::dwc3::{self, Dwc3};
use fujimod_emulator_xp4_io::adc::Adc;
use fujimod_emulator_xp4_storage::storage::{self, DmaController, SerialFlashController};
use fujimod_emulator_devices::{Cx, Device, Host, Record};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

struct Connected<T>(Arc<Mutex<T>>);
impl<T: Device> Device for Connected<T> {
    fn name(&self) -> &'static str {
        self.0.lock().unwrap().name()
    }
    fn read(&mut self, cx: &mut Cx<'_>, address: u64, size: u8) -> u64 {
        self.0.lock().unwrap().read(cx, address, size)
    }
    fn write(&mut self, cx: &mut Cx<'_>, address: u64, size: u8, value: u64) {
        self.0.lock().unwrap().write(cx, address, size, value);
    }
    fn advance(&mut self, cx: &mut Cx<'_>) {
        self.0.lock().unwrap().advance(cx);
    }
    fn event(&mut self, cx: &mut Cx<'_>, token: u64) {
        self.0.lock().unwrap().event(cx, token);
    }
    fn pollable(&self, address: u64, size: u8) -> bool {
        self.0.lock().unwrap().pollable(address, size)
    }
    fn set_scene(&mut self, scene: &serde_json::Value) -> Result<bool, String> {
        self.0.lock().unwrap().set_scene(scene)
    }
    fn finish(&mut self) -> Result<(), String> {
        self.0.lock().unwrap().finish()
    }
    fn next_deadline(&self, now: u64) -> Option<u64> {
        self.0.lock().unwrap().next_deadline(now)
    }
    fn save(&self) -> serde_json::Value {
        self.0.lock().unwrap().save()
    }
    fn restore(&mut self, state: &serde_json::Value) -> Result<(), String> {
        self.0.lock().unwrap().restore(state)
    }
}

pub struct Devices {
    models: Vec<Option<Box<dyn Device>>>,
    events: BTreeSet<(u64, usize, u64)>,
    windows: Vec<(u64, u64, usize)>,
    pub log: Vec<Record>,
    prior_records: usize,
    pins: Option<fujimod_emulator_ip::socionext::gpio::SharedPins>,
    adc: Option<(usize, Arc<Mutex<Adc>>)>,
    usb: Option<Arc<Mutex<Dwc3>>>,
    pub frames: BTreeMap<usize, Frame>,
    pub outputs: BTreeMap<usize, serde_json::Value>,
}
struct PhysicalHost<'a> {
    cluster: &'a Cluster,
    events: &'a mut BTreeSet<(u64, usize, u64)>,
    log: &'a mut Vec<Record>,
    prior_records: usize,
    frames: &'a mut BTreeMap<usize, Frame>,
    outputs: &'a mut BTreeMap<usize, serde_json::Value>,
}
impl Host for PhysicalHost<'_> {
    fn now(&self) -> u64 {
        unsafe { (*self.cluster.state.get()).ticks }
    }
    fn schedule(&mut self, device: usize, at: u64, token: u64) {
        self.events.insert((at, device, token));
    }
    fn set_irq(&mut self, line: u32, level: bool) {
        unsafe {
            (*self.cluster.state.get())
                .gic
                .set_line(line as usize, level);
        }
    }
    fn raise_irq(&mut self, _cpu: usize, _line: u32) -> Result<(), String> {
        Err("Per-core interrupt input is not wired for ARMv7-A".into())
    }
    fn ram_read(&self, address: u64, out: &mut [u8]) -> Result<(), String> {
        let source = self.cluster.memory_pointer(address, out.len(), 1)?;
        unsafe {
            std::ptr::copy_nonoverlapping(
                source,
                out.as_mut_ptr(),
                out.len(),
            );
        }
        Ok(())
    }
    fn ram_write(&mut self, address: u64, bytes: &[u8]) -> Result<(), String> {
        let target = self.cluster.memory_pointer(address, bytes.len(), 2)?;
        unsafe {
            std::ptr::copy_nonoverlapping(
                bytes.as_ptr(),
                target,
                bytes.len(),
            );
            let state = &mut *self.cluster.state.get();
            for monitor in &mut state.exclusive {
                if monitor.is_some_and(|(a, n)| {
                    a < address + bytes.len() as u64 && address < a + n as u64
                }) {
                    *monitor = None;
                }
            }
            state.cache_dirty = true;
        }
        Ok(())
    }
    fn scanout(&mut self, frame: Frame) -> Result<(), String> {
        self.frames.insert(frame.output, frame);
        Ok(())
    }
    fn output_status(&mut self, output: usize, state: serde_json::Value) -> Result<(), String> {
        if state["active"] != true {
            self.frames.remove(&output);
        }
        self.outputs.insert(output, state);
        Ok(())
    }
    fn record(&mut self, record: Record) {
        if self.prior_records + self.log.len() < 65536 {
            self.log.push(record);
        }
    }
}
impl Devices {
    pub fn set_adc_input(
        &mut self,
        c: &Cluster,
        channel: usize,
        sample: u16,
    ) -> Result<(), String> {
        let (id, adc) = self.adc.as_ref().ok_or("XP4 ADC inputs are not wired")?;
        let mut host = PhysicalHost {
            cluster: c,
            events: &mut self.events,
            log: &mut self.log,
            prior_records: self.prior_records,
            frames: &mut self.frames,
            outputs: &mut self.outputs,
        };
        adc.lock()
            .unwrap()
            .set_input(&mut Cx::new(&mut host, 0, 0, *id), channel, sample)
    }
    pub fn set_gpio_input(&mut self, port: usize, pin: usize, high: bool) -> Result<(), String> {
        if port >= 16 || pin >= 16 {
            return Err("Invalid XP4 external GPIO pin".into());
        }
        self.pins
            .as_ref()
            .ok_or("XP4 GPIO inputs are not wired")?
            .lock()
            .unwrap()
            .set_input(port, pin, high);
        Ok(())
    }
    pub fn gpio(&mut self, inputs: [u16; 16]) -> Result<(), String> {
        let base = 0xfff41000;
        if self.pins.is_some()
            || self
                .windows
                .iter()
                .any(|&(lo, hi, _)| base < hi && lo < base + 0x400)
        {
            return Err("Overlapping XP4 GPIO wiring".into());
        }
        let pins = fujimod_emulator_ip::socionext::gpio::Pins::new(inputs);
        let id = self.models.len();
        self.models
            .push(Some(Box::new(fujimod_emulator_ip::socionext::gpio::Gpio::new(
                base,pins.clone(),
            )?)));
        self.windows.push((base, base + 0x400, id));
        self.pins = Some(pins);
        Ok(())
    }
    pub fn temperature(&mut self, value: i32, ticks: u64) -> Result<(), String> {
        let ranges = [
            (0xfff4700c, 0xfff47010),
            (0xfff47028, 0xfff4702c),
            (0xfff47810, 0xfff47854),
        ];
        if ranges
            .iter()
            .any(|&(a, b)| self.windows.iter().any(|&(lo, hi, _)| a < hi && lo < b))
        {
            return Err("Overlapping XP4 temperature wiring".into());
        }
        let model = fujimod_emulator_xp4_io::temperature::Temperature::new(value, ticks)?;
        let id = self.models.len();
        self.models.push(Some(Box::new(model)));
        self.windows.extend(ranges.map(|(a, b)| (a, b, id)));
        Ok(())
    }
    pub fn power_spi(
        &mut self,
        base: u64,
        idle: bool,
        port: usize,
        pin: usize,
        active_high: bool,
        inputs: BTreeMap<u8, u16>,
        rtc_ticks: u64,
    ) -> Result<(), String> {
        let pins = self
            .pins
            .as_ref()
            .ok_or("Power SPI requires physical GPIO wiring")?
            .clone();
        let part = fujimod_emulator_xp4_io::power_serial::PowerSerial::new(
            pins,
            port,
            pin,
            active_high,
            idle,
            inputs,
            rtc_ticks,
        )?;
        self.spi_input(base, Box::new(part))
    }

    pub fn checksum_power_spi(
        &mut self,
        base: u64,
        idle: bool,
        port: usize,
        pin: usize,
        active_high: bool,
        inputs: BTreeMap<u8, u16>,
        disable_writes: Vec<(u8, u16)>,
        rtc: Option<fujimod_emulator_xp4_io::rtc::StagedConfig>,
        retained_registers: Vec<u8>,
    ) -> Result<(), String> {
        let pins = self
            .pins
            .as_ref()
            .ok_or("Checksum power SPI requires physical GPIO wiring")?
            .clone();
        let part = fujimod_emulator_xp4_io::checksum_power_serial::ChecksumPowerSerial::new(
            pins,
            port,
            pin,
            active_high,
            idle,
            inputs,
            disable_writes,
            rtc,
            retained_registers,
        )?;
        self.spi_input(base, Box::new(part))
    }

    pub fn adc(&mut self, inputs: [u16; 12], sample_ticks: u64) -> Result<(), String> {
        let base = fujimod_emulator_xp4_io::adc::BASE;
        if self
            .windows
            .iter()
            .any(|&(lo, hi, _)| base < hi && lo < base + 0x48)
        {
            return Err("Overlapping XP4 ADC window".into());
        }
        let model = Arc::new(Mutex::new(Adc::new(inputs, sample_ticks)?));
        let id = self.models.len();
        self.models.push(Some(Box::new(Connected(model.clone()))));
        self.adc = Some((id, model));
        self.windows.push((base, base + 0x48, id));
        Ok(())
    }

    pub fn usb(&mut self, config: dwc3::Config) -> Result<(), String> {
        let model = Dwc3::new(config)?;
        let ranges = [
            (0xc100, 0xc114),
            (0xc200, 0xc204),
            (0xc2c0, 0xc2c4),
            (0xc400, 0xc410),
            (0xc700, 0xc718),
            (0xc720, 0xc724),
            (0xc800, 0xca00),
        ];
        if self.usb.is_some()
            || ranges.iter().any(|&(a, b)| {
                self.windows
                    .iter()
                    .any(|&(lo, hi, _)| config.base + a < hi && lo < config.base + b)
            })
        {
            return Err("Duplicate or overlapping A7 DWC3 wiring".into());
        }
        let id = self.models.len();
        let model = Arc::new(Mutex::new(model));
        self.models.push(Some(Box::new(Connected(model.clone()))));
        self.windows
            .extend(ranges.map(|(a, b)| (config.base + a, config.base + b, id)));
        self.usb = Some(model);
        Ok(())
    }
    pub fn usb_phy(&mut self, config: fujimod_emulator_xp4_io::usb_phy::Config) -> Result<(), String> {
        let model = fujimod_emulator_xp4_io::usb_phy::Phy::new(config)?;
        let ranges = [
            (config.base, config.base + 8),
            (config.base + 0x400, config.base + 0x404),
        ];
        if ranges
            .iter()
            .any(|&(a, b)| self.windows.iter().any(|&(lo, hi, _)| a < hi && lo < b))
        {
            return Err("Overlapping XP4 USB PHY wiring".into());
        }
        let id = self.models.len();
        self.models.push(Some(Box::new(model)));
        self.windows.extend(ranges.map(|(a, b)| (a, b, id)));
        Ok(())
    }
    fn usb_model(&self) -> Result<std::sync::MutexGuard<'_, Dwc3>, String> {
        Ok(self
            .usb
            .as_ref()
            .ok_or("DWC3 is not wired")?
            .lock()
            .unwrap())
    }
    pub fn usb_link(&self, now: u64, connected: bool) -> Result<(), String> {
        self.usb_model()?.link(now, connected)
    }
    pub fn usb_reset(&self, now: u64) -> Result<(), String> {
        self.usb_model()?.bus_reset(now)
    }
    pub fn usb_transaction(
        &self,
        now: u64,
        endpoint: usize,
        setup: bool,
        out: Option<Vec<u8>>,
    ) -> Result<u64, String> {
        self.usb_model()?.transaction(now, endpoint, setup, out)
    }
    pub fn usb_reply(&self, id: u64) -> Result<Option<dwc3::Reply>, String> {
        Ok(self.usb_model()?.reply(id).cloned())
    }
    pub fn usb_consume(&self, id: u64) -> Result<(), String> {
        self.usb_model()?.consume_reply(id)
    }

    pub fn dmp(&mut self, base: u64, irq: u32) -> Result<(), String> {
        if irq >= 512
            || self
                .windows
                .iter()
                .any(|&(lo, hi, _)| base < hi && lo < base + 0x3000)
        {
            return Err("Invalid or overlapping XP4 DMP wiring".into());
        }
        let model = fujimod_emulator_ip_dmp::smaph::Renderer::new(base, irq)?;
        let id = self.models.len();
        self.models.push(Some(Box::new(model)));
        self.windows.push((base, base + 0x3000, id));
        Ok(())
    }
    pub fn lcdc(
        &mut self,
        base: u64,
        pixel_ticks: u64,
        irqs: [u32; 2],
        layout: Option<&str>,
        video_layout: Option<&str>,
    ) -> Result<(), String> {
        if self
            .windows
            .iter()
            .any(|&(lo, hi, _)| base < hi && lo < base + 0x604)
        {
            return Err("Duplicate XP4 LCDC window".into());
        }
        let picture = match layout {
            None => None,
            Some("bgra8888-straight") => Some(fujimod_emulator_xp4_display::lcdc::Picture::Bgra8888Straight),
            _ => return Err("Unsupported XP4 LCDC picture layout".into()),
        };
        let video = match video_layout {
            None => None,
            Some("yycbcr422") => Some(fujimod_emulator_xp4_display::lcdc::Video::YyCbCr422),
            _ => return Err("Unsupported XP4 LCDC video layout".into()),
        };
        let model = fujimod_emulator_xp4_display::lcdc::Lcdc::new(base, pixel_ticks, irqs, picture, video)?;
        let id = self.models.len();
        self.models.push(Some(Box::new(model)));
        self.windows.push((base, base + 0x604, id));
        Ok(())
    }
    pub fn sd_clocks(&mut self, words: &[(u64, u64)]) -> Result<(), String> {
        if words.iter().any(|&(address, _)| {
            self.windows
                .iter()
                .any(|&(lo, hi, _)| address < hi && lo < address + 2)
        }) {
            return Err("Overlapping SDHCI clock wiring".into());
        }
        let model = fujimod_emulator_ip::sdhci::Clocks::new(words)?;
        let id = self.models.len();
        self.windows
            .extend(words.iter().map(|&(address, _)| (address, address + 2, id)));
        self.models.push(Some(Box::new(model)));
        Ok(())
    }
    pub fn requests(&mut self, inputs: Vec<(u64, u64)>) -> Result<(), String> {
        let id = self.models.len();
        let model = fujimod_emulator_xp4_io::requests::Requests::new(&inputs)?;
        self.windows
            .extend(inputs.iter().map(|&(a, _)| (a, a + 4, id)));
        self.models.push(Some(Box::new(model)));
        Ok(())
    }
    pub fn movdc(&mut self, outputs: Vec<(u64, u32, u64, u64, u8)>) -> Result<(), String> {
        for (base, irq, reset, period, luma) in outputs {
            if self
                .windows
                .iter()
                .any(|&(lo, hi, _)| base < hi && lo < base + 0x10c)
            {
                return Err("Overlapping XP4 MOVDC window".into());
            }
            let model = fujimod_emulator_xp4_display::movdc::Movdc::new(base, irq, reset, period, luma)?;
            let id = self.models.len();
            self.models.push(Some(Box::new(model)));
            self.windows.push((base, base + 0x10c, id));
        }
        Ok(())
    }
    pub fn imaging(
        &mut self,
        map: (u64, u64, Vec<u32>, Vec<u32>, Vec<u32>),
        timing: (u64, u64, u64, u64),
        read_gate: (u16, u8),
        ibuf: Option<(u64, u32, u64, u64, u64, String)>,
    ) -> Result<(), String> {
        use fujimod_emulator_sensor_input::{sensor, slvs};
        let (command_base, receiver_base, normal_irqs, error_irqs, timing_irqs) = map;
        let (period, active, command_ticks, reset_ticks) = timing;
        if command_base != 0xfff6c000
            || receiver_base != 0xfff69000
            || normal_irqs.len() != 2
            || error_irqs.len() != 2
            || timing_irqs.len() != 2
            || normal_irqs
                .iter()
                .chain(&error_irqs)
                .chain(&timing_irqs)
                .any(|i| !(32..512).contains(i))
        {
            return Err("Invalid XP4 imaging map".into());
        }
        let sensor = sensor::Sensor::new(
            Some(sensor::ImageRoutes {
                dma: None,
                timing: timing_irqs.try_into().unwrap(),
            }),
            read_gate,
        );
        let command = sensor::Command::new(command_base, sensor.clone(), command_ticks)?;
        let mut receiver = slvs::Receiver::new(
            sensor,
            slvs::Wiring {
                base: receiver_base,
                normal_irqs,
                error_irqs,
                output_channels: vec![],
                timing_output: true,
                control: slvs::Control {
                    run_mask: 0x11,
                    run_value: 0,
                    reset_pulse: 0x10,
                    reset_ticks,
                },
            },
            period,
            active,
        )?;
        let mut extra: Vec<(u64, u64, Box<dyn Device>)> = vec![];
        if let Some((base, irq, reset, sram, bank_size, scene)) = ibuf {
            use fujimod_emulator_sensor_input::{input_buffer::{InputBuffer, Output, Wiring}, statistics::{Planes, Statistics}};
            if base != 0xfff70000
                || sram != 0xfff10000
                || bank_size != 0x7000
                || !(32..512).contains(&irq)
                || reset == 0
            {
                return Err("Invalid XP4 input-buffer wiring".into());
            }
            let mut statistics = Statistics::new(sram, bank_size as usize)?;
            statistics.set_scene(
                &serde_json::from_str(&scene).map_err(|e| format!("statistics scene: {e}"))?,
            )?;
            let input = InputBuffer::new(
                Wiring {
                    base,
                    irq,
                    status: 0xc,
                    irq_control: 0x2e4,
                    ready: 0x2e8,
                    max_offset: 0x2e8,
                    write_limit: 0x2e4,
                    enables: [0x100, 0],
                    causes: [0x100, 0],
                    outputs: vec![Output {
                        enable_word: 0,
                        bit: 0x100,
                        low: 0x254,
                        high: 0x258,
                        high_mask: 3,
                        columns: 16,
                        rows: 16,
                        planes: Planes::Rgb,
                    }],
                    reset_ticks: reset,
                    period_ticks: None,
                },
                Some(statistics.clone()),
            )?;
            receiver = receiver.with_input_buffer(input.clone());
            extra.push((base, 0x2ec, Box::new(input)));
            extra.push((sram, bank_size * 2, Box::new(statistics)));
        }
        for (base, size, model) in [
            (command_base, 0x244, Box::new(command) as Box<dyn Device>),
            (receiver_base, 0x254, Box::new(receiver) as Box<dyn Device>),
        ]
        .into_iter()
        .chain(extra)
        {
            if self
                .windows
                .iter()
                .any(|&(lo, hi, _)| base < hi && lo < base + size)
            {
                return Err("Overlapping XP4 imaging wiring".into());
            }
            let id = self.models.len();
            self.models.push(Some(model));
            self.windows.push((base, base + size, id));
        }
        Ok(())
    }
    pub fn empty() -> Self {
        Self {
            models: vec![],
            events: Default::default(),
            windows: vec![],
            log: vec![],
            prior_records: 0,
            pins: None,
            adc: None,
            usb: None,
            frames: BTreeMap::new(),
            outputs: BTreeMap::new(),
        }
    }
    pub fn io_groups(&mut self, base: u64) {
        let id = self.models.len();
        self.models
            .push(Some(Box::new(fujimod_emulator_devices::keyed_io::KeyedIoGroup::new(
                base,
            ))));
        self.windows.push((base, base + 0xa0, id));
    }
    pub fn decompressor(&mut self, base: u64, irq: u32) {
        let id = self.models.len();
        let high = std::sync::Arc::new(std::sync::Mutex::new([0; 2]));
        self.models
            .push(Some(Box::new(fujimod_emulator_devices::lz_osd::Decompressor::new(
                base, irq, high, None,
            ))));
        self.windows.push((base, base + 0x6c, id));
    }
    pub fn spi(&mut self, base: u64, miso: bool) -> Result<(), String> {
        self.spi_input(base, Box::new(fujimod_emulator_xp4_io::spi::FixedPin(miso)))
    }
    fn spi_input(
        &mut self,
        base: u64,
        input: Box<dyn fujimod_emulator_xp4_io::spi::Input>,
    ) -> Result<(), String> {
        if !(0xfff32000..=0xfff38000).contains(&base)
            || base & 0xfff != 0
            || self
                .windows
                .iter()
                .any(|&(lo, hi, _)| base < hi && lo < base + 0x28)
        {
            return Err("Invalid or duplicate XP4 SPI controller".into());
        }
        let id = self.models.len();
        self.models
            .push(Some(Box::new(fujimod_emulator_xp4_io::spi::Spi::new(
                base, input,
            ))));
        self.windows.push((base, base + 0x28, id));
        Ok(())
    }
    pub fn storage(
        jedec: u32,
        routes: Vec<u32>,
        image: Vec<u8>,
        geometry: (usize, usize, u64, u64),
        irq: u32,
    ) -> Result<Self, String> {
        let routes: [u32; 16] = routes
            .try_into()
            .map_err(|_| "XP4 DMA requires 16 physical IRQ routes")?;
        if routes.iter().any(|irq| !(32..512).contains(irq)) {
            return Err("Invalid XP4 DMA IRQ route".into());
        }
        if !(32..512).contains(&irq) {
            return Err("Invalid XP4 serial-flash IRQ".into());
        }
        let port = storage::port(
            image,
            jedec,
            storage::Geometry {
                erase_bytes: geometry.0,
                page_bytes: geometry.1,
                erase_ticks: geometry.2,
                page_ticks: geometry.3,
            },
        )?;
        Ok(Self {
            models: vec![
                Some(Box::new(SerialFlashController::new(port.clone(), irq))),
                Some(Box::new(DmaController::new(port, routes))),
            ],
            events: Default::default(),
            log: Vec::new(),
            prior_records: 0,
            pins: None,
            adc: None,
            usb: None,
            frames: BTreeMap::new(),
            outputs: BTreeMap::new(),
            windows: vec![
                (storage::CONTROLLER, storage::CONTROLLER + 0x100, 0),
                (storage::FLASH, storage::FLASH + storage::FLASH_SIZE, 0),
                (storage::DMA, storage::DMA + 0x10000, 1),
                (0xfff4a500, 0xfff4a520, 1),
            ],
        })
    }
    /// X-Processor Pro flash controller, its read aperture and the PL330 that
    /// copies from it. The flash contents are an explicit board input.
    pub fn xpp_storage(
        &mut self,
        storage: super::definition::XppStorage,
        image: Vec<u8>,
    ) -> Result<(), String> {
        use fujimod_emulator_xpp::flash::{self, Aperture, FlashController};
        struct ApertureSource(Arc<Aperture>);
        impl fujimod_emulator_ip_arm::pl330::Source for ApertureSource {
            fn contains(&self, address: u64, len: usize) -> bool {
                self.0.contains(address, len)
            }
            fn read(&self, address: u64, len: usize) -> Result<Vec<u8>, String> {
                self.0.read(address, len)
            }
        }
        let aperture = Arc::new(Aperture::new(
            storage.aperture,
            storage.aperture_size,
            image,
        )?);
        let controller = FlashController::new(
            flash::Options {base:storage.flash_controller,transfer_ticks:1},
            aperture.clone(), BTreeMap::new())?;
        let dma = fujimod_emulator_ip_arm::pl330::Pl330::new(
            storage.dma,
            storage.dma_irq_routes,
            vec![Arc::new(ApertureSource(aperture.clone()))],
            fujimod_emulator_ip_arm::pl330::Throughput {
                setup_ticks: storage.dma_setup_ticks,
                bytes_per_tick: storage.dma_bytes_per_tick,
            },
        )?;
        let ranges = [
            (
                storage.flash_controller,
                storage.flash_controller + flash::REGISTERS,
            ),
            (aperture.base(), aperture.base() + aperture.size()),
            (storage.dma, storage.dma + 0x1000),
        ];
        if ranges
            .iter()
            .any(|&(a, b)| self.windows.iter().any(|&(lo, hi, _)| a < hi && lo < b))
        {
            return Err("Overlapping X-Processor Pro storage window".into());
        }
        let id = self.models.len();
        self.models.push(Some(Box::new(controller)));
        self.models.push(Some(Box::new(dma)));
        self.windows.push((ranges[0].0, ranges[0].1, id));
        self.windows.push((ranges[1].0, ranges[1].1, id));
        self.windows.push((ranges[2].0, ranges[2].1, id + 1));
        Ok(())
    }
    /// One X-Processor Pro serial port without a peer.
    pub fn xpp_serial(&mut self, base: u64) -> Result<(), String> {
        use fujimod_emulator_xpp::serial::{REGISTERS, DiscoveryPort};
        if self
            .windows
            .iter()
            .any(|&(lo, hi, _)| base < hi && lo < base + REGISTERS)
        {
            return Err("Overlapping X-Processor Pro serial port".into());
        }
        let model = DiscoveryPort::new(base)?;
        let id = self.models.len();
        self.models.push(Some(Box::new(model)));
        self.windows.push((base, base + REGISTERS, id));
        Ok(())
    }
    fn owner(&self, address: u64, size: u32) -> Option<usize> {
        self.windows
            .iter()
            .find(|&&(lo, hi, _)| lo <= address && address + u64::from(size) <= hi)
            .map(|x| x.2)
    }
    pub fn owns(&self, address: u64, size: u32) -> bool {
        self.owner(address, size).is_some()
    }
    pub fn access(
        &mut self,
        c: &Cluster,
        core: usize,
        address: u64,
        size: u32,
        write: bool,
        value: u64,
    ) -> Result<Option<(u64, bool)>, String> {
        let Some(id) = self.owner(address, size) else {
            return Ok(None);
        };
        let mut model = self.models[id].take().unwrap();
        let mut host = PhysicalHost {
            cluster: c,
            events: &mut self.events,
            log: &mut self.log,
            prior_records: self.prior_records,
            frames: &mut self.frames,
            outputs: &mut self.outputs,
        };
        let mut cx = Cx::new(&mut host, core, 0, id);
        let reply = if write {
            model.write(&mut cx, address, size as u8, value);
            0
        } else {
            model.read(&mut cx, address, size as u8)
        };
        let unmodeled = cx.is_unmodeled();
        let refusal = cx.take_refusal();
        self.models[id] = Some(model);
        if let Some(reason) = refusal
            && !unmodeled
        {
            return Err(reason);
        }
        Ok(Some((reply, unmodeled)))
    }
    pub fn next_deadline(&self, now: u64) -> Option<u64> {
        self.events
            .iter()
            .next()
            .map(|e| e.0)
            .into_iter()
            .chain(
                self.models
                    .iter()
                    .flatten()
                    .filter_map(|m| m.next_deadline(now)),
            )
            .min()
    }
    pub fn due(&mut self, c: &Cluster) -> Result<(), String> {
        let now = unsafe { (*c.state.get()).ticks };
        for _ in 0..64 {
            let event = self.events.iter().next().copied().filter(|e| e.0 <= now);
            let id = event.map(|e| e.1).or_else(|| {
                self.models.iter().position(|m| {
                    m.as_ref()
                        .is_some_and(|m| m.next_deadline(now).is_some_and(|at| at <= now))
                })
            });
            let Some(id) = id else {
                return Ok(());
            };
            let mut model = self.models[id].take().unwrap();
            if let Some(e) = event {
                self.events.remove(&e);
            }
            let mut host = PhysicalHost {
                cluster: c,
                events: &mut self.events,
                log: &mut self.log,
                prior_records: self.prior_records,
                frames: &mut self.frames,
                outputs: &mut self.outputs,
            };
            let mut cx = Cx::new(&mut host, 0, 0, id);
            if let Some((_, _, token)) = event {
                model.event(&mut cx, token);
            } else {
                model.advance(&mut cx);
            }
            let refusal = cx.take_refusal();
            self.models[id] = Some(model);
            if let Some(reason) = refusal {
                return Err(reason);
            }
        }
        Err("ARMv7-A device made no deadline progress".into())
    }
    pub fn snapshots(&self) -> std::collections::BTreeMap<String, String> {
        self.models
            .iter()
            .flatten()
            .map(|m| (m.name().into(), m.save().to_string()))
            .collect()
    }
}

impl Cluster {
    pub fn devices_due(&self) -> Result<(), String> {
        let devices = unsafe { (*self.state.get()).devices.take() };
        if let Some(mut devices) = devices {
            let result = devices.due(self);
            unsafe {
                (*self.state.get()).devices = Some(devices);
            }
            result?;
        }
        Ok(())
    }
    pub fn device_access(
        &self,
        core: usize,
        address: u64,
        size: u32,
        write: bool,
        value: u64,
    ) -> Result<Option<(u64, bool)>, String> {
        let devices = unsafe { (*self.state.get()).devices.take() };
        let Some(mut devices) = devices else {
            return Ok(None);
        };
        let result = devices.access(self, core, address, size, write, value);
        unsafe {
            (*self.state.get()).devices = Some(devices);
        }
        result
    }
    pub fn flush_dma(&self) -> Result<(), String> {
        if unsafe { (*self.state.get()).cache_dirty } {
            for cpu in &self.cpus {
                cpu.invalidate_code(None)?;
            }
            unsafe {
                (*self.state.get()).cache_dirty = false;
            }
        }
        Ok(())
    }
}
