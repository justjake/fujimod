//! X70 run-policy and EXR II device-model assembly.
//!
//! This module binds the checked X70 hardware and firmware identities to
//! the EXR II device implementations. Fixed model values are implementation
//! policy; physical addresses, links, and starting inputs are resolved below.

use fujimod_catalog::reference::{
    ConnectionSignal, ControlBusChip, LensProtocol, Part, PartModel, ResolvedSelection,
    SensorRadiometry, SocBlock, SocBlockKind,
};
use fujimod_catalog_configuration::{
    Exr2RunInputs, Exr2RunScene, RunBrowserCamera, RunConfiguration, RunPurpose, RunStart,
};
use fujimod_emulator_exr2_plan::{BootEntry, Model, Node, Plan, WireKind};
use fujimod_emulator_devices::{graph::Mapping as DeviceMapping, parts::pins::Pin};
use fujimod_catalog_configuration::DEFAULT_CLOCK_SECONDS_2000;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

fn model_recipe(name: &str) -> Result<Value, String> {
    let recipe = match name {
        "irq0" => serde_json::from_str::<Value>("{\"kind\":\"exr2-interrupt-controller\",\"options\":{\"arbitration\":\"active-bitmap\"},\"core\":0,\"base\":4294651904,\"routes\":\"imaging_routes\",\"software\":[]}").map_err(|e| e.to_string())?,
        "irq1" => serde_json::from_str::<Value>("{\"kind\":\"exr2-interrupt-controller\",\"options\":{\"arbitration\":\"active-bitmap\"},\"core\":1,\"base\":4294656000,\"routes\":\"imaging_routes\",\"software\":[{\"line\":120,\"pulse\":\"release\"}]}").map_err(|e| e.to_string())?,
        "timers" => serde_json::from_str::<Value>("{\"kind\":\"exr2-system-timers\",\"options\":{\"main_targets\":[0,1]},\"controllers\":[\"irq0\",\"irq1\"]}").map_err(|e| e.to_string())?,
        "release" => serde_json::from_str::<Value>("{\"kind\":\"core-release\",\"options\":{\"released\":false},\"core\":1,\"request\":\"release\"}").map_err(|e| e.to_string())?,
        "serial_gpio" => serde_json::from_str::<Value>("{\"kind\":\"gpio\",\"options\":{\"input_levels\":[3118,0,4608,777,0,0,0,0,0,0,0,0,128,0,0,0],\"output_levels\":[{\"pin\":{\"port\":11,\"pin\":3},\"high\":true},{\"pin\":{\"port\":12,\"pin\":3},\"high\":true},{\"pin\":{\"port\":13,\"pin\":9},\"high\":true}]},\"irqs\":[{\"pin\":{\"port\":0,\"pin\":0},\"edge\":\"both\",\"outputs\":[{\"Core\":{\"core\":0,\"line\":8}}]},{\"pin\":{\"port\":0,\"pin\":2},\"edge\":\"falling\",\"outputs\":[{\"Core\":{\"core\":0,\"line\":10}}]},{\"pin\":{\"port\":0,\"pin\":3},\"edge\":\"falling\",\"outputs\":[{\"Core\":{\"core\":0,\"line\":11}}]}]}").map_err(|e| e.to_string())?,
        "serial_spi1" => serde_json::from_str::<Value>("{\"kind\":\"spi-controller\",\"routes\":[{\"options\":{\"base\":4294459456,\"transports\":[{\"control\":224,\"configuration\":196612,\"bits\":16,\"receive\":true},{\"control\":65760,\"configuration\":196612,\"bits\":16,\"receive\":false}],\"selection\":[{\"pin\":{\"port\":12,\"pin\":3},\"high\":false},{\"pin\":{\"port\":11,\"pin\":3},\"high\":true}],\"transfer_ticks\":4,\"queue\":\"single\"},\"gpio\":\"serial_gpio\",\"endpoint\":{\"kind\":\"registers\",\"options\":{\"registers\":[[0,0],[1,0],[3,0],[4,0],[5,0],[6,0],[7,0],[8,0],[9,0],[10,0],[11,0],[12,0],[13,0],[14,0],[15,0],[16,0],[18,0],[19,0],[23,0],[24,0],[25,0],[26,0],[27,0],[28,0],[29,0],[30,0],[31,0],[32,0],[33,0],[34,0],[35,0],[36,0],[37,0],[38,0],[39,0],[40,0],[41,0],[42,0],[43,0],[44,0],[45,0],[46,0],[47,0]],\"readable\":false,\"status_registers\":[],\"clock\":null}}},{\"options\":{\"base\":4294459456,\"transports\":[{\"control\":224,\"configuration\":6,\"bits\":16,\"receive\":true},{\"control\":65760,\"configuration\":6,\"bits\":16,\"receive\":false}],\"selection\":[{\"pin\":{\"port\":12,\"pin\":3},\"high\":false},{\"pin\":{\"port\":11,\"pin\":3},\"high\":true}],\"transfer_ticks\":4,\"queue\":\"single\"},\"gpio\":\"serial_gpio\",\"endpoint\":{\"kind\":\"registers\",\"options\":{\"registers\":[[65,0],[66,0],[67,0],[68,0],[69,0],[70,0],[73,0],[74,0],[75,0],[76,0],[78,0],[80,0],[81,0],[82,0],[83,0],[84,0],[85,0],[86,0],[87,0],[88,0],[89,0],[90,0],[91,0],[92,0],[93,0],[94,170],[95,86],[96,0],[104,0],[105,0],[106,0]],\"readable\":true,\"status_registers\":[96],\"clock\":{\"registers\":[65,66,67,68],\"ticks_per_second\":12000000,\"maximum_seconds\":1609459199,\"started\":0}}}},{\"options\":{\"base\":4294459456,\"transports\":[{\"control\":224,\"configuration\":6,\"bits\":16,\"receive\":false}],\"selection\":[{\"pin\":{\"port\":11,\"pin\":3},\"high\":false},{\"pin\":{\"port\":12,\"pin\":3},\"high\":true}],\"transfer_ticks\":4,\"queue\":\"single\"},\"gpio\":\"serial_gpio\",\"endpoint\":{\"kind\":\"panel\",\"options\":{\"used_mask\":65408}}}]}").map_err(|e| e.to_string())?,
        "dram_controller" => serde_json::from_str::<Value>("{\"kind\":\"dram\",\"options\":{}}").map_err(|e| e.to_string())?,
        "dram_phy" => serde_json::from_str::<Value>("{\"kind\":\"dram-phy\",\"options\":{}}").map_err(|e| e.to_string())?,
        "reset_control" => serde_json::from_str::<Value>("{\"kind\":\"reset\",\"options\":{}}").map_err(|e| e.to_string())?,
        "clocks" => serde_json::from_str::<Value>("{\"kind\":\"clocks\",\"options\":{\"output_settle_ticks\":2,\"reset_ready\":true}}").map_err(|e| e.to_string())?,
        "block98000" => serde_json::from_str::<Value>("{\"kind\":\"block-98000\",\"options\":{\"boot_source\":1,\"selection_reset\":0},\"wiring\":{\"decoder\":0,\"selections\":[{\"word\":0,\"choice\":0},{\"word\":1,\"choice\":1},{\"word\":2,\"choice\":1}]}}").map_err(|e| e.to_string())?,
        "image_scaler" => serde_json::from_str::<Value>("{\"kind\":\"scaler\",\"options\":{\"completion_ticks\":2},\"irq\":{\"core\":0,\"line\":0}}").map_err(|e| e.to_string())?,
        "raw_conversion" => serde_json::from_str::<Value>("{\"kind\":\"raw-conversion\",\"options\":{}}").map_err(|e| e.to_string())?,
        "prefilter" => serde_json::from_str::<Value>("{\"kind\":\"prefilter\",\"options\":{\"completion_ticks\":1000,\"reset_ticks\":16},\"irq\":{\"core\":0,\"line\":0}}").map_err(|e| e.to_string())?,
        "raw_statistics" => serde_json::from_str::<Value>("{\"kind\":\"raw-statistics\",\"options\":{\"completion_ticks\":1000,\"reset_ticks\":16},\"irq\":{\"core\":0,\"line\":0}}").map_err(|e| e.to_string())?,
        "still_processor" => serde_json::from_str::<Value>("{\"kind\":\"still-processor\",\"options\":{\"completion_ticks\":1000,\"cfa\":[]},\"irq\":{\"core\":0,\"line\":0}}").map_err(|e| e.to_string())?,
        "warp" => serde_json::from_str::<Value>("{\"kind\":\"warp\",\"options\":{\"completion_ticks\":1000,\"stop_ticks\":16},\"irq\":{\"core\":0,\"line\":0},\"stream\":\"still_stream\"}").map_err(|e| e.to_string())?,
        "jpeg_encoder" => serde_json::from_str::<Value>("{\"kind\":\"jpeg-encoder\",\"options\":{\"completion_ticks\":1000,\"reset_ticks\":2},\"irq\":{\"core\":0,\"line\":0},\"stream\":\"still_stream\"}").map_err(|e| e.to_string())?,
        "jpeg_decoder" => serde_json::from_str::<Value>("{\"kind\":\"jpeg-decoder\",\"options\":{\"completion_ticks\":1000,\"reset_ticks\":2},\"irq\":{\"core\":0,\"line\":0}}").map_err(|e| e.to_string())?,
        "sd_clocks" => serde_json::from_str::<Value>("{\"kind\":\"sdhci-clocks\",\"words\":[[4294246444,2]]}").map_err(|e| e.to_string())?,
        "adc" => serde_json::from_str::<Value>("{\"kind\":\"adc\",\"options\":{\"conversion_ticks\":4,\"samples\":[0,0,1023,63,534,640,0,825,1023,60,0,0]}}").map_err(|e| e.to_string())?,
        "statistics" => serde_json::from_str::<Value>("{\"kind\":\"statistics\",\"options\":{\"clock\":{\"kind\":\"fixed\",\"ticks\":400000},\"ack_ticks\":2,\"stop_ticks\":2,\"histogram\":true,\"external_statistics\":false},\"routes\":\"imaging_routes\",\"frames\":\"live_statistics\",\"images\":\"live_image\"}").map_err(|e| e.to_string())?,
        "image_output_0" => serde_json::from_str::<Value>("{\"kind\":\"output-channel\",\"options\":{\"stop_ticks\":2,\"clock\":{\"kind\":\"fixed\",\"ticks\":400000},\"geometry\":\"skip_incomplete\"},\"base\":4293992448,\"irq\":44,\"channel\":0,\"channels\":\"image_outputs\",\"routes\":\"imaging_routes\",\"frames\":\"live_image\",\"memory_outputs\":\"memory_outputs\"}").map_err(|e| e.to_string())?,
        "image_output_1" => serde_json::from_str::<Value>("{\"kind\":\"output-channel\",\"options\":{\"stop_ticks\":2,\"clock\":{\"kind\":\"fixed\",\"ticks\":400000},\"geometry\":\"skip_incomplete\"},\"base\":4293996544,\"irq\":45,\"channel\":1,\"channels\":\"image_outputs\",\"routes\":\"imaging_routes\",\"frames\":\"live_image\",\"memory_outputs\":\"memory_outputs\"}").map_err(|e| e.to_string())?,
        "face_detector" => serde_json::from_str::<Value>("{\"kind\":\"face-detector\",\"options\":{\"mode\":\"retained_words\"}}").map_err(|e| e.to_string())?,
        // One PDAF extraction block completes in 1,800 ticks (0.15 ms at 12 MHz),
        // the median block interval of a stock X70 AF-S performance trace. It is
        // independent of the sensor frame period.
        "phase_detection" => serde_json::from_str::<Value>("{\"kind\":\"phase-detection\",\"options\":{\"arithmetic\":\"aperture_sad\",\"data_ticks\":1800,\"stop_ticks\":2},\"routes\":\"imaging_routes\"}").map_err(|e| e.to_string())?,
        "live_frames" => serde_json::from_str::<Value>("{\"kind\":\"external-frames\",\"activity\":{\"gpio\":\"serial_gpio\",\"selection\":{\"pin\":{\"port\":0,\"pin\":2},\"high\":false}},\"options\":{\"period_ticks\":400000},\"images\":\"live_image\",\"statistics\":\"live_statistics\"}").map_err(|e| e.to_string())?,
        "sdio" => serde_json::from_str::<Value>("{\"kind\":\"sdio-host\",\"options\":{\"transfer_ticks\":100,\"clock_ticks\":2,\"rca\":4660,\"ram_bytes\":536870912},\"module\":{\"protocol\":\"mt5931\",\"firmware_bytes\":136640,\"mt5931\":{\"mac_address\":[2,0,0,0,0,1]}},\"irq\":{\"Core\":{\"core\":0,\"line\":79}}}").map_err(|e| e.to_string())?,
        "nand" => serde_json::from_str::<Value>("{\"kind\":\"nand\",\"options\":{\"erase_ticks\":64},\"medium\":\"nand\"}").map_err(|e| e.to_string())?,
        "lz" => serde_json::from_str::<Value>("{\"kind\":\"lz\",\"options\":{\"resize\":\"declared_packed_pyramid\",\"resize_ticks\":400000},\"base\":4294193152,\"controllers\":[\"irq0\",\"irq1\"]}").map_err(|e| e.to_string())?,
        "memory_dma" => serde_json::from_str::<Value>("{\"kind\":\"memory-dma\",\"options\":{},\"base\":4293926912,\"routes\":\"imaging_routes\"}").map_err(|e| e.to_string())?,
        "usb" => serde_json::from_str::<Value>("{\"kind\":\"usb\",\"options\":{\"endpoints\":null},\"cable\":\"usb_cable\",\"endpoints\":null}").map_err(|e| e.to_string())?,
        "power_peripherals" => serde_json::from_str::<Value>("{\"kind\":\"power\",\"options\":{\"transfer_ticks\":4,\"max_program_bytes\":536870912,\"spi0\":{\"required\":[192,48,0],\"selection\":{\"pin\":{\"port\":11,\"pin\":2},\"high\":false},\"presence\":{\"pin\":{\"port\":11,\"pin\":2},\"high\":false}}},\"gpio\":\"serial_gpio\"}").map_err(|e| e.to_string())?,
        // Channel 1 transmit DMA with both serial-audio banks: a beep's
        // samples are consumed and IRQ 5C reports its completion, which the
        // stock POWOFF sequence waits for before saving the user profile.
        "audio_registers" => serde_json::from_str::<Value>("{\"kind\":\"serial-audio-transmitter\",\"options\":{\"ram_bytes\":536870912,\"max_transfer_bytes\":1048576,\"ticks_per_byte\":64,\"minimum_ticks\":1000},\"sinks\":[{\"connected\":true},{\"connected\":true}],\"routes\":\"imaging_routes\"}").map_err(|e| e.to_string())?,
        "pulse" => serde_json::from_str::<Value>("{\"kind\":\"exr2-pulse\",\"options\":{\"counter_channels\":[0,4],\"timer_channels\":[0,2,4,6,8,9,10,11],\"quadrature\":false,\"initial_phase\":0,\"capture_channel\":null},\"irqs\":[{\"Core\":{\"core\":0,\"line\":20}},{\"Core\":{\"core\":0,\"line\":21}},{\"Core\":{\"core\":0,\"line\":22}},{\"Core\":{\"core\":0,\"line\":23}},{\"Core\":{\"core\":0,\"line\":24}},{\"Core\":{\"core\":0,\"line\":25}},{\"Core\":{\"core\":0,\"line\":26}},{\"Core\":{\"core\":0,\"line\":27}},{\"Core\":{\"core\":0,\"line\":28}},{\"Core\":{\"core\":0,\"line\":29}},{\"Core\":{\"core\":0,\"line\":30}},{\"Core\":{\"core\":0,\"line\":31}}]}").map_err(|e| e.to_string())?,
        "lens_serial" => serde_json::from_str::<Value>("{\"kind\":\"spi-controller\",\"routes\":[{\"options\":{\"base\":4294459520,\"transports\":[{\"control\":224,\"configuration\":4,\"bits\":16,\"receive\":false}],\"selection\":[{\"pin\":{\"port\":13,\"pin\":9},\"high\":false}],\"transfer_ticks\":4,\"queue\":\"single\"},\"gpio\":\"serial_gpio\",\"endpoint\":{\"kind\":\"lens-unit\",\"options\":{\"protocol\":{\"kind\":\"banked\",\"positive_direction_high\":false,\"stop_response\":\"settled\",\"clock\":[{\"kind\":\"microstep_stages\",\"ticks_per_quantum\":3},{\"kind\":\"encoder_stages\",\"ticks_per_quantum\":3}]},\"motors\":[{\"initial_position\":0,\"travel_limits\":[-32767,32767],\"ticks_per_step\":1000,\"settle_ticks\":4000,\"home\":{\"pin\":{\"port\":0,\"pin\":0},\"threshold\":8,\"side\":\"above\"},\"counters\":[{\"channel\":4,\"divider\":1},{\"channel\":0,\"divider\":4}]},{\"initial_position\":0,\"travel_limits\":[-32767,32767],\"ticks_per_step\":1000,\"settle_ticks\":4000,\"home\":null,\"counters\":[]}],\"encoder\":null,\"zoom\":null},\"pulses\":\"pulse\",\"gpio\":\"serial_gpio\",\"completion\":[{\"Core\":{\"core\":0,\"line\":14}},{\"Core\":{\"core\":0,\"line\":12}}],\"optics\":{\"output\":\"lens_aperture\",\"positions\":[[6,16.000000000000004],[10,14.25437949024543],[14,12.699208415745595],[18,11.313708498984761],[22,10.079368399158986],[26,8.979696386474986],[30,8.000000000000002],[34,7.127189745122715],[38,6.349604207872798],[42,5.656854249492381],[46,5.039684199579493],[50,4.489848193237492],[54,4.000000000000001],[58,3.5635948725613575],[62,3.174802103936399],[78,2.8284271247461903]]},\"focus\":{\"output\":\"lens_focus\",\"infinity_position\":-111,\"steps_per_mm\":267.0}}}]}").map_err(|e| e.to_string())?,
        "control_bus0" => serde_json::from_str::<Value>("{\"kind\":\"i2c\",\"options\":{\"base\":4294463488,\"irq\":101,\"modes\":[9],\"transfer_ticks\":4,\"profile\":\"endpoint7-stationary-and9-heartbeat-latched-irq-unmeasured\",\"parts\":{\"kind\":\"primary\",\"touch\":{\"register\":0,\"core\":0,\"irq\":15,\"ticks_per_second\":12000000},\"sensor_address\":28,\"prefix_address\":85,\"sensor_fields\":[14,15,17,21,23,24,29,31,32,33,35,36,37,38,39,40,41,42,43,44,45,46,47,48,49],\"prefix_high\":28,\"heartbeat_register\":7174,\"topology\":[24,68,57,25,12,28,85],\"stationary\":false,\"status\":[],\"commands\":[]}},\"controllers\":[\"irq0\",\"irq1\"],\"cable\":null,\"bridge\":null}").map_err(|e| e.to_string())?,
        "control_bus1" => serde_json::from_str::<Value>("{\"kind\":\"i2c\",\"options\":{\"base\":4294463552,\"irq\":102,\"modes\":[1,9],\"transfer_ticks\":4,\"profile\":\"usbcharge-disconnected-pwic-startup-tagged\",\"parts\":{\"kind\":\"secondary\",\"register_address\":10,\"pwic_address\":48,\"register_modes\":[9],\"pwic_modes\":[1],\"register_values\":[[2,0],[5,0],[12,0],[17,0],[19,0]],\"register_masks\":[[12,255],[17,255]],\"pwic_values\":[[97,0],[32,128]],\"pwic_masks\":[[97,16],[32,2]],\"pwic_read_prefixes\":[[32,[0,0,0]]],\"topology\":[48,114,10],\"connected\":false}},\"controllers\":[\"irq0\",\"irq1\"],\"cable\":\"usb_cable\",\"bridge\":null}").map_err(|e| e.to_string())?,
        "companion" => serde_json::from_str::<Value>("{\"kind\":\"fe80-processing\",\"transport\":{},\"options\":{\"irq\":88,\"pixels_per_tick\":64},\"frames\":\"live_image\",\"routes\":\"imaging_routes\"}").map_err(|e| e.to_string())?,
        // A RAW channel transfer completes 153,600 ticks (12.8 ms at 12 MHz) after
        // it starts, the post-wait interval of a stock X70 AF-S performance trace.
        "image_input" => serde_json::from_str::<Value>("{\"kind\":\"image-input\",\"options\":{\"reset_ticks\":2,\"command_ticks\":2,\"phase_ticks\":2,\"command\":\"packed_writes\",\"command_controls\":[0],\"phase_ack\":\"inactive\",\"clock\":{\"mode\":\"input_enable\",\"source\":{\"kind\":\"periodic\",\"period_ticks\":400000,\"first_delay_ticks\":null},\"secondary\":true,\"companion_secondary\":true},\"sensor_period_ticks\":null,\"raw_dma_ticks\":153600,\"readback_ticks\":750000,\"cfa\":[0,2,1,2,0,1,1,1,0,1,1,2,1,1,2,1,1,0,2,0,1,0,2,1,1,1,2,1,1,0,1,1,0,1,1,2]},\"phase_irqs\":[{\"core\":0,\"line\":39},{\"core\":0,\"line\":41}],\"raw_irq\":{\"core\":0,\"line\":36},\"readback_irq\":{\"core\":0,\"line\":38},\"shutter_output\":null,\"sensor\":null,\"parameters\":\"parameters\",\"memory_outputs\":\"memory_outputs\",\"statistics_grid\":null,\"frame_period\":null,\"edges\":\"phase_edges\"}").map_err(|e| e.to_string())?,
        "input_auxiliary" => serde_json::from_str::<Value>("{\"kind\":\"block11000\",\"options\":{\"control_mode\":{\"ClearBit0Ack\":{\"delay_ticks\":2}}}}").map_err(|e| e.to_string())?,
        "parameters" => serde_json::from_str::<Value>("{\"kind\":\"parameters\",\"options\":{},\"bank\":\"parameters\"}").map_err(|e| e.to_string())?,
        "hdmisink" => serde_json::from_str::<Value>("{\"kind\":\"hdmi-sink\",\"options\":{\"connected\":false,\"edid\":[]}}").map_err(|e| e.to_string())?,
        "hdmi" => serde_json::from_str::<Value>("{\"kind\":\"hdmi\",\"options\":{},\"sink\":\"hdmisink\",\"hot_plug\":{\"Core\":{\"core\":0,\"line\":0}}}").map_err(|e| e.to_string())?,
        "output" => serde_json::from_str::<Value>("{\"kind\":\"block86000\",\"options\":{}}").map_err(|e| e.to_string())?,
        "graphics" => serde_json::from_str::<Value>("{\"kind\":\"vector-gpu\",\"options\":{\"ram_bytes\":536870912,\"max_fill_bytes\":67108864,\"max_copy_bytes\":67108864,\"max_clear_bytes\":67108864,\"bytes_per_tick\":16,\"max_command_capture_bytes\":524288,\"copy_profile\":\"vg-32bpp-untile-unrotated\",\"clear_profile\":\"declared-vg-ui-primitives-unmeasured\"},\"controllers\":[\"irq0\",\"irq1\"]}").map_err(|e| e.to_string())?,
        "scanout" => serde_json::from_str::<Value>("{\"kind\":\"scanout\",\"options\":{\"ram_bytes\":536870912,\"packing\":\"yycbcr\",\"matrix\":\"bt601-full\",\"graphics_profile\":\"mode1-abgr8888-straight\",\"backdrop_profile\":\"declared-black-canvas-unmeasured\",\"rectangle_profile\":\"exr2-signed-ycbcr-rectangle-overlay-unmeasured\",\"max_pixels\":8847360,\"reset_ticks\":2}}").map_err(|e| e.to_string())?,
        "display_timing" => serde_json::from_str::<Value>("{\"kind\":\"display-timing\",\"options\":{\"mode\":\"configured_clocked\",\"frame_ticks\":400000,\"stop_ticks\":2},\"clock\":\"clocks\",\"controllers\":[\"irq0\",\"irq1\"]}").map_err(|e| e.to_string())?,
        "display_formatter" => serde_json::from_str::<Value>("{\"kind\":\"formatter\",\"options\":{\"latch_delay\":2}}").map_err(|e| e.to_string())?,
        "image_transfer" => json!({"kind":"image-transfer", "irq":{"core":0,"line":73}}),
        "avc_codec" => json!({"kind":"avc-codec", "options":{"backend":"software"}, "irq":{"core":0,"line":75}}),
        "image_combination" => serde_json::from_str::<Value>("{\"kind\":\"image-combination\",\"options\":{\"reset_ticks\":16,\"arithmetic\":\"packed_image_unmeasured\",\"clock\":{\"kind\":\"throughput\",\"bytes_per_tick\":256}},\"irq\":{\"core\":0,\"line\":68}}").map_err(|e| e.to_string())?,
        "encoder_control_ring" => serde_json::from_str::<Value>("{\"kind\":\"encoder\",\"options\":{\"pins\":[{\"port\":3,\"pin\":6},{\"port\":3,\"pin\":7}],\"phase_ticks\":120000,\"rest_phase\":0},\"gpio\":\"serial_gpio\",\"counter\":null}").map_err(|e| e.to_string())?,
        _ => return Err(format!("unknown X70 implementation node {name}")),
    };
    Ok(recipe)
}

#[derive(Clone, Copy)]
struct NodeRule {
    name: &'static str,
    blocks: &'static [&'static str],
    dependencies: &'static [&'static str],
}

const NODE_RULES: &[NodeRule] = &[
    NodeRule {
        name: "irq0",
        blocks: &["interrupt-core0"],
        dependencies: &[],
    },
    NodeRule {
        name: "irq1",
        blocks: &["interrupt-core1"],
        dependencies: &[],
    },
    NodeRule {
        name: "timers",
        blocks: &["system-timers"],
        dependencies: &[],
    },
    NodeRule {
        name: "release",
        blocks: &[],
        dependencies: &[],
    },
    NodeRule {
        name: "serial_gpio",
        blocks: &["gpio", "gpio-aux"],
        dependencies: &[],
    },
    NodeRule {
        name: "serial_spi1",
        blocks: &["serial-spi1"],
        dependencies: &["serial_gpio"],
    },
    NodeRule {
        name: "dram_controller",
        blocks: &["dram-controller"],
        dependencies: &[],
    },
    NodeRule {
        name: "dram_phy",
        blocks: &["dram-phy"],
        dependencies: &[],
    },
    NodeRule {
        name: "reset_control",
        blocks: &["reset-controller"],
        dependencies: &[],
    },
    NodeRule {
        name: "clocks",
        blocks: &["clock-controller", "clock-controller-aux"],
        dependencies: &[],
    },
    NodeRule {
        name: "block98000",
        blocks: &["block-98000"],
        dependencies: &[],
    },
    NodeRule {
        name: "image_scaler",
        blocks: &["image-scaler"],
        dependencies: &["irq0", "irq1"],
    },
    NodeRule {
        name: "raw_conversion",
        blocks: &["raw-conversion"],
        dependencies: &[],
    },
    NodeRule {
        name: "prefilter",
        blocks: &["prefilter"],
        dependencies: &[],
    },
    NodeRule {
        name: "raw_statistics",
        blocks: &["raw-statistics"],
        dependencies: &[],
    },
    NodeRule {
        name: "still_processor",
        blocks: &[
            "still-processor-0",
            "still-processor-1",
            "still-processor-2",
            "still-processor-3",
            "still-processor-4",
            "still-processor-5",
            "still-processor-6",
            "still-processor-7",
        ],
        dependencies: &[],
    },
    NodeRule {
        name: "warp",
        blocks: &["warp"],
        dependencies: &[],
    },
    NodeRule {
        name: "jpeg_encoder",
        blocks: &["jpeg-encoder"],
        dependencies: &[],
    },
    NodeRule {
        name: "jpeg_decoder",
        blocks: &["jpeg-decoder"],
        dependencies: &[],
    },
    NodeRule {
        name: "sd_clocks",
        blocks: &["sd-clocks-0"],
        dependencies: &[],
    },
    NodeRule {
        name: "adc",
        blocks: &["adc"],
        dependencies: &[],
    },
    NodeRule {
        name: "statistics",
        blocks: &["statistics", "statistics-aux"],
        dependencies: &["live_frames"],
    },
    NodeRule {
        name: "image_output_0",
        blocks: &["image-output-0"],
        dependencies: &["live_frames"],
    },
    NodeRule {
        name: "image_output_1",
        blocks: &["image-output-1"],
        dependencies: &["live_frames"],
    },
    NodeRule {
        name: "face_detector",
        blocks: &["face-detector-0"],
        dependencies: &[],
    },
    NodeRule {
        name: "phase_detection",
        blocks: &["phase-detection", "phase-detection-aux"],
        dependencies: &[],
    },
    NodeRule {
        name: "live_frames",
        blocks: &[],
        dependencies: &["serial_gpio"],
    },
    NodeRule {
        name: "sdio",
        blocks: &["sdio-host", "sdio-1"],
        dependencies: &[],
    },
    NodeRule {
        name: "nand",
        blocks: &["nand"],
        dependencies: &[],
    },
    NodeRule {
        name: "lz",
        blocks: &["decompressor"],
        dependencies: &["irq0", "irq1"],
    },
    NodeRule {
        name: "memory_dma",
        blocks: &["memory-dma", "memory-dma-1"],
        dependencies: &["irq0", "irq1"],
    },
    NodeRule {
        name: "usb",
        blocks: &["usb-core", "usb-phy"],
        dependencies: &[],
    },
    NodeRule {
        name: "power_peripherals",
        blocks: &[
            "power-peripherals",
            "power-registers-1",
            "power-registers-2",
            "power-registers-3",
        ],
        dependencies: &["serial_gpio"],
    },
    NodeRule {
        name: "audio_registers",
        blocks: &["audio-dma", "serial-audio", "serial-audio-aux"],
        dependencies: &["irq0", "irq1"],
    },
    NodeRule {
        name: "pulse",
        blocks: &["pulse-outputs"],
        dependencies: &[],
    },
    NodeRule {
        name: "lens_serial",
        blocks: &["serial-spi2"],
        dependencies: &["pulse", "serial_gpio"],
    },
    NodeRule {
        name: "control_bus0",
        blocks: &["control-bus0"],
        dependencies: &["irq0", "irq1"],
    },
    NodeRule {
        name: "control_bus1",
        blocks: &["control-bus1"],
        dependencies: &["irq0", "irq1"],
    },
    NodeRule {
        name: "companion",
        blocks: &[
            "peripheral-window-fe318000",
            "peripheral-window-fe3b0000",
            "companion-fe80c",
            "companion-fe80d",
        ],
        dependencies: &["image_output_0", "image_output_1", "live_frames"],
    },
    NodeRule {
        name: "image_input",
        blocks: &["image-input"],
        dependencies: &["parameters"],
    },
    NodeRule {
        name: "input_auxiliary",
        blocks: &["input-auxiliary-control"],
        dependencies: &[],
    },
    NodeRule {
        name: "parameters",
        blocks: &["image-parameters"],
        dependencies: &["image_input"],
    },
    NodeRule {
        name: "hdmisink",
        blocks: &[],
        dependencies: &[],
    },
    NodeRule {
        name: "hdmi",
        blocks: &["hdmi-0", "hdmi-1", "hdmi-2"],
        dependencies: &["hdmisink"],
    },
    NodeRule {
        name: "output",
        blocks: &["output-control"],
        dependencies: &[],
    },
    NodeRule {
        name: "graphics",
        blocks: &["graphics"],
        dependencies: &["irq0", "irq1"],
    },
    NodeRule {
        name: "scanout",
        blocks: &["scanout", "scanout-aux"],
        dependencies: &[],
    },
    NodeRule {
        name: "display_timing",
        blocks: &["display-timing", "display-timing-aux"],
        dependencies: &[],
    },
    NodeRule {
        name: "display_formatter",
        blocks: &["display-formatter", "display-formatter-aux"],
        dependencies: &[],
    },
    NodeRule {
        name: "image_transfer",
        blocks: &["image-transfer"],
        dependencies: &["irq0", "irq1"],
    },
    NodeRule {
        name: "avc_codec",
        blocks: &["avc-codec-input", "avc-codec-control"],
        dependencies: &["irq0", "irq1"],
    },
    NodeRule {
        name: "image_combination",
        blocks: &["image-combination"],
        dependencies: &["irq0", "irq1"],
    },
    NodeRule {
        name: "encoder_control_ring",
        blocks: &[],
        dependencies: &["serial_gpio"],
    },
];

/// The SDHC card host: present only when a browser camera declares its virtual card.
const CARD_NODE_RULE: NodeRule = NodeRule {
    name: "sd",
    // The host window covers 0xfff5002c (SDHCI clock) and 0xfff5002e (reset/timeout),
    // which the host model owns; no standalone sd_clocks node exists alongside it.
    blocks: &["sdhci-card-0", "sdhci-card-2", "sdhci-card-1"],
    dependencies: &["serial_gpio"],
};

/// The one GPIO signal of a card contact, with its active level.
fn card_contact(selection: &ResolvedSelection, connection_id: &str) -> Result<(Pin, bool), String> {
    match gpio_signals(selection, connection_id)?.as_slice() {
        [(pin, active)] => Ok((*pin, *active)),
        _ => Err(format!(
            "X70 card contact {connection_id} must declare exactly one GPIO signal"
        )),
    }
}

/// The SDHC host with its virtual card: formatted to the declared geometry and
/// inserted through the body's detect and write-protect contacts.
fn card_recipe(selection: &ResolvedSelection, browser: &RunBrowserCamera) -> Result<Value, String> {
    let slot = selection
        .body
        .card_slot
        .as_ref()
        .ok_or("X70 body declares no card slot")?;
    let (detect, present_high) = card_contact(selection, &slot.detect_connection_id)?;
    let (protect, protected_high) = card_contact(selection, &slot.write_protect_connection_id)?;
    let card = &browser.virtual_card;
    // Legacy budget for a card without preloaded files: 65536 + 16384 dirty sectors.
    let max_dirty_sectors: u32 = 65536 + 16384;
    Ok(json!({
        "kind": "sd-host",
        "options": {
            "base": 0xfff50000u32,
            "protocol": "sdhc",
            "transfer_ticks": 100,
            "clock_ticks": 2,
            "rca": 4660,
            "ram_bytes": selection.body.dram_bytes,
        },
        "card": {
            "kind": "formatted",
            "options": {
                "sectors": card.sectors,
                "max_dirty_sectors": max_dirty_sectors,
                "label": card.label,
                "partition_start": card.partition_start,
                "sectors_per_cluster": card.sectors_per_cluster,
            },
            "files": "virtual_card_files",
        },
        "pins": {
            "gpio": "serial_gpio",
            "detect": {"port": detect.port, "pin": detect.pin},
            "present_high": present_high,
            "protect": {"port": protect.port, "pin": protect.pin},
            "protected_high": protected_high,
        },
        "present": true,
        "protected": false,
    }))
}

pub(super) fn node_for_block(block_id: &str) -> Result<String, String> {
    NODE_RULES
        .iter()
        .find(|rule| rule.blocks.contains(&block_id))
        .map(|rule| rule.name.to_owned())
        .ok_or_else(|| format!("X70 implementation has no node for SoC block {block_id}"))
}

fn implementation_wires() -> BTreeMap<String, WireKind> {
    BTreeMap::from([
        ("release".into(), WireKind::SoftwarePulse),
        ("lens_aperture".into(), WireKind::Aperture),
        ("lens_focus".into(), WireKind::Focus),
        ("usb_cable".into(), WireKind::LogicLevel(false)),
        ("imaging_routes".into(), WireKind::IrqRoutes),
        ("image_outputs".into(), WireKind::OutputChannels),
        ("memory_outputs".into(), WireKind::MemoryOutputs),
        ("parameters".into(), WireKind::Parameters),
        ("phase_edges".into(), WireKind::PhaseEdges),
        ("still_stream".into(), WireKind::Stream),
    ])
}

fn replace_bases(value: &mut Value, base: u64) {
    match value {
        Value::Object(object) => {
            if object.get("base").is_some_and(Value::is_number) {
                object.insert("base".into(), json!(base));
            }
            for child in object.values_mut() {
                replace_bases(child, base);
            }
        }
        Value::Array(items) => {
            for child in items {
                replace_bases(child, base);
            }
        }
        _ => (),
    }
}

fn apply_soc_blocks(
    recipe: &mut Value,
    rule: &NodeRule,
    selection: &ResolvedSelection,
) -> Result<(), String> {
    let Some(first) = rule.blocks.first() else {
        return Ok(());
    };
    let base = block(selection, first)?.base;
    replace_bases(recipe, base);
    if rule.name == "sd_clocks" {
        let words = recipe["words"]
            .as_array_mut()
            .ok_or("X70 SD clock model has no register words")?;
        if let Some(word) = words.first_mut().and_then(Value::as_array_mut) {
            word[0] = json!(base);
        } else {
            return Err("X70 SD clock model has no base register".into());
        }
    }
    Ok(())
}

fn block<'a>(selection: &'a ResolvedSelection, id: &str) -> Result<&'a SocBlock, String> {
    selection
        .soc
        .blocks
        .iter()
        .find(|item| item.id == id)
        .ok_or_else(|| format!("SoC {} has no block {id}", selection.soc.id))
}

fn irq(
    selection: &ResolvedSelection,
    source_block: &str,
    output_id: &str,
) -> Result<Value, String> {
    let mut routes = selection.soc.interrupts.iter().filter(|route| {
        route.source_block == source_block && route.output_id.as_deref() == Some(output_id)
    });
    let route = routes.next().ok_or_else(|| {
        format!(
            "SoC {} has no {source_block} interrupt output {output_id}",
            selection.soc.id
        )
    })?;
    if routes.next().is_some() {
        return Err(format!(
            "SoC {} has ambiguous {source_block} interrupt output {output_id}",
            selection.soc.id
        ));
    }
    let core = selection
        .soc
        .processors
        .iter()
        .position(|processor| processor.id == route.core)
        .ok_or_else(|| {
            format!(
                "interrupt output {output_id} targets absent core {}",
                route.core
            )
        })?;
    Ok(json!({"Core":{"core":core,"line":route.line}}))
}

fn apply_interrupts(
    recipe: &mut Value,
    name: &str,
    selection: &ResolvedSelection,
) -> Result<(), String> {
    match name {
        "irq0" => {
            recipe["core"] = json!(interrupt_controller_core(selection, "interrupt-core0")?);
        }
        "irq1" => {
            recipe["core"] = json!(interrupt_controller_core(selection, "interrupt-core1")?);
            let release = selection
                .soc
                .reset
                .release
                .as_ref()
                .ok_or("SoC has no secondary-core release facts")?;
            let route = irq(selection, "interrupt-core1", "software-release")?;
            if route["Core"]["line"] != release.source {
                return Err("SoC secondary-core release source differs from its IRQ route".into());
            }
            recipe["software"] = json!([{
                "line":route["Core"]["line"],
                "pulse":"release"
            }]);
        }
        "serial_gpio" => {
            let home = selection
                .body
                .lens
                .as_ref()
                .and_then(|lens| lens.home_connections.iter().find(|home| home.motor == 0))
                .ok_or("X70 lens motor 0 has no declared home connection")?;
            let pin = |connection_id: &str| -> Result<Value, String> {
                let signals = gpio_signals(selection, connection_id)?;
                if signals.len() != 1 {
                    return Err(format!(
                        "GPIO interrupt connection {connection_id} must resolve to one pin"
                    ));
                }
                Ok(json!({"port":signals[0].0.port,"pin":signals[0].0.pin}))
            };
            recipe["irqs"] = json!([
                {
                    "pin":pin(&home.connection_id)?,
                    "edge":"both",
                    "outputs":[irq(selection,"gpio","port-0-pin-0")?]
                },
                {
                    "pin":pin("s1")?,
                    "edge":"falling",
                    "outputs":[irq(selection,"gpio","port-0-pin-2")?]
                },
                {
                    "pin":pin("s2")?,
                    "edge":"falling",
                    "outputs":[irq(selection,"gpio","port-0-pin-3")?]
                }
            ]);
        }
        "release" => {
            let route = irq(selection, "interrupt-core1", "software-release")?;
            let core_index = route["Core"]["core"]
                .as_u64()
                .and_then(|value| usize::try_from(value).ok())
                .ok_or("secondary-core release route has no processor core")?;
            let processor = selection
                .soc
                .processors
                .get(core_index)
                .ok_or("secondary-core release route targets an absent processor")?;
            recipe["core"] = json!(core_index);
            recipe["options"]["released"] = json!(
                processor
                    .released
                    .ok_or("SoC does not declare the secondary-core reset state")?
            );
        }
        "pulse" => {
            recipe["irqs"] = Value::Array(
                (0..12)
                    .map(|index| irq(selection, "pulse-outputs", &format!("output-{index}")))
                    .collect::<Result<Vec<_>, _>>()?,
            );
        }
        "lens_serial" => {
            recipe["routes"][0]["endpoint"]["completion"] = json!([
                irq(selection, "serial-spi2", "complete-0")?,
                irq(selection, "serial-spi2", "complete-1")?
            ]);
        }
        "control_bus0" => {
            recipe["options"]["irq"] =
                json!(irq(selection, "control-bus0", "controller")?["Core"]["line"]);
            let (touch_core, touch_line) = connection_interrupt(selection, "touch-bus")?;
            recipe["options"]["parts"]["touch"]["irq"] = json!(touch_line);
            recipe["options"]["parts"]["touch"]["core"] = json!(touch_core);
        }
        "control_bus1" => {
            recipe["options"]["irq"] =
                json!(irq(selection, "control-bus1", "controller")?["Core"]["line"]);
        }
        "image_output_0" => {
            recipe["irq"] = json!(irq(selection, "image-output-0", "output")?["Core"]["line"]);
        }
        "image_output_1" => {
            recipe["irq"] = json!(irq(selection, "image-output-1", "output")?["Core"]["line"]);
        }
        "hdmi" => recipe["hot_plug"] = irq(selection, "hdmi-1", "hot-plug")?,
        "sdio" => recipe["irq"] = irq(selection, "sdio-host", "host")?,
        "sd" => recipe["irq"] = irq(selection, "sdhci-card-0", "host")?,
        "companion" => {
            recipe["options"]["irq"] =
                json!(irq(selection, "companion-fe80", "processing")?["Core"]["line"]);
        }
        "image_input" => {
            recipe["phase_irqs"] = json!([
                {"core":core(selection,"core0")?,"line":irq(selection,"image-input","phase-0")?["Core"]["line"]},
                {"core":core(selection,"core0")?,"line":irq(selection,"image-input","phase-1")?["Core"]["line"]}
            ]);
            recipe["raw_irq"] = json!({"core":core(selection,"core0")?,"line":irq(selection,"image-input","raw")?["Core"]["line"]});
            recipe["readback_irq"] = json!({"core":core(selection,"core0")?,"line":irq(selection,"image-input","readback")?["Core"]["line"]});
        }
        "prefilter" | "raw_statistics" | "still_processor" | "image_scaler" | "warp"
        | "jpeg_encoder" | "jpeg_decoder" | "avc_codec" | "image_transfer" => {
            let source = match name {
                "prefilter" => "prefilter",
                "raw_statistics" => "raw-statistics",
                "still_processor" => "still-processor-0",
                "image_scaler" => "image-scaler",
                "warp" => "warp",
                "jpeg_encoder" => "jpeg-encoder",
                "jpeg_decoder" => "jpeg-decoder",
                "avc_codec" => "avc-codec-control",
                "image_transfer" => "image-transfer",
                _ => unreachable!(),
            };
            let route = irq(selection, source, "complete")?;
            recipe["irq"] = json!({"core":route["Core"]["core"],"line":route["Core"]["line"]});
        }
        "image_combination" => {
            let route = irq(selection, "image-combination", "complete")?;
            recipe["irq"] = json!({
                "core":route["Core"]["core"],
                "line":route["Core"]["line"]
            });
        }
        _ => (),
    }
    Ok(())
}

fn core(selection: &ResolvedSelection, id: &str) -> Result<usize, String> {
    selection
        .soc
        .processors
        .iter()
        .position(|processor| processor.id == id)
        .ok_or_else(|| format!("SoC {} has no processor core {id}", selection.soc.id))
}

fn interrupt_controller_core(selection: &ResolvedSelection, id: &str) -> Result<usize, String> {
    let controller = block(selection, id)?;
    if controller.kind != SocBlockKind::InterruptController {
        return Err(format!("SoC block {id} is not an interrupt controller"));
    }
    let processor_id = controller
        .processor_core_id
        .as_deref()
        .ok_or_else(|| format!("SoC interrupt controller {id} has no processor-core binding"))?;
    core(selection, processor_id)
}

fn connection_interrupt(
    selection: &ResolvedSelection,
    connection_id: &str,
) -> Result<(usize, u32), String> {
    let mut interrupts = connection(selection, connection_id)?
        .signals
        .iter()
        .filter_map(|signal| match signal {
            ConnectionSignal::Interrupt {
                controller_id,
                line,
            } => Some((controller_id.as_str(), *line)),
            _ => None,
        });
    let (controller, line) = interrupts
        .next()
        .ok_or_else(|| format!("body connection {connection_id} has no interrupt attachment"))?;
    if interrupts.next().is_some() {
        return Err(format!(
            "body connection {connection_id} has multiple interrupt attachments"
        ));
    }
    let controller = block(selection, controller)?;
    if controller.kind != SocBlockKind::InterruptController {
        return Err(format!(
            "body connection {connection_id} uses non-interrupt block {}",
            controller.id
        ));
    }
    let processor_id = controller.processor_core_id.as_deref().ok_or_else(|| {
        format!(
            "SoC interrupt controller {} has no processor-core attachment",
            controller.id
        )
    })?;
    Ok((core(selection, processor_id)?, line))
}

fn connection<'a>(
    selection: &'a ResolvedSelection,
    id: &str,
) -> Result<&'a fujimod_catalog::reference::BodyConnection, String> {
    selection
        .body
        .connections
        .iter()
        .find(|item| item.id == id)
        .ok_or_else(|| format!("body {} has no connection {id}", selection.body.id))
}

fn part<'a>(selection: &'a ResolvedSelection, id: &str) -> Result<&'a Part, String> {
    selection
        .parts
        .iter()
        .find(|item| item.id == id)
        .ok_or_else(|| format!("body {} has no resolved part {id}", selection.body.id))
}

fn control_bus_chip<'a>(
    selection: &'a ResolvedSelection,
    id: &str,
) -> Result<&'a ControlBusChip, String> {
    selection
        .body
        .control_bus_chips
        .iter()
        .find(|chip| chip.id == id)
        .ok_or_else(|| format!("X70 body has no control-bus endpoint {id}"))
}

fn register_range(first: u32, count: u32, label: &str) -> Result<Vec<u8>, String> {
    first
        .checked_add(count)
        .ok_or_else(|| format!("{label} register range overflows"))?;
    (0..count)
        .map(|offset| {
            first
                .checked_add(offset)
                .and_then(|address| u8::try_from(address).ok())
                .ok_or_else(|| format!("{label} register address exceeds one byte"))
        })
        .collect()
}

fn serial_endpoint(
    selection: &ResolvedSelection,
    connection_id: &str,
) -> Result<(String, u8, u8), String> {
    let mut serial = connection(selection, connection_id)?
        .signals
        .iter()
        .filter_map(|signal| match signal {
            ConnectionSignal::Serial {
                block_id,
                channel,
                address,
            } => Some((block_id.clone(), *channel, *address)),
            _ => None,
        });
    let endpoint = serial
        .next()
        .ok_or_else(|| format!("body connection {connection_id} has no serial endpoint"))?;
    if serial.next().is_some() {
        return Err(format!(
            "body connection {connection_id} has multiple serial endpoints"
        ));
    }
    Ok(endpoint)
}

fn gpio_selection(
    selection: &ResolvedSelection,
    connection_id: &str,
) -> Result<Vec<Value>, String> {
    gpio_signals(selection, connection_id).map(|signals| {
        signals
            .into_iter()
            .map(|(pin, active_level)| {
                json!({
                    "pin":{"port":pin.port,"pin":pin.pin},
                    "high":active_level,
                })
            })
            .collect()
    })
}

fn set_spi_route(
    recipe: &mut Value,
    selection: &ResolvedSelection,
    controller_id: &str,
    channel: u8,
    endpoint_id: &str,
    select_connection_id: &str,
    word_bits: Option<u8>,
    clock_hz: Option<u64>,
) -> Result<(), String> {
    if clock_hz.is_some() {
        return Err(format!(
            "X70 SPI controller {controller_id} has no configured clock-rate model"
        ));
    }
    if block(selection, controller_id)?.kind != SocBlockKind::SerialController {
        return Err(format!(
            "SoC block {controller_id} is not a serial controller"
        ));
    }
    let routes = recipe["routes"]
        .as_array_mut()
        .ok_or("X70 SPI implementation has no routes")?;
    let route = routes
        .get_mut(usize::from(channel))
        .ok_or_else(|| format!("X70 SPI implementation has no channel {channel}"))?;
    if route["endpoint"]["kind"] == "lens-unit" {
        if endpoint_id
            != selection
                .body
                .lens
                .as_ref()
                .map_or("", |lens| lens.part_id.as_str())
        {
            return Err("X70 lens SPI endpoint identity differs from its body attachment".into());
        }
    }
    route["options"]["selection"] = json!(gpio_selection(selection, select_connection_id)?);
    if let Some(bits) = word_bits {
        if bits == 0 {
            return Err(format!(
                "X70 SPI endpoint {endpoint_id} has zero word width"
            ));
        }
        let transports = route["options"]["transports"]
            .as_array_mut()
            .ok_or("X70 SPI route has no transport formats")?;
        for transport in transports {
            transport["bits"] = json!(bits);
        }
    }
    Ok(())
}

fn control_bus_topology(
    selection: &ResolvedSelection,
    controller_id: &str,
    channel: u8,
) -> Result<Vec<u8>, String> {
    let mut addresses = BTreeSet::new();
    for chip in &selection.body.control_bus_chips {
        if chip.controller_id == controller_id && chip.channel == channel {
            addresses.insert(chip.address);
        }
    }
    for connection in &selection.body.connections {
        for signal in &connection.signals {
            if let ConnectionSignal::Serial {
                block_id,
                channel: signal_channel,
                address,
            } = signal
                && block_id == controller_id
                && *signal_channel == channel
            {
                addresses.insert(*address);
            }
        }
    }
    if addresses.is_empty() {
        return Err(format!(
            "X70 control bus {controller_id} channel {channel} has no declared endpoints"
        ));
    }
    Ok(addresses.into_iter().collect())
}

fn apply_body_facts(
    recipe: &mut Value,
    name: &str,
    selection: &ResolvedSelection,
) -> Result<(), String> {
    if recipe["options"].get("ram_bytes").is_some() {
        recipe["options"]["ram_bytes"] = json!(selection.body.dram_bytes);
    }
    match name {
        "serial_spi1" => {
            let mcu = selection
                .body
                .microcontroller
                .as_ref()
                .ok_or("X70 body has no MCU attachment")?;
            if mcu.controller_id != "serial-spi1" {
                return Err("X70 MCU is not attached to serial-spi1".into());
            }
            set_spi_route(
                recipe,
                selection,
                &mcu.controller_id,
                mcu.channel,
                &mcu.part_id,
                &mcu.select_connection_id,
                Some(mcu.word_bits),
                None,
            )?;

            let PartModel::Microcontroller {
                registers,
                packet_registers,
                clock_registers,
                status_register,
                initial_registers,
            } = &part(selection, &mcu.part_id)?.model
            else {
                return Err("X70 MCU attachment is not a microcontroller part".into());
            };
            if registers.width_bytes != 1
                || packet_registers.width_bytes != 1
                || clock_registers.width_bytes != 1
            {
                return Err("X70 MCU register model requires byte-wide registers".into());
            }
            let mut addresses = BTreeSet::new();
            addresses.extend(register_range(
                packet_registers.first,
                packet_registers.count,
                "X70 MCU packet",
            )?);
            let clock_addresses = register_range(
                clock_registers.first,
                clock_registers.count,
                "X70 MCU clock",
            )?;
            addresses.extend(clock_addresses.iter().copied());
            let status_addresses = status_register
                .map(|address| {
                    u8::try_from(address)
                        .map_err(|_| "X70 MCU status register exceeds one byte".to_owned())
                })
                .transpose()?;
            addresses.extend(status_addresses);
            for address in initial_registers {
                addresses.insert(
                    u8::try_from(*address)
                        .map_err(|_| "X70 MCU initial register exceeds one byte".to_owned())?,
                );
            }
            if addresses.iter().any(|address| {
                u32::from(*address) < registers.first
                    || u32::from(*address) >= registers.first.saturating_add(registers.count)
            }) {
                return Err("X70 MCU initialized register lies outside its declared extent".into());
            }
            let mcu_options =
                &mut recipe["routes"][usize::from(mcu.channel)]["endpoint"]["options"];
            mcu_options["registers"] = Value::Array(
                addresses
                    .into_iter()
                    .map(|address| json!([address, 0]))
                    .collect(),
            );
            mcu_options["status_registers"] =
                json!(status_addresses.into_iter().collect::<Vec<_>>());
            mcu_options["clock"]["registers"] = json!(clock_addresses);

            let display = selection
                .body
                .display
                .as_ref()
                .ok_or("X70 body has no display attachment")?;
            let route = display
                .route
                .as_ref()
                .ok_or("X70 display has no serial route")?;
            if route.endpoint_id != display.part_id {
                return Err("X70 display route endpoint differs from installed part".into());
            }
            set_spi_route(
                recipe,
                selection,
                &route.controller_id,
                route.channel,
                &route.endpoint_id,
                &route.select_connection_id,
                Some(route.word_bits),
                route.clock_hz,
            )?;
            let display_part = part(selection, &display.part_id)?;
            let PartModel::DisplayPanel { used_mask, .. } = &display_part.model else {
                return Err("X70 display attachment is not a panel part".into());
            };
            recipe["routes"][usize::from(route.channel)]["endpoint"]["options"]["used_mask"] =
                json!(used_mask);
        }
        "lens_serial" => {
            let lens = selection
                .body
                .lens
                .as_ref()
                .ok_or("X70 body has no lens attachment")?;
            if lens.route.endpoint_id != lens.part_id {
                return Err("X70 lens route endpoint differs from installed part".into());
            }
            set_spi_route(
                recipe,
                selection,
                &lens.route.controller_id,
                lens.route.channel,
                &lens.route.endpoint_id,
                &lens.route.select_connection_id,
                Some(lens.route.word_bits),
                lens.route.clock_hz,
            )?;
            let PartModel::LensMechanism {
                protocol,
                motors,
                encoder,
                zoom,
                ..
            } = &part(selection, &lens.part_id)?.model
            else {
                return Err("X70 lens attachment is not a lens mechanism part".into());
            };
            if *protocol != LensProtocol::Banked {
                return Err(format!(
                    "X70 lens assembler does not support the {protocol:?} protocol"
                ));
            }
            if encoder.is_some() != lens.encoder_connection_id.is_some() {
                return Err("X70 lens encoder part and body connection do not agree".into());
            }
            if encoder.is_some() {
                return Err(
                    "X70 native lens implementation does not support an encoder part".into(),
                );
            }
            if zoom.is_some() {
                return Err(
                    "X70 native lens implementation does not support a zoom mechanism".into(),
                );
            }
            if motors.len() != 2 {
                return Err("X70 banked lens implementation requires two motor records".into());
            }
            let options =
                &mut recipe["routes"][usize::from(lens.route.channel)]["endpoint"]["options"];
            options["protocol"]["kind"] = json!("banked");
            let motor_values = motors
                .iter()
                .enumerate()
                .map(|(index, motor)| {
                    // The banked implementation routes the body's counter
                    // channels through motor zero. Dividers are constructor
                    // policy; channel identities come from the selected body.
                    let counters = if index == 0 {
                        lens.pulse
                            .counter_channels
                            .iter()
                            .map(|channel| {
                                let divider = match channel {
                                    0 => 4,
                                    4 => 1,
                                    _ => {
                                        return Err(format!(
                                            "X70 banked lens has no native divider policy for pulse channel {channel}"
                                        ));
                                    }
                                };
                                Ok(json!({"channel":channel,"divider":divider}))
                            })
                            .collect::<Result<Vec<_>, String>>()?
                    } else {
                        Vec::new()
                    };
                    let home = lens
                        .home_connections
                        .iter()
                        .find(|connection| usize::from(connection.motor) == index);
                    let home_value = match (&motor.home, home) {
                        (None, None) => Value::Null,
                        (Some(switch), Some(connection)) => {
                            let pins = gpio_signals(selection, &connection.connection_id)?;
                            if pins.len() != 1 {
                                return Err(format!(
                                    "lens home connection {} must resolve to one GPIO pin",
                                    connection.connection_id
                                ));
                            }
                            json!({
                                "pin":{"port":pins[0].0.port,"pin":pins[0].0.pin},
                                "threshold":switch.threshold,
                                "side":"above",
                            })
                        }
                        (Some(_), None) => {
                            return Err(format!("lens motor {index} has no home connection"));
                        }
                        (None, Some(_)) => {
                            return Err(format!(
                                "lens motor {index} has an undeclared home switch"
                            ));
                        }
                    };
                    Ok(json!({
                        "initial_position":motor.initial_position,
                        "travel_limits":motor.travel_limits,
                        "ticks_per_step":motor.ticks_per_step,
                        "settle_ticks":motor.settle_ticks,
                        "home":home_value,
                        "counters":counters,
                    }))
                })
                .collect::<Result<Vec<_>, String>>()?;
            options["motors"] = Value::Array(motor_values);
            options["encoder"] = Value::Null;
            options["zoom"] = Value::Null;
            if let Some(iris) = &lens.iris {
                if iris.positions.is_empty()
                    || iris.positions.iter().any(|position| {
                        !(iris.travel_limits[0]..=iris.travel_limits[1])
                            .contains(&position.position)
                    })
                {
                    return Err("X70 lens iris positions exceed declared travel limits".into());
                }
                options["motors"][1]["travel_limits"] = json!(iris.travel_limits);
                options["motors"][1]["boundary"] = json!("end_stop");
            }
            let optics = lens
                .iris
                .as_ref()
                .map(|iris| {
                    json!({
                        "output":iris.output,
                        "positions":iris.positions.iter().map(|position| {
                            json!([position.position,position.f_number])
                        }).collect::<Vec<_>>(),
                    })
                })
                .unwrap_or(Value::Null);
            recipe["routes"][usize::from(lens.route.channel)]["endpoint"]["optics"] = optics;
            recipe["routes"][usize::from(lens.route.channel)]["endpoint"]["focus"] =
                serde_json::to_value(&lens.focus).map_err(|error| error.to_string())?;
        }
        "power_peripherals" => {
            let power = selection
                .body
                .power_control
                .as_ref()
                .ok_or("X70 body has no power-control attachment")?;
            if power.controller_id != "power-peripherals" || power.channel != 0 {
                return Err("X70 power controller has no supported power-peripherals route".into());
            }
            let pins = gpio_selection(selection, &power.select_connection_id)?;
            if pins.len() != 1 {
                return Err("X70 power-controller select must resolve to one GPIO signal".into());
            }
            recipe["options"]["spi0"]["selection"] = pins[0].clone();
            recipe["options"]["spi0"]["presence"] = pins[0].clone();
        }
        "pulse" => {
            let lens = selection
                .body
                .lens
                .as_ref()
                .ok_or("X70 body has no lens attachment")?;
            recipe["options"]["counter_channels"] = json!(lens.pulse.counter_channels);
            recipe["options"]["timer_channels"] = json!(lens.pulse.timer_channels);
            recipe["options"]["quadrature"] = json!(lens.pulse.quadrature);
            recipe["options"]["initial_phase"] = json!(lens.pulse.initial_phase);
        }
        "control_bus0" | "control_bus1" => {
            let controller_id = if name == "control_bus0" {
                "control-bus0"
            } else {
                "control-bus1"
            };
            let mut channels = selection
                .body
                .control_bus_chips
                .iter()
                .filter(|chip| chip.controller_id == controller_id)
                .map(|chip| chip.channel)
                .collect::<BTreeSet<_>>();
            for connection in &selection.body.connections {
                for signal in &connection.signals {
                    if let ConnectionSignal::Serial {
                        block_id, channel, ..
                    } = signal
                        && block_id == controller_id
                    {
                        channels.insert(*channel);
                    }
                }
            }
            if channels.len() != 1 {
                return Err(format!(
                    "X70 control bus {controller_id} must resolve to one documented channel"
                ));
            }
            let channel = *channels.first().ok_or("control bus channel is absent")?;
            recipe["options"]["parts"]["topology"] =
                json!(control_bus_topology(selection, controller_id, channel)?);
            if name == "control_bus0" {
                let sensor = selection
                    .body
                    .sensor
                    .as_ref()
                    .ok_or("X70 body has no sensor attachment")?;
                let sensor_bus = sensor
                    .control_bus
                    .as_ref()
                    .ok_or("X70 sensor has no control-bus route")?;
                let (sensor_controller, sensor_channel, sensor_address) =
                    serial_endpoint(selection, &sensor.connection_id)?;
                if sensor_controller != sensor_bus.controller_id
                    || sensor_channel != sensor_bus.channel
                    || sensor_address != sensor_bus.address
                {
                    return Err(
                        "X70 sensor control-bus attachment differs from its connection".into(),
                    );
                }
                recipe["options"]["parts"]["sensor_address"] = json!(sensor_address);
                let (touch_controller, touch_channel, touch_address) =
                    serial_endpoint(selection, "touch-bus")?;
                if touch_controller != controller_id || touch_channel != channel {
                    return Err("X70 touch endpoint is not attached to control-bus0".into());
                }
                recipe["options"]["parts"]["prefix_address"] = json!(touch_address);
            } else {
                let register = control_bus_chip(selection, "secondary-register-endpoint")?;
                let pwic = control_bus_chip(selection, "secondary-pwic")?;
                let sink = control_bus_chip(selection, "secondary-sink")?;
                for chip in [register, pwic, sink] {
                    if chip.controller_id != controller_id || chip.channel != channel {
                        return Err(format!(
                            "X70 named control-bus endpoint {} uses a different controller or channel",
                            chip.id
                        ));
                    }
                }
                recipe["options"]["parts"]["register_address"] = json!(register.address);
                recipe["options"]["parts"]["pwic_address"] = json!(pwic.address);
                if !recipe["options"]["parts"]["topology"]
                    .as_array()
                    .is_some_and(|addresses| addresses.contains(&json!(sink.address)))
                {
                    return Err(
                        "X70 secondary bus sink is absent from the endpoint topology".into(),
                    );
                }
            }
        }
        "image_input" | "still_processor" => {
            let sensor = selection
                .body
                .sensor
                .as_ref()
                .ok_or("X70 body has no sensor attachment")?;
            let PartModel::ImageSensor {
                raster,
                color_filter,
                ..
            } = &part(selection, &sensor.part_id)?.model
            else {
                return Err("X70 sensor attachment is not an image-sensor part".into());
            };
            let mut side = 1usize;
            while side
                .checked_mul(side)
                .is_some_and(|area| area < color_filter.len())
            {
                side = side
                    .checked_add(1)
                    .ok_or("X70 sensor CFA dimensions overflow")?;
            }
            if side.checked_mul(side) != Some(color_filter.len()) {
                return Err("X70 sensor CFA is not a square pattern".into());
            }
            let origin_x = usize::try_from(raster.image_area[0] % side as u32)
                .map_err(|_| "X70 sensor CFA x origin is invalid")?;
            let origin_y = usize::try_from(raster.image_area[1] % side as u32)
                .map_err(|_| "X70 sensor CFA y origin is invalid")?;
            // The image-input CFA is the sensor pattern viewed at the active
            // crop origin; this derives the processing phase without storing
            // a second copy of the sensor's color-filter array.
            let processing_cfa = (0..side)
                .flat_map(|y| {
                    (0..side).map(move |x| {
                        color_filter[((y + origin_y) % side) * side + (x + origin_x) % side]
                    })
                })
                .collect::<Vec<_>>();
            recipe["options"]["cfa"] = json!(processing_cfa);
        }
        _ => (),
    }
    Ok(())
}

/// The sensor part's exposure calibration, as the xtrans16 and live-view models read it.
fn sensor_calibration(selection: &ResolvedSelection) -> Result<(Value, &SensorRadiometry), String> {
    let sensor = selection
        .body
        .sensor
        .as_ref()
        .ok_or("X70 body has no sensor attachment")?;
    let PartModel::ImageSensor {
        color_filter,
        full_scale,
        reference_white,
        radiometry,
        ..
    } = &part(selection, &sensor.part_id)?.model
    else {
        return Err("X70 sensor attachment is not an image-sensor part".into());
    };
    let radiometry = radiometry
        .as_ref()
        .ok_or("X70 image sensor declares no radiometry")?;
    let reference_white: [u16; 3] = reference_white
        .iter()
        .map(|v| u16::try_from(*v).map_err(|_| "X70 sensor reference white exceeds u16".to_owned()))
        .collect::<Result<Vec<_>, _>>()?
        .try_into()
        .map_err(|_| "X70 sensor reference white has no three channels".to_owned())?;
    let full_scale = u16::try_from(*full_scale).map_err(|_| "X70 sensor full scale exceeds u16")?;
    let mut color_sample_weights = [0u8; 3];
    for &color in color_filter {
        let channel = color_sample_weights
            .get_mut(usize::from(color))
            .ok_or("X70 sensor color filter has an unknown channel")?;
        *channel = channel
            .checked_add(1)
            .ok_or("X70 sensor color filter weight overflow")?;
    }
    let calibration = json!({
        "full_scale": full_scale,
        "reference_white": reference_white,
        "saturation_lux_seconds": radiometry.saturation_lux_seconds,
        "color_sample_weights": color_sample_weights,
    });
    Ok((calibration, radiometry))
}

/// The lens projection of the sensor, with its focus wire.
fn sensor_projection(radiometry: &SensorRadiometry) -> Result<Value, String> {
    let mut projection = serde_json::to_value(&radiometry.projection).map_err(|e| e.to_string())?;
    projection["focus"] = json!("lens_focus");
    Ok(projection)
}

/// The browser's image sensor: an xtrans16 exposure model fed by the declared scene
/// input, with phase acknowledgment enabled and its shutter, frame clock and statistics
/// wired. Every value comes from the sensor part's catalog facts.
fn apply_image_sensor(
    recipe: &mut Value,
    selection: &ResolvedSelection,
    browser: &RunBrowserCamera,
) -> Result<(), String> {
    let sensor = selection
        .body
        .sensor
        .as_ref()
        .ok_or("X70 body has no sensor attachment")?;
    let PartModel::ImageSensor {
        raster,
        color_filter,
        black_level,
        full_scale,
        reference_white,
        ..
    } = &part(selection, &sensor.part_id)?.model
    else {
        return Err("X70 sensor attachment is not an image-sensor part".into());
    };
    let (calibration, radiometry) = sensor_calibration(selection)?;
    let pixel_clock_hz = raster
        .pixel_clock_hz
        .ok_or("X70 image sensor declares no pixel clock")?;
    let black = u16::try_from(*black_level).map_err(|_| "X70 sensor black level exceeds u16")?;
    let full_scale = u16::try_from(*full_scale).map_err(|_| "X70 sensor full scale exceeds u16")?;
    let reference_white: Vec<u16> = reference_white
        .iter()
        .map(|v| u16::try_from(*v).map_err(|_| "X70 sensor reference white exceeds u16".to_owned()))
        .collect::<Result<_, _>>()?;
    recipe["options"]["phase_ack"] = json!("enabled");
    recipe["options"]["clock"]["source"] = json!({"kind": "sensor"});
    recipe["options"]["sensor_period_ticks"] = json!(radiometry.period_ticks);
    recipe["shutter_output"] = json!(radiometry.shutter_output);
    recipe["frame_period"] = json!("sensor_frame_period");
    recipe["statistics_grid"] = json!("sensor_statistics_grid");
    recipe["sensor"] = json!({
        "frames": browser.sensor.frames,
        "options": {
            "width": raster.width,
            "height": raster.height,
            "image_area": raster.image_area,
            "cfa": color_filter,
            "black": black,
            "full_scale": full_scale,
            "reference_white": reference_white,
        },
        "controls": {
            "kind": "xtrans16",
            "options": {
                "pixel_clock_hz": pixel_clock_hz,
                "shutter": {"closing_delay_seconds": radiometry.shutter_closing_delay_seconds},
                "rolling_references": radiometry.rolling_references,
                // Stock AF-S traces: the first RAW wait after a drive-mode change
                // spans about three frame periods. The two frames read out before
                // it carry no valid sync, raise no source edge and start no DMA.
                "mode_change_frames": radiometry.mode_change_frames,
                "preview_field": radiometry.preview_field,
            },
            "calibration": calibration,
            "exposure": "sensor_exposure",
            "aperture": "lens_aperture",
            "projection": sensor_projection(radiometry)?,
            "frame_clock": {
                "output": "sensor_frame_period",
                "ticks_per_second": radiometry.ticks_per_second,
            },
        },
    });
    Ok(())
}

/// The live view reads the same sensor: its frames, exposure and parameters are the
/// sensor's radiometric inputs, so the preview follows the sensor's exposure.
fn apply_live_sensor(
    recipe: &mut Value,
    selection: &ResolvedSelection,
    browser: &RunBrowserCamera,
) -> Result<(), String> {
    let (calibration, radiometry) = sensor_calibration(selection)?;
    recipe["sensor_clock"] = json!(browser.sensor.frames);
    recipe["frame_period"] = Value::Null;
    recipe["radiometric"] = json!({
        "frames": browser.sensor.frames,
        "exposure": "sensor_exposure",
        "aperture": "lens_aperture",
        "parameters": "parameters",
        "calibration": calibration,
        "projection": sensor_projection(radiometry)?,
    });
    Ok(())
}

fn gpio_signals(
    selection: &ResolvedSelection,
    connection_id: &str,
) -> Result<Vec<(Pin, bool)>, String> {
    let item = connection(selection, connection_id)?;
    let pins = item
        .signals
        .iter()
        .filter_map(|signal| match signal {
            ConnectionSignal::Gpio {
                port,
                pin,
                active_level,
            } => Some((
                Pin {
                    port: *port,
                    pin: *pin,
                },
                *active_level,
            )),
            _ => None,
        })
        .collect::<Vec<_>>();
    if pins.is_empty() {
        return Err(format!(
            "body connection {connection_id} has no GPIO signal for this run input"
        ));
    }
    Ok(pins)
}

fn input_level(
    levels: &mut [u32; 16],
    assigned: &mut BTreeMap<(u8, u8), String>,
    pin: Pin,
    high: bool,
    owner: &str,
) -> Result<(), String> {
    pin.validate()?;
    if pin.port >= 16 {
        return Err("run GPIO input pin exceeds the native input-level array".into());
    }
    if let Some(previous) = assigned.insert((pin.port, pin.pin), owner.to_owned()) {
        return Err(format!(
            "run GPIO input assignments {previous} and {owner} collide on GPIO {}.{}",
            pin.port, pin.pin,
        ));
    }
    let mask = 1u32 << pin.pin;
    if high {
        levels[usize::from(pin.port)] |= mask;
    } else {
        levels[usize::from(pin.port)] &= !mask;
    }
    Ok(())
}

/// `card_present` is the presence of the declared virtual card, which drives its detect contact.
fn gpio_levels(
    selection: &ResolvedSelection,
    inputs: &Exr2RunInputs,
    card_present: bool,
) -> Result<([u32; 16], Vec<Value>), String> {
    let unassigned = inputs.gpio.unassigned_input_level;
    let mut levels = [if unassigned { u32::MAX } else { 0 }; 16];
    let mut assigned = BTreeMap::<(u8, u8), String>::new();
    let mut handled = BTreeSet::new();
    for state in &inputs.gpio.keys {
        for (pin, active) in gpio_signals(selection, &state.connection_id)? {
            input_level(
                &mut levels,
                &mut assigned,
                pin,
                if state.pressed { active } else { !active },
                &state.connection_id,
            )?;
        }
        handled.insert(state.connection_id.as_str());
    }
    for state in &inputs.gpio.connections {
        for (pin, active) in gpio_signals(selection, &state.connection_id)? {
            input_level(
                &mut levels,
                &mut assigned,
                pin,
                if state.connected { active } else { !active },
                &state.connection_id,
            )?;
        }
        handled.insert(state.connection_id.as_str());
    }

    for state in &inputs.encoders {
        let signals = gpio_signals(selection, &state.connection_id)?;
        if signals.len() != 2 {
            return Err(format!(
                "X70 encoder {} must expose exactly two GPIO phase signals",
                state.connection_id
            ));
        }
        for (index, (pin, active)) in signals.into_iter().enumerate() {
            let phase_active = state.phase & (1 << index) != 0;
            input_level(
                &mut levels,
                &mut assigned,
                pin,
                if phase_active { active } else { !active },
                &state.connection_id,
            )?;
        }
        handled.insert(state.connection_id.as_str());
    }

    let card = selection
        .body
        .card_slot
        .as_ref()
        .ok_or("X70 run requires a declared card-slot attachment")?;
    if inputs.card.present {
        return Err("X70 has no removable-card controller".into());
    }
    for (id, asserted) in [
        (card.detect_connection_id.as_str(), card_present),
        (
            card.write_protect_connection_id.as_str(),
            inputs.card.write_protected,
        ),
    ] {
        for (pin, active) in gpio_signals(selection, id)? {
            input_level(
                &mut levels,
                &mut assigned,
                pin,
                if asserted { active } else { !active },
                id,
            )?;
        }
        handled.insert(id);
    }

    for item in &selection.body.connections {
        if item
            .signals
            .iter()
            .any(|signal| matches!(signal, ConnectionSignal::Gpio { .. }))
            && !handled.contains(item.id.as_str())
        {
            let is_select = selection
                .body
                .lens
                .as_ref()
                .is_some_and(|part| part.route.select_connection_id == item.id)
                || selection
                    .body
                    .display
                    .as_ref()
                    .and_then(|part| part.route.as_ref())
                    .is_some_and(|part| part.select_connection_id == item.id)
                || selection
                    .body
                    .microcontroller
                    .as_ref()
                    .is_some_and(|part| part.select_connection_id == item.id)
                || selection
                    .body
                    .power_control
                    .as_ref()
                    .is_some_and(|part| part.select_connection_id == item.id);
            let is_lens_endpoint = selection.body.lens.as_ref().is_some_and(|lens| {
                lens.home_connections
                    .iter()
                    .any(|home| home.connection_id == item.id)
            });
            if !is_select && !is_lens_endpoint {
                return Err(format!(
                    "GPIO connection {} has no explicit run input or installed endpoint",
                    item.id
                ));
            }
        }
    }

    let mut outputs = BTreeMap::<(u8, u8), bool>::new();
    let mut output_owners = BTreeMap::<(u8, u8), String>::new();
    for state in &inputs.gpio.outputs {
        let target = connection(selection, &state.connection_id)?;
        let signal = target
            .signals
            .get(usize::from(state.signal_index))
            .ok_or_else(|| {
                format!(
                    "GPIO output {} has no signal at index {}",
                    state.connection_id, state.signal_index
                )
            })?;
        let ConnectionSignal::Gpio { port, pin, .. } = signal else {
            return Err(format!(
                "GPIO output {} signal {} is not a GPIO pad",
                state.connection_id, state.signal_index
            ));
        };
        let pin = Pin {
            port: *port,
            pin: *pin,
        };
        pin.validate()?;
        let identity = (pin.port, pin.pin);
        if let Some(previous) = output_owners.insert(identity, state.connection_id.clone()) {
            return Err(format!(
                "run GPIO output assignments {previous} and {} collide on GPIO {}.{}",
                state.connection_id, pin.port, pin.pin
            ));
        }
        outputs.insert(identity, state.high);
    }
    let output_values = outputs
        .into_iter()
        .map(|((port, pin), high)| json!({"pin":{"port":port,"pin":pin},"high":high}))
        .collect();
    Ok((levels, output_values))
}

fn apply_run_inputs(
    recipe: &mut Value,
    name: &str,
    selection: &ResolvedSelection,
    inputs: &Exr2RunInputs,
    scene: &Exr2RunScene,
    card_present: bool,
) -> Result<(), String> {
    match name {
        "serial_gpio" => {
            let (levels, outputs) = gpio_levels(selection, inputs, card_present)?;
            recipe["options"]["input_levels"] = json!(levels);
            recipe["options"]["output_levels"] = json!(outputs);
        }
        "adc" => {
            let samples: [u16; 12] = inputs
                .adc
                .samples
                .as_slice()
                .try_into()
                .map_err(|_| "X70 ADC requires twelve configured samples")?;
            recipe["options"]["samples"] = json!(samples);
        }
        "usb" => {
            if inputs.usb.connection_id != "usb" {
                return Err("X70 USB input does not resolve to its installed connection".into());
            }
        }
        "power_peripherals" => {
            if !inputs.power.powered_on {
                return Err("X70 run model does not support a powered-off supply".into());
            }
            if inputs.power.boot_reply != 0 || !inputs.power.spi_status.is_empty() {
                return Err(
                    "X70 power endpoint only supports boot reply zero and no SPI status inputs"
                        .into(),
                );
            }
        }
        "control_bus0" => {
            // The body control-bus chip inventory is validated by the catalog
            // resolver. Their modeled transaction contracts are an explicit
            // X70 implementation policy in this module.
        }
        "control_bus1" => {
            // The secondary charge state and the USB cable wire come from the
            // same run input, so a connected cable selects the declared
            // connected secondary profile. The disconnected stock profile stays
            // in place otherwise.
            if inputs.usb.connected {
                recipe["options"]["profile"] = json!("declared-bus1-register-replies-unmeasured");
                recipe["options"]["parts"]["connected"] = json!(true);
            }
        }
        "live_frames" => {
            let s1 = inputs
                .gpio
                .keys
                .iter()
                .find(|state| state.connection_id == "s1")
                .ok_or("X70 frame input requires an explicit S1 reset state")?;
            let signal = gpio_signals(selection, &s1.connection_id)?;
            if signal.len() != 1 {
                return Err("X70 S1 frame input must resolve to one GPIO signal".into());
            }
            recipe["activity"]["selection"] = json!({
                "pin":{"port":signal[0].0.port,"pin":signal[0].0.pin},
                "high":signal[0].1,
            });
        }
        "encoder_control_ring" => {
            if inputs.encoders.len() != 1 {
                return Err(
                    "X70 implementation requires exactly one configured control-ring encoder"
                        .into(),
                );
            }
            let state = &inputs.encoders[0];
            if state.connection_id != "controls-encoder-3-6-3-7" {
                return Err(format!(
                    "X70 implementation does not support encoder connection {}",
                    state.connection_id
                ));
            }
            let signals = gpio_signals(selection, &state.connection_id)?;
            if signals.len() != 2 {
                return Err("X70 control-ring encoder must expose two GPIO signals".into());
            }
            recipe["options"]["pins"] = json!(
                signals
                    .iter()
                    .map(|(pin, _)| { json!({"port":pin.port,"pin":pin.pin}) })
                    .collect::<Vec<_>>()
            );
            recipe["options"]["rest_phase"] = json!(state.phase);
        }
        _ => (),
    }

    let frame_period = scene.frame_period_ticks;
    match name {
        "statistics" | "image_output_0" | "image_output_1" => {
            recipe["options"]["clock"]["ticks"] = json!(frame_period);
        }
        "live_frames" => {
            recipe["options"]["period_ticks"] = json!(frame_period);
        }
        "image_input" if recipe["options"]["clock"]["source"]["kind"] == "periodic" => {
            recipe["options"]["clock"]["source"]["period_ticks"] = json!(frame_period);
        }
        "display_timing" => {
            recipe["options"]["frame_ticks"] = json!(frame_period);
        }
        _ => (),
    }

    if name == "serial_spi1" {
        let packet = &inputs.mcu.packet;
        let body_mcu = selection
            .body
            .microcontroller
            .as_ref()
            .ok_or("X70 has no MCU attachment")?;
        let part = selection
            .parts
            .iter()
            .find(|part| part.id == body_mcu.part_id)
            .ok_or("X70 MCU part is unresolved")?;
        let PartModel::Microcontroller {
            packet_registers,
            clock_registers,
            status_register,
            initial_registers,
            ..
        } = &part.model
        else {
            return Err("X70 MCU attachment is not a microcontroller part".into());
        };
        let mut values = BTreeMap::<u8, u8>::new();
        for (offset, value) in packet.iter().enumerate() {
            let address = packet_registers
                .first
                .checked_add(offset as u32)
                .and_then(|value| u8::try_from(value).ok())
                .ok_or("X70 MCU packet address exceeds one byte")?;
            values.insert(address, *value);
        }
        let rtc = inputs.mcu.rtc;
        for offset in 0..clock_registers.count {
            let address = clock_registers
                .first
                .checked_add(offset)
                .and_then(|value| u8::try_from(value).ok())
                .ok_or("X70 MCU clock address exceeds one byte")?;
            values.insert(address, (DEFAULT_CLOCK_SECONDS_2000 >> (offset * 8)) as u8);
        }
        if let Some(status) = status_register {
            values.entry(*status as u8).or_insert(0);
        }
        for address in initial_registers {
            values.entry(*address as u8).or_insert(0);
        }
        for input in &inputs.serial_registers {
            if input.part_id != body_mcu.part_id {
                return Err(format!(
                    "X70 has no serial-register input implementation for part {}",
                    input.part_id
                ));
            }
            for value in &input.values {
                values.insert(value.register, value.value);
            }
        }
        {
            let clock = recipe["routes"][body_mcu.channel as usize]["endpoint"]["options"]["clock"]
                .as_object_mut()
                .ok_or("X70 MCU implementation has no clock input")?;
            clock.insert("initial_seconds".into(), json!(DEFAULT_CLOCK_SECONDS_2000));
            clock.insert("ticks_per_second".into(), json!(rtc.ticks_per_second));
            clock.insert("maximum_seconds".into(), json!(rtc.maximum_seconds));
            clock.insert("started".into(), json!(rtc.started));
        }
        let registers =
            recipe["routes"][body_mcu.channel as usize]["endpoint"]["options"]["registers"]
                .as_array_mut()
                .ok_or("X70 MCU implementation has no register bank")?;
        for entry in registers.iter_mut() {
            let address = entry[0]
                .as_u64()
                .and_then(|value| u8::try_from(value).ok())
                .ok_or("X70 MCU register address is invalid")?;
            if let Some(value) = values.get(&address) {
                entry[1] = json!(value);
            }
        }
        for address in values.keys() {
            if !registers
                .iter()
                .any(|row| row[0].as_u64() == Some(u64::from(*address)))
            {
                return Err(format!(
                    "X70 MCU register {address} has no installed endpoint"
                ));
            }
        }
    }
    if name == "usb_cable" {
        // The typed wire is updated in assemble after all recipes are built.
    }

    Ok(())
}

fn scene(selection: &ResolvedSelection, scene: &Exr2RunScene) -> Result<Value, String> {
    let nand = selection.body.nand.ok_or("X70 body has no NAND geometry")?;
    let image = &scene.images.live_image;
    let statistics = scene
        .statistics
        .live_statistics
        .iter()
        .map(|span| json!({"destination":span.destination,"offset":span.offset,"asset":span.asset}))
        .collect::<Vec<_>>();
    Ok(json!({
        "images": { "live_image": {
            "width": image.width,
            "height": image.height,
            "stride": image.stride,
            "asset": image.asset,
        }},
        "statistics": { "live_statistics": statistics },
        "nands": { "nand": {
            "kind": "raw",
            "options": {
                "bytes": nand.bytes,
                "page_bytes": nand.page_bytes,
                "pages_per_block": nand.pages_per_block,
                "spare_bytes": nand.spare_bytes,
            },
            "base": "nand_base",
            "overlays": [],
            "spare": [],
        }},
    }))
}

/// Build the closed EXR II plan from a resolved X70 identity and its run inputs.
pub(super) fn assemble(
    selection: &ResolvedSelection,
    configuration: &RunConfiguration,
    inputs: &Exr2RunInputs,
    scene_input: &Exr2RunScene,
) -> Result<(Plan, Value, Value), String> {
    if selection.soc.id != "exr2"
        || selection.body.id != "x70"
        || selection.firmware.id != "x70-110"
        || configuration.id != "x70-110-rec"
        || configuration.purpose != RunPurpose::RomBoot
        || configuration.start != RunStart::Rom
    {
        return Err(
            "X70 native boot implementation does not support this selected identity".into(),
        );
    }
    let browser = configuration.browser_camera.as_ref();
    let rules = NODE_RULES
        .iter()
        .filter(|rule| browser.is_none() || rule.name != "sd_clocks")
        .chain(browser.map(|_| &CARD_NODE_RULE));
    let mut nodes = Vec::with_capacity(NODE_RULES.len() + 1);
    let mut serialized_nodes = Vec::with_capacity(NODE_RULES.len() + 1);
    let receive_audio = configuration
        .preset
        .as_ref()
        .is_some_and(|p| p.factory_options.audio_input_profile.is_some());
    for rule in rules {
        let mut recipe = match (rule.name, browser) {
            ("sd", Some(browser)) => card_recipe(selection, browser)?,
            _ => model_recipe(rule.name)?,
        };
        if receive_audio && rule.name == "audio_registers" {
            recipe = json!({"kind":"serial-audio-duplex", "options": {
                "receive": {"ram_bytes":536870912, "max_transfer_bytes":1048576,
                    "ticks_per_second":12000000, "sample_rate":48000, "sample":0},
                "transmit": {"ram_bytes":536870912, "max_transfer_bytes":1048576,
                    "ticks_per_byte":64, "minimum_ticks":1000}, "sinks":[true,true]},
                "routes":"imaging_routes"});
        }
        if rule.name == "image_input" {
            if let Some(ticks) = configuration
                .preset
                .as_ref()
                .and_then(|p| p.factory_options.raw_readback_ticks)
            {
                if ticks == 0 {
                    return Err("RAW readback requires positive virtual transfer ticks".into());
                }
                recipe["options"]["readback_ticks"] = json!(ticks);
            }
        }
        if rule.name == "avc_codec" {
            use fujimod_catalog_construction::VideoEncoderBackend;
            // A prepared browser graph is portable across hosts. Native runs
            // may request hardware encoding explicitly.
            let selected = if browser.is_some() {
                VideoEncoderBackend::Software
            } else {
                configuration
                    .preset
                    .as_ref()
                    .and_then(|p| p.factory_options.video_encoder_backend)
                    .unwrap_or(VideoEncoderBackend::Software)
            };
            recipe["options"]["backend"] = json!(selected);
        }
        apply_soc_blocks(&mut recipe, rule, selection)?;
        apply_body_facts(&mut recipe, rule.name, selection)?;
        if let ("image_input", Some(browser)) = (rule.name, browser) {
            apply_image_sensor(&mut recipe, selection, browser)?;
        }
        if let ("live_frames", Some(browser)) = (rule.name, browser) {
            apply_live_sensor(&mut recipe, selection, browser)?;
        }
        apply_interrupts(&mut recipe, rule.name, selection)?;
        apply_run_inputs(
            &mut recipe,
            rule.name,
            selection,
            inputs,
            scene_input,
            browser.is_some(),
        )?;
        let model: Model = serde_json::from_value(recipe.clone())
            .map_err(|error| format!("X70 node {}: {error}", rule.name))?;
        let windows = rule
            .blocks
            .iter()
            .map(|id| {
                let block = block(selection, id)?;
                Ok::<_, String>(DeviceMapping {
                    base: block.base,
                    size: block.size,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        nodes.push(Node {
            name: rule.name.into(),
            boot_entries: vec![BootEntry::Rom],
            windows,
            dependencies: rule
                .dependencies
                .iter()
                .map(|name| (*name).into())
                .collect(),
            model,
        });
        serialized_nodes.push(json!({
            "name": rule.name,
            "boot_entries": ["rom"],
            "windows": rule.blocks.iter().map(|id| {
                let block = block(selection, id)?;
                Ok::<_, String>(json!({"base":block.base,"size":block.size}))
            }).collect::<Result<Vec<_>,_>>()?,
            "dependencies": rule.dependencies,
            "model": recipe,
        }));
    }
    let mut wires = implementation_wires();
    wires.insert(
        "usb_cable".into(),
        WireKind::LogicLevel(inputs.usb.connected),
    );
    if browser.is_some() {
        wires.insert("sensor_exposure".into(), WireKind::Exposure);
        wires.insert("sensor_frame_period".into(), WireKind::FramePeriod);
        wires.insert("sensor_statistics_grid".into(), WireKind::ImageGrid);
    }
    let plan = Plan {
        schema: 1,
        boot_entry: BootEntry::Rom,
        wires,
        nodes,
    };
    plan.validate()?;
    let mut plan_value = json!({
        "schema":1,
        "boot_entry":"rom",
        "wires": {
            "release":"software-pulse",
            "lens_aperture":"aperture",
            "lens_focus":"focus",
            "usb_cable":{"logic-level":inputs.usb.connected},
            "imaging_routes":"irq-routes",
            "image_outputs":"output-channels",
            "memory_outputs":"memory-outputs",
            "parameters":"parameters",
            "phase_edges":"phase-edges",
            "still_stream":"stream"
        },
        "nodes":serialized_nodes,
    });
    if browser.is_some() {
        plan_value["wires"]["sensor_exposure"] = json!("exposure");
        plan_value["wires"]["sensor_frame_period"] = json!("frame-period");
        plan_value["wires"]["sensor_statistics_grid"] = json!("image-grid");
    }
    let mut scene_value = scene(selection, scene_input)?;
    if let Some(browser) = browser {
        scene_value["files"] = json!({"virtual_card_files": {}});
        let frames = &browser.sensor.frames;
        scene_value["sensors"] =
            json!({frames: {"width": 1, "height": 1, "pixels": "rgb24", "asset": frames}});
    }
    Ok((plan, plan_value, scene_value))
}
