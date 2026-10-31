//! X-Processor Pro graph assembly from the selected SoC profile and body reset inputs.
//!
//! The SoC profile owns register addresses, interrupt routes and model timing;
//! the body's hardware inputs own what is attached to them. Devices without a
//! declared map get no node, so a strict run stops at their first access.
//! Firmware images, the flash backing and the processor reset state stay with
//! the caller.

use crate::graph::{self, ProcessorStart};
use fujimod_catalog::{
    profiles::{RegisterIdleBits, RegisterLayouts, SocProfile},
    reference::{MemoryKind, MemoryRegion, RegionSize},
    soc::ApplicationHardware,
};
use fujimod_catalog_configuration::RunInstructionRate;
use fujimod_catalog_resolution::ResolvedConfiguration;
use fujimod_catalog_run_inputs::HardwareReset;
use fujimod_emulator::board::factory;
use fujimod_emulator_cpu::Register;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// Performance-counter cycles per board tick of each application core.
const CYCLES_PER_TICK: u64 = 64;
/// MPIDR multiprocessor-extension bit; the affinity is the core index.
const MPIDR_MULTIPROCESSOR: u64 = 0x8000_0000;

/// A checked X-Processor Pro board: the execution graph and the device
/// graph that `fujimod_emulator_xpp::board::build` accepts.
pub struct Assembly {
    pub execution: factory::Graph,
    pub devices: Value,
    /// Instructions per board tick, as numerator and denominator.
    pub rate: (u64, u64),
    /// Flash backing asset the flash node reads, supplied by the caller.
    pub flash_asset: &'static str,
    /// GPIO bank and pin of the USB cable's VBUS contact, when a cable is declared.
    pub usb_vbus: Option<(u64, u64)>,
}

/// Caller-owned inputs that the catalog does not hold.
pub struct Inputs<'a> {
    /// Reset state of each application core, from the firmware being started.
    pub starts: &'a [ProcessorStart],
    /// Sub-CPU registers added to the declared ones, such as a user-profile header.
    pub sub_cpu_registers: &'a [(u8, u8)],
    /// Configuration assets by name; declared statistics buffers are read from here.
    pub assets: &'a BTreeMap<String, Vec<u8>>,
}

fn soc_profile<'a>(resolved: &'a ResolvedConfiguration) -> Result<&'a SocProfile, String> {
    let selection = resolved
        .configuration
        .preset
        .as_ref()
        .and_then(|preset| preset.hardware_selection.as_ref())
        .ok_or("X-Processor Pro assembly requires a hardware selection")?;
    let soc = &resolved.reference.soc;
    soc.hardware
        .as_ref()
        .and_then(|hardware| hardware.profiles.get(&selection.soc_profile_id))
        .ok_or_else(|| {
            format!(
                "SoC {} has no hardware profile {}",
                soc.id, selection.soc_profile_id
            )
        })
}

fn bits(bits: &RegisterIdleBits) -> Value {
    json!([bits.offset, bits.mask])
}

fn hex_bytes(text: &str, label: &str) -> Result<Vec<u8>, String> {
    if text.len() % 2 != 0 {
        return Err(format!("{label} is not whole bytes of hexadecimal"));
    }
    (0..text.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&text[i..i + 2], 16)
                .map_err(|_| format!("{label} is not hexadecimal"))
        })
        .collect()
}

/// The words of a declared statistics buffer, checked against its identity.
fn buffer_words(
    assets: &BTreeMap<String, Vec<u8>>,
    asset: &str,
    sha256: &str,
) -> Result<Vec<u32>, String> {
    let data = assets
        .get(asset)
        .ok_or_else(|| format!("statistics buffer asset {asset} was not supplied"))?;
    let digest: String = Sha256::digest(data)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if data.is_empty() || data.len() % 4 != 0 || digest != sha256 {
        return Err(format!("statistics buffer asset {asset} identity differs"));
    }
    Ok(data
        .chunks_exact(4)
        .map(|word| u32::from_le_bytes([word[0], word[1], word[2], word[3]]))
        .collect())
}

/// Hexadecimal rows of a declared live-view host image, after its identity check.
fn host_image(
    assets: &BTreeMap<String, Vec<u8>>,
    image: &fujimod_catalog_run_inputs::hardware_inputs::LiveViewHostImage,
) -> Result<String, String> {
    let asset = &image.asset;
    let data = assets
        .get(asset)
        .ok_or_else(|| format!("live-view image asset {asset} was not supplied"))?;
    let digest: String = Sha256::digest(data)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let size = u64::from(image.width) * u64::from(image.height) * 3;
    if size == 0 || data.len() as u64 != size || digest != image.sha256 {
        return Err(format!("live-view image asset {asset} identity differs"));
    }
    Ok(data.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn serial_nodes(
    maps: &RegisterLayouts,
    reset: &HardwareReset,
    extra: &[(u8, u8)],
) -> Result<Vec<Value>, String> {
    let Some(ports) = &maps.serial_ports else {
        if reset.sub_cpu.is_some()
            || reset.battery_gauge.is_some()
            || reset.lens.is_some()
            || !reset.idle_serial_ports.is_empty()
        {
            return Err("serial peers require the serial port map".into());
        }
        return Ok(Vec::new());
    };
    let mut claimed = Vec::new();
    let mut claim = |port: u64, label: &str| {
        if !ports.bases.contains(&port) || claimed.contains(&port) {
            return Err(format!("{label} port {port:#x} is not a free serial port"));
        }
        claimed.push(port);
        Ok(())
    };
    if let Some(lens) = &reset.lens {
        claim(lens.port, "lens")?;
    }
    if let Some(sub_cpu) = &reset.sub_cpu {
        claim(sub_cpu.port, "sub-CPU")?;
    }
    if let Some(gauge) = &reset.battery_gauge {
        claim(gauge.port, "battery gauge")?;
    }
    for idle in &reset.idle_serial_ports {
        claim(idle.port, "idle serial")?;
    }
    if !extra.is_empty() && reset.sub_cpu.is_none() {
        return Err("sub-CPU registers require a declared sub-CPU".into());
    }
    let mut nodes = Vec::new();
    for (n, &base) in ports.bases.iter().enumerate() {
        if reset.lens.as_ref().is_some_and(|lens| lens.port == base) {
            continue;
        }
        let peer = if let Some(sub_cpu) = reset.sub_cpu.as_ref().filter(|s| s.port == base) {
            let mut registers: Vec<Value> = sub_cpu
                .registers
                .iter()
                .map(|r| json!([r.register, r.value]))
                .collect();
            registers.extend(extra.iter().map(|&(r, v)| json!([r, v])));
            json!({
                "kind": "registers",
                "registers": registers,
                "clock": sub_cpu.clock.as_ref().map(|c| json!({
                    "first": c.first, "hold": c.hold,
                    "seconds": c.seconds, "ticks_per_second": c.ticks_per_second,
                })),
            })
        } else if let Some(gauge) = reset.battery_gauge.as_ref().filter(|g| g.port == base) {
            json!({
                "kind": "frames",
                "registers": gauge.registers.iter()
                    .map(|r| json!([r.register, r.value])).collect::<Vec<_>>(),
            })
        } else if reset.idle_serial_ports.iter().any(|idle| idle.port == base) {
            json!({ "kind": "idle" })
        } else {
            json!({ "kind": "none" })
        };
        nodes.push(json!({
            "name": format!("serial{n}"),
            "model": { "kind": "serial-port", "base": base, "peer": peer },
        }));
    }
    Ok(nodes)
}

/// Graph nodes for the declared X-Processor Pro register maps and body inputs,
/// in construction order: GPIO and reset gates precede the SD slot, lens,
/// shutter and key lines that take their lines.
fn nodes(
    maps: &RegisterLayouts,
    reset: &HardwareReset,
    ram_bytes: u64,
    ram_banks: &[(u64, u64)],
    inputs: &Inputs<'_>,
) -> Result<Vec<Value>, String> {
    let paired = |map: bool, input: bool, label: &str| {
        if map != input {
            return Err(format!(
                "{label} map and body input must be declared together"
            ));
        }
        Ok(())
    };
    paired(maps.gpio.is_some(), reset.gpio_input.is_some(), "GPIO")?;
    paired(maps.adc.is_some(), reset.adc_input.is_some(), "ADC")?;
    paired(maps.xpp_sd.is_some(), reset.sd_card.is_some(), "SD")?;
    paired(maps.xpp_usb.is_some(), reset.usb_cable.is_some(), "USB")?;
    paired(
        maps.dram_controller.is_some(),
        reset.dram_refresh.is_some(),
        "DRAM controller",
    )?;
    paired(
        maps.flash_controller.is_some(),
        reset.flash_aperture.is_some(),
        "flash",
    )?;
    paired(
        maps.flash_controller.is_some(),
        maps.pl330.is_some(),
        "flash DMA",
    )?;
    if maps
        .display_frames
        .as_ref()
        .is_some_and(|f| f.timing.is_some())
    {
        return Err("X-Processor Pro display timing banks have no graph node".into());
    }
    // The imaging pipeline's frame period, when declared, replaces each
    // imaging law's own period; display frames keep theirs.
    let frame_period = |declared: u64| {
        reset
            .imaging_frames
            .as_ref()
            .map_or(declared, |frames| frames.period_ticks)
    };
    let mut out = Vec::new();
    let mut node = |name: String, model: Value| out.push(json!({ "name": name, "model": model }));

    if let Some(timer) = &maps.interval_timer {
        node(
            "timers".into(),
            json!({
                "kind": "timer", "base": timer.base,
                "routes": timer.routes.iter().map(|r| json!([r.channel, r.irq])).collect::<Vec<_>>(),
            }),
        );
    }
    if let (Some(gpio), Some(levels)) = (&maps.gpio, &reset.gpio_input) {
        node(
            "gpio".into(),
            json!({ "kind": "gpio", "base": gpio.base, "inputs": levels.levels }),
        );
    }
    if let Some(clocks) = &maps.pll_clocks {
        node(
            "clocks".into(),
            json!({ "kind": "clock-control", "options": {
                "base": clocks.base, "lock_ticks": clocks.lock_ticks,
                "running": reset.pll_input.as_ref().map_or(Vec::new(), |p| p.running.clone()),
            }}),
        );
    } else if reset.pll_input.is_some() {
        return Err("running PLLs require the clock bank".into());
    }
    if let Some(gates) = &maps.reset_gates {
        node(
            "resets".into(),
            json!({ "kind": "reset-gates", "base": gates.base }),
        );
    }
    if let (Some(flash), Some(dma), Some(backing)) =
        (&maps.flash_controller, &maps.pl330, &reset.flash_aperture)
    {
        if backing.contents != "registered-update" {
            return Err("X-Processor Pro flash backing must be the registered update".into());
        }
        node(
            "flash".into(),
            json!({
                "kind": "flash", "controller": flash.base, "aperture": flash.aperture,
                "size": flash.size, "asset": "flash", "irq": flash.irq_route,
                "dma": dma.base, "irq_routes": dma.irq_routes,
                "setup_ticks": dma.setup_ticks, "bytes_per_tick": dma.bytes_per_tick,
                "transfer_ticks": 1,
                "nor": backing.nor.iter().map(|nor| json!({
                    "chip_select": nor.chip_select,
                    "options": {
                        "enhanced_volatile_reset": nor.enhanced_volatile_reset,
                        "write_ticks": nor.register_write_ticks,
                    },
                    "array": {
                        "page_bytes": nor.page_bytes, "erase_ticks": nor.erase_ticks,
                        "page_ticks": nor.page_ticks,
                    },
                })).collect::<Vec<_>>(),
            }),
        );
    }
    for serial in serial_nodes(maps, reset, inputs.sub_cpu_registers)? {
        out.push(serial);
    }
    let mut node = |name: String, model: Value| out.push(json!({ "name": name, "model": model }));
    if let (Some(adc), Some(samples)) = (&maps.adc, &reset.adc_input) {
        node(
            "adc".into(),
            json!({
                "kind": "adc", "base": adc.base, "samples": samples.samples,
                "conversion_ticks": adc.sample_ticks,
            }),
        );
    }
    let image = reset.live_view_image.as_ref();
    if let Some(image) = image
        && !maps
            .register_blocks
            .iter()
            .any(|block| block.base == image.unit && block.frames.is_some())
    {
        return Err("the live-view image unit must be a register block with frames".into());
    }
    let development = reset.still_development.as_ref();
    if let Some(development) = development
        && [development.developer, development.resizer]
            .iter()
            .chain(&development.colour)
            .any(|&unit| !maps.register_blocks.iter().any(|block| block.base == unit))
    {
        return Err("still development units must be register blocks with jobs".into());
    }
    for (n, block) in maps.register_blocks.iter().enumerate() {
        let mut model = json!({
            "kind": "register-block", "base": block.base, "size": block.size,
            "idle": block.idle_bits.iter().map(bits).collect::<Vec<_>>(),
            "frames": block.frames.as_ref().map(|f| json!({
                "start": bits(&f.start), "stopped": bits(&f.stopped),
                "irq": f.irq_route, "period_ticks": frame_period(f.period_ticks),
            })),
        });
        model["job"] = json!(block.job.as_ref().map(|job| json!({
            "start": bits(&job.start), "status": job.status.as_ref().map(bits),
            "stop": job.stop.as_ref().map(bits), "irq": job.irq_route, "ticks": job.ticks,
        })));
        model["fixed"] = json!(
            block
                .fixed_words
                .iter()
                .map(|w| json!([w.offset, w.value]))
                .collect::<Vec<_>>()
        );
        model["commands"] = json!(block
            .commands
            .iter()
            .map(|c| json!({
                "word": c.word, "value": c.value, "status": c.status, "mask": c.mask, "state": c.state,
            }))
            .collect::<Vec<_>>());
        if let Some(development) = development {
            let law = if block.base == development.developer {
                let mut law = json!({ "law": "develop", "profile": development.profile,
                                      "phase": development.source_phase });
                if let Some(colour) = development.colour {
                    law["colour"] = json!(colour);
                    law["white_balance"] = json!(development.white_balance_reference);
                }
                Some(law)
            } else if block.base == development.resizer {
                Some(json!({ "law": "resize", "profile": development.profile }))
            } else {
                None
            };
            if let Some(law) = law {
                if block.job.is_none() {
                    return Err("still development units must be register blocks with jobs".into());
                }
                model["work"] = law;
            }
        }
        if let Some(image) = image.filter(|image| image.unit == block.base) {
            model["live_view_image"] =
                json!({ "lines": image.lines, "ycbcr": image.ycbcr, "scene": image.scene });
            if let Some(host) = &image.host_image {
                model["live_view_image"]["host_image"] = json!({
                    "width": host.width, "height": host.height,
                    "rgb": host_image(inputs.assets, host)?,
                });
            }
        }
        node(format!("block{n}"), model);
    }
    let scene = reset.zone_statistics.as_ref();
    if let Some(statistics) = &maps.imaging_statistics {
        let mut model = json!({
            "kind": "statistics", "base": statistics.base, "irq": statistics.irq_route,
            "period_ticks": frame_period(statistics.period_ticks),
        });
        if let Some(scene) = scene {
            model["scene"] = json!(scene.zone_sums);
            model["buffers"] = scene
                .buffers
                .iter()
                .map(|b| {
                    Ok(json!([
                        b.output,
                        buffer_words(inputs.assets, &b.asset, &b.sha256)?
                    ]))
                })
                .collect::<Result<Value, String>>()?;
        }
        node("statistics".into(), model);
    } else if scene.is_some() {
        return Err("a statistics scene requires the statistics engine".into());
    }
    if let Some(face) = &maps.face_detection {
        node(
            "face".into(),
            json!({
                "kind": "face", "base": face.base, "irq": face.irq_route, "run_ticks": face.run_ticks,
            }),
        );
    }
    for (name, kind, codec) in [
        ("jpeg-encoder", "jpeg", &maps.jpeg_encoder),
        ("jpeg-decoder", "jpeg-decoder", &maps.jpeg_decoder),
    ] {
        if let Some(codec) = codec {
            node(
                name.into(),
                json!({
                    "kind": kind, "base": codec.base, "irq": codec.irq_route,
                    "run_ticks": codec.run_ticks,
                }),
            );
        }
    }
    if let Some(hdmi) = &maps.hdmi_tx {
        node("hdmi".into(), json!({ "kind": "hdmi", "base": hdmi.base }));
    }
    if let Some(power) = &maps.power_control {
        node(
            "power".into(),
            json!({ "kind": "power", "base": power.base }),
        );
    }
    if let (Some(usb), Some(cable)) = (&maps.xpp_usb, &reset.usb_cable) {
        let speed = match cable.speed.as_str() {
            "high" => 0,
            "super" => 4,
            other => return Err(format!("unsupported USB link speed {other}")),
        };
        node(
            "usb".into(),
            json!({ "kind": "usb", "config": {
                "base": usb.base, "irq": usb.irq_route, "command_ticks": usb.command_ticks,
                "transaction_ticks": usb.transaction_ticks, "speed": speed,
            }}),
        );
    }
    if let (Some(dram), Some(refresh)) = (&maps.dram_controller, &reset.dram_refresh) {
        node(
            "dram".into(),
            json!({ "kind": "dram", "base": dram.base, "refresh_level": refresh.level }),
        );
    }
    if let Some(buses) = &maps.xpp_i2c {
        if let Some(part) = reset
            .i2c_parts
            .iter()
            .find(|part| part.bus as usize >= buses.irq_routes.len())
        {
            return Err(format!("I2C part {:#x} names an absent bus", part.address));
        }
        for (bus, &irq) in buses.irq_routes.iter().enumerate() {
            let parts: Vec<Value> = reset
                .i2c_parts
                .iter()
                .filter(|part| part.bus as usize == bus)
                .map(|part| {
                    json!({
                        "address": part.address, "select_mask": part.select_mask,
                        "width": part.width, "little_endian": part.little_endian,
                        "registers": part.registers.iter()
                            .map(|(r, v)| (r.to_string(), json!(v)))
                            .collect::<serde_json::Map<_, _>>(),
                    })
                })
                .collect();
            node(
                format!("i2c{bus}"),
                json!({ "kind": "i2c", "options": {
                "base": buses.base + bus as u64 * buses.stride, "irq": irq,
                "fifo_depth": buses.fifo_depth, "clock_hz": buses.clock_hz, "tick_hz": buses.tick_hz,
            }, "parts": parts }),
            );
        }
    } else if !reset.i2c_parts.is_empty() {
        return Err("I2C parts require the I2C buses".into());
    }
    for (n, output) in maps.display_outputs.iter().flatten().enumerate() {
        let status = |rows: &[fujimod_catalog::profiles::DisplayStatusBits]| {
            rows.iter()
                .map(|b| json!([b.offset, b.mask]))
                .collect::<Vec<_>>()
        };
        node(format!("display{n}"), {
            let mut model = json!({
                "kind": "display-output", "base": output.base, "size": output.size,
                "set_bits": status(&output.set_bits), "clear_bits": status(&output.clear_bits),
                "scanout": output.scanout.as_ref().map(|s| json!([s.output, s.period_ticks])),
            });
            if let Some(ticks) = output.handshake_ticks {
                model["handshake_ticks"] = json!(ticks);
            }
            model
        });
    }
    if let Some(frames) = &maps.display_frames {
        node(
            "display-frames".into(),
            json!({
                "kind": "display-frames", "irqs": frames.irq_routes, "period_ticks": frames.period_ticks,
            }),
        );
    }
    if let Some(control) = &maps.imaging_control {
        if reset.sensor_frame.is_some() && control.readout.is_none() {
            return Err("a sensor frame requires the sensor read-out unit".into());
        }
        node(
            "imaging".into(),
            json!({
                "kind": "imaging-control", "base": control.base,
                "hole": maps.frame_sync.as_ref().map(|sync| sync.interrupts),
                "readout": control.readout.as_ref().map(|r| json!({
                    "offset": r.offset, "irq": r.irq_route, "ticks": r.ticks,
                })),
                "sensor_frame": reset.sensor_frame.as_ref().map(|frame| &frame.profile),
            }),
        );
    } else if reset.sensor_frame.is_some() {
        return Err("a sensor frame requires the imaging control page".into());
    }
    if let Some(sync) = &maps.frame_sync {
        node(
            "frame-sync".into(),
            json!({
                "kind": "frame-sync", "base": sync.base, "interrupts": sync.interrupts,
                "frame_lines": sync.sources.iter()
                    .map(|s| json!([s.status_bit / 8, s.irq_route])).collect::<Vec<_>>(),
                "period_ticks": frame_period(sync.period_ticks),
            }),
        );
    }
    if let (Some(sd), Some(card)) = (&maps.xpp_sd, &reset.sd_card) {
        if card.format.filesystem != "fat32" {
            return Err("the SD card input must be a FAT32 card".into());
        }
        node(
            "sd".into(),
            json!({
                "kind": "sd",
                "options": {
                    "base": sd.base, "register_offset": sd.register_offset,
                    "transfer_ticks": sd.transfer_ticks, "clock_ticks": sd.clock_ticks,
                    "reset_ticks": sd.reset_ticks, "rca": card.rca, "ram_bytes": ram_bytes,
                    "high_speed": card.high_speed, "ram_banks": ram_banks,
                },
                "format": {
                    "sectors": card.sectors, "max_dirty_sectors": 1 << 21,
                    "partition_start": card.format.partition_start,
                    "sectors_per_cluster": card.format.sectors_per_cluster,
                    "label": card.format.label,
                },
                "files": {}, "irq": sd.irq_route, "reset_channel": sd.reset_channel,
            }),
        );
    }
    if let Some(decompressor) = &maps.decompressor {
        let irq = decompressor
            .irq_route
            .ok_or("the X-Processor Pro decompressor requires its interrupt route")?;
        node(
            "decompressor".into(),
            json!({ "kind": "decompressor", "base": decompressor.base, "irq": irq }),
        );
    }
    if let Some(dmp) = &maps.dmp {
        node(
            "renderer".into(),
            json!({ "kind": "renderer", "base": dmp.base, "irq": dmp.irq_route }),
        );
    }
    if let Some(lens) = &reset.lens {
        node(
            "lens".into(),
            json!({
                "kind": "lens", "port": lens.port, "page": hex_bytes(&lens.page, "lens page")?,
                "replies": lens.replies, "power": lens.power, "ready": lens.ready, "line": lens.line,
                "contacts": lens.contacts, "ready_ticks": lens.ready_ticks,
                "release_ticks": lens.release_ticks, "drive_ticks": lens.drive_ticks,
                "poll_ticks": lens.poll_ticks,
            }),
        );
    }
    if let Some(shutter) = &reset.shutter {
        node(
            "shutter".into(),
            json!({
                "kind": "shutter", "mode": shutter.mode, "motor": shutter.motor,
                "sensors": shutter.sensors, "line": shutter.line, "positions": shutter.positions,
                "step_ticks": shutter.step_ticks, "poll_ticks": shutter.poll_ticks,
            }),
        );
    }
    if let Some(keys) = &reset.key_lines {
        node(
            "key-lines".into(),
            json!({
                "kind": "key-lines", "lines": keys.lines, "poll_ticks": keys.poll_ticks,
            }),
        );
    }
    Ok(out)
}

/// Assemble the X-Processor Pro execution and device graphs of a resolved run.
/// DRAM regions of fixed size beside the body DRAM at 0, which DMA
/// engines such as the SD host reach.
fn ram_banks(regions: &[MemoryRegion]) -> Vec<(u64, u64)> {
    regions
        .iter()
        .filter_map(|region| match region.size {
            RegionSize::Bytes { bytes } if region.base != 0 && region.kind == MemoryKind::Dram => {
                Some((region.base, bytes))
            }
            _ => None,
        })
        .collect()
}

pub fn assemble(resolved: &ResolvedConfiguration, inputs: &Inputs<'_>) -> Result<Assembly, String> {
    let selection = &resolved.reference;
    let profile = soc_profile(resolved)?;
    if profile.runtime_abi != "armv7a-unicorn-v1" || profile.cpu_model != "Cortex-A7" {
        return Err(format!(
            "SoC profile of {} is not an ARMv7-A Cortex-A7 application profile",
            selection.soc.id
        ));
    }
    let maps = profile
        .register_maps
        .as_ref()
        .ok_or("X-Processor Pro assembly requires the SoC register maps")?;
    let gic = maps
        .gic
        .as_ref()
        .ok_or("X-Processor Pro assembly requires the GIC map")?;
    let Some(ApplicationHardware::Armv7a {
        configuration_base,
        caches,
    }) = selection
        .soc
        .hardware
        .as_ref()
        .and_then(|hardware| hardware.application.as_ref())
    else {
        return Err("X-Processor Pro assembly requires ARMv7-A application hardware".into());
    };
    let reset = resolved
        .hardware_inputs()?
        .ok_or("X-Processor Pro assembly requires hardware inputs")?
        .reset;
    let (numerator, denominator) = reset
        .execution_clock
        .as_ref()
        .map_or((1, 1), |clock| clock.instructions_per_tick);
    let execution = graph::assemble_with_starts(
        selection,
        RunInstructionRate {
            numerator,
            denominator,
        },
        None,
        inputs.starts,
    )?;
    let cores = execution.processors.len() as u64;
    let devices = json!({
        "kind": "xpp",
        "cpus": (0..cores).map(|core| json!({
            "mpidr": MPIDR_MULTIPROCESSOR | core,
            "configuration_base": configuration_base,
            "cycles_per_tick": CYCLES_PER_TICK,
            "cache_sizes": [caches.l1_data_bytes, caches.l1_instruction_bytes, caches.l2_bytes],
        })).collect::<Vec<_>>(),
        "gic": { "distributor": gic.distributor, "cpu_interface": gic.cpu_interface, "lines": gic.lines },
        "nodes": nodes(maps, &reset, selection.body.dram_bytes, &ram_banks(&selection.soc.memory_regions), inputs)?,
    });
    Ok(Assembly {
        execution,
        devices,
        rate: (numerator, denominator),
        flash_asset: "flash",
        usb_vbus: reset
            .usb_cable
            .as_ref()
            .map(|cable| (cable.vbus_bank, cable.vbus_pin)),
    })
}

/// The assembled graphs of a bundled run configuration as JSON, for language
/// bindings: `execution` in the shape `factory::Graph` accepts, `devices`,
/// the flash asset name and the USB VBUS pin. `registers` holds each
/// application core's reset registers, in SoC processor order; every core is
/// released.
pub fn definition(
    configuration_id: &str,
    overlays: &[String],
    registers: Value,
    sub_cpu_registers: &[(u8, u8)],
    assets: &BTreeMap<String, Vec<u8>>,
) -> Result<Value, String> {
    let resolved = ResolvedConfiguration::with_overlays(configuration_id, overlays)?;
    let registers: Vec<Vec<(Register, u64)>> =
        serde_json::from_value(registers).map_err(|error| error.to_string())?;
    let processors = &resolved.reference.soc.processors;
    if registers.len() != processors.len() {
        return Err(format!(
            "reset registers name {} cores; SoC {} has {}",
            registers.len(),
            resolved.reference.soc.id,
            processors.len()
        ));
    }
    let starts: Vec<ProcessorStart> = processors
        .iter()
        .zip(registers)
        .map(|(processor, registers)| ProcessorStart {
            processor_id: processor.id.clone(),
            released: true,
            registers,
        })
        .collect();
    let assembly = assemble(
        &resolved,
        &Inputs {
            starts: &starts,
            sub_cpu_registers,
            assets,
        },
    )?;
    let (numerator, denominator) = assembly.rate;
    Ok(json!({
        "execution": graph::value(
            &resolved.reference,
            &assembly.execution,
            RunInstructionRate { numerator, denominator },
        ),
        "devices": assembly.devices,
        "flash_asset": assembly.flash_asset,
        "usb_vbus": assembly.usb_vbus,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use fujimod_emulator::board::bus::Mode;
    use fujimod_emulator_xpp::board::{Spec, build};
    use std::path::Path;

    fn starts() -> Vec<ProcessorStart> {
        ["core0", "core1"]
            .map(|id| ProcessorStart {
                processor_id: id.into(),
                released: true,
                registers: vec![(Register::Pc, 0), (Register::Pstate, 0xd3)],
            })
            .into()
    }

    fn assets() -> BTreeMap<String, Vec<u8>> {
        let name = "xt20-211-statistics-window";
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../configurations/emulator/assets")
            .join(format!("{name}.bin"));
        BTreeMap::from([(name.to_string(), std::fs::read(path).unwrap())])
    }

    fn xt20(assets: &BTreeMap<String, Vec<u8>>) -> Result<Assembly, String> {
        let resolved = ResolvedConfiguration::bundled("xt20-211")?;
        assemble(
            &resolved,
            &Inputs {
                starts: &starts(),
                sub_cpu_registers: &[],
                assets,
            },
        )
    }

    fn node<'a>(devices: &'a Value, name: &str) -> &'a Value {
        devices["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["name"] == name)
            .map(|node| &node["model"])
            .unwrap_or_else(|| panic!("no node {name}"))
    }

    #[test]
    fn x_t20_graph_is_built_from_the_catalog_and_body_inputs() {
        let assets = assets();
        let assembly = xt20(&assets).unwrap();
        let devices = &assembly.devices;
        let names: Vec<_> = devices["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|node| node["name"].as_str().unwrap())
            .collect();
        // Construction order: the lines' owners precede the lens, shutter and keys.
        let position = |name| names.iter().position(|n| *n == name).unwrap();
        assert!(position("gpio") < position("resets"));
        assert!(position("resets") < position("sd"));
        assert!(position("sd") < position("lens"));
        assert_eq!(names[names.len() - 3..], ["lens", "shutter", "key-lines"]);
        assert_eq!(devices["cpus"].as_array().unwrap().len(), 2);
        assert_eq!(devices["cpus"][1]["mpidr"], 0x8000_0001_u64);
        assert_eq!(devices["cpus"][0]["configuration_base"], 0xffff_0000_u64);

        // The AF-S imaging block's job law, from the SoC profile.
        let blocks = devices["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| &n["model"]);
        let job = blocks
            .clone()
            .find(|m| m["base"] == 0xfff7_f000_u64)
            .unwrap();
        assert_eq!(
            job["job"],
            json!({"start": [4, 1], "status": [0x44, 1], "stop": null, "irq": 128, "ticks": 880000})
        );
        // Still capture: development engines, the second DRAM controller's
        // commands, the read-out unit, the JPEG codecs and the second DRAM bank.
        let engine = |base: u64| blocks.clone().find(|m| m["base"] == base).unwrap();
        assert_eq!(
            engine(0xfff9_8000)["job"],
            json!({"start": [4, 1], "status": null, "stop": [0, 1], "irq": 130, "ticks": 144000})
        );
        assert_eq!(engine(0xfff7_e000)["fixed"], json!([[0x1c, 0x0100_0100]]));
        assert_eq!(
            engine(0xfff0_f000)["commands"][0],
            json!({"word": 0xb8, "value": 0x6a02_0202, "status": 0xbc, "mask": 0x3f, "state": 0x27})
        );
        assert_eq!(
            node(devices, "imaging")["readout"],
            json!({"offset": 0xc0, "irq": 98, "ticks": 2_880_000})
        );
        assert_eq!(node(devices, "jpeg-encoder")["kind"], "jpeg");
        assert_eq!(node(devices, "jpeg-decoder")["irq"], 135);
        assert_eq!(
            node(devices, "sd")["options"]["ram_banks"],
            json!([[0x8000_0000_u64, 0x4000_0000_u64]])
        );
        // The imaging frame period replaces each imaging law's period; display frames keep theirs.
        assert_eq!(node(devices, "frame-sync")["period_ticks"], 880000);
        assert_eq!(node(devices, "statistics")["period_ticks"], 880000);
        assert_eq!(node(devices, "display-frames")["period_ticks"], 800000);
        let unit = blocks
            .clone()
            .find(|m| m["base"] == 0xfff5_6000_u64)
            .unwrap();
        assert_eq!(unit["frames"]["period_ticks"], 880000);
        assert_eq!(unit["live_view_image"]["lines"], 480);
        // Measured statistics scene and the window buffer from its asset.
        let statistics = node(devices, "statistics");
        assert_eq!(statistics["scene"], json!([144736, 383321, 33695]));
        assert_eq!(statistics["buffers"][0][0], 0x1d0);
        assert_eq!(statistics["buffers"][0][1].as_array().unwrap().len(), 80);
        // Serial peers: the lens port has its own node; sub-CPU, gauge frames, idle channel 3.
        assert!(!names.contains(&"serial0"));
        assert_eq!(node(devices, "serial2")["peer"]["kind"], "registers");
        assert_eq!(node(devices, "serial3")["peer"], json!({"kind": "idle"}));
        assert_eq!(node(devices, "serial4")["peer"]["kind"], "frames");
        assert_eq!(node(devices, "serial1")["peer"], json!({"kind": "none"}));
        // The fuel gauge at 0x36 on I2C bus 1.
        let gauge = node(devices, "i2c1")["parts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|part| part["address"] == 0x36)
            .unwrap();
        assert_eq!(gauge["registers"]["4"], 0x5440);
        assert_eq!(
            node(devices, "key-lines")["lines"],
            json!([[0, 0, 0], [1, 0, 1]])
        );
        assert!(assembly.usb_vbus.is_some());

        // CPU side from the catalog: DRAM, its read-only alias and the peripheral window.
        let execution = &assembly.execution;
        assert_eq!(execution.processors.len(), 2);
        assert!(
            execution
                .mappings
                .iter()
                .any(|m| m.address == 0x4000_0000 && m.permissions == 1)
        );
        assert!(
            execution
                .peripherals
                .iter()
                .all(|p| p.address == 0xf000_0000 && p.size == 0x1000_0000)
        );
    }

    #[test]
    fn x_t20_graph_builds_a_strict_board() {
        let assets = assets();
        let assembly = xt20(&assets).unwrap();
        let spec: Spec = serde_json::from_value(assembly.devices).unwrap();
        let flash = BTreeMap::from([(assembly.flash_asset.to_string(), vec![0xff; 0x1000])]);
        let built = build(assembly.execution, spec, &flash, Mode::Strict).unwrap();
        for name in [
            "gpio",
            "resets",
            "flash",
            "statistics",
            "key-lines",
            "lens",
            "shutter",
            "usb",
            "sd",
        ] {
            assert!(built.nodes.contains_key(name), "missing node {name}");
        }
    }

    #[test]
    fn a_live_view_host_image_is_checked_and_passed_to_its_unit() {
        use fujimod_catalog_run_inputs::hardware_inputs::LiveViewHostImage;
        let resolved = ResolvedConfiguration::bundled("xt20-211").unwrap();
        let maps = soc_profile(&resolved)
            .unwrap()
            .register_maps
            .as_ref()
            .unwrap();
        let selection = &resolved.reference;
        let banks = ram_banks(&selection.soc.memory_regions);
        let mut reset = resolved.hardware_inputs().unwrap().unwrap().reset;
        let image = reset.live_view_image.as_mut().unwrap();
        let unit = image.unit;
        image.host_image = Some(LiveViewHostImage {
            asset: "host".into(),
            basis: "test".into(),
            width: 2,
            height: 1,
            sha256: "0".repeat(64),
        });
        let starts = starts();
        let mut assets = assets();
        let pixels = vec![255, 0, 0, 0, 0, 255];
        assets.insert("host".into(), pixels.clone());
        let built = |reset: &HardwareReset, assets: &BTreeMap<String, Vec<u8>>| {
            let inputs = Inputs {
                starts: &starts,
                sub_cpu_registers: &[],
                assets,
            };
            nodes(maps, reset, selection.body.dram_bytes, &banks, &inputs)
        };
        assert_eq!(
            built(&reset, &assets).err().unwrap(),
            "live-view image asset host identity differs"
        );
        let host = reset
            .live_view_image
            .as_mut()
            .unwrap()
            .host_image
            .as_mut()
            .unwrap();
        host.sha256 = Sha256::digest(&pixels)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let nodes = built(&reset, &assets).unwrap();
        let block = nodes
            .iter()
            .map(|node| &node["model"])
            .find(|model| model["base"] == unit)
            .unwrap();
        assert_eq!(
            block["live_view_image"]["host_image"],
            json!({"width": 2, "height": 1, "rgb": "ff00000000ff"})
        );
    }

    #[test]
    fn a_declared_sensor_frame_reaches_the_read_out_unit() {
        use fujimod_catalog_run_inputs::hardware_inputs::SensorFrame;
        let resolved = ResolvedConfiguration::bundled("xt20-211").unwrap();
        let maps = soc_profile(&resolved)
            .unwrap()
            .register_maps
            .as_ref()
            .unwrap();
        let mut reset = resolved.hardware_inputs().unwrap().unwrap().reset;
        reset.sensor_frame = Some(SensorFrame {
            basis: "test".into(),
            profile: "declared".into(),
        });
        let (starts, assets) = (starts(), assets());
        let inputs = Inputs {
            starts: &starts,
            sub_cpu_registers: &[],
            assets: &assets,
        };
        let banks = ram_banks(&resolved.reference.soc.memory_regions);
        let nodes = nodes(
            maps,
            &reset,
            resolved.reference.body.dram_bytes,
            &banks,
            &inputs,
        )
        .unwrap();
        let imaging = nodes.iter().find(|node| node["name"] == "imaging").unwrap();
        assert_eq!(imaging["model"]["sensor_frame"], "declared");
        let mut maps = maps.clone();
        maps.imaging_control.as_mut().unwrap().readout = None;
        assert_eq!(
            super::nodes(&maps, &reset, 0, &banks, &inputs)
                .err()
                .unwrap(),
            "a sensor frame requires the sensor read-out unit"
        );
    }

    #[test]
    fn declared_still_development_reaches_the_developer_and_the_resizer() {
        use fujimod_catalog_run_inputs::hardware_inputs::StillDevelopment;
        let resolved = ResolvedConfiguration::bundled("xt20-211").unwrap();
        let maps = soc_profile(&resolved)
            .unwrap()
            .register_maps
            .as_ref()
            .unwrap();
        let mut reset = resolved.hardware_inputs().unwrap().unwrap().reset;
        reset.still_development = Some(StillDevelopment {
            basis: "test".into(),
            profile: "declared".into(),
            developer: 0xfff9_8000,
            resizer: 0xfffc_3000,
            source_phase: (2, 2),
            colour: None,
            white_balance_reference: None,
        });
        let (starts, assets) = (starts(), assets());
        let inputs = Inputs {
            starts: &starts,
            sub_cpu_registers: &[],
            assets: &assets,
        };
        let banks = ram_banks(&resolved.reference.soc.memory_regions);
        let nodes = nodes(maps, &reset, 0, &banks, &inputs).unwrap();
        let block = |base: u64| {
            nodes
                .iter()
                .find(|node| node["model"]["base"] == base)
                .unwrap()
                .clone()
        };
        assert_eq!(
            block(0xfff9_8000)["model"]["work"],
            json!({ "law": "develop", "profile": "declared", "phase": [2, 2] })
        );
        assert_eq!(
            block(0xfffc_3000)["model"]["work"],
            json!({ "law": "resize", "profile": "declared" })
        );
        assert!(block(0xfff7_e000)["model"].get("work").is_none());
        reset.still_development.as_mut().unwrap().colour = Some(0xfff4_7000);
        let nodes = super::nodes(maps, &reset, 0, &banks, &inputs).unwrap();
        let developer = nodes
            .iter()
            .find(|node| node["model"]["base"] == 0xfff9_8000u64)
            .unwrap();
        assert_eq!(developer["model"]["work"]["colour"], 0xfff4_7000u64);
        reset.still_development.as_mut().unwrap().colour = Some(0xfff4_8000);
        assert!(super::nodes(maps, &reset, 0, &banks, &inputs).is_err());
        reset.still_development.as_mut().unwrap().colour = None;
        reset.still_development.as_mut().unwrap().resizer = 0xfff7_f004;
        assert_eq!(
            super::nodes(maps, &reset, 0, &banks, &inputs)
                .err()
                .unwrap(),
            "still development units must be register blocks with jobs"
        );
    }

    #[test]
    fn assembly_refuses_missing_or_mismatched_inputs() {
        fn error(assets: &BTreeMap<String, Vec<u8>>) -> String {
            xt20(assets).err().unwrap()
        }
        assert_eq!(
            error(&BTreeMap::new()),
            "statistics buffer asset xt20-211-statistics-window was not supplied"
        );
        let wrong = BTreeMap::from([("xt20-211-statistics-window".to_string(), vec![0; 320])]);
        assert_eq!(
            error(&wrong),
            "statistics buffer asset xt20-211-statistics-window identity differs"
        );
        // A run without a hardware selection has no SoC profile to assemble from.
        let mut resolved = ResolvedConfiguration::bundled("xt20-211").unwrap();
        resolved
            .configuration
            .preset
            .as_mut()
            .unwrap()
            .hardware_selection = None;
        let (starts, assets) = (starts(), assets());
        let inputs = Inputs {
            starts: &starts,
            sub_cpu_registers: &[],
            assets: &assets,
        };
        assert_eq!(
            assemble(&resolved, &inputs).err().unwrap(),
            "X-Processor Pro assembly requires a hardware selection"
        );
        // Profile-header registers need the declared sub-CPU; with it they are appended.
        let resolved = ResolvedConfiguration::bundled("xt20-211").unwrap();
        let inputs = Inputs {
            starts: &starts,
            sub_cpu_registers: &[(0x70, 0xa5)],
            assets: &assets,
        };
        let devices = assemble(&resolved, &inputs).unwrap().devices;
        let registers = node(&devices, "serial2")["peer"]["registers"]
            .as_array()
            .unwrap()
            .clone();
        assert_eq!(registers.last().unwrap(), &json!([0x70, 0xa5]));
    }

    #[test]
    fn definition_json_reads_back_as_the_board_graphs() {
        let registers = json!([
            [["Pc", 0x40000], ["Pstate", 0xd3], [{"Arm32": {"R": 0}}, 7]],
            [["Pc", 0x40000], ["Pstate", 0xd3], [{"Arm32": {"R": 1}}, 9]],
        ]);
        let value = definition("xt20-211", &[], registers, &[], &assets()).unwrap();
        let execution: factory::Graph = serde_json::from_value(value["execution"].clone()).unwrap();
        assert_eq!(
            execution.processors[1].registers[2],
            (
                Register::Arm32(fujimod_emulator_cpu::arm::Register::R(1)),
                9
            )
        );
        let _: Spec = serde_json::from_value(value["devices"].clone()).unwrap();
        assert_eq!(value["flash_asset"], "flash");
        let error = definition("xt20-211", &[], json!([[]]), &[], &assets()).err();
        assert_eq!(
            error.unwrap(),
            "reset registers name 1 cores; SoC xpp has 2"
        );
    }
}
