//! Common Device/Host adapter owned by the Unicorn application CPU profile.
//! Devices receive physical RAM, guest time and IRQ lines, never CPU state.
use fujimod_emulator_devices::{Cx, Device, checkpoint::Checkpoint};
use fujimod_emulator_ip::usb::dwc3::{self, Dwc3};
use fujimod_emulator_xp4_io::adc::Adc;
use fujimod_emulator_xp4_io::spi::Spi;
use fujimod_emulator_xp4_storage::serial_link::{self, SerialLinks};
use fujimod_emulator_xp4_storage::storage::{self, DmaController, SerialFlashController};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

/// A declared SPI controller as the XP4 DMA engine sees it: channels whose
/// fixed side is its data register move words through it.
struct SpiLink(Arc<Mutex<Spi>>);
impl SpiLink {
    fn spi(&self) -> Result<std::sync::MutexGuard<'_, Spi>, String> {
        self.0
            .lock()
            .map_err(|_| "XP4 SPI lock poisoned".to_owned())
    }
}
impl serial_link::SerialPeer for SpiLink {
    fn requests(&self) -> Result<u32, String> {
        Ok(self.spi()?.dma_requests())
    }
    fn check_idle(&self) -> Result<(), String> {
        self.spi()?.dma_check_idle()
    }
    fn begin(&self, start: u64, bytes: u64, width: u64) -> Result<serial_link::Exchange, String> {
        let exchange = self.spi()?.dma_begin(start, bytes, width)?;
        Ok(serial_link::Exchange {
            bits: exchange.bits,
            word_ticks: exchange.word_ticks,
            end: exchange.end,
        })
    }
    fn shift(&self, tx: u16, tick: u64) -> Result<u16, String> {
        self.spi()?.dma_shift(tx, tick)
    }
    fn finish(&self, start: u64, end: u64) -> Result<(), String> {
        self.spi()?.dma_finish(start, end)
    }
}

pub(crate) struct Connected<T>(pub(crate) Arc<Mutex<T>>);
/// The shared model's execute-in-place epoch, read through its lock.
impl<T: Device> fujimod_emulator_devices::capability::ExecuteInPlace for Connected<T> {
    fn epoch(&self) -> u64 {
        self.0
            .lock()
            .unwrap()
            .execute_in_place()
            .map_or(u64::MAX, |model| model.epoch())
    }
}
/// The shared model's analog inputs, through its lock.
impl<T: Device> fujimod_emulator_devices::capability::AnalogInputs for Connected<T> {
    fn samples(&self) -> Vec<u16> {
        self.0
            .lock()
            .unwrap()
            .analog_inputs()
            .map(|adc| adc.samples())
            .unwrap_or_default()
    }
    fn set_sample(&mut self, channel: usize, value: u16) -> Result<(), String> {
        self.0
            .lock()
            .unwrap()
            .analog_inputs_mut()
            .ok_or("not an analog input")?
            .set_sample(channel, value)
    }
}
/// The shared model's USB port, through its lock.
impl<T: Device> fujimod_emulator_devices::capability::UsbPort for Connected<T> {
    fn cable_connected(&self) -> bool {
        self.0
            .lock()
            .unwrap()
            .usb_port()
            .is_some_and(|port| port.cable_connected())
    }
    fn bus_status(&self) -> Result<serde_json::Value, String> {
        self.0
            .lock()
            .unwrap()
            .usb_port()
            .ok_or("not a USB port")?
            .bus_status()
    }
    fn host_reset(&mut self, cx: &mut Cx<'_>) -> Result<(), String> {
        self.0
            .lock()
            .unwrap()
            .usb_port_mut()
            .ok_or("not a USB port")?
            .host_reset(cx)
    }
    fn host_setup(&mut self, cx: &mut Cx<'_>, packet: &[u8]) -> Result<(), String> {
        self.0
            .lock()
            .unwrap()
            .usb_port_mut()
            .ok_or("not a USB port")?
            .host_setup(cx, packet)
    }
    fn host_out(&mut self, cx: &mut Cx<'_>, endpoint: u8, packet: &[u8]) -> Result<(), String> {
        self.0
            .lock()
            .unwrap()
            .usb_port_mut()
            .ok_or("not a USB port")?
            .host_out(cx, endpoint, packet)
    }
    fn host_in(&mut self, cx: &mut Cx<'_>, endpoint: u8, size: usize) -> Result<Vec<u8>, String> {
        self.0
            .lock()
            .unwrap()
            .usb_port_mut()
            .ok_or("not a USB port")?
            .host_in(cx, endpoint, size)
    }
    fn host_in_token(&mut self, cx: &mut Cx<'_>, endpoint: u8) -> Result<(), String> {
        self.0
            .lock()
            .unwrap()
            .usb_port_mut()
            .ok_or("not a USB port")?
            .host_in_token(cx, endpoint)
    }
    fn host_ready(
        &self,
        wait: fujimod_emulator_devices::capability::UsbHostWait,
    ) -> Result<bool, String> {
        self.0
            .lock()
            .unwrap()
            .usb_port()
            .ok_or("not a USB port")?
            .host_ready(wait)
    }
    fn host_in_wants_token(&self, endpoint: u8) -> Result<bool, String> {
        self.0
            .lock()
            .unwrap()
            .usb_port()
            .ok_or("not a USB port")?
            .host_in_wants_token(endpoint)
    }
    fn packet_bytes(&self) -> Result<usize, String> {
        self.0
            .lock()
            .unwrap()
            .usb_port()
            .ok_or("not a USB port")?
            .packet_bytes()
    }
}
impl<T: Device> Device for Connected<T> {
    fn name(&self) -> &'static str {
        self.0.lock().unwrap().name()
    }
    fn analog_inputs(
        &self,
    ) -> Option<&(dyn fujimod_emulator_devices::capability::AnalogInputs + 'static)> {
        if self.0.lock().unwrap().analog_inputs().is_some() {
            Some(self)
        } else {
            None
        }
    }
    fn analog_inputs_mut(
        &mut self,
    ) -> Option<&mut (dyn fujimod_emulator_devices::capability::AnalogInputs + 'static)> {
        if self.0.lock().unwrap().analog_inputs().is_some() {
            Some(self)
        } else {
            None
        }
    }
    fn usb_port(&self) -> Option<&(dyn fujimod_emulator_devices::capability::UsbPort + 'static)> {
        if self.0.lock().unwrap().usb_port().is_some() {
            Some(self)
        } else {
            None
        }
    }
    fn usb_port_mut(
        &mut self,
    ) -> Option<&mut (dyn fujimod_emulator_devices::capability::UsbPort + 'static)> {
        if self.0.lock().unwrap().usb_port().is_some() {
            Some(self)
        } else {
            None
        }
    }
    fn execute_in_place(
        &self,
    ) -> Option<&(dyn fujimod_emulator_devices::capability::ExecuteInPlace + 'static)> {
        if self.0.lock().unwrap().execute_in_place().is_some() {
            Some(self)
        } else {
            None
        }
    }
    fn read_without_advance(&self, address: u64, size: u8, now: u64) -> Option<u64> {
        self.0
            .try_lock()
            .ok()?
            .read_without_advance(address, size, now)
    }
    fn projected_value_until(
        &self,
        address: u64,
        size: u8,
    ) -> Option<fujimod_emulator_devices::bus::ReadValidity> {
        self.0.try_lock().ok()?.projected_value_until(address, size)
    }
    fn future_deadline_is_stable(&self) -> bool {
        self.0
            .try_lock()
            .is_ok_and(|device| device.future_deadline_is_stable())
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
    fn read_stable(&self, address: u64, size: u8) -> bool {
        self.0.lock().unwrap().read_stable(address, size)
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
    fn checkpoint(&self) -> Result<Checkpoint, String> {
        self.0
            .lock()
            .map_err(|_| "device lock poisoned")?
            .checkpoint()
    }
    fn validate_checkpoint(&self, saved: &Checkpoint) -> Result<(), String> {
        self.0
            .lock()
            .map_err(|_| "device lock poisoned")?
            .validate_checkpoint(saved)
    }
    fn restore_checkpoint(&mut self, saved: &Checkpoint) -> Result<(), String> {
        self.0
            .lock()
            .map_err(|_| "device lock poisoned")?
            .restore_checkpoint(saved)
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
    windows: Vec<(u64, u64, usize)>,
    pins: Option<fujimod_emulator_ip::socionext::gpio::SharedPins>,
    dials: BTreeMap<String, fujimod_emulator_devices::parts::encoder_input::EncoderInput>,
    adc: Option<(usize, Arc<Mutex<Adc>>)>,
    xpp_adc: Option<Arc<Mutex<fujimod_emulator_xpp::adc::Converter>>>,
    usb: Option<Arc<Mutex<Dwc3>>>,
    usb_presence: Option<Arc<std::sync::atomic::AtomicBool>>,
    /// Single words owned by a declared model inside another block's range.
    carved: Vec<(u64, u64)>,
    /// Declared SPI controllers the XP4 DMA engine can move words through,
    /// whichever of the two is declared first.
    serial_links: SerialLinks,
}

impl Devices {
    pub fn byte_i2c(&mut self, input: super::definition::ByteI2c) -> Result<(), String> {
        if !(32..512).contains(&input.irq)
            || input.base.checked_add(0x20).is_none_or(|end| end > 1 << 32)
        {
            return Err("Invalid ARM32 byte I2C wiring".into());
        }
        let model = fujimod_emulator_xp4_io::i2c::ByteI2c::new(
            input.base,
            input.irq,
            input.profile,
            input.peer,
        )?;
        let ranges = model.windows();
        if ranges
            .iter()
            .any(|&(a, b)| self.windows.iter().any(|&(lo, hi, _)| a < hi && lo < b))
        {
            return Err("Overlapping byte I2C window".into());
        }
        let id = self.models.len();
        self.windows
            .extend(ranges.into_iter().map(|(a, b)| (a, b, id)));
        self.models.push(Some(Box::new(model)));
        Ok(())
    }

    pub fn gpio(
        &mut self,
        base: u64,
        inputs: [u16; 16],
        interrupts: Vec<fujimod_emulator_devices::parts::gpio::InputIrq>,
    ) -> Result<(), String> {
        let end = base + 0x400;
        let carved: Vec<(u64, u64)> = self
            .carved
            .iter()
            .copied()
            .filter(|&(lo, hi)| base <= lo && hi <= end)
            .collect();
        if self.pins.is_some()
            || self
                .windows
                .iter()
                .any(|&(lo, hi, _)| base < hi && lo < end && !carved.contains(&(lo, hi)))
        {
            return Err("Overlapping GPIO wiring".into());
        }
        let pins = fujimod_emulator_ip::socionext::gpio::Pins::new(inputs);
        let id = self.models.len();
        self.models.push(Some(Box::new(
            fujimod_emulator_ip::socionext::gpio::Gpio::new(base, pins.clone())?
                .with_interrupts(interrupts)?,
        )));
        let mut lo = base;
        for &(a, b) in carved.iter().collect::<std::collections::BTreeSet<_>>() {
            if lo < a {
                self.windows.push((lo, a, id));
            }
            lo = b;
        }
        if lo < end {
            self.windows.push((lo, end, id));
        }
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
        command_output: Option<fujimod_emulator_xp4_io::power_serial::ExactCommandOutput>,
        bank_inputs: Vec<(bool, u8, u16)>,
        eye_commit: bool,
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
            command_output,
        )?
        .with_bank_inputs(bank_inputs)?
        .with_eye_commit(eye_commit);
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
    pub fn usb_phy(
        &mut self,
        config: fujimod_emulator_xp4_io::usb_phy::Config,
    ) -> Result<(), String> {
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

    pub fn dmp(&mut self, base: u64, irq: u32) -> Result<(), String> {
        if irq >= 512
            || self
                .windows
                .iter()
                .any(|&(lo, hi, _)| base < hi && lo < base + 0x3000)
        {
            return Err("Invalid or overlapping XP4 DMP wiring".into());
        }
        // The X-T3 LCD pixmap holds the renderer's RGBA8 words unchanged
        // (memory bytes A,B,G,R): see PixmapReadback::PreservesWords.
        let model = fujimod_emulator_ip_dmp::smaph::Renderer::new(
            base,
            irq,
            fujimod_emulator_ip_dmp::smaph::PixmapReadback::PreservesWords,
        )?;
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
            Some("abgr8888-straight") => Some(fujimod_framebuffer_xp4::Picture::Abgr8888Straight),
            _ => return Err("Unsupported XP4 LCDC picture layout".into()),
        };
        let video = match video_layout {
            None => None,
            Some("yycbcr422") => Some(fujimod_framebuffer_xp4::Video::YyCbCr422),
            _ => return Err("Unsupported XP4 LCDC video layout".into()),
        };
        let model =
            fujimod_emulator_xp4_display::lcdc::Lcdc::new(base, pixel_ticks, irqs, picture, video)?;
        let id = self.models.len();
        self.models.push(Some(Box::new(model)));
        self.windows.push((base, base + 0x604, id));
        Ok(())
    }
    pub fn sd_host(
        &mut self,
        host: crate::definition::SdHost,
        files: &[(String, Vec<u8>)],
        medium: Option<fujimod_emulator_devices::parts::sd_card::Card>,
    ) -> Result<(), String> {
        let base = host.options.base;
        if self
            .windows
            .iter()
            .any(|&(lo, hi, _)| base < hi && lo < base.saturating_add(0x1000))
        {
            return Err("Overlapping XP4 SD host".into());
        }
        let card = match medium {
            Some(mut card) => {
                if !files.is_empty() {
                    return Err(
                        "An SD host takes SD card files or an SD card medium, not both".into(),
                    );
                }
                card.validate()?;
                if card.sectors() != host.card.sectors {
                    return Err("SD card medium capacity differs from the declared SD card".into());
                }
                card.set_allocation_limit(host.card.max_dirty_sectors)?;
                card
            }
            None => fujimod_emulator_storage::fat::format(&host.card, files)?,
        };
        let contacts = fujimod_emulator_devices::parts::card_contacts::Contacts::new(true, false);
        let model = fujimod_emulator_xp4_io::sd::Controller::new(
            host.options,
            card,
            contacts,
            host.irq
                .map(fujimod_emulator_devices::bus::InterruptLine::Shared),
        )?;
        let id = self.models.len();
        self.models.push(Some(Box::new(model)));
        self.windows.push((base, base + 0x1000, id));
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
    pub fn functional_system_control(
        &mut self,
        config: fujimod_emulator_xp4_system::system_control::Config,
    ) -> Result<(), String> {
        let model = fujimod_emulator_xp4_system::system_control::SystemControl::new(config)?;
        let windows = model.windows();
        if windows
            .iter()
            .any(|&(a, b)| self.windows.iter().any(|&(lo, hi, _)| a < hi && lo < b))
        {
            return Err("Overlapping XP4 functional system-control window".into());
        }
        let id = self.models.len();
        self.windows
            .extend(windows.into_iter().map(|(a, b)| (a, b, id)));
        self.models.push(Some(Box::new(model)));
        Ok(())
    }
    pub fn requests(&mut self, inputs: Vec<(u64, u64)>) -> Result<(), String> {
        if inputs.iter().any(|&(a, _)| {
            self.windows
                .iter()
                .any(|&(lo, hi, _)| a < hi && lo < a.saturating_add(4))
        }) {
            return Err("Overlapping XP4 control request window".into());
        }
        let id = self.models.len();
        let model = fujimod_emulator_xp4_io::requests::Requests::new(&inputs)?;
        // Each request word is the `+0x80` word of a block; own the whole block
        // so its command, gate and completion words are modeled too.
        self.windows.extend(
            inputs
                .iter()
                .map(|&(a, _)| (a - 0x80, a - 0x80 + 0x100, id)),
        );
        self.models.push(Some(Box::new(model)));
        Ok(())
    }
    pub fn register_banks(&mut self, windows: Vec<(u64, u64)>) -> Result<(), String> {
        if windows
            .iter()
            .any(|&(lo, hi)| self.windows.iter().any(|&(a, b, _)| lo < b && a < hi))
        {
            return Err("Overlapping XP4 register bank window".into());
        }
        let model = fujimod_emulator_xp4_io::register_bank::RegisterBank::new(&windows)?;
        let id = self.models.len();
        self.windows
            .extend(windows.into_iter().map(|(lo, hi)| (lo, hi, id)));
        self.models.push(Some(Box::new(model)));
        Ok(())
    }
    pub fn request_acknowledgment(&mut self, ticks: u64) -> Result<(), String> {
        use fujimod_emulator_xp4_io::request_acknowledgment::{
            ADDRESS, RequestAcknowledgment, WIDTH,
        };
        let end = ADDRESS + u64::from(WIDTH);
        if self
            .windows
            .iter()
            .any(|&(lo, hi, _)| ADDRESS < hi && lo < end)
        {
            return Err("Overlapping XP4 request acknowledgment window".into());
        }
        let model = RequestAcknowledgment::new(ticks)?;
        let id = self.models.len();
        self.windows.push((ADDRESS, end, id));
        self.models.push(Some(Box::new(model)));
        Ok(())
    }
    pub fn countdown_timers(
        &mut self,
        base: u64,
        routes: &[(usize, u32)],
        dials: &[crate::definition::CountdownDial],
    ) -> Result<(), String> {
        use fujimod_emulator_xp4_io::countdown_timers::{CountdownTimers, DialWiring};
        let wiring: Vec<_> = dials
            .iter()
            .map(|d| DialWiring {
                channel: d.channel,
                phase_ticks: d.phase_ticks,
                rest_phase: d.rest_phase,
            })
            .collect();
        let model = CountdownTimers::new(base, routes, &wiring)?;
        for d in dials {
            if d.name.is_empty() || self.dials.contains_key(&d.name) {
                return Err("Invalid XP4 countdown dial name".into());
            }
            self.dials.insert(
                d.name.clone(),
                model
                    .dial_input(d.channel)
                    .ok_or("Unwired XP4 countdown dial")?,
            );
        }
        let windows = model.windows();
        if windows
            .iter()
            .any(|&(lo, hi)| self.windows.iter().any(|&(a, b, _)| lo < b && a < hi))
        {
            return Err("Overlapping XP4 countdown timer window".into());
        }
        let id = self.models.len();
        self.windows.extend(windows.map(|(lo, hi)| (lo, hi, id)));
        self.models.push(Some(Box::new(model)));
        Ok(())
    }
    pub fn jpeg_encoder(&mut self, wiring: &crate::definition::JpegEncoder) -> Result<(), String> {
        let model = fujimod_emulator_ip::jpeg::Encoder::new(
            "xp4-jpeg-encoder",
            wiring.base,
            wiring.irq,
            wiring.run_ticks,
            fujimod_emulator_ip::jpeg::Registers::Declared(crate::jpeg::contract(
                wiring.memory_type,
            )),
        )?;
        let windows = model.windows();
        if windows
            .iter()
            .any(|&(lo, hi)| self.windows.iter().any(|&(a, b, _)| lo < b && a < hi))
        {
            return Err("Overlapping XP4 JPEG encoder window".into());
        }
        let id = self.models.len();
        self.windows
            .extend(windows.into_iter().map(|(lo, hi)| (lo, hi, id)));
        self.models.push(Some(Box::new(model)));
        Ok(())
    }
    pub fn jpeg_decoder(&mut self, wiring: &crate::definition::JpegDecoder) -> Result<(), String> {
        let model = fujimod_emulator_ip::jpeg::Decoder::new(
            "xp4-jpeg-decoder",
            wiring.base,
            wiring.irq,
            wiring.run_ticks,
        )?;
        let lo = wiring.base;
        let hi = lo
            .checked_add(fujimod_emulator_ip::jpeg::WINDOW)
            .ok_or("Invalid JPEG decoder")?;
        if self.windows.iter().any(|&(a, b, _)| lo < b && a < hi) {
            return Err("Overlapping XP4 JPEG decoder window".into());
        }
        let id = self.models.len();
        self.windows.push((lo, hi, id));
        self.models.push(Some(Box::new(model)));
        Ok(())
    }
    pub fn still_engine(&mut self, wiring: &crate::definition::StillEngine) -> Result<(), String> {
        let mut model =
            crate::still::Engine::new(wiring.engine, wiring.base, wiring.irq, wiring.run_ticks)?;
        if wiring.compute {
            model = model.computing()?;
        }
        if let Some(raw) = wiring.raw {
            model = model.developing(raw)?;
        }
        if wiring.firmware_color {
            model = model.firmware_color()?;
        }
        if let Some(reference) = wiring.firmware_white_balance {
            model = model.firmware_white_balance(reference)?;
        }
        if wiring.firmware_luma_bands {
            model = model.firmware_luma_bands()?;
        }
        if wiring.firmware_chroma_matrix {
            model = model.firmware_chroma_matrix()?;
        }
        let windows = model.windows();
        for &(lo, hi) in &windows {
            if let Some(&(base, end, owner)) =
                self.windows.iter().find(|&&(a, b, _)| lo < b && a < hi)
            {
                let name = self
                    .models
                    .get(owner)
                    .and_then(Option::as_ref)
                    .map_or("unknown model", |device| device.name());
                return Err(format!(
                    "XP4 still engine {} at {lo:#x}..{hi:#x} overlaps {name} at {base:#x}..{end:#x}",
                    model.name(),
                ));
            }
        }
        let id = self.models.len();
        self.windows
            .extend(windows.into_iter().map(|(lo, hi)| (lo, hi, id)));
        self.models.push(Some(Box::new(model)));
        Ok(())
    }
    pub fn dsp(&mut self, wiring: &crate::definition::Dsp) -> Result<(), String> {
        let model = crate::dsp::Dsp::new(wiring.irq, wiring.run_ticks)?;
        let windows = model.windows();
        for &(lo, hi) in &windows {
            if let Some(&(a, b, id)) = self.windows.iter().find(|&&(a, b, _)| lo < b && a < hi) {
                let owner = self.models[id].as_ref().map_or("?", |m| m.name());
                return Err(format!(
                    "XP4 DSP window {lo:#x}..{hi:#x} overlaps {owner} {a:#x}..{b:#x}"
                ));
            }
        }
        let id = self.models.len();
        self.windows
            .extend(windows.into_iter().map(|(lo, hi)| (lo, hi, id)));
        self.models.push(Some(Box::new(model)));
        Ok(())
    }
    pub fn face_detector(
        &mut self,
        wiring: &crate::definition::FaceDetector,
    ) -> Result<(), String> {
        let model = fujimod_emulator_xp4_io::face_detector::FaceDetector::new(
            wiring.base,
            wiring.irq,
            wiring.run_ticks,
        )?;
        let windows = model.windows();
        if windows
            .iter()
            .any(|&(lo, hi)| self.windows.iter().any(|&(a, b, _)| lo < b && a < hi))
        {
            return Err("Overlapping XP4 face detector window".into());
        }
        let id = self.models.len();
        self.windows
            .extend(windows.into_iter().map(|(lo, hi)| (lo, hi, id)));
        self.models.push(Some(Box::new(model)));
        Ok(())
    }
    pub fn pl011_uart(&mut self, wiring: &crate::definition::Pl011Uart) -> Result<(), String> {
        use fujimod_emulator_ip_arm::pl011::{Pl011, WINDOW};
        let model = Pl011::with_rl78(wiring.base, wiring.irq, wiring.rl78.clone())?;
        let (lo, hi) = (wiring.base, wiring.base + WINDOW);
        if self.windows.iter().any(|&(a, b, _)| lo < b && a < hi) {
            return Err("Overlapping PL011 UART window".into());
        }
        let id = self.models.len();
        self.windows.push((lo, hi, id));
        self.models.push(Some(Box::new(model)));
        Ok(())
    }
    pub fn filter_engine(
        &mut self,
        wiring: &crate::definition::FilterEngine,
    ) -> Result<(), String> {
        let model = fujimod_emulator_xp4_io::filter_engine::FilterEngine::new(
            wiring.base,
            wiring.memory_types,
            wiring.irq,
            wiring.run_ticks,
        )?;
        let windows = model.windows();
        if windows
            .iter()
            .any(|&(lo, hi)| self.windows.iter().any(|&(a, b, _)| lo < b && a < hi))
        {
            return Err("Overlapping XP4 filter engine window".into());
        }
        let id = self.models.len();
        self.windows
            .extend(windows.into_iter().map(|(lo, hi)| (lo, hi, id)));
        self.models.push(Some(Box::new(model)));
        Ok(())
    }
    pub fn mali_v61(
        &mut self,
        options: fujimod_emulator_xp4_video::mali_v61::Options,
    ) -> Result<(), String> {
        let model = fujimod_emulator_xp4_video::mali_v61::MaliV61::new(options)?;
        let windows = model.windows();
        if windows
            .iter()
            .any(|&(lo, hi)| self.windows.iter().any(|&(a, b, _)| lo < b && a < hi))
        {
            return Err("Overlapping XP4 Mali-V61 window".into());
        }
        let id = self.models.len();
        self.windows
            .extend(windows.into_iter().map(|(lo, hi)| (lo, hi, id)));
        self.models.push(Some(Box::new(model)));
        Ok(())
    }
    pub fn usb_condition(&mut self, base: u64, present: bool) -> Result<(), String> {
        if self
            .windows
            .iter()
            .any(|&(lo, hi, _)| base < hi && lo < base + 4)
        {
            return Err("Overlapping XP4 USB condition register".into());
        }
        let model = fujimod_emulator_xp4_io::usb_condition::UsbCondition::new(base, present)?;
        self.usb_presence = Some(model.presence());
        let id = self.models.len();
        self.windows.push((base, base + 4, id));
        self.models.push(Some(Box::new(model)));
        Ok(())
    }
    pub fn movdc(
        &mut self,
        outputs: Vec<(u64, u32, u64, u64, u8)>,
        packed10: bool,
    ) -> Result<(), String> {
        for (base, irq, reset, period, luma) in outputs {
            if self
                .windows
                .iter()
                .any(|&(lo, hi, _)| base < hi && lo < base + 0x10c)
            {
                return Err("Overlapping XP4 MOVDC window".into());
            }
            let mut model =
                fujimod_emulator_xp4_display::movdc::Movdc::new(base, irq, reset, period, luma)?;
            if packed10 {
                model = model.accept_packed10();
            }
            let id = self.models.len();
            self.models.push(Some(Box::new(model)));
            self.windows.push((base, base + 0x10c, id));
        }
        Ok(())
    }
    pub fn imaging(
        &mut self,
        map: (u64, u64, Vec<u32>, Vec<u32>, Vec<u32>, Option<Vec<u32>>),
        timing: (u64, u64, u64, u64),
        gates: (Option<(u16, u8)>, Option<(u16, u8)>),
        ibuf: Option<(u64, u32, u64, u64, u64, String)>,
        statistics_in_memory: bool,
        frame_start_pulse: Option<u32>,
    ) -> Result<(), String> {
        use fujimod_emulator_sensor_input::{sensor, slvs};
        let (command_base, receiver_base, normal_irqs, error_irqs, timing_irqs, raw_dma_irqs) = map;
        let (period, active, command_ticks, reset_ticks) = timing;
        let raw_dma = raw_dma_irqs.as_deref().unwrap_or(&[]);
        if command_base != 0xfff6c000
            || receiver_base != 0xfff69000
            || normal_irqs.len() != 2
            || error_irqs.len() != 2
            || timing_irqs.len() != 2
            || raw_dma_irqs.as_ref().is_some_and(|irqs| irqs.len() != 2)
            || normal_irqs
                .iter()
                .chain(&error_irqs)
                .chain(&timing_irqs)
                .chain(raw_dma)
                .chain(&frame_start_pulse)
                .any(|i| !(32..512).contains(i))
        {
            return Err("Invalid XP4 imaging map".into());
        }
        // Declared RAW read-out DMA lines let the receiver write each frame
        // into the buffer an enabled channel names; without them the
        // channel windows refuse.
        let dma: Option<[u32; 2]> = raw_dma_irqs.map(|irqs| irqs.try_into().unwrap());
        let sensor = sensor::Sensor::new(
            Some(sensor::ImageRoutes {
                dma,
                timing: timing_irqs.try_into().unwrap(),
                // The X-T3 5.11 sensor driver writes the stride with the
                // buffer address: +0x58 = 0x31e0 for 6276 samples.
                line_stride: true,
                // Declared, an edge at every frame start with no enable or
                // status word, as on XP5. Whether X-T3 5.11 gates the line
                // with a +0x140 cause bit is not read.
                frame_start_pulse,
            }),
            sensor::Gates {
                read: gates.0,
                standby: gates.1,
            },
        );
        let command = sensor::Command::new(command_base, sensor.clone(), command_ticks)?
            .with_retained_words();
        let mut receiver = slvs::Receiver::new(
            sensor,
            slvs::Wiring {
                base: receiver_base,
                normal_irqs,
                error_irqs,
                output_channels: if dma.is_some() { vec![0, 1] } else { vec![] },
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
            use fujimod_emulator_sensor_input::{
                input_buffer::{InputBuffer, Output, Wiring},
                statistics::{Planes, Statistics},
            };
            if base != 0xfff70000
                || sram != 0xfff10000
                || bank_size != 0x7000
                || !(32..512).contains(&irq)
                || reset == 0
            {
                return Err("Invalid XP4 input-buffer wiring".into());
            }
            let mut statistics = if statistics_in_memory {
                Statistics::in_memory(sram, bank_size as usize)?
            } else {
                Statistics::new(sram, bank_size as usize)?
            };
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
                    enables: [0x3ff, 0x3df],
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
                        window: None,
                    }],
                    reset_ticks: reset,
                    period_ticks: None,
                },
                Some(statistics.clone()),
            )?;
            receiver = receiver.with_input_buffer(input.clone());
            extra.push((base, 0x2ec, Box::new(input)));
            if !statistics_in_memory {
                extra.push((sram, bank_size * 2, Box::new(statistics)));
            }
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
    /// The device models as nodes of the shared board's device graph, in
    /// model order, each with its windows. A word another model owns inside
    /// a block's range (`carved`) is cut out of that block's mappings.
    /// The models as a device graph, each node named by its model (a second
    /// model of a name takes its node index as a suffix).
    pub fn into_graph(
        self,
    ) -> Result<
        (
            fujimod_emulator_devices::graph::DeviceGraph,
            BTreeMap<String, usize>,
        ),
        String,
    > {
        use fujimod_emulator_devices::graph::{DeviceGraph, Mapping};
        let mut graph = DeviceGraph::new();
        let mut names = BTreeMap::new();
        // A model's node is named by its kind, numbered `kind#n` in
        // registration order when the board has several of that kind.
        let mut counts = BTreeMap::new();
        for model in self.models.iter().flatten() {
            *counts.entry(model.name()).or_insert(0usize) += 1;
        }
        let mut seen = BTreeMap::new();
        for (index, model) in self.models.into_iter().enumerate() {
            let model = model.ok_or("an XP4 device model is lent out")?;
            let kind = model.name();
            let name = if counts[kind] == 1 {
                kind.to_string()
            } else {
                let n = seen.entry(kind).or_insert(0usize);
                *n += 1;
                format!("{kind}#{}", *n - 1)
            };
            names.insert(name, index);
            let mut mappings = Vec::new();
            for &(lo, hi, owner) in &self.windows {
                if owner != index {
                    continue;
                }
                let mut start = lo;
                let mut holes: Vec<(u64, u64)> = self
                    .carved
                    .iter()
                    .copied()
                    .filter(|&(a, b)| (a, b) != (lo, hi) && lo <= a && b <= hi)
                    .collect();
                holes.sort_unstable();
                for (a, b) in holes {
                    if start < a {
                        mappings.push(Mapping {
                            base: start,
                            size: a - start,
                        });
                    }
                    start = b;
                }
                if start < hi {
                    mappings.push(Mapping {
                        base: start,
                        size: hi - start,
                    });
                }
            }
            if mappings.is_empty() {
                graph.attach(model);
            } else {
                graph.register_windows(mappings, model)?;
            }
        }
        // XP4 models share state through ports they do not declare (the
        // serial-flash controller and its DMA, the imaging chain), and this
        // board re-checks every model's deadline after any device access.
        let count = graph.nodes.len();
        for device in 0..count {
            graph.depend_on(
                device,
                (0..count).filter(|&source| source != device).collect(),
            )?;
        }
        Ok((graph, names))
    }
    pub fn empty() -> Self {
        Self {
            models: vec![],
            windows: vec![],
            pins: None,
            dials: BTreeMap::new(),
            adc: None,
            xpp_adc: None,
            usb: None,
            usb_presence: None,
            carved: Vec::new(),
            serial_links: SerialLinks::default(),
        }
    }
    pub fn io_groups(&mut self, base: u64) -> Result<(), String> {
        let end = base
            .checked_add(0xa0)
            .ok_or("Invalid XP4 IO-group extent")?;
        if self
            .windows
            .iter()
            .any(|&(lo, hi, _)| base < hi && lo < end)
        {
            return Err("Overlapping XP4 IO-group window".into());
        }
        let id = self.models.len();
        self.models.push(Some(Box::new(
            fujimod_emulator_devices::keyed_io::KeyedIoGroup::new(base),
        )));
        self.windows.push((base, end, id));
        Ok(())
    }
    pub fn decompressor(&mut self, base: u64, irq: u32) -> Result<(), String> {
        let end = base
            .checked_add(0x6c)
            .ok_or("Invalid decompressor extent")?;
        if self
            .windows
            .iter()
            .any(|&(lo, hi, _)| base < hi && lo < end)
        {
            return Err("Overlapping decompressor window".into());
        }
        let id = self.models.len();
        let high = std::sync::Arc::new(std::sync::Mutex::new([0; 2]));
        self.models.push(Some(Box::new(
            fujimod_emulator_devices::lz_osd::Decompressor::new(base, irq, high, None),
        )));
        self.windows.push((base, end, id));
        Ok(())
    }
    /// The in-body stabilizer controller behind an SPI controller: its input
    /// peer, which also follows the GPIO outputs it is wired to and drives
    /// its ready pad through the GPIO bank.
    pub fn stabilizer_link(
        &mut self,
        config: fujimod_emulator_xp4_io::stabilizer_link::Config,
    ) -> Result<(), String> {
        let pins = self
            .pins
            .as_ref()
            .ok_or("The stabilizer link requires physical GPIO wiring")?
            .clone();
        let base = config.base;
        let link = fujimod_emulator_xp4_io::stabilizer_link::StabilizerLink::new(pins, config)?;
        self.spi_input(base, Box::new(link))
    }
    /// Attach the X-mount lens to its declared SPI and GPIO controllers.
    pub fn xp4_lens_link(
        &mut self,
        config: fujimod_emulator_xp4_io::lens_link::Config,
    ) -> Result<fujimod_emulator_xp4_io::lens_link::LensLink, String> {
        let pins = self
            .pins
            .clone()
            .ok_or("The XP4 lens link requires GPIO wiring")?;
        let base = config.base;
        let lens = fujimod_emulator_xp4_io::lens_link::LensLink::new(pins, config)?;
        self.spi_input(base, Box::new(lens.clone()))?;
        Ok(lens)
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
        // The MMIO model and the DMA engine's link share one controller.
        let spi = Arc::new(Mutex::new(Spi::new(base, input)));
        let data_register = spi
            .lock()
            .map_err(|_| "XP4 SPI lock poisoned")?
            .data_register();
        self.serial_links
            .register(data_register, Arc::new(SpiLink(spi.clone())))?;
        let id = self.models.len();
        self.models.push(Some(Box::new(Connected(spi))));
        self.windows.push((base, base + 0x28, id));
        Ok(())
    }
    pub fn standalone_dma(
        &mut self,
        input: super::definition::StandaloneDma,
    ) -> Result<(), String> {
        let routes: [u32; 16] = input
            .irq_routes
            .try_into()
            .map_err(|_| "Standalone XP4 DMA requires 16 explicit IRQ routes")?;
        let model = DmaController::standalone(routes, &input.reset_profile, &input.timing_profile)?
            .with_serial_links(self.serial_links.clone());
        let windows = [
            (storage::DMA, storage::DMA + 0x10000),
            (0xfff4a500, 0xfff4a520),
            (0xfff4a800, 0xfff4a880),
        ];
        if windows
            .iter()
            .any(|&(a, b)| self.windows.iter().any(|&(lo, hi, _)| a < hi && lo < b))
        {
            return Err("Overlapping standalone XP4 DMA window".into());
        }
        let id = self.models.len();
        self.windows
            .extend(windows.into_iter().map(|(a, b)| (a, b, id)));
        self.models.push(Some(Box::new(model)));
        Ok(())
    }
    pub fn storage(
        jedec: u32,
        routes: Vec<u32>,
        image: Vec<u8>,
        geometry: (usize, usize, u64, u64),
        irq: u32,
        receiver_input: Option<super::definition::DiscoveryTxReceiver>,
        prefix: Option<storage::ImmutablePrefix>,
        executable: Option<storage::ExecutableRange>,
        mapped_read: Option<storage::MappedReadGate>,
        write_protect: bool,
        write_ranges: Vec<storage::WriteRange>,
        strict_backing: bool,
        erased_ranges: Vec<storage::ErasedRange>,
        audio: Option<storage::AudioPacing>,
        linked: Vec<usize>,
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
        let mut nor = storage::Nor::new(
            image,
            jedec,
            storage::Geometry {
                erase_bytes: geometry.0,
                page_bytes: geometry.1,
                erase_ticks: geometry.2,
                page_ticks: geometry.3,
            },
        )?;
        if let Some(prefix) = prefix {
            nor = nor.with_immutable_prefix(prefix)?;
        }
        if let Some(range) = executable {
            nor = nor.with_executable_range(range)?;
        }
        if write_protect {
            nor = nor.with_write_protect();
        }
        if strict_backing {
            nor = nor.with_strict_backing();
        }
        if !erased_ranges.is_empty() {
            nor = nor.with_erased_ranges(erased_ranges)?;
        }
        if !write_ranges.is_empty() {
            nor = nor.with_write_ranges(write_ranges)?;
        }
        let port = storage::try_port_from_nor(nor)?;
        let serial_links = SerialLinks::default();
        let mut dma = DmaController::new(port.clone(), routes)
            .with_audio(audio)?
            .with_linked(linked)?
            .with_serial_links(serial_links.clone());
        if let Some(input) = receiver_input {
            if input.address < 0xff000000 {
                return Err("Discovery transmit receiver requires a peripheral destination".into());
            }
            let mut receiver = fujimod_emulator_xp4_storage::tx_receiver::TxReceiver::new(
                input.address,
                input.request,
                input.capacity,
            )?;
            receiver.offer(input.offer_at, input.offer_bytes)?;
            dma = dma.with_receiver(std::sync::Arc::new(std::sync::Mutex::new(receiver)));
        }

        let mut controller = SerialFlashController::new(port.clone(), irq);
        if let Some(gate) = mapped_read {
            controller = controller.with_mapped_read_gate(gate)?;
        }
        let pad = controller.pad_register();
        let controller = Arc::new(Mutex::new(controller));
        let mut devices = Self {
            models: vec![Some(Box::new(Connected(controller))), Some(Box::new(dma))],
            pins: None,
            dials: BTreeMap::new(),
            adc: None,
            xpp_adc: None,
            usb: None,
            usb_presence: None,
            windows: vec![
                (storage::CONTROLLER, storage::CONTROLLER + 0x100, 0),
                (storage::FLASH, storage::FLASH + storage::FLASH_SIZE, 0),
                (storage::DMA, storage::DMA + 0x10000, 1),
                (0xfff4a500, 0xfff4a520, 1),
                (0xfff4a800, 0xfff4a880, 1),
            ],
            carved: Vec::new(),
            serial_links,
        };
        if let Some(pad) = pad {
            if devices
                .windows
                .iter()
                .any(|&(lo, hi, _)| pad < hi && lo < pad + 4)
            {
                return Err("Flash pad register overlaps the flash controller windows".into());
            }
            // One word the controller owns inside another block's register
            // range; that block is wired around it.
            devices.windows.push((pad, pad + 4, 0));
            devices.carved.push((pad, pad + 4));
        }
        Ok(devices)
    }
    /// X-Processor Pro flash controller, its read aperture and the PL330 that
    /// copies from it. The flash contents are an explicit board input.
    pub fn xpp_storage(
        &mut self,
        storage: super::definition::XppStorage,
        image: Vec<u8>,
        windows: Vec<Arc<dyn fujimod_emulator_ip_arm::pl330::Source>>,
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
            fn writable(&self) -> bool {
                true
            }
            fn write(&self, address: u64, bytes: &[u8]) -> Result<(), String> {
                self.0.store(address, bytes)
            }
        }
        let aperture = Arc::new(Aperture::new(
            storage.aperture,
            storage.aperture_size,
            image,
        )?);
        let (array, nor) = match storage.nor {
            Some(nor) => (
                Some(nor.array),
                BTreeMap::from([(
                    nor.chip_select,
                    fujimod_emulator_devices::parts::micron_nor::Registers::new(nor.registers)?,
                )]),
            ),
            None => (None, BTreeMap::new()),
        };
        let controller = FlashController::new(
            flash::Options {
                base: storage.flash_controller,
                transfer_ticks: 1,
                irq: storage.irq,
                array,
            },
            aperture.clone(),
            nor,
        )?;
        let dma = fujimod_emulator_ip_arm::pl330::Pl330::new(
            storage.dma,
            storage.dma_irq_routes,
            std::iter::once(Arc::new(ApertureSource(aperture.clone())) as Arc<_>)
                .chain(windows)
                .collect(),
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
    /// One X-Processor Pro serial port with its declared peer.
    pub fn xpp_serial(
        &mut self,
        base: u64,
        peer: fujimod_emulator_xpp::serial::Peer,
        clock: Option<fujimod_emulator_xpp::serial::Clock>,
        replies: BTreeMap<u32, u32>,
    ) -> Result<(), String> {
        use fujimod_emulator_xpp::serial::{DiscoveryPort, REGISTERS};
        if self
            .windows
            .iter()
            .any(|&(lo, hi, _)| base < hi && lo < base + REGISTERS)
        {
            return Err("Overlapping X-Processor Pro serial port".into());
        }
        let mut model = DiscoveryPort::attached(base, peer)?.with_replies(replies)?;
        if let Some(clock) = clock {
            model = model.with_clock(clock)?;
        }
        let id = self.models.len();
        self.models.push(Some(Box::new(model)));
        self.windows.push((base, base + REGISTERS, id));
        Ok(())
    }
    pub fn xpp_boot(
        &mut self,
        options: fujimod_emulator_xpp::boot::Options,
        card_strap: bool,
    ) -> Result<
        (
            Vec<fujimod_emulator_xpp::resets::ResetPulses>,
            fujimod_emulator_xpp::resets::ExternalLines,
        ),
        String,
    > {
        let bases: Vec<u64> = [
            options.selector_base,
            options.clock_base,
            options.ddr_base,
            options.remap_base,
        ]
        .into_iter()
        .chain(options.bus_bases.iter().copied())
        .chain(options.memory_port_bases.iter().copied())
        .collect();
        if bases.iter().any(|&base| {
            self.windows
                .iter()
                .any(|&(lo, hi, _)| base < hi && lo < base + 0x1000)
        }) {
            return Err("Overlapping X-Processor Pro boot controller".into());
        }
        let model = fujimod_emulator_xpp::boot::BootController::new(
            options,
            vec![fujimod_emulator_devices::signals::Level::new(card_strap)],
        )?;
        let lines = (0..3)
            .map(|n| model.sd_reset(n))
            .collect::<Result<Vec<_>, _>>()?;
        let external = model.external_lines();
        let id = self.models.len();
        self.models.push(Some(Box::new(model)));
        for base in bases {
            self.windows.push((base, base + 0x1000, id));
        }
        Ok((lines, external))
    }
    /// X-Processor Pro ADC with declared samples.
    pub fn xpp_adc(
        &mut self,
        base: u64,
        samples: &[u16],
        conversion_ticks: u64,
    ) -> Result<(), String> {
        use fujimod_emulator_xpp::adc::{Converter, WINDOW};
        if self
            .windows
            .iter()
            .any(|&(lo, hi, _)| base < hi && lo < base + WINDOW)
        {
            return Err("Overlapping X-Processor Pro ADC".into());
        }
        let model = Arc::new(Mutex::new(Converter::new(base, samples, conversion_ticks)?));
        let id = self.models.len();
        self.models.push(Some(Box::new(Connected(model.clone()))));
        self.xpp_adc = Some(model);
        self.windows.push((base, base + WINDOW, id));
        Ok(())
    }
    pub fn xpp_display_output(
        &mut self,
        output: &crate::definition::XppDisplayOutput,
    ) -> Result<(), String> {
        let (base, size) = (output.base, output.size);
        if self
            .windows
            .iter()
            .any(|&(lo, hi, _)| base < hi && lo < base.saturating_add(size))
        {
            return Err("Overlapping X-Processor Pro display output".into());
        }
        let model = fujimod_emulator_xpp::display::Output::new(
            base,
            size,
            &output.set_bits,
            &output.clear_bits,
            output
                .scanout
                .map(|(output, period)| fujimod_emulator_xpp::display::Scanout { output, period }),
        )?;
        let model = match output.handshake_ticks {
            Some(ticks) => model.with_handshake(ticks)?,
            None => model,
        };
        let id = self.models.len();
        self.models.push(Some(Box::new(model)));
        self.windows.push((base, base + size, id));
        Ok(())
    }
    pub fn xpp_clocks(
        &mut self,
        base: u64,
        lock_ticks: u64,
        running: Vec<usize>,
    ) -> Result<(), String> {
        use fujimod_emulator_xpp::clocks::{ClockControl, Options};
        if self
            .windows
            .iter()
            .any(|&(lo, hi, _)| base < hi && lo < base.saturating_add(0x1000))
        {
            return Err("Overlapping X-Processor Pro clock bank".into());
        }
        let model = ClockControl::new(Options {
            base,
            lock_ticks,
            running,
        })?;
        let id = self.models.len();
        self.models.push(Some(Box::new(model)));
        self.windows.push((base, base + 0x1000, id));
        Ok(())
    }
    /// The reset/gate bank; returns the SD channel reset lines and the
    /// external interrupt lines.
    pub fn xpp_reset_gates(
        &mut self,
        base: u64,
    ) -> Result<
        (
            Vec<fujimod_emulator_xpp::resets::ResetPulses>,
            fujimod_emulator_xpp::resets::ExternalLines,
        ),
        String,
    > {
        use fujimod_emulator_xpp::resets::{Gates, WINDOW};
        if self
            .windows
            .iter()
            .any(|&(lo, hi, _)| base < hi && lo < base.saturating_add(WINDOW))
        {
            return Err("Overlapping X-Processor Pro reset/gate bank".into());
        }
        let model = Gates::new(base)?;
        let lines = (0..3)
            .map(|channel| model.sd_reset(channel))
            .collect::<Result<Vec<_>, _>>()?;
        let external = model.external_lines();
        let id = self.models.len();
        self.models.push(Some(Box::new(model)));
        self.windows.push((base, base + WINDOW, id));
        Ok((lines, external))
    }
    /// An X-mount lens on its serial port, sharing `link` with the PL330.
    pub fn xpp_lens(
        &mut self,
        lens: crate::definition::XppLens,
        link: &fujimod_emulator_xpp::lens::LensLink,
        lines: fujimod_emulator_xpp::resets::ExternalLines,
    ) -> Result<(), String> {
        use fujimod_emulator_xpp::lens::{LensPort, Profile, Timing, Wiring};
        use fujimod_emulator_xpp::serial::REGISTERS;
        let base = lens.port;
        if self
            .windows
            .iter()
            .any(|&(lo, hi, _)| base < hi && lo < base + REGISTERS)
        {
            return Err("Overlapping X-Processor Pro lens port".into());
        }
        let pins = self
            .pins
            .as_ref()
            .ok_or("The lens requires GPIO wiring")?
            .clone();
        let model = LensPort::new(
            link,
            Profile {
                page: lens.page,
                replies: lens.replies.into_iter().collect(),
            },
            Timing {
                ready: lens.ready_ticks,
                release: lens.release_ticks,
                drive: lens.drive_ticks,
                poll: lens.poll_ticks,
            },
            Wiring {
                base,
                power: lens.power,
                ready: lens.ready,
                line: lens.line,
                contacts: lens.contacts,
            },
            pins,
            lines,
        )?;
        let id = self.models.len();
        self.models.push(Some(Box::new(model)));
        self.windows.push((base, base + REGISTERS, id));
        Ok(())
    }
    pub fn xpp_shutter(&mut self, shutter: crate::definition::XppShutter) -> Result<(), String> {
        use fujimod_emulator_xpp::shutter::{Shutter, Timing, Wiring};
        let pins = self
            .pins
            .as_ref()
            .ok_or("The shutter requires GPIO wiring")?
            .clone();
        let model = Shutter::new(
            Wiring {
                mode: shutter.mode,
                motor: shutter.motor,
                sensors: shutter.sensors,
                line: shutter.line,
            },
            Timing {
                step: shutter.step_ticks,
                poll: shutter.poll_ticks,
            },
            shutter.positions,
            pins,
        )?;
        self.models.push(Some(Box::new(model)));
        Ok(())
    }
    pub fn xpp_key_lines(
        &mut self,
        keys: crate::definition::XppKeyLines,
        lines: fujimod_emulator_xpp::resets::ExternalLines,
    ) -> Result<(), String> {
        let pins = self
            .pins
            .as_ref()
            .ok_or("Key lines require GPIO wiring")?
            .clone();
        let model =
            fujimod_emulator_xpp::keys::KeyLines::new(&keys.lines, keys.poll_ticks, pins, lines)?;
        self.models.push(Some(Box::new(model)));
        Ok(())
    }
    pub fn xpp_sd(
        &mut self,
        sd: crate::definition::XppSd,
        reset: Option<fujimod_emulator_xpp::resets::ResetPulses>,
        files: &[(String, Vec<u8>)],
    ) -> Result<(), String> {
        let base = sd.options.base;
        if self
            .windows
            .iter()
            .any(|&(lo, hi, _)| base < hi && lo < base.saturating_add(0x1000))
        {
            return Err("Overlapping X-Processor Pro SD channel".into());
        }
        let card = fujimod_emulator_storage::fat::format(&sd.card, files)?;
        let contacts = fujimod_emulator_devices::parts::card_contacts::Contacts::new(true, false);
        let mut model = fujimod_emulator_xpp::sd::Controller::new(
            sd.options,
            card,
            contacts,
            sd.irq
                .map(fujimod_emulator_devices::bus::InterruptLine::Shared),
        )?;
        if let Some(line) = reset {
            model = model.with_reset(line);
        }
        let id = self.models.len();
        self.models.push(Some(Box::new(model)));
        self.windows.push((base, base + 0x1000, id));
        Ok(())
    }
    pub fn xpp_register_block(
        &mut self,
        block: crate::definition::XppRegisterBlock,
        image: Option<(u32, [u8; 3])>,
    ) -> Result<(), String> {
        let frames = block.frames.map(|f| fujimod_emulator_xpp::blocks::Frames {
            start: f.start,
            stopped: f.stopped,
            irq: f.irq,
            period: f.period_ticks,
        });
        let mut model =
            fujimod_emulator_xpp::blocks::Block::new(block.base, block.size, block.idle, frames)?;
        if let Some(j) = block.job {
            model = model.with_job(fujimod_emulator_xpp::blocks::Job {
                start: j.start,
                status: j.status,
                stop: j.stop,
                irq: j.irq,
                ticks: j.ticks,
            })?;
        }
        if block.audio_converter {
            model = model.with_work(fujimod_emulator_xpp::blocks::Work::Audio)?;
        }
        if !block.fixed.is_empty() {
            model = model.with_fixed(block.fixed)?;
        }
        if !block.commands.is_empty() {
            model = model.with_commands(
                block
                    .commands
                    .iter()
                    .map(|c| fujimod_emulator_xpp::blocks::Command {
                        word: c.word,
                        value: c.value,
                        status: c.status,
                        mask: c.mask,
                        state: c.state,
                    })
                    .collect(),
            )?;
        }
        if let Some((lines, ycbcr)) = image {
            model = model.with_live_view_image(lines, ycbcr)?;
        }
        let end = block.base + model.window();
        if self
            .windows
            .iter()
            .any(|&(lo, hi, _)| block.base < hi && lo < end)
        {
            return Err("Overlapping X-Processor Pro register block".into());
        }
        let id = self.models.len();
        self.models.push(Some(Box::new(model)));
        self.windows.push((block.base, end, id));
        Ok(())
    }
    pub fn xpp_statistics(
        &mut self,
        base: u64,
        irq: u32,
        period: u64,
        scene: Option<[u32; 3]>,
        buffers: &[(u64, Vec<u32>)],
    ) -> Result<(), String> {
        use fujimod_emulator_xpp::statistics::{Engine, WINDOW};
        if self
            .windows
            .iter()
            .any(|&(lo, hi, _)| base < hi && lo < base.saturating_add(WINDOW))
        {
            return Err("Overlapping X-Processor Pro statistics engine".into());
        }
        let mut model = Engine::new(base, irq, period)?;
        if let Some(scene) = scene {
            model = model.with_scene(scene);
        }
        for (output, words) in buffers {
            model = model.with_buffer(*output, words)?;
        }
        let id = self.models.len();
        self.models.push(Some(Box::new(model)));
        self.windows.push((base, base + WINDOW, id));
        Ok(())
    }
    pub fn xpp_face(&mut self, base: u64, irq: u32, run_ticks: u64) -> Result<(), String> {
        use fujimod_emulator_xpp::face::{Engine, WINDOW};
        if self
            .windows
            .iter()
            .any(|&(lo, hi, _)| base < hi && lo < base.saturating_add(WINDOW))
        {
            return Err("Overlapping X-Processor Pro face detection engine".into());
        }
        let model = Engine::new(base, irq, run_ticks)?;
        let id = self.models.len();
        self.models.push(Some(Box::new(model)));
        self.windows.push((base, base + WINDOW, id));
        Ok(())
    }
    /// DesignWare HDMI TX with no sink attached.
    pub fn xpp_hdmi(&mut self, base: u64) -> Result<(), String> {
        use fujimod_emulator_ip::dw_hdmi::{Transmitter, WINDOW};
        if self
            .windows
            .iter()
            .any(|&(lo, hi, _)| base < hi && lo < base.saturating_add(WINDOW))
        {
            return Err("Overlapping DesignWare HDMI TX".into());
        }
        let model = Transmitter::new(base)?;
        let id = self.models.len();
        self.models.push(Some(Box::new(model)));
        self.windows.push((base, base + WINDOW, id));
        Ok(())
    }
    pub fn xpp_power(&mut self, base: u64) -> Result<(), String> {
        use fujimod_emulator_xpp::power::{Control, WINDOW};
        if self
            .windows
            .iter()
            .any(|&(lo, hi, _)| base < hi && lo < base.saturating_add(WINDOW))
        {
            return Err("Overlapping X-Processor Pro power control".into());
        }
        let model = Control::new(base)?;
        let id = self.models.len();
        self.models.push(Some(Box::new(model)));
        self.windows.push((base, base + WINDOW, id));
        Ok(())
    }
    pub fn xpp_dram(&mut self, base: u64, refresh_level: u8) -> Result<(), String> {
        use fujimod_emulator_xpp::dram::{Controller, WINDOW};
        if self
            .windows
            .iter()
            .any(|&(lo, hi, _)| base < hi && lo < base.saturating_add(WINDOW))
        {
            return Err("Overlapping X-Processor Pro DRAM controller".into());
        }
        let model = Controller::new(base, refresh_level)?;
        let id = self.models.len();
        self.models.push(Some(Box::new(model)));
        self.windows.push((base, base + WINDOW, id));
        Ok(())
    }
    pub fn xpp_i2c(&mut self, bus: crate::definition::XppI2c) -> Result<(), String> {
        use fujimod_emulator_ip::dw_i2c::{Controller, WINDOW};
        let base = bus.options.base;
        if self
            .windows
            .iter()
            .any(|&(lo, hi, _)| base < hi && lo < base.saturating_add(WINDOW))
        {
            return Err("Overlapping X-Processor Pro I2C bus".into());
        }
        let parts = fujimod_emulator_xpp::i2c::Parts::new(bus.parts)?;
        let model = Controller::new(bus.options, parts)?;
        let id = self.models.len();
        self.models.push(Some(Box::new(model)));
        self.windows.push((base, base + WINDOW, id));
        Ok(())
    }
    /// The imaging control page, less the frame interrupt words at `hole`
    /// that the frame sync owns.
    pub fn xpp_imaging_control(&mut self, base: u64, hole: Option<u64>) -> Result<(), String> {
        use fujimod_emulator_xpp::imaging::{Control, WINDOW};
        use fujimod_emulator_xpp::sync::INTERRUPT_WINDOW;
        if self
            .windows
            .iter()
            .any(|&(lo, hi, _)| base < hi && lo < base.saturating_add(WINDOW))
        {
            return Err("Overlapping X-Processor Pro imaging control".into());
        }
        let end = base + WINDOW;
        let ranges = match hole {
            None => vec![(base, end)],
            Some(at) if at > base && at & 7 == 0 && at + INTERRUPT_WINDOW < end => {
                vec![(base, at), (at + INTERRUPT_WINDOW, end)]
            }
            Some(_) => return Err("Frame interrupt words outside the imaging control page".into()),
        };
        let model = Control::new(base)?;
        let id = self.models.len();
        self.models.push(Some(Box::new(model)));
        self.windows
            .extend(ranges.into_iter().map(|(lo, hi)| (lo, hi, id)));
        Ok(())
    }
    pub fn xpp_frame_sync(&mut self, sync: crate::definition::XppFrameSync) -> Result<(), String> {
        use fujimod_emulator_xpp::sync::{Generator, INTERRUPT_WINDOW, WINDOW};
        let ranges = [
            (sync.base, sync.base.saturating_add(WINDOW)),
            (
                sync.interrupts,
                sync.interrupts.saturating_add(INTERRUPT_WINDOW),
            ),
        ];
        if ranges
            .iter()
            .any(|&(a, b)| self.windows.iter().any(|&(lo, hi, _)| a < hi && lo < b))
        {
            return Err("Overlapping X-Processor Pro frame sync".into());
        }
        let model = Generator::new(
            sync.base,
            sync.interrupts,
            sync.frame_lines,
            sync.period_ticks,
        )?;
        let id = self.models.len();
        self.models.push(Some(Box::new(model)));
        self.windows.extend(ranges.map(|(lo, hi)| (lo, hi, id)));
        Ok(())
    }
    /// X-Processor Pro display frame interrupts; the law has no register window.
    pub fn xpp_display_frames(
        &mut self,
        irqs: Vec<u32>,
        period: u64,
        timing: Option<([u64; 2], u64)>,
    ) -> Result<(), String> {
        if let Some((bases, stop_ticks)) = timing {
            let routes: Vec<u8> = irqs
                .iter()
                .map(|&n| u8::try_from(n).map_err(|_| "Display phase IRQ exceeds 255"))
                .collect::<Result<_, _>>()?;
            let routes: [[u8; 3]; 2] = routes
                .chunks(3)
                .map(|r| {
                    r.try_into()
                        .map_err(|_| "Display needs three IRQs per bank")
                })
                .collect::<Result<Vec<_>, _>>()?
                .try_into()
                .map_err(|_| "Display needs two timing banks")?;
            let model = fujimod_emulator_xp4_display::timing::XppTiming::new(
                bases, routes, period, stop_ticks,
            )?;
            let windows = model.windows();
            if windows.iter().any(|&(base, end)| {
                self.windows
                    .iter()
                    .any(|&(lo, hi, _)| base < hi && lo < end)
            }) {
                return Err("Overlapping display timing bank".into());
            }
            let id = self.models.len();
            self.models.push(Some(Box::new(model)));
            self.windows.extend(windows.map(|(lo, hi)| (lo, hi, id)));
            return Ok(());
        }
        let model = fujimod_emulator_xpp::display::FrameClock::new(irqs, period)?;
        self.models.push(Some(Box::new(model)));
        Ok(())
    }
    pub fn guard_windows(&self) -> Vec<(u64, u64)> {
        self.windows.iter().map(|&(lo, hi, _)| (lo, hi)).collect()
    }
}

#[cfg(test)]
mod io_group_admission_tests {
    use super::*;
    #[test]
    fn overflow_and_duplicate_windows_refuse_before_mutating_devices() {
        let mut devices = Devices::empty();
        let before = (devices.models.len(), devices.windows.clone());
        assert!(devices.io_groups(u64::MAX - 0x9f).is_err());
        assert_eq!((devices.models.len(), devices.windows.clone()), before);
        devices.io_groups(0xfff42000).unwrap();
        let before = (devices.models.len(), devices.windows.clone());
        for base in [0xfff42000, 0xfff41fff, 0xfff4209f] {
            assert!(devices.io_groups(base).is_err());
            assert_eq!((devices.models.len(), devices.windows.clone()), before);
        }
    }
}

#[cfg(test)]
mod decompressor_admission_tests {
    use super::*;
    #[test]
    fn overflow_and_overlap_refuse_before_mutation() {
        let mut devices = Devices::empty();
        let before = (devices.models.len(), devices.windows.clone());
        assert!(devices.decompressor(u64::MAX - 0x6b, 128).is_err());
        assert_eq!((devices.models.len(), devices.windows.clone()), before);
        devices.decompressor(0xfff79000, 128).unwrap();
        let before = (devices.models.len(), devices.windows.clone());
        for base in [0xfff79000, 0xfff78fff, 0xfff7906b] {
            assert!(devices.decompressor(base, 128).is_err());
            assert_eq!((devices.models.len(), devices.windows.clone()), before);
        }
    }
}

#[cfg(test)]
mod imaging_admission_tests {
    use super::*;
    fn map(
        raw_dma: Option<Vec<u32>>,
    ) -> (u64, u64, Vec<u32>, Vec<u32>, Vec<u32>, Option<Vec<u32>>) {
        (
            0xfff6c000,
            0xfff69000,
            vec![296, 298],
            vec![297, 297],
            vec![102, 100],
            raw_dma,
        )
    }
    #[test]
    fn raw_dma_lines_are_two_valid_interrupts_or_absent() {
        for raw_dma in [Some(vec![97]), Some(vec![97, 98, 99]), Some(vec![97, 600])] {
            let mut devices = Devices::empty();
            assert!(
                devices
                    .imaging(
                        map(raw_dma),
                        (100, 50, 10, 8),
                        (Some((0x59, 4)), Some((0, 1))),
                        None,
                        false,
                        None
                    )
                    .is_err()
            );
            assert!(devices.models.is_empty() && devices.windows.is_empty());
        }
        for raw_dma in [None, Some(vec![97, 98])] {
            let mut devices = Devices::empty();
            devices
                .imaging(
                    map(raw_dma),
                    (100, 50, 10, 8),
                    (Some((0x59, 4)), Some((0, 1))),
                    None,
                    false,
                    None,
                )
                .unwrap();
            assert_eq!(devices.windows.len(), 2);
        }
    }
    #[test]
    fn a_frame_start_pulse_line_is_a_valid_interrupt() {
        for line in [0, 31, 512, 600] {
            let mut devices = Devices::empty();
            assert!(
                devices
                    .imaging(
                        map(None),
                        (100, 50, 10, 8),
                        (Some((0x59, 4)), Some((0, 1))),
                        None,
                        false,
                        Some(line)
                    )
                    .is_err(),
                "line {line}"
            );
            assert!(devices.models.is_empty() && devices.windows.is_empty());
        }
    }
    /// Edges on `line` while the sensor streams three frames: one sensor
    /// command writes register 0 = 0 (out of standby), then the receiver's
    /// link runs.
    fn frame_start_edges(pulse: Option<u32>, line: u32) -> Vec<bool> {
        use fujimod_emulator_devices::test_host::recording::RecordingHost;
        let mut devices = Devices::empty();
        devices
            .imaging(
                map(None),
                (100, 50, 10, 8),
                (Some((0x59, 4)), Some((0, 1))),
                None,
                false,
                pulse,
            )
            .unwrap();
        let mut command = devices.models[0].take().unwrap();
        let mut receiver = devices.models[1].take().unwrap();
        let mut host = RecordingHost {
            ram: vec![0x81, 0, 0, 0, 0, 0, 0, 0],
            ram_base: 0x1000,
            ..Default::default()
        };
        for (offset, value) in [(0x210, 0x1000), (0x208, 0x10401), (0x218, 8), (0x204, 1)] {
            command.write(
                &mut Cx::new(&mut host, 0, 0, 0),
                0xfff6c000 + offset,
                4,
                value,
            );
        }
        // The one packet completes after `command_ticks`.
        host.now = 10;
        command.read(&mut Cx::new(&mut host, 0, 0, 0), 0xfff6c204, 4);
        receiver.write(&mut Cx::new(&mut host, 0, 0, 0), 0xfff69000, 4, 0);
        for _ in 0..3 {
            host.now += 100;
            receiver.read(&mut Cx::new(&mut host, 0, 0, 0), 0xfff69000, 4);
        }
        host.irqs
            .iter()
            .filter(|(l, _)| *l == line)
            .map(|(_, level)| *level)
            .collect()
    }
    #[test]
    fn a_declared_frame_start_pulse_is_an_edge_at_every_frame_start() {
        assert_eq!(
            frame_start_edges(Some(101), 101),
            [true, false, true, false, true, false]
        );
        // The frame start line (timing[0] = 102) still follows its enable
        // word, which is zero here.
        assert!(frame_start_edges(Some(101), 102).iter().all(|level| !level));
        assert!(frame_start_edges(None, 101).is_empty());
    }
}

#[cfg(test)]
mod graph_tests {
    use super::Devices;

    #[test]
    fn models_become_graph_nodes_with_their_windows() {
        let mut devices = Devices::empty();
        devices.io_groups(0xfff10000).unwrap();
        devices.spi(0xfff32000, true).unwrap();
        let (graph, _) = devices.into_graph().unwrap();
        assert_eq!(graph.nodes.len(), 2);
        assert_eq!(graph.at(0xfff10000).unwrap(), 0);
        assert_eq!(graph.at(0xfff32024).unwrap(), 1);
        assert!(graph.at(0xfff32028).is_err());
    }

    #[test]
    fn a_carved_word_leaves_its_host_block() {
        let mut devices = Devices::empty();
        devices.io_groups(0xfff10000).unwrap();
        devices.spi(0xfff32000, true).unwrap();
        // The SPI controller owns one word inside the IO groups' range.
        devices.windows.push((0xfff10010, 0xfff10014, 1));
        devices.carved.push((0xfff10010, 0xfff10014));
        let (graph, _) = devices.into_graph().unwrap();
        assert_eq!(graph.at(0xfff1000c).unwrap(), 0);
        assert_eq!(graph.at(0xfff10010).unwrap(), 1);
        assert_eq!(graph.at(0xfff10014).unwrap(), 0);
    }
}
