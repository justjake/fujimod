//! X-Processor Pro graph assembly from the selected SoC profile and body reset inputs.
//!
//! The SoC profile owns register addresses, interrupt routes and model timing;
//! the body's hardware inputs own what is attached to them. Devices without a
//! declared map get no node, so a strict run stops at their first access.
//! Firmware images, the flash backing and the processor reset state stay with
//! the caller.

use crate::graph::{self, ProcessorStart};
use fujimod_catalog::{
    profiles::{BlockOverride, BodyProfile, RegisterIdleBits, RegisterLayouts},
    reference::{
        AProfileState, Body, ConnectionSignal, MemoryKind, MemoryRegion, RegionSize, Soc, SocBlock,
    },
    soc::BlockDevice,
};
use fujimod_catalog_configuration::RunInstructionRate;
use fujimod_catalog_resolution::ResolvedConfiguration;
use fujimod_catalog_run_inputs::HardwareReset;
use fujimod_emulator::board::factory;
use fujimod_emulator_cpu::Register;
use serde_json::{Value, json};
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
    /// Host RGB image the declared sensor frame writes instead of its virtual
    /// scene, such as a local picture that is not a catalog input.
    pub sensor_image: Option<&'a SensorImage>,
}

/// A caller-owned RGB picture: `width` x `height` rows of red, green and blue bytes.
pub struct SensorImage {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
    /// The light the picture's channels carry.
    pub light: SensorImageLight,
}

/// The light a caller's picture carries, as the declared sensor frame writes it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SensorImageLight {
    /// Display-referred sRGB at the declared wall's light (the default).
    /// It keeps the established, warm-wall path byte for byte: a neutral
    /// picture develops as the wall does, under the wall's AUTO gains. Camera
    /// raw would become the default once callers rely on neutral pictures
    /// developing neutral at any light, and its radiance has a measured basis.
    #[default]
    Rendered,
    /// The camera's own linear light per site, unbalanced: each channel is a
    /// light ratio against the live-view film's declared `camera_raw_radiance`,
    /// so development applies the programmed white balance absolutely.
    CameraRaw,
}

impl SensorImageLight {
    /// The light kind a language binding or configuration names.
    pub fn parse(name: &str) -> Result<Self, String> {
        match name {
            "rendered" => Ok(Self::Rendered),
            "camera-raw" => Ok(Self::CameraRaw),
            other => Err(format!("unknown sensor picture light {other}")),
        }
    }
}

fn bits(bits: &RegisterIdleBits) -> Value {
    json!([bits.offset, bits.mask])
}

/// A declared white-balance reference with its green value in all three
/// channels: programmed gains then act on the light absolutely, by their ratio
/// to green, instead of relative to a scene that renders neutral.
fn flat_reference(reference: [u32; 3]) -> [u32; 3] {
    [reference[1]; 3]
}

/// The selected body hardware profile of a resolved run.
fn body_profile(resolved: &ResolvedConfiguration) -> Result<&BodyProfile, String> {
    let choice = resolved
        .configuration
        .preset
        .as_ref()
        .and_then(|preset| preset.hardware_selection.as_ref())
        .ok_or("X-Processor Pro assembly requires a hardware selection")?;
    let body = &resolved.reference.body;
    body.hardware
        .as_ref()
        .and_then(|hardware| hardware.profiles.get(&choice.body_profile_id))
        .ok_or_else(|| {
            format!(
                "body {} has no hardware profile {}",
                body.id, choice.body_profile_id
            )
        })
}

/// The device model of a block, after the selected body's overrides; the last
/// override naming the block wins.
fn device_for<'a>(block: &'a SocBlock, overrides: &'a [BlockOverride]) -> Option<&'a BlockDevice> {
    overrides
        .iter()
        .rev()
        .find(|item| item.block_id == block.id)
        .map(|item| &item.device)
        .or(block.device.as_ref())
}

/// Build the register layout the legacy board builder consumes from the
/// processor blocks, applying the selected body's device overrides.
fn block_register_layouts(
    soc: &Soc,
    overrides: &[BlockOverride],
) -> Result<RegisterLayouts, String> {
    use fujimod_catalog::profiles::*;

    let blocks: BTreeMap<&str, &SocBlock> = soc
        .blocks
        .iter()
        .map(|block| (block.id.as_str(), block))
        .collect();
    for override_ in overrides {
        if !blocks.contains_key(override_.block_id.as_str()) {
            return Err(format!(
                "body override names absent SoC {} block {}",
                soc.id, override_.block_id
            ));
        }
    }
    let first_irq = |block: &SocBlock| block.interrupts.first().map(|item| item.line as u64);
    let named_irq = |block: &SocBlock, output: &str| {
        block
            .interrupts
            .iter()
            .find(|item| item.output == output)
            .map(|item| item.line as u64)
    };
    let irqs = |block: &SocBlock| {
        block
            .interrupts
            .iter()
            .map(|item| item.line as u64)
            .collect::<Vec<_>>()
    };

    let mut maps = RegisterLayouts::default();
    let mut sd_clocks = Vec::new();
    let mut control_requests = Vec::new();
    let mut display_outputs = Vec::new();
    let mut register_blocks = Vec::new();
    let mut lcdc = Vec::new();
    let mut movdc = Vec::new();
    let mut sd_hosts = Vec::new();
    let mut serial_bases = Vec::new();
    let mut spi = BTreeMap::new();
    let mut i2c = Vec::new();
    let mut usb_core = None;
    let mut usb_phy = None;

    for block in &soc.blocks {
        let Some(device) = device_for(block, overrides) else {
            continue;
        };
        match device {
            BlockDevice::ControlBus { offsets, modes } => {
                maps.control_bus0 = Some(PrimaryBusRegisters {
                    base: block.base,
                    irq: first_irq(block).unwrap_or_default(),
                    modes: modes.clone(),
                    offsets: offsets.clone(),
                });
            }
            BlockDevice::SecondaryBus {} => {
                maps.control_bus1 = Some(SecondaryBusRegistersAddress {
                    base: block.base,
                    irq: first_irq(block).unwrap_or_default(),
                });
            }
            BlockDevice::Nand {} => maps.nand = Some(LoaderDma { base: block.base }),
            BlockDevice::LoaderDma {} => maps.loader_dma = Some(LoaderDma { base: block.base }),
            BlockDevice::Decompressor { basis } => {
                maps.decompressor = Some(Decompressor {
                    base: block.base,
                    basis: basis.clone(),
                    irq_route: first_irq(block),
                });
            }
            BlockDevice::Usb { basis } => usb_core = Some((block, basis.clone())),
            BlockDevice::UsbPhy { basis } => {
                usb_phy = Some((block, basis.clone()));
            }
            BlockDevice::SdHostClock {
                settle_ticks,
                basis,
            } => {
                sd_clocks.push((block.base, *settle_ticks, basis.clone()));
            }
            BlockDevice::Gpio { basis } => {
                maps.gpio = Some(Gpio {
                    base: block.base,
                    basis: basis.clone(),
                    size: block.size,
                });
            }
            BlockDevice::Adc {
                sample_ticks,
                basis,
            } => {
                let value = Adc {
                    base: block.base,
                    basis: basis.clone(),
                    sample_ticks: *sample_ticks,
                    size: block.size,
                };
                if block.id == "temperature" {
                    maps.temperature = Some(value);
                } else {
                    maps.adc = Some(value);
                }
            }
            BlockDevice::Dmp { basis } => {
                maps.dmp = Some(Dmp {
                    base: block.base,
                    basis: basis.clone(),
                    irq_route: first_irq(block).unwrap_or_default(),
                    size: block.size,
                });
            }
            BlockDevice::Movdc { reset_ticks, .. } => movdc.push(Movdc {
                base: block.base,
                irq: first_irq(block).unwrap_or_default(),
                reset_ticks: *reset_ticks,
            }),
            BlockDevice::ControlRequest {
                ack_ticks,
                request_offset,
                basis,
            } => {
                control_requests.push((block.base + *request_offset, *ack_ticks, basis.clone()));
            }
            BlockDevice::IoGroups { basis } => {
                maps.io_groups = Some(IoGroups {
                    base: block.base,
                    basis: basis.clone(),
                });
            }
            BlockDevice::Spi {
                parent_clock_ticks,
                basis,
            } => {
                if let Some(name) = block.id.strip_prefix("spi") {
                    spi.insert(
                        name.into(),
                        Spi {
                            base: block.base,
                            basis: basis.clone(),
                            parent_clock_ticks: *parent_clock_ticks,
                        },
                    );
                }
            }
            BlockDevice::SerialFlash {
                aperture_block_id,
                aperture_size,
                timing,
                native_dependencies,
                basis,
            } => {
                let aperture = blocks.get(aperture_block_id.as_str()).ok_or_else(|| {
                    format!(
                        "serial flash {} names absent aperture {}",
                        block.id, aperture_block_id
                    )
                })?;
                let size = aperture_size.unwrap_or(aperture.size);
                if timing.is_some() || !native_dependencies.is_empty() {
                    maps.serial_flash = Some(SerialFlash {
                        aperture: aperture.base,
                        base: block.base,
                        basis: basis.clone().unwrap_or_default(),
                        irq_route: first_irq(block).unwrap_or_default(),
                        native_dependencies: native_dependencies.clone(),
                        size,
                        timing: timing.clone().unwrap_or_default(),
                    });
                } else {
                    maps.flash_controller = Some(FlashController {
                        aperture: aperture.base,
                        base: block.base,
                        basis: basis.clone().unwrap_or_default(),
                        irq_route: first_irq(block),
                        size,
                    });
                }
            }
            BlockDevice::Dma {
                channels,
                request_mux_block_id,
                timing,
                native_dependencies,
                basis,
            } => {
                let mux = blocks.get(request_mux_block_id.as_str()).ok_or_else(|| {
                    format!(
                        "DMA {} names absent request mux {}",
                        block.id, request_mux_block_id
                    )
                })?;
                maps.dma = Some(Dma {
                    base: block.base,
                    basis: basis.clone().unwrap_or_default(),
                    channels: u64::from(*channels),
                    irq_routes: irqs(block),
                    native_dependencies: native_dependencies.clone(),
                    request_mux: mux.base,
                    timing: timing.clone().unwrap_or_default(),
                });
            }
            BlockDevice::IntervalTimer {
                layout,
                clock,
                basis,
            } => {
                maps.interval_timer = Some(IntervalTimer {
                    base: block.base,
                    basis: basis.clone(),
                    clock: clock.clone(),
                    layout: *layout,
                    routes: irqs(block)
                        .into_iter()
                        .enumerate()
                        .map(|(channel, irq)| TimerRoute {
                            channel: channel as u64,
                            irq,
                        })
                        .collect(),
                });
            }
            BlockDevice::RegisterBlock {
                idle_bits,
                fixed_words,
                commands,
                frames,
                job,
                basis,
                order,
            } => {
                let frames = frames.as_ref().map(|frames| RegisterBlockFrames {
                    start: frames.start.clone(),
                    stopped: frames.stopped.clone(),
                    irq_route: first_irq(block).unwrap_or_default() as u32,
                    period_ticks: frames.period_ticks,
                });
                let job = job.as_ref().map(|job| RegisterBlockJob {
                    start: job.start.clone(),
                    status: job.status.clone(),
                    stop: job.stop.clone(),
                    irq_route: first_irq(block).unwrap_or_default() as u32,
                    ticks: job.ticks,
                });
                register_blocks.push((
                    order.unwrap_or(u32::MAX),
                    RegisterBlock {
                        base: block.base,
                        basis: basis.clone(),
                        idle_bits: idle_bits.clone(),
                        frames,
                        job,
                        fixed_words: fixed_words.clone(),
                        commands: commands.clone(),
                        size: block.size,
                    },
                ));
            }
            BlockDevice::Pl330 {
                setup_ticks,
                bytes_per_tick,
                timing,
                basis,
            } => {
                maps.pl330 = Some(Pl330 {
                    base: block.base,
                    irq_routes: irqs(block),
                    setup_ticks: *setup_ticks,
                    bytes_per_tick: *bytes_per_tick,
                    timing: timing.clone(),
                    basis: basis.clone(),
                });
            }
            BlockDevice::SerialPort { basis } => serial_bases.push((block.base, basis.clone())),
            BlockDevice::JpegCodec {
                run_ticks, basis, ..
            } => {
                let value = JpegCodec {
                    base: block.base,
                    basis: basis.clone(),
                    irq_route: first_irq(block).unwrap_or_default() as u32,
                    run_ticks: *run_ticks,
                    size: block.size,
                };
                if block.id.contains("decoder") {
                    maps.jpeg_decoder = Some(value);
                } else {
                    maps.jpeg_encoder = Some(value);
                }
            }
            BlockDevice::FaceDetection { run_ticks, basis } => {
                maps.face_detection = Some(FaceDetection {
                    base: block.base,
                    basis: basis.clone(),
                    irq_route: first_irq(block).unwrap_or_default() as u32,
                    run_ticks: *run_ticks,
                    size: block.size,
                });
            }
            BlockDevice::SdHost {
                capabilities,
                capabilities_2,
                clock_ticks,
                slot_and_version,
                transfer_ticks,
                vendor_offsets,
                basis,
            } => {
                sd_hosts.push(SdHost {
                    base: block.base,
                    basis: basis.clone(),
                    capabilities: *capabilities,
                    capabilities_2: *capabilities_2,
                    clock_ticks: *clock_ticks,
                    irq_route: first_irq(block).map(|line| line as u32),
                    size: block.size,
                    slot_and_version: *slot_and_version,
                    transfer_ticks: *transfer_ticks,
                    vendor_offsets: vendor_offsets.clone(),
                });
            }
            BlockDevice::XppUsb {
                command_ticks,
                transaction_ticks,
                basis,
            } => {
                maps.xpp_usb = Some(XppUsb {
                    base: block.base,
                    basis: basis.clone(),
                    command_ticks: *command_ticks,
                    irq_route: first_irq(block).unwrap_or_default() as u32,
                    size: block.size,
                    transaction_ticks: *transaction_ticks,
                });
            }
            BlockDevice::DisplayOutput {
                set_bits,
                clear_bits,
                handshake_ticks,
                scanout,
                basis,
                order,
            } => {
                display_outputs.push((
                    order.unwrap_or(u32::MAX),
                    DisplayOutput {
                        handshake_ticks: *handshake_ticks,
                        base: block.base,
                        basis: basis.clone(),
                        clear_bits: clear_bits.clone(),
                        set_bits: set_bits.clone(),
                        size: block.size,
                        scanout: scanout.clone(),
                    },
                ));
            }
            BlockDevice::PllClocks { lock_ticks, basis } => {
                maps.pll_clocks = Some(PllClocks {
                    base: block.base,
                    basis: basis.clone(),
                    lock_ticks: *lock_ticks,
                    size: block.size,
                });
            }
            BlockDevice::ImagingControl { readout, basis } => {
                maps.imaging_control = Some(ImagingControl {
                    base: block.base,
                    basis: basis.clone(),
                    readout: readout.as_ref().map(|readout| SensorReadout {
                        basis: readout.basis.clone(),
                        irq_route: named_irq(block, "readout").unwrap_or_default() as u32,
                        offset: readout.offset,
                        ticks: readout.ticks,
                        starts_on_frame: readout.starts_on_frame,
                    }),
                    size: block.size,
                });
            }
            BlockDevice::ImagingStatistics {
                period_ticks,
                basis,
            } => {
                maps.imaging_statistics = Some(ImagingStatistics {
                    base: block.base,
                    basis: basis.clone(),
                    irq_route: first_irq(block).unwrap_or_default() as u32,
                    period_ticks: *period_ticks,
                    size: block.size,
                });
            }
            BlockDevice::HdmiTx { basis } => {
                maps.hdmi_tx = Some(HdmiTransmitter {
                    base: block.base,
                    basis: basis.clone(),
                    size: block.size,
                });
            }
            BlockDevice::FrameSync {
                interrupts,
                period_ticks,
                sources,
                basis,
                reads_set_after_clear,
            } => {
                maps.frame_sync = Some(FrameSync {
                    base: block.base,
                    basis: basis.clone(),
                    interrupts: *interrupts,
                    period_ticks: *period_ticks,
                    reads_set_after_clear: *reads_set_after_clear,
                    size: block.size,
                    sources: sources
                        .iter()
                        .map(|source| FrameSource {
                            irq_route: named_irq(
                                block,
                                &format!("frame-status-{}", source.status_bit),
                            )
                            .or_else(|| first_irq(block))
                            .unwrap_or_default() as u32,
                            status_bit: source.status_bit,
                        })
                        .collect(),
                });
            }
            BlockDevice::ResetGates { basis } => {
                maps.reset_gates = Some(ResetGates {
                    base: block.base,
                    basis: basis.clone(),
                    size: block.size,
                });
            }
            BlockDevice::DramController { basis } => {
                maps.dram_controller = Some(DramController {
                    base: block.base,
                    basis: basis.clone(),
                    size: block.size,
                });
            }
            BlockDevice::PowerControl { basis } => {
                maps.power_control = Some(PowerControl {
                    base: block.base,
                    basis: basis.clone(),
                    size: block.size,
                });
            }
            BlockDevice::I2c {
                fifo_depth,
                clock_hz,
                tick_hz,
                basis,
            } => i2c.push((
                block.base,
                basis.clone(),
                *clock_hz,
                *fifo_depth,
                first_irq(block).unwrap_or_default() as u32,
                block.size,
                *tick_hz,
            )),
            BlockDevice::XppSd {
                register_offset,
                reset_channel,
                transfer_ticks,
                clock_ticks,
                reset_ticks,
                basis,
            } => {
                maps.xpp_sd = Some(XppSdHost {
                    base: block.base,
                    basis: basis.clone(),
                    clock_ticks: *clock_ticks,
                    irq_route: first_irq(block).unwrap_or_default() as u32,
                    register_offset: *register_offset,
                    reset_channel: *reset_channel,
                    reset_ticks: *reset_ticks,
                    size: block.size,
                    transfer_ticks: *transfer_ticks,
                });
            }
            BlockDevice::H264Encoder { run_ticks, basis } => {
                maps.h264_encoder = Some(H264Encoder {
                    base: block.base,
                    basis: basis.clone(),
                    irq_route: first_irq(block).unwrap_or_default() as u32,
                    run_ticks: *run_ticks,
                    size: block.size,
                });
            }
            BlockDevice::SensorCommand {
                command_ticks,
                reset_ticks,
                basis,
            } => {
                let _ = (command_ticks, reset_ticks, basis);
            }
            BlockDevice::ImagingReceiver { basis } => {
                let _ = basis;
            }
            BlockDevice::Display {
                picture_layout,
                video_layout,
                pixel_clock_ticks,
                basis,
                frame_basis,
                period_ticks,
                timing,
                set_bits,
                clear_bits,
                handshake_ticks,
                scanout,
                order,
            } => {
                if let Some(pixel_clock_ticks) = pixel_clock_ticks {
                    lcdc.push(Lcdc {
                        base: block.base,
                        basis: basis.clone().unwrap_or_default(),
                        irq_routes: irqs(block),
                        picture_layout: picture_layout.as_ref().map(|_| "abgr8888-straight".into()),
                        pixel_clock_ticks: *pixel_clock_ticks,
                        video_layout: video_layout.as_ref().map(|_| "yycbcr422".into()),
                    });
                }
                if let Some(period_ticks) = period_ticks {
                    maps.display_frames = Some(DisplayFrames {
                        timing: timing.clone(),
                        basis: frame_basis.clone().unwrap_or_default(),
                        irq_routes: irqs(block),
                        period_ticks: *period_ticks,
                    });
                }
                if !set_bits.is_empty()
                    || !clear_bits.is_empty()
                    || scanout.is_some()
                    || handshake_ticks.is_some()
                {
                    display_outputs.push((
                        order.unwrap_or(u32::MAX),
                        DisplayOutput {
                            handshake_ticks: *handshake_ticks,
                            base: block.base,
                            basis: basis.clone().unwrap_or_default(),
                            clear_bits: clear_bits.clone(),
                            set_bits: set_bits.clone(),
                            size: block.size,
                            scanout: scanout.clone(),
                        },
                    ));
                }
            }
            BlockDevice::ImagingInputBuffer {
                reset_ticks, basis, ..
            } => {
                let _ = (reset_ticks, basis);
            }
            BlockDevice::SerialDma { .. }
            | BlockDevice::CompanionDma { .. }
            | BlockDevice::GicV2 { .. }
            | BlockDevice::GicV3 { .. }
            | BlockDevice::StillEngine { .. }
            | BlockDevice::FilterEngine { .. }
            | BlockDevice::Dsp { .. }
            | BlockDevice::CountdownTimer { .. }
            | BlockDevice::ControlGroup { .. }
            | BlockDevice::UsbCondition { .. }
            | BlockDevice::Xp5Timer { .. }
            | BlockDevice::Xp5TimerExtended { .. }
            | BlockDevice::SerialFlashAperture { .. }
            | BlockDevice::SerialFlashController { .. }
            | BlockDevice::FlashStorageDma { .. }
            | BlockDevice::Gates { .. }
            | BlockDevice::ClockBanks { .. }
            | BlockDevice::PanelPll { .. }
            | BlockDevice::GeneralDma { .. }
            | BlockDevice::AudioDma { .. }
            | BlockDevice::Xp5Gpio { .. }
            | BlockDevice::Xp5Adc { .. }
            | BlockDevice::Temperature { .. }
            | BlockDevice::InterprocessorMailbox { .. }
            | BlockDevice::AudioSerial { .. }
            | BlockDevice::MemoryConfiguration { .. }
            | BlockDevice::DdrPhy { .. }
            | BlockDevice::SdInterface { .. }
            | BlockDevice::SdhciUhs2 { .. }
            | BlockDevice::DisplayOutputLink { .. }
            | BlockDevice::Renderer { .. }
            | BlockDevice::ImagingPhy { .. }
            | BlockDevice::ImagingInput { .. }
            | BlockDevice::ImagingResize { .. }
            | BlockDevice::ImagingPaf { .. }
            | BlockDevice::ImagingPreview { .. }
            | BlockDevice::ImagingRifpIo { .. }
            | BlockDevice::ImagingRifp { .. }
            | BlockDevice::ImagingTnr { .. }
            | BlockDevice::ImagingFcnv { .. }
            | BlockDevice::ImagingFilterScaler { .. }
            | BlockDevice::ImagingRzl2 { .. }
            | BlockDevice::ImagingResz { .. }
            | BlockDevice::InputCapture { .. }
            | BlockDevice::Pl011 { .. }
            | BlockDevice::Pl022 { .. }
            | BlockDevice::Completion { .. }
            | BlockDevice::VideoCodec { .. }
            | BlockDevice::ImagingDsp { .. }
            | BlockDevice::ByteI2c { .. }
            | BlockDevice::UnmodeledBusDrain { .. } => {}
        }
    }

    if let Some((block, basis)) = usb_core {
        let phy_base = usb_phy.map(|(phy, _)| phy.base).unwrap_or_default();
        maps.usb = Some(UsbRegisters {
            basis,
            core: block.base,
            dma_irq: named_irq(block, "host").unwrap_or_default(),
            irq: named_irq(block, "device").unwrap_or_default(),
            phy: phy_base,
        });
    }
    if let Some((_, _, basis)) = sd_clocks.first() {
        maps.sd_host_clocks = Some(SdHostClocks {
            basis: basis.clone(),
            settle_ticks: sd_clocks.first().map(|item| item.1).unwrap_or_default(),
            words: sd_clocks.iter().map(|item| item.0).collect(),
        });
    }
    if let Some((ack_ticks, basis)) = control_requests
        .first()
        .map(|item| (item.1, item.2.clone()))
    {
        control_requests.sort_by_key(|item| item.0);
        maps.control_requests = Some(ControlRequests {
            ack_ticks,
            basis,
            registers: control_requests.into_iter().map(|item| item.0).collect(),
        });
    }
    display_outputs.sort_by_key(|item| item.0);
    if !display_outputs.is_empty() {
        maps.display_outputs = Some(display_outputs.into_iter().map(|item| item.1).collect());
    }
    register_blocks.sort_by_key(|item| item.0);
    maps.register_blocks = register_blocks.into_iter().map(|item| item.1).collect();
    lcdc.sort_by_key(|item| item.base);
    if !lcdc.is_empty() {
        maps.lcdc = Some(lcdc);
    }
    movdc.sort_by_key(|item| item.base);
    if !movdc.is_empty() {
        maps.movdc = Some(movdc);
    }
    sd_hosts.sort_by_key(|item| item.base);
    maps.sd_hosts = sd_hosts;
    serial_bases.sort_by_key(|item| item.0);
    if !serial_bases.is_empty() {
        maps.serial_ports = Some(SerialPorts {
            bases: serial_bases.iter().map(|item| item.0).collect(),
            basis: serial_bases
                .first()
                .map(|item| item.1.clone())
                .unwrap_or_default(),
        });
    }
    if !spi.is_empty() {
        maps.spi = Some(spi);
    }
    i2c.sort_by_key(|item| item.0);
    maps.xpp_i2c = i2c.first().map(|first| XppI2cBuses {
        base: first.0,
        basis: first.1.clone(),
        clock_hz: first.2,
        fifo_depth: first.3,
        irq_routes: i2c.iter().map(|item| item.4).collect(),
        size: first.5,
        stride: i2c
            .get(1)
            .map(|second| second.0 - first.0)
            .unwrap_or(first.5),
        tick_hz: first.6,
    });
    Ok(maps)
}

/// The GIC node of the selected SoC: the distributor block that carries the
/// GICv2 device, the CPU-interface block it names and its implemented lines.
fn gic_node(soc: &Soc) -> Result<Value, String> {
    let controller = soc.blocks.iter().find_map(|block| match &block.device {
        Some(BlockDevice::GicV2 {
            cpu_interface_block_id,
            lines,
            ..
        }) => Some((block, cpu_interface_block_id, lines)),
        _ => None,
    });
    let Some((distributor, cpu_interface_block_id, lines)) = controller else {
        let gicv3 = soc
            .blocks
            .iter()
            .any(|block| matches!(block.device, Some(BlockDevice::GicV3 { .. })));
        return Err(if gicv3 {
            "X-Processor Pro assembly requires a GICv2 interrupt controller".to_string()
        } else {
            "X-Processor Pro assembly requires an interrupt controller".to_string()
        });
    };
    let cpu_interface = soc
        .blocks
        .iter()
        .find(|block| block.id == *cpu_interface_block_id)
        .map(|block| block.base)
        .ok_or_else(|| {
            format!("interrupt controller names absent block {cpu_interface_block_id}")
        })?;
    Ok(json!({
        "distributor": distributor.base,
        "cpu_interface": cpu_interface,
        "lines": lines,
    }))
}

/// Command-dial encoder nodes of the selected body profile. Each declaration
/// names a body connection whose two GPIO signals are the dial's phase pads, so
/// the pads come from the body record and only the edge pacing is model policy.
fn encoder_nodes(body: &Body, profile: &BodyProfile) -> Result<Vec<Value>, String> {
    let mut out = Vec::new();
    for declaration in profile.encoders.iter().flatten() {
        let connection = body
            .connections
            .iter()
            .find(|connection| connection.id == declaration.name)
            .ok_or_else(|| {
                format!(
                    "command-dial encoder {} is not a declared body connection",
                    declaration.name
                )
            })?;
        let signals: Vec<(u8, u8)> = connection
            .signals
            .iter()
            .filter_map(|signal| match signal {
                ConnectionSignal::Gpio { port, pin, .. } => Some((*port, *pin)),
                _ => None,
            })
            .collect();
        let declared: Vec<(u8, u8)> = declaration
            .options
            .pins
            .iter()
            .flatten()
            .map(|pin| (pin.port, pin.pin))
            .collect();
        if declared.len() != 2 || signals != declared {
            return Err(format!(
                "command-dial encoder {} pins differ from its connection's two GPIO signals",
                declaration.name
            ));
        }
        if declaration.options.phase_ticks == 0 || declaration.options.rest_phase > 3 {
            return Err(format!(
                "command-dial encoder {} needs a positive edge period and a two-bit rest phase",
                declaration.name
            ));
        }
        out.push(json!({
            "name": declaration.name,
            "model": {
                "kind": "encoder",
                "options": {
                    "pins": declared.iter()
                        .map(|&(port, pin)| json!({ "port": port, "pin": pin }))
                        .collect::<Vec<_>>(),
                    "phase_ticks": declaration.options.phase_ticks,
                    "rest_phase": declaration.options.rest_phase,
                },
                "counter": declaration.counter,
            },
        }));
    }
    Ok(out)
}

fn hex_bytes(text: &str, label: &str) -> Result<Vec<u8>, String> {
    if !text.len().is_multiple_of(2) {
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

/// The words of a declared statistics buffer, checked for complete words.
fn buffer_words(assets: &BTreeMap<String, Vec<u8>>, asset: &str) -> Result<Vec<u32>, String> {
    let data = assets
        .get(asset)
        .ok_or_else(|| format!("statistics buffer asset {asset} was not supplied"))?;
    if data.is_empty() || data.len() % 4 != 0 {
        return Err(format!(
            "statistics buffer asset {asset} must contain complete words"
        ));
    }
    Ok(data
        .as_chunks::<4>()
        .0
        .iter()
        .map(|word| u32::from_le_bytes([word[0], word[1], word[2], word[3]]))
        .collect())
}

/// Hexadecimal rows of a declared live-view host image with the declared size.
fn host_image(
    assets: &BTreeMap<String, Vec<u8>>,
    image: &fujimod_catalog_run_inputs::hardware_inputs::LiveViewHostImage,
) -> Result<String, String> {
    let asset = &image.asset;
    let data = assets
        .get(asset)
        .ok_or_else(|| format!("live-view image asset {asset} was not supplied"))?;
    let size = u64::from(image.width)
        .checked_mul(u64::from(image.height))
        .and_then(|pixels| pixels.checked_mul(3))
        .filter(|size| *size > 0)
        .ok_or_else(|| format!("live-view image asset {asset} dimensions are invalid"))?;
    if data.len() as u64 != size {
        return Err(format!("live-view image asset {asset} size differs"));
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
            json!({ "kind": "gpio", "base": gpio.base, "options": { "input_levels": levels.levels } }),
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
                        "identification": nor.identification,
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
                "kind": "adc", "base": adc.base,
                "options": { "samples": samples.samples, "conversion_ticks": adc.sample_ticks },
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
            .chain(&development.color)
            .any(|&unit| !maps.register_blocks.iter().any(|block| block.base == unit))
    {
        return Err("still development units must be register blocks with jobs".into());
    }
    // A camera-raw picture holds the camera's own light per site, not light
    // rendered under the declared wall: the wall's camera-space radiance is its
    // reference (zone sums and the film's uniform light), and both white-balance
    // references go flat so the programmed gains act on it absolutely.
    let camera_raw_radiance = match inputs
        .sensor_image
        .filter(|picture| picture.light == SensorImageLight::CameraRaw)
    {
        None => None,
        Some(_) => {
            let radiance = image
                .and_then(|image| image.film.as_ref())
                .and_then(|film| film.camera_raw_radiance.as_ref())
                .map(|declared| declared.radiance)
                .ok_or("a camera-raw sensor picture needs a declared camera-raw radiance")?;
            if radiance.contains(&0) {
                return Err("a camera-raw radiance needs light in every channel".into());
            }
            Some(radiance)
        }
    };
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
                if let Some(color) = development.color {
                    law["color"] = json!(color);
                    law["white_balance"] = json!(match camera_raw_radiance {
                        Some(_) => development.white_balance_reference.map(flat_reference),
                        None => development.white_balance_reference,
                    });
                }
                if let Some(tone) = development.tone {
                    if development.color.is_none() {
                        return Err("a still tone unit needs the still color unit".into());
                    }
                    law["tone"] = json!(tone);
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
            if let Some(film) = &image.film {
                if development.is_none_or(|d| d.color.is_none())
                    || image.scene.is_some()
                    || image.host_image.is_some()
                {
                    return Err("a live-view film needs the still color unit and no scene".into());
                }
                model["live_view_image"]["film"] = json!({
                    "linear_rgb": camera_raw_radiance.unwrap_or(film.linear_rgb),
                    "white_balance_reference": match camera_raw_radiance {
                        Some(_) => flat_reference(film.white_balance_reference),
                        None => film.white_balance_reference,
                    },
                });
                // The sensor's host picture is the scene the film develops in live view.
                if let Some(picture) = inputs.sensor_image {
                    model["live_view_image"]["film"]["picture"] = json!({
                        "width": picture.width, "height": picture.height,
                        "rgb": picture.rgb.iter().map(|byte| format!("{byte:02x}")).collect::<String>(),
                    });
                }
            }
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
            if let Some(exposure) = &scene.exposure {
                if maps.imaging_control.is_none() {
                    return Err("a zone-sum exposure requires the imaging control page".into());
                }
                model["exposure_reference"] = json!(exposure.reference);
            }
            model["buffers"] = scene
                .buffers
                .iter()
                .map(|b| Ok(json!([b.output, buffer_words(inputs.assets, &b.asset)?])))
                .collect::<Result<Value, String>>()?;
            let light_words: Vec<Value> = scene
                .buffers
                .iter()
                .filter(|b| !b.light_words.is_empty())
                .map(|b| json!([b.output, b.light_words]))
                .collect();
            if !light_words.is_empty() {
                model["buffer_light_words"] = json!(light_words);
            }
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
    if let Some(codec) = &maps.h264_encoder {
        node(
            "h264-encoder".into(),
            json!({
                "kind": "h264", "base": codec.base, "irq": codec.irq_route,
                "run_ticks": codec.run_ticks,
            }),
        );
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
            let touch_parts: Vec<_> = reset
                .i2c_parts
                .iter()
                .filter(|part| part.bus as usize == bus && part.touch_panel.is_some())
                .collect();
            if touch_parts.len() > 1 {
                return Err(format!("I2C bus {bus} has multiple touch-panel models"));
            }
            let parts: Vec<Value> = reset
                .i2c_parts
                .iter()
                .filter(|part| part.bus as usize == bus && part.touch_panel.is_none())
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
            let options = json!({
                "base": buses.base + bus as u64 * buses.stride, "irq": irq,
                "fifo_depth": buses.fifo_depth, "clock_hz": buses.clock_hz, "tick_hz": buses.tick_hz,
            });
            if let Some(part) = touch_parts.first() {
                let touch = part
                    .touch_panel
                    .as_ref()
                    .ok_or("touch model options are absent")?;
                node(
                    format!("i2c{bus}"),
                    json!({ "kind": "xt20-touch-i2c", "options": options, "parts": parts,
                        "touch": {
                            "address": part.address,
                            "report_register": touch.report_register,
                            "input_ready_irq": touch.input_ready_irq,
                            "ticks_per_second": buses.tick_hz,
                        }
                    }),
                );
            } else {
                node(
                    format!("i2c{bus}"),
                    json!({ "kind": "i2c", "options": options, "parts": parts }),
                );
            }
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
                "timing": frames.timing.as_ref().map(|t| json!([t.bases, t.stop_ticks])),
            }),
        );
    }
    if inputs.sensor_image.is_some() && reset.sensor_frame.is_none() {
        return Err("a sensor host image requires a declared sensor frame".into());
    }
    if let Some(image) = inputs.sensor_image {
        let size = u64::from(image.width) * u64::from(image.height) * 3;
        if size == 0 || image.rgb.len() as u64 != size {
            return Err("sensor host image size differs from its rows".into());
        }
    }
    if let Some(control) = &maps.imaging_control {
        if reset.sensor_frame.is_some() && control.readout.is_none() {
            return Err("a sensor frame requires the sensor read-out unit".into());
        }
        // A read-out that starts on a frame takes the frame sync's period.
        let readout_frames = match (&control.readout, &maps.frame_sync) {
            (Some(r), Some(sync)) if r.starts_on_frame => Some(frame_period(sync.period_ticks)),
            (Some(r), None) if r.starts_on_frame => {
                return Err("a read-out that starts on a frame requires the frame sync".into());
            }
            _ => None,
        };
        node(
            "imaging".into(),
            json!({
                "kind": "imaging-control", "base": control.base,
                "hole": maps.frame_sync.as_ref().map(|sync| sync.interrupts),
                "readout": control.readout.as_ref().map(|r| json!({
                    "offset": r.offset, "irq": r.irq_route, "ticks": r.ticks,
                    "frame_period": readout_frames,
                })),
                "sensor_frame": reset.sensor_frame.as_ref().map(|frame| &frame.profile),
                "sensor_image": inputs.sensor_image.map(|image| json!({
                    "width": image.width, "height": image.height,
                    "rgb": image.rgb.iter().map(|byte| format!("{byte:02x}")).collect::<String>(),
                })),
                "integration": reset
                    .zone_statistics
                    .as_ref()
                    .and_then(|scene| scene.exposure.as_ref())
                    .map(|e| json!([e.select, e.low, e.high])),
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
                "reads_set_after_clear": sync.reads_set_after_clear,
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
    let emulation = resolved.processor_emulation()?;
    if emulation.runtime_abi != "armv7a-unicorn-v1" || emulation.cpu_model != "Cortex-A7" {
        return Err(format!(
            "SoC profile of {} is not an ARMv7-A Cortex-A7 application profile",
            selection.soc.id
        ));
    }
    let reset = resolved
        .hardware_inputs()?
        .ok_or("X-Processor Pro assembly requires hardware inputs")?
        .reset;
    let body = &selection.body;
    let profile = body_profile(resolved)?;
    let maps = block_register_layouts(&selection.soc, &profile.block_overrides)?;
    let gic = gic_node(&selection.soc)?;
    let (numerator, denominator) = reset
        .execution_clock
        .as_ref()
        .map_or((1, 1), |clock| clock.instructions_per_tick);
    let dram_address_space_limit_bytes = resolved.dram_address_space_limit_bytes()?;
    let execution = graph::assemble_with_starts(
        selection,
        dram_address_space_limit_bytes,
        RunInstructionRate {
            numerator,
            denominator,
        },
        None,
        inputs.starts,
    )?;
    let cores = execution.processors.len();
    let mut cpus = Vec::with_capacity(cores);
    for core in 0..cores {
        let processor = selection.soc.processors.get(core).ok_or_else(|| {
            format!(
                "X-Processor Pro assembly starts core {core}; the SoC declares {} processors",
                selection.soc.processors.len()
            )
        })?;
        let Some(AProfileState::Armv7a {
            configuration_base,
            caches,
        }) = &processor.aprofile
        else {
            return Err(format!(
                "X-Processor Pro assembly requires ARMv7-A application hardware on processor {}",
                processor.id
            ));
        };
        cpus.push(json!({
            "mpidr": MPIDR_MULTIPROCESSOR | core as u64,
            "configuration_base": configuration_base,
            "cycles_per_tick": CYCLES_PER_TICK,
            "cache_sizes": [caches.l1_data_bytes, caches.l1_instruction_bytes, caches.l2_bytes],
        }));
    }
    let mut node_list = nodes(
        &maps,
        &reset,
        dram_address_space_limit_bytes,
        &ram_banks(&selection.soc.memory_regions),
        inputs,
    )?;
    // Command dials last: they take GPIO pads the bank node declares.
    node_list.extend(encoder_nodes(body, profile)?);
    let devices = json!({
        "kind": "xpp",
        "cpus": cpus,
        "gic": gic,
        "nodes": node_list,
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
    sensor_image: Option<&SensorImage>,
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
            parked_until_sgi: false,
            registers,
        })
        .collect();
    let assembly = assemble(
        &resolved,
        &Inputs {
            starts: &starts,
            sub_cpu_registers,
            assets,
            sensor_image,
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
                parked_until_sgi: false,
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
                sensor_image: None,
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
    fn only_the_xt20_body_adds_the_h264_and_playback_registers() {
        for (configuration, expected) in [
            ("gfx50r-311", false),
            ("xt2-441", false),
            ("xt20-211", true),
        ] {
            let resolved = ResolvedConfiguration::bundled(configuration).unwrap();
            let maps = block_register_layouts(
                &resolved.reference.soc,
                &body_profile(&resolved).unwrap().block_overrides,
            )
            .unwrap();
            assert_eq!(
                maps.h264_encoder.is_some(),
                expected,
                "{configuration} H.264 encoder"
            );
            assert_eq!(
                maps.register_blocks
                    .iter()
                    .any(|block| block.base == 0xfff5_b000),
                expected,
                "{configuration} H.264 idle block"
            );
            assert_eq!(
                maps.register_blocks
                    .iter()
                    .any(|block| block.base == 0xfffd_5000),
                expected,
                "{configuration} playback display block"
            );
        }
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
        assert_eq!(
            names[names.len() - 5..],
            [
                "lens",
                "shutter",
                "key-lines",
                "controls-encoder-front",
                "controls-encoder-rear"
            ]
        );
        assert_eq!(devices["cpus"].as_array().unwrap().len(), 2);
        assert_eq!(devices["cpus"][1]["mpidr"], 0x8000_0001_u64);
        assert_eq!(devices["cpus"][0]["configuration_base"], 0xffff_0000_u64);
        assert_eq!(node(devices, "h264-encoder")["base"], 0xfff5_c000_u64);
        assert!(
            devices["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|node| node["model"]["base"] == 0xfff5_b000_u64),
            "the body adds the 0xfff5b000 idle register block"
        );

        // Command dials: one encoder node per declared body connection, its pads
        // taken from the connection and its edge pacing from the body profile.
        assert!(position("gpio") < position("controls-encoder-front"));
        assert_eq!(
            node(devices, "controls-encoder-front")["options"],
            json!({
                "pins": [{"port": 1, "pin": 2}, {"port": 1, "pin": 3}],
                "phase_ticks": 240000,
                "rest_phase": 2,
            })
        );
        assert_eq!(
            node(devices, "controls-encoder-rear")["options"]["pins"],
            json!([{"port": 1, "pin": 4}, {"port": 1, "pin": 5}])
        );
        assert_eq!(
            node(devices, "controls-encoder-rear")["counter"],
            Value::Null
        );

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
        // Playback display engine: the completed descriptor's scanout raises
        // source 0x43 / GIC line 143, and +0x064 acknowledges the job.
        assert_eq!(
            engine(0xfffd_5000)["job"],
            json!({"start": [0, 1], "status": [0x64, 1], "stop": null, "irq": 143, "ticks": 100000})
        );
        assert_eq!(engine(0xfffd_5000)["idle"], json!([[0, 0x8000_0000u32]]));
        assert_eq!(engine(0xfffd_5000)["fixed"], json!([[0x60, 1]]));
        assert_eq!(
            engine(0xfff9_8000)["job"],
            json!({"start": [4, 5], "status": null, "stop": [0, 1], "irq": 130, "ticks": 144000})
        );
        assert_eq!(engine(0xfff7_e000)["fixed"], json!([[0x1c, 0x0100_0100]]));
        assert_eq!(
            engine(0xfff0_f000)["commands"][0],
            json!({"word": 0xb8, "value": 0x6a02_0202, "status": 0xbc, "mask": 0x3f, "state": 0x27})
        );
        assert_eq!(
            node(devices, "imaging")["readout"],
            json!({"offset": 0xc0, "irq": 98, "ticks": 2_605_440, "frame_period": 880_000})
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
        // The declared light sums of that buffer reach the node.
        assert_eq!(statistics["buffer_light_words"][0][0], 0x1d0);
        assert_eq!(
            statistics["buffer_light_words"][0][1],
            json!([
                0, 2, 4, 8, 10, 12, 16, 18, 20, 24, 26, 28, 32, 34, 36, 40, 42, 44, 48, 50, 52, 56,
                58, 60, 64, 66, 68
            ])
        );
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
        let built = build(
            assembly.execution,
            spec,
            &flash,
            Mode::Strict,
            fujimod_catalog_configuration::RunEngine::Unicorn,
        )
        .unwrap();
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
    fn a_live_view_film_reaches_its_unit_and_needs_the_still_color_unit() {
        let resolved = ResolvedConfiguration::bundled("xt20-211").unwrap();
        let maps = block_register_layouts(
            &resolved.reference.soc,
            &body_profile(&resolved).unwrap().block_overrides,
        )
        .unwrap();
        let selection = &resolved.reference;
        let banks = ram_banks(&selection.soc.memory_regions);
        let reset = resolved.hardware_inputs().unwrap().unwrap().reset;
        let unit = reset.live_view_image.as_ref().unwrap().unit;
        let (starts, assets) = (starts(), assets());
        let built = |reset: &HardwareReset| {
            let inputs = Inputs {
                starts: &starts,
                sub_cpu_registers: &[],
                assets: &assets,
                sensor_image: None,
            };
            nodes(
                &maps,
                reset,
                selection.body.dram_bytes.unwrap(),
                &banks,
                &inputs,
            )
        };
        let find = |nodes: &[Value]| {
            nodes
                .iter()
                .map(|node| node["model"].clone())
                .find(|model| model["base"] == unit)
                .unwrap()
        };
        // The X-T20 declares its wall as film light developed with the color unit.
        let film = reset
            .live_view_image
            .as_ref()
            .unwrap()
            .film
            .clone()
            .unwrap();
        assert_eq!(film.linear_rgb, [1015, 764, 577]);
        let block = find(&built(&reset).unwrap());
        assert_eq!(
            block["live_view_image"]["film"],
            json!({"linear_rgb": [1015, 764, 577], "white_balance_reference": [1138, 604, 1096]})
        );
        let mut without = reset.clone();
        without.still_development.as_mut().unwrap().color = None;
        assert!(built(&without).is_err());
        let mut scene = reset.clone();
        scene.live_view_image.as_mut().unwrap().scene =
            Some("declared-rgb-bars-ramp-moving-square-bt601-limited-v1".into());
        assert_eq!(
            built(&scene).err().unwrap(),
            "a live-view film needs the still color unit and no scene"
        );
    }

    #[test]
    fn asset_words_and_host_image_dimensions_are_checked_directly() {
        use fujimod_catalog_run_inputs::hardware_inputs::LiveViewHostImage;
        let mut assets = BTreeMap::from([("input".into(), vec![0x78, 0x56, 0x34, 0x12])]);
        assert_eq!(buffer_words(&assets, "input").unwrap(), [0x1234_5678]);
        let mut image = LiveViewHostImage {
            asset: "input".into(),
            basis: "test".into(),
            width: 1,
            height: 1,
            sha256: "unused cache key".into(),
        };
        assert!(host_image(&assets, &image).unwrap_err().contains("size"));
        assets.get_mut("input").unwrap().pop();
        assert_eq!(host_image(&assets, &image).unwrap(), "785634");
        for (width, height) in [(0, 1), (1, 0), (u32::MAX, u32::MAX)] {
            image.width = width;
            image.height = height;
            assert!(
                host_image(&assets, &image)
                    .unwrap_err()
                    .contains("dimensions")
            );
        }
        assets.clear();
        assert!(
            host_image(&assets, &image)
                .unwrap_err()
                .contains("not supplied")
        );
    }

    #[test]
    fn a_live_view_host_image_is_checked_and_passed_to_its_unit() {
        use fujimod_catalog_run_inputs::hardware_inputs::LiveViewHostImage;
        let resolved = ResolvedConfiguration::bundled("xt20-211").unwrap();
        let maps = block_register_layouts(
            &resolved.reference.soc,
            &body_profile(&resolved).unwrap().block_overrides,
        )
        .unwrap();
        let selection = &resolved.reference;
        let banks = ram_banks(&selection.soc.memory_regions);
        let mut reset = resolved.hardware_inputs().unwrap().unwrap().reset;
        let image = reset.live_view_image.as_mut().unwrap();
        let unit = image.unit;
        image.film = None;
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
                sensor_image: None,
            };
            nodes(
                &maps,
                reset,
                selection.body.dram_bytes.unwrap(),
                &banks,
                &inputs,
            )
        };
        assets.get_mut("host").unwrap().pop();
        assert_eq!(
            built(&reset, &assets).err().unwrap(),
            "live-view image asset host size differs"
        );
        assets.insert("host".into(), pixels);
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
    fn a_caller_sensor_image_needs_a_declared_frame_and_matching_rows() {
        use fujimod_catalog_run_inputs::hardware_inputs::SensorFrame;
        let resolved = ResolvedConfiguration::bundled("xt20-211").unwrap();
        let maps = block_register_layouts(
            &resolved.reference.soc,
            &body_profile(&resolved).unwrap().block_overrides,
        )
        .unwrap();
        let selection = &resolved.reference;
        let banks = ram_banks(&selection.soc.memory_regions);
        let starts = starts();
        let assets = assets();
        let built = |reset: &HardwareReset, image: &SensorImage| {
            let inputs = Inputs {
                starts: &starts,
                sub_cpu_registers: &[],
                assets: &assets,
                sensor_image: Some(image),
            };
            nodes(
                &maps,
                reset,
                resolved.dram_address_space_limit_bytes().unwrap(),
                &banks,
                &inputs,
            )
        };
        let mut reset = resolved.hardware_inputs().unwrap().unwrap().reset;
        let image = SensorImage {
            width: 2,
            height: 1,
            rgb: vec![255, 0, 0, 0, 0, 255],
            light: SensorImageLight::Rendered,
        };
        reset.sensor_frame = None;
        assert_eq!(
            built(&reset, &image).err().unwrap(),
            "a sensor host image requires a declared sensor frame"
        );
        reset.sensor_frame = Some(SensorFrame {
            profile: fujimod_emulator_xpp::imaging::SENSOR_PROFILE.into(),
            basis: "test".into(),
        });
        let short = SensorImage {
            width: 2,
            height: 2,
            rgb: image.rgb.clone(),
            light: SensorImageLight::Rendered,
        };
        assert!(built(&reset, &short).is_err());
        let nodes = built(&reset, &image).unwrap();
        let imaging = nodes
            .iter()
            .find(|node| node["model"]["kind"] == "imaging-control")
            .unwrap();
        assert_eq!(
            imaging["model"]["sensor_image"],
            json!({"width": 2, "height": 1, "rgb": "ff00000000ff"})
        );
    }

    /// The graph nodes of one reset with an optional camera picture.
    fn picture_test(
        resolved: &ResolvedConfiguration,
        reset: &HardwareReset,
        picture: Option<&SensorImage>,
    ) -> Result<Vec<Value>, String> {
        let maps = block_register_layouts(
            &resolved.reference.soc,
            &body_profile(resolved).unwrap().block_overrides,
        )
        .unwrap();
        let selection = &resolved.reference;
        let banks = ram_banks(&selection.soc.memory_regions);
        let (starts, assets) = (starts(), assets());
        nodes(
            &maps,
            reset,
            resolved.dram_address_space_limit_bytes().unwrap(),
            &banks,
            &Inputs {
                starts: &starts,
                sub_cpu_registers: &[],
                assets: &assets,
                sensor_image: picture,
            },
        )
    }

    fn model_of(nodes: &[Value], base: u64) -> Value {
        nodes
            .iter()
            .find(|node| node["model"]["base"] == base)
            .unwrap()["model"]
            .clone()
    }

    fn picture() -> SensorImage {
        SensorImage {
            width: 2,
            height: 1,
            rgb: vec![255, 0, 0, 0, 0, 255],
            light: SensorImageLight::Rendered,
        }
    }

    #[test]
    fn a_rendered_sensor_picture_keeps_the_declared_film_and_still_references() {
        let resolved = ResolvedConfiguration::bundled("xt20-211").unwrap();
        let reset = resolved.hardware_inputs().unwrap().unwrap().reset;
        let unit = reset.live_view_image.as_ref().unwrap().unit;
        let developer = reset.still_development.as_ref().unwrap().developer;
        let without = picture_test(&resolved, &reset, None).unwrap();
        let rendered = picture_test(&resolved, &reset, Some(&picture())).unwrap();
        // The default light leaves the still law and the film light as declared.
        let declared_law = json!([734, 604, 2284]);
        assert_eq!(
            model_of(&without, developer)["work"]["white_balance"],
            declared_law
        );
        assert_eq!(
            model_of(&rendered, developer)["work"]["white_balance"],
            declared_law
        );
        let mut film = model_of(&rendered, unit)["live_view_image"]["film"].clone();
        assert_eq!(
            film["picture"],
            json!({"width": 2, "height": 1, "rgb": "ff00000000ff"})
        );
        film.as_object_mut().unwrap().remove("picture");
        assert_eq!(film, model_of(&without, unit)["live_view_image"]["film"]);
        assert_eq!(
            film,
            json!({"linear_rgb": [1015, 764, 577], "white_balance_reference": [1138, 604, 1096]})
        );
    }

    #[test]
    fn a_camera_raw_sensor_picture_takes_the_wall_radiance_and_flat_gains() {
        let resolved = ResolvedConfiguration::bundled("xt20-211").unwrap();
        let reset = resolved.hardware_inputs().unwrap().unwrap().reset;
        let unit = reset.live_view_image.as_ref().unwrap().unit;
        let developer = reset.still_development.as_ref().unwrap().developer;
        let radiance = reset
            .live_view_image
            .as_ref()
            .unwrap()
            .film
            .as_ref()
            .unwrap()
            .camera_raw_radiance
            .as_ref()
            .unwrap()
            .radiance;
        assert_eq!(radiance, [721, 764, 168]);
        let raw = SensorImage {
            light: SensorImageLight::CameraRaw,
            ..picture()
        };
        let nodes = picture_test(&resolved, &reset, Some(&raw)).unwrap();
        assert_eq!(
            model_of(&nodes, unit)["live_view_image"]["film"],
            json!({
                "linear_rgb": [721, 764, 168],
                "white_balance_reference": [604, 604, 604],
                "picture": {"width": 2, "height": 1, "rgb": "ff00000000ff"},
            })
        );
        assert_eq!(
            model_of(&nodes, developer)["work"]["white_balance"],
            json!([604, 604, 604])
        );
        let imaging = nodes
            .iter()
            .find(|node| node["model"]["kind"] == "imaging-control")
            .unwrap();
        assert_eq!(imaging["model"]["sensor_image"]["rgb"], "ff00000000ff");
    }

    #[test]
    fn a_camera_raw_sensor_picture_needs_the_declared_radiance_and_a_known_kind() {
        use fujimod_catalog_run_inputs::hardware_inputs::CameraRawRadiance;
        assert_eq!(
            SensorImageLight::parse("rendered"),
            Ok(SensorImageLight::Rendered)
        );
        assert_eq!(
            SensorImageLight::parse("camera-raw"),
            Ok(SensorImageLight::CameraRaw)
        );
        assert_eq!(
            SensorImageLight::parse("balanced"),
            Err("unknown sensor picture light balanced".into())
        );
        let resolved = ResolvedConfiguration::bundled("xt20-211").unwrap();
        let raw = SensorImage {
            light: SensorImageLight::CameraRaw,
            ..picture()
        };
        let mut reset = resolved.hardware_inputs().unwrap().unwrap().reset;
        let film = reset
            .live_view_image
            .as_mut()
            .unwrap()
            .film
            .as_mut()
            .unwrap();
        film.camera_raw_radiance = None;
        assert_eq!(
            picture_test(&resolved, &reset, Some(&raw)).err().unwrap(),
            "a camera-raw sensor picture needs a declared camera-raw radiance"
        );
        let mut reset = resolved.hardware_inputs().unwrap().unwrap().reset;
        let film = reset
            .live_view_image
            .as_mut()
            .unwrap()
            .film
            .as_mut()
            .unwrap();
        film.camera_raw_radiance = Some(CameraRawRadiance {
            basis: "test".into(),
            radiance: [721, 764, 0],
        });
        assert_eq!(
            picture_test(&resolved, &reset, Some(&raw)).err().unwrap(),
            "a camera-raw radiance needs light in every channel"
        );
    }

    #[test]
    fn a_declared_zone_exposure_reaches_the_imaging_and_statistics_nodes() {
        let resolved = ResolvedConfiguration::bundled("xt20-211").unwrap();
        let maps = block_register_layouts(
            &resolved.reference.soc,
            &body_profile(&resolved).unwrap().block_overrides,
        )
        .unwrap();
        let reset = resolved.hardware_inputs().unwrap().unwrap().reset;
        let (starts, assets) = (starts(), assets());
        let inputs = Inputs {
            starts: &starts,
            sub_cpu_registers: &[],
            assets: &assets,
            sensor_image: None,
        };
        let banks = ram_banks(&resolved.reference.soc.memory_regions);
        let nodes = nodes(
            &maps,
            &reset,
            resolved.dram_address_space_limit_bytes().unwrap(),
            &banks,
            &inputs,
        )
        .unwrap();
        let named = |name: &str| nodes.iter().find(|node| node["name"] == name).unwrap();
        assert_eq!(
            named("imaging")["model"]["integration"],
            json!([4, 0x1d, 0x1e])
        );
        assert_eq!(named("statistics")["model"]["exposure_reference"], 1566);
    }

    #[test]
    fn a_declared_sensor_frame_reaches_the_read_out_unit() {
        use fujimod_catalog_run_inputs::hardware_inputs::SensorFrame;
        let resolved = ResolvedConfiguration::bundled("xt20-211").unwrap();
        let maps = block_register_layouts(
            &resolved.reference.soc,
            &body_profile(&resolved).unwrap().block_overrides,
        )
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
            sensor_image: None,
        };
        let banks = ram_banks(&resolved.reference.soc.memory_regions);
        let nodes = nodes(
            &maps,
            &reset,
            resolved.dram_address_space_limit_bytes().unwrap(),
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
        let maps = block_register_layouts(
            &resolved.reference.soc,
            &body_profile(&resolved).unwrap().block_overrides,
        )
        .unwrap();
        let mut reset = resolved.hardware_inputs().unwrap().unwrap().reset;
        reset.live_view_image.as_mut().unwrap().film = None;
        reset.still_development = Some(StillDevelopment {
            basis: "test".into(),
            profile: "declared".into(),
            developer: 0xfff9_8000,
            resizer: 0xfffc_3000,
            source_phase: (2, 2),
            color: None,
            tone: None,
            white_balance_reference: None,
        });
        let (starts, assets) = (starts(), assets());
        let inputs = Inputs {
            starts: &starts,
            sub_cpu_registers: &[],
            assets: &assets,
            sensor_image: None,
        };
        let banks = ram_banks(&resolved.reference.soc.memory_regions);
        let nodes = nodes(&maps, &reset, 0, &banks, &inputs).unwrap();
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
        reset.still_development.as_mut().unwrap().color = Some(0xfff4_7000);
        let nodes = super::nodes(&maps, &reset, 0, &banks, &inputs).unwrap();
        let developer = nodes
            .iter()
            .find(|node| node["model"]["base"] == 0xfff9_8000u64)
            .unwrap();
        assert_eq!(developer["model"]["work"]["color"], 0xfff4_7000u64);
        reset.still_development.as_mut().unwrap().tone = Some(0xfffa_c000);
        let nodes = super::nodes(&maps, &reset, 0, &banks, &inputs).unwrap();
        let developer = nodes
            .iter()
            .find(|node| node["model"]["base"] == 0xfff9_8000u64)
            .unwrap();
        assert_eq!(developer["model"]["work"]["tone"], 0xfffa_c000u64);
        reset.still_development.as_mut().unwrap().color = None;
        assert_eq!(
            super::nodes(&maps, &reset, 0, &banks, &inputs)
                .err()
                .unwrap(),
            "a still tone unit needs the still color unit"
        );
        reset.still_development.as_mut().unwrap().color = Some(0xfff4_8000);
        reset.still_development.as_mut().unwrap().tone = None;
        assert!(super::nodes(&maps, &reset, 0, &banks, &inputs).is_err());
        reset.still_development.as_mut().unwrap().color = None;
        reset.still_development.as_mut().unwrap().resizer = 0xfff7_f004;
        assert_eq!(
            super::nodes(&maps, &reset, 0, &banks, &inputs)
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
        for bytes in [vec![], vec![0; 3], vec![0; 319]] {
            let wrong = BTreeMap::from([("xt20-211-statistics-window".to_string(), bytes)]);
            assert_eq!(
                error(&wrong),
                "statistics buffer asset xt20-211-statistics-window must contain complete words"
            );
        }
        let changed = BTreeMap::from([("xt20-211-statistics-window".to_string(), vec![0; 320])]);
        let assembly = xt20(&changed).unwrap();
        assert_eq!(
            node(&assembly.devices, "statistics")["buffers"][0][1],
            json!(vec![0u32; 80])
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
            sensor_image: None,
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
            sensor_image: None,
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
        let value = definition("xt20-211", &[], registers, &[], &assets(), None).unwrap();
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
        let error = definition("xt20-211", &[], json!([[]]), &[], &assets(), None).err();
        assert_eq!(
            error.unwrap(),
            "reset registers name 1 cores; SoC xpp has 2"
        );
    }
}
