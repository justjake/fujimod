//! Every XP4 device a definition selects, built apart from the processors
//! so that any board can host them.
use super::*;

/// The devices and the serial-flash views the processors map.
pub(crate) struct Built {
    pub devices: Option<devices::Devices>,
    pub frozen_nor: Option<fujimod_emulator_xp4_storage::storage::ImmutableView>,
    pub executable_nor: Option<fujimod_emulator_xp4_storage::storage::ExecutableView>,
}

/// Build the definition's devices. `memory` holds the declared memory
/// windows, which device-owned banks must not overlap.
pub(crate) fn build(
    definition: Definition,
    memory: &[memory::Window],
    storage_image: Option<Vec<u8>>,
    card_files: Vec<(String, Vec<u8>)>,
    card_media: Vec<(usize, fujimod_emulator_devices::parts::sd_card::Card)>,
) -> Result<Built, String> {
    let Definition {
        discovery,
        byte_i2c,
        discovery_tx_receiver,
        storage_jedec,
        storage_immutable_prefix,
        storage_executable_range,
        storage_mapped_read,
        storage_write_protect,
        storage_write_ranges,
        storage_strict_backing,
        storage_erased_ranges,
        standalone_dma,
        storage_dma_routes,
        spi_inputs,
        control_requests,
        request_acknowledgment_ticks,
        xp4_functional_system_control,
        storage_geometry,
        storage_irq,
        storage_dma_audio,
        xpp_storage,
        xpp_serial,
        xpp_sub_cpu,
        xpp_battery_gauge,
        xpp_idle_serial,
        xpp_lens,
        xpp_shutter,
        xpp_key_lines,
        xpp_display_frames,
        xpp_adc,
        xpp_display_outputs,
        xpp_imaging_control,
        xpp_reset_gates,
        xpp_sd,
        xpp_i2c,
        xpp_dram,
        xpp_power,
        xpp_usb,
        xpp_frame_sync,
        xpp_hdmi,
        xpp_statistics,
        xpp_statistics_scene,
        xpp_statistics_buffers,
        xpp_live_view_image,
        xpp_face,
        xpp_register_blocks,
        xpp_clocks,
        io_group_base,
        decompressor,
        lcdc,
        dmp,
        adc,
        gpio,
        gpio_base,
        gpio_interrupts,
        temperature,
        power_spi,
        power_spi_command_output,
        power_spi_bank_inputs,
        power_spi_eye_commit,
        checksum_power_spi,
        imaging,
        movdc,
        sd_clocks,
        sd_hosts,
        register_banks,
        countdown_timers,
        countdown_dials,
        jpeg_encoder,
        jpeg_decoder,
        still_engines,
        dsp,
        face_detector,
        filter_engine,
        pl011_uarts,
        mali_v61,
        usb_condition,
        usb,
        usb_phy,
        xpp_boot,
        xpp_serial_replies,
        ..
    } = definition;
        if power_spi_command_output.is_some() && power_spi.is_none() {
            return Err("Command output requires power SPI peer".into());
        }
        if (!power_spi_bank_inputs.is_empty() || power_spi_eye_commit) && power_spi.is_none() {
            return Err("Power SPI options require a power SPI peer".into());
        }
        // Check the optional byte-controller declarations before building any device.
        for input in &byte_i2c {
            if !(32..512).contains(&input.irq) || input.base.checked_add(0x20).is_none_or(|end| end > 1 << 32) {
                return Err("Invalid ARM32 byte I2C wiring".into());
            }
            fujimod_emulator_xp4_io::i2c::ByteI2c::new(input.base, input.irq, input.profile.clone(), input.peer.clone())?;
        }
        let has_xpp_storage = xpp_storage.is_some();
        let lens_link = match &xpp_lens {
            Some(lens) if xpp_storage.is_some() && xpp_reset_gates.is_some() => {
                Some(fujimod_emulator_xpp::lens::LensLink::new(lens.port)?)
            }
            Some(_) => {
                return Err(
                    "The X-Processor Pro lens needs its storage PL330 and reset gates".into(),
                );
            }
            None => None,
        };
        if standalone_dma.is_some() && (storage_jedec.is_some() || xpp_storage.is_some()) {
            return Err("Standalone XP4 DMA cannot be combined with a storage controller".into());
        }
        if storage_dma_audio.is_some() && storage_jedec.is_none() {
            return Err("XP4 DMA audio channels require the XP4 storage controller".into());
        }
        if storage_jedec.is_some() && xpp_storage.is_some() {
            return Err("XP4 and X-Processor Pro storage cannot be selected together".to_owned());
        }
        if (storage_immutable_prefix.is_some() || storage_executable_range.is_some()) && storage_jedec.is_none() {
            return Err("Immutable NOR prefix requires an XP4 NOR owner".into());
        }
        if (storage_mapped_read.is_some() || storage_write_protect) && storage_jedec.is_none() {
            return Err("NOR read gate and write protection require an XP4 NOR owner".into());
        }
        if storage_strict_backing && (storage_jedec.is_none() || storage_image.is_none()) {
            return Err("Strict NOR backing requires an XP4 NOR owner and explicit contents".into());
        }
        if !storage_erased_ranges.is_empty() && !storage_strict_backing {
            return Err("Erased NOR ranges require strict read backing".into());
        }
        if storage_write_protect && !storage_write_ranges.is_empty() {
            return Err("NOR write ranges contradict whole-array write protection".into());
        }
        if !storage_write_ranges.is_empty()
            && (storage_jedec.is_none() || !storage_strict_backing)
        {
            return Err("NOR write ranges require an XP4 NOR owner and strict read backing".into());
        }
        if storage_immutable_prefix.is_some() && storage_executable_range.is_some() {
            return Err("Only one NOR execution prefix can be declared".into());
        }
        if storage_mapped_read.is_some() && storage_immutable_prefix.is_some() {
            // The prefix is mapped as memory, so its reads would bypass the gate.
            return Err("A mapped NOR read gate cannot be combined with an immutable executable prefix".into());
        }
        if discovery_tx_receiver.is_some() && (!discovery || storage_jedec.is_none()) {
            return Err("Transmit receiver input requires discovery and XP4 storage DMA".into());
        }
        let mut frozen_nor = None;
        let mut executable_nor = None;
        let mut devices = if let Some(jedec) = storage_jedec {
            let (devices, view, executable_view) = devices::Devices::storage(
                jedec,
                storage_dma_routes,
                storage_image.unwrap_or_default(),
                storage_geometry
                    .ok_or_else(|| "NOR requires explicit geometry and timing".to_owned())?,
                storage_irq
                    .ok_or_else(|| "Serial flash requires an explicit IRQ route".to_owned())?,
                discovery_tx_receiver,
                storage_immutable_prefix.map(|p| fujimod_emulator_xp4_storage::storage::ImmutablePrefix::new(p.bytes)).transpose()?,
                storage_executable_range,
                storage_mapped_read,
                storage_write_protect,
                storage_write_ranges,
                storage_strict_backing,
                storage_erased_ranges,
                storage_dma_audio,
            )?;
            frozen_nor = view;
            executable_nor = executable_view;
            Some(devices)
        } else if let Some(storage) = xpp_storage {
            if !storage_dma_routes.is_empty() || storage_geometry.is_some() || storage_irq.is_some()
            {
                return Err("XP4 storage routes require the XP4 controller".to_owned());
            }
            let image = storage_image
                .ok_or_else(|| "X-Processor Pro flash requires explicit contents".to_owned())?;
            let mut devices = devices::Devices::empty();
            let windows = lens_link
                .iter()
                .map(|link| {
                    std::sync::Arc::new(link.clone())
                        as std::sync::Arc<dyn fujimod_emulator_ip_arm::pl330::Source>
                })
                .collect();
            devices.xpp_storage(storage, image, windows)?;
            Some(devices)
        } else {
            if !storage_dma_routes.is_empty()
                || storage_image.is_some()
                || storage_geometry.is_some()
                || storage_irq.is_some()
            {
                return Err(
                    "XP4 storage data/routes require explicit controller selection".to_owned(),
                );
            }
            None
        };
        if let Some(input) = standalone_dma {
            devices.get_or_insert_with(devices::Devices::empty).standalone_dma(input)?;
        }
        let boot_wiring = xpp_boot.as_ref().map(|b| b.options.clone());
        if let Some(wiring) = &boot_wiring
            && (xpp_reset_gates != Some(wiring.selector_base)
                || xpp_power != Some(wiring.clock_base)
                || xpp_dram != Some((wiring.ddr_base, wiring.refresh_level)))
        {
            return Err("Cold boot requires matching reset, power and DRAM declarations".into());
        }
        let boot_lines = match xpp_boot {
            Some(boot) => Some(
                devices
                    .get_or_insert_with(devices::Devices::empty)
                    .xpp_boot(boot.options, boot.card_strap)?,
            ),
            None => None,
        };
        if xpp_serial_replies
            .iter()
            .any(|(base, _, _)| !xpp_serial.contains(base))
        {
            return Err("Serial reply names no X-Processor Pro serial port".to_owned());
        }
        let mut sub_cpu_clock = None;
        let mut sub_cpu = match xpp_sub_cpu {
            Some(peer) => {
                if !xpp_serial.contains(&peer.port) {
                    return Err("X-Processor Pro sub-CPU needs a declared serial port".to_owned());
                }
                let mut registers = vec![0; fujimod_emulator_xpp::serial::PEER_REGISTERS];
                for (register, value) in peer.registers {
                    let slot = registers
                        .get_mut(usize::from(register))
                        .ok_or("X-Processor Pro sub-CPU register out of range")?;
                    *slot = value;
                }
                sub_cpu_clock = peer.clock;
                Some((peer.port, registers))
            }
            None => None,
        };
        let mut battery_gauge = match xpp_battery_gauge {
            Some(peer) if xpp_serial.contains(&peer.port) => {
                Some((peer.port, peer.registers.into_iter().collect()))
            }
            Some(_) => {
                return Err("X-Processor Pro battery gauge needs a declared serial port".to_owned());
            }
            None => None,
        };
        let peers = [
            sub_cpu.as_ref().map(|(port, _)| *port),
            battery_gauge.as_ref().map(|(port, _)| *port),
            xpp_lens.as_ref().map(|lens| lens.port),
        ];
        if xpp_idle_serial
            .iter()
            .any(|port| !xpp_serial.contains(port) || peers.contains(&Some(*port)))
        {
            return Err("X-Processor Pro idle serial port needs a free declared port".to_owned());
        }
        if let Some(adc) = xpp_adc {
            devices
                .get_or_insert_with(devices::Devices::empty)
                .xpp_adc(adc.base, &adc.samples, adc.conversion_ticks)?;
        }
        if let Some((base, lock_ticks, running)) = xpp_clocks {
            devices
                .get_or_insert_with(devices::Devices::empty)
                .xpp_clocks(base, lock_ticks, running)?;
        }
        if let Some(base) = xpp_imaging_control {
            let hole = xpp_frame_sync.as_ref().map(|sync| sync.interrupts);
            devices
                .get_or_insert_with(devices::Devices::empty)
                .xpp_imaging_control(base, hole)?;
        }
        if let Some(sync) = xpp_frame_sync {
            devices
                .get_or_insert_with(devices::Devices::empty)
                .xpp_frame_sync(sync)?;
        }
        let (mut external_lines, boot_resets) = match boot_lines {
            Some((resets, external)) => (Some(external), Some(resets)),
            None => (None, None),
        };
        let reset_gates = if boot_wiring.is_some() {
            boot_resets
        } else {
            match xpp_reset_gates {
                Some(base) => Some(
                    devices
                        .get_or_insert_with(devices::Devices::empty)
                        .xpp_reset_gates(base)
                        .map(|(resets, lines)| {
                            external_lines = Some(lines);
                            resets
                        })?,
                ),
                None => None,
            }
        };
        // Card files go on the first host's card.
        let mut card_files = Some(card_files);
        if let Some(sd) = xpp_sd {
            let card_files = card_files.take().unwrap_or_default();
            let reset = match (sd.reset_channel, &reset_gates) {
                (Some(channel), Some(lines)) => Some(
                    lines
                        .get(channel)
                        .cloned()
                        .ok_or("Invalid X-Processor Pro SD reset channel")?,
                ),
                (Some(_), None) => {
                    return Err("X-Processor Pro SD reset line without reset gates".into());
                }
                (None, _) => None,
            };
            devices
                .get_or_insert_with(devices::Devices::empty)
                .xpp_sd(sd, reset, &card_files)?;
        }
        let mut card_media = card_media;
        if card_media.iter().any(|(index, _)| *index >= sd_hosts.len()) {
            return Err("SD card medium names an undeclared SD host".into());
        }
        for (index, host) in sd_hosts.into_iter().enumerate() {
            let files = card_files.take().unwrap_or_default();
            let medium = card_media
                .iter()
                .position(|(slot, _)| *slot == index)
                .map(|position| card_media.swap_remove(position).1);
            devices
                .get_or_insert_with(devices::Devices::empty)
                .sd_host(host, &files, medium)?;
        }
        if !card_media.is_empty() {
            return Err("Two SD card media name the same SD host".into());
        }
        if card_files.is_some_and(|files| !files.is_empty()) {
            return Err("SD card files require an SD host".to_owned());
        }
        if let Some((unit, ..)) = xpp_live_view_image
            && !xpp_register_blocks
                .iter()
                .any(|b| b.base == unit && b.frames.is_some())
        {
            return Err("X-Processor Pro live-view image needs a unit with frames".into());
        }
        for block in xpp_register_blocks {
            let image = xpp_live_view_image
                .filter(|(unit, ..)| *unit == block.base)
                .map(|(_, lines, ycbcr)| (lines, ycbcr));
            devices
                .get_or_insert_with(devices::Devices::empty)
                .xpp_register_block(block, image)?;
        }
        if let Some((base, irq, period)) = xpp_statistics {
            devices
                .get_or_insert_with(devices::Devices::empty)
                .xpp_statistics(
                    base,
                    irq,
                    period,
                    xpp_statistics_scene,
                    &xpp_statistics_buffers,
                )?;
        } else if xpp_statistics_scene.is_some() || !xpp_statistics_buffers.is_empty() {
            return Err("X-Processor Pro statistics scene needs the statistics engine".into());
        }
        if let Some((base, irq, run_ticks)) = xpp_face {
            devices
                .get_or_insert_with(devices::Devices::empty)
                .xpp_face(base, irq, run_ticks)?;
        }
        if let Some(base) = xpp_hdmi {
            devices
                .get_or_insert_with(devices::Devices::empty)
                .xpp_hdmi(base)?;
        }
        if let Some((base, irq, command_ticks, transaction_ticks, speed)) = xpp_usb {
            devices.get_or_insert_with(devices::Devices::empty).usb(
                fujimod_emulator_ip::usb::dwc3::Config {
                    base,
                    irq,
                    command_ticks,
                    transaction_ticks,
                    speed,
                },
            )?;
        }
        if let Some(base) = xpp_power.filter(|_| boot_wiring.is_none()) {
            devices
                .get_or_insert_with(devices::Devices::empty)
                .xpp_power(base)?;
        }
        if let Some((base, level)) = xpp_dram.filter(|_| boot_wiring.is_none()) {
            devices
                .get_or_insert_with(devices::Devices::empty)
                .xpp_dram(base, level)?;
        }
        for bus in xpp_i2c {
            devices
                .get_or_insert_with(devices::Devices::empty)
                .xpp_i2c(bus)?;
        }
        for output in &xpp_display_outputs {
            devices
                .get_or_insert_with(devices::Devices::empty)
                .xpp_display_output(output)?;
        }
        if let Some(frames) = xpp_display_frames {
            devices
                .get_or_insert_with(devices::Devices::empty)
                .xpp_display_frames(frames.irqs, frames.period_ticks, frames.timing)?;
        }
        for base in xpp_serial {
            if xpp_lens.as_ref().is_some_and(|lens| lens.port == base) {
                continue;
            }
            use fujimod_emulator_xpp::serial::Peer;
            let peer = match (&sub_cpu, &battery_gauge) {
                (Some((port, _)), _) if *port == base => {
                    Peer::Registers(sub_cpu.take().map(|(_, r)| r).unwrap_or_default())
                }
                (_, Some((port, _))) if *port == base => {
                    Peer::Frames(battery_gauge.take().map(|(_, r)| r).unwrap_or_default())
                }
                _ if xpp_idle_serial.contains(&base) => Peer::Idle,
                _ => Peer::None,
            };
            let clock = match peer {
                Peer::Registers(_) => sub_cpu_clock.take(),
                _ => None,
            };
            let mut replies = BTreeMap::new();
            for &(_, command, reply) in xpp_serial_replies.iter().filter(|r| r.0 == base) {
                if replies.insert(command, reply).is_some() {
                    return Err("Repeated serial reply command".to_owned());
                }
            }
            devices
                .get_or_insert_with(devices::Devices::empty)
                .xpp_serial(base, peer, clock, replies)?;
        }
        if battery_gauge.is_some() {
            return Err("X-Processor Pro battery gauge shares its serial port".to_owned());
        }
        if let Some((inputs, sample_ticks)) = adc {
            devices
                .get_or_insert_with(devices::Devices::empty)
                .adc(inputs, sample_ticks)?;
        }
        if let Some((map, timing, read_gate, ibuf)) = imaging {
            // The statistics banks are guest memory when a declared memory
            // region holds them (the boot ROM start keeps its SRAM there).
            let statistics_in_memory = ibuf.as_ref().is_some_and(|(_, _, _, sram, bank, _)| {
                memory.iter().any(|window| {
                    window.base <= *sram
                        && sram + bank * 2 <= window.base + window.data.len() as u64
                })
            });
            devices
                .get_or_insert_with(devices::Devices::empty)
                .imaging(map, timing, read_gate, ibuf, statistics_in_memory)?;
        }
        if !movdc.is_empty() {
            devices
                .get_or_insert_with(devices::Devices::empty)
                .movdc(movdc)?;
        }
        if let Some((base, irq)) = dmp {
            devices
                .get_or_insert_with(devices::Devices::empty)
                .dmp(base, irq)?;
        }
        for (base, pixel_ticks, first, second, layout, video) in lcdc {
            devices.get_or_insert_with(devices::Devices::empty).lcdc(
                base,
                pixel_ticks,
                [first, second],
                layout.as_deref(),
                video.as_deref(),
            )?;
        }
        if let Some(inputs) = gpio {
            devices.get_or_insert_with(devices::Devices::empty).gpio(
                gpio_base.unwrap_or(0xfff41000),
                inputs,
                gpio_interrupts,
            )?;
        } else if !gpio_interrupts.is_empty() {
            return Err("GPIO interrupts require GPIO wiring".into());
        }
        if let Some(keys) = xpp_key_lines {
            let lines = external_lines
                .clone()
                .ok_or("Key lines require the reset and gate controls")?;
            devices
                .get_or_insert_with(devices::Devices::empty)
                .xpp_key_lines(keys, lines)?;
        }
        if let (Some(lens), Some(link), Some(lines)) = (xpp_lens, &lens_link, external_lines) {
            devices
                .get_or_insert_with(devices::Devices::empty)
                .xpp_lens(lens, link, lines)?;
        }
        if let Some(shutter) = xpp_shutter {
            devices
                .get_or_insert_with(devices::Devices::empty)
                .xpp_shutter(shutter)?;
        }
        if let Some((value, ticks)) = temperature {
            devices
                .get_or_insert_with(devices::Devices::empty)
                .temperature(value, ticks)?;
        }
        if let Some((base, idle, port, pin, high, inputs, rtc_ticks)) = power_spi {
            devices
                .get_or_insert_with(devices::Devices::empty)
                .power_spi(
                    base,
                    idle,
                    port,
                    pin,
                    high,
                    inputs,
                    rtc_ticks,
                    power_spi_command_output,
                    power_spi_bank_inputs,
                    power_spi_eye_commit,
                )?;
        }
        if let Some((
            base,
            idle,
            port,
            pin,
            high,
            inputs,
            disable_writes,
            rtc,
            retained_registers,
        )) = checksum_power_spi
        {
            devices
                .get_or_insert_with(devices::Devices::empty)
                .checksum_power_spi(
                    base,
                    idle,
                    port,
                    pin,
                    high,
                    inputs,
                    disable_writes,
                    rtc,
                    retained_registers,
                )?;
        }
        for (base, miso) in spi_inputs {
            devices
                .get_or_insert_with(devices::Devices::empty)
                .spi(base, miso)?;
        }
        if let Some(config) = xp4_functional_system_control {
            devices
                .get_or_insert_with(devices::Devices::empty)
                .functional_system_control(config)?;
        }
        if !control_requests.is_empty() {
            devices
                .get_or_insert_with(devices::Devices::empty)
                .requests(control_requests)?;
        }
        if !sd_clocks.is_empty() {
            if sd_clocks.iter().any(|(address, ticks)| {
                ![0xfffc402c, 0xfffc622c, 0xfffc802c, 0xfffca22c].contains(address)
                    || !(1..=54000000).contains(ticks)
            }) {
                return Err("Invalid XP4 SD host clock wiring".to_owned());
            }
            devices
                .get_or_insert_with(devices::Devices::empty)
                .sd_clocks(&sd_clocks)?;
        }
        if !register_banks.is_empty() {
            devices
                .get_or_insert_with(devices::Devices::empty)
                .register_banks(register_banks)?;
        }
        if let Some((base, routes)) = countdown_timers {
            devices
                .get_or_insert_with(devices::Devices::empty)
                .countdown_timers(base, &routes, &countdown_dials)?;
        } else if !countdown_dials.is_empty() {
            return Err("XP4 countdown dials require the countdown timer bank".into());
        }
        if let Some(encoder) = jpeg_encoder {
            devices
                .get_or_insert_with(devices::Devices::empty)
                .jpeg_encoder(&encoder)?;
        }
        if let Some(decoder) = jpeg_decoder {
            devices
                .get_or_insert_with(devices::Devices::empty)
                .jpeg_decoder(&decoder)?;
        }
        for engine in &still_engines {
            devices
                .get_or_insert_with(devices::Devices::empty)
                .still_engine(engine)?;
        }
        if let Some(wiring) = dsp {
            devices
                .get_or_insert_with(devices::Devices::empty)
                .dsp(&wiring)?;
        }
        if let Some(detector) = face_detector {
            devices
                .get_or_insert_with(devices::Devices::empty)
                .face_detector(&detector)?;
        }
        if let Some(engine) = filter_engine {
            devices
                .get_or_insert_with(devices::Devices::empty)
                .filter_engine(&engine)?;
        }
        for uart in &pl011_uarts {
            devices
                .get_or_insert_with(devices::Devices::empty)
                .pl011_uart(uart)?;
        }
        if let Some(options) = mali_v61 {
            devices
                .get_or_insert_with(devices::Devices::empty)
                .mali_v61(options)?;
        }
        if let Some((base, present)) = usb_condition {
            devices
                .get_or_insert_with(devices::Devices::empty)
                .usb_condition(base, present)?;
        }
        if let Some((base, irq, command_ticks, transaction_ticks, speed)) = usb {
            if base != 0xffb00000
                || irq != 246
                || command_ticks > 54000000
                || transaction_ticks > 54000000
            {
                return Err("Invalid XP4 DWC3 wiring or functional latency".to_owned());
            }
            devices.get_or_insert_with(devices::Devices::empty).usb(
                fujimod_emulator_ip::usb::dwc3::Config {
                    base,
                    irq,
                    command_ticks,
                    transaction_ticks,
                    speed,
                },
            )?;
        }
        if let Some((base, initial, parallel)) = usb_phy {
            if base != 0xffb23800 {
                return Err("Invalid XP4 USB PHY base".to_owned());
            }
            devices
                .get_or_insert_with(devices::Devices::empty)
                .usb_phy(fujimod_emulator_xp4_io::usb_phy::Config {
                    base,
                    initial,
                    parallel,
                })?;
        }
        if let Some(base) = io_group_base {
            if base != 0xfff42000 {
                return Err("Invalid XP4 IO-group base".to_owned());
            }
            devices
                .get_or_insert_with(devices::Devices::empty)
                .io_groups(base)?;
        }
        if let Some((base, irq)) = decompressor {
            // The same engine sits at 0xfff79000 on XP4 and at 0xfff80000 on
            // the X-Processor Pro, whose storage is the flash-controller map.
            let expected = if has_xpp_storage {
                0xfff80000
            } else {
                0xfff79000
            };
            if base != expected || !(32..512).contains(&irq) {
                return Err("Invalid XP4 decompressor wiring".to_owned());
            }
            devices
                .get_or_insert_with(devices::Devices::empty)
                .decompressor(base, irq)?;
        }
        for input in byte_i2c {
            devices.get_or_insert_with(devices::Devices::empty).byte_i2c(input)?;
        }
        if let Some(ticks) = request_acknowledgment_ticks {
            devices
                .get_or_insert_with(devices::Devices::empty)
                .request_acknowledgment(ticks)?;
        }
    Ok(Built { devices, frozen_nor, executable_nor })
}
