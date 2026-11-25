# Generated from the Rust catalog schemas.
from __future__ import annotations

from typing import Literal, Never, NotRequired, Required, TypedDict


type AProfileState = AProfileStateVariant0 | AProfileStateVariant1


class AProfileStateVariant0(TypedDict, total=False):
    caches: Required[CacheGeometry]
    configuration_base: Required[int]
    kind: Required[Literal["armv7a"]]


class AProfileStateVariant1(TypedDict, total=False):
    affinity: Required[int]
    counter_clock_id: Required[str]
    kind: Required[Literal["armv8a"]]
    midr_el1: Required[int]
    physical_timer_irq: Required[int]
    pmu_clock_id: Required[str]
    virtual_timer_irq: Required[int]


type AdcCondition = AdcConditionVariant0 | AdcConditionVariant1


class AdcConditionVariant0(TypedDict, total=False):
    kind: Required[Literal["rows_inactive"]]
    rows: Required[list[int]]


class AdcConditionVariant1(TypedDict, total=False):
    kind: Required[Literal["self_active"]]


class AdcEncoderCalibration(TypedDict, total=False):
    centers: Required[list[int]]
    direction: Required[int]
    edges_per_step: Required[int]
    hysteresis: Required[int]
    offset: Required[int]


type AdcInterval = AdcIntervalVariant0 | AdcIntervalVariant1


class AdcIntervalVariant0(TypedDict, total=False):
    kind: Required[Literal["literal"]]
    lower: Required[int]
    unless: NotRequired[list[AdcInterval]]
    upper: Required[int]
    when: NotRequired[AdcCondition | None]


class AdcIntervalVariant1(TypedDict, total=False):
    kind: Required[Literal["configuration_u16"]]
    lower: Required[int]
    unless: NotRequired[list[AdcInterval]]
    upper: Required[int]
    when: NotRequired[AdcCondition | None]


class AdcTerm(TypedDict, total=False):
    connection_id: Required[str]
    interval: Required[AdcInterval]


class AdditionalUiModes(TypedDict, total=False):
    play: Required[int]
    setup_play: Required[int]


class AddressDecoder(TypedDict, total=False):
    choices: Required[list[DecoderChoice]]
    id: Required[str]
    initial: Required[DecoderSelection]


class AddressIdentity(TypedDict, total=False):
    address: Required[int]
    sha256: Required[str]
    size: Required[int]


class AddressMapping(TypedDict, total=False):
    address: Required[int]
    core: Required[str]
    id: Required[str]
    permissions: Required[int]
    physical: Required[int]
    size: Required[RegionSize]


class AlarmInterval(TypedDict, total=False):
    seconds: Required[list[int]]


class AlarmRegisters(TypedDict, total=False):
    enable: Required[int]
    enable_value: Required[int]
    seconds: Required[list[int]]


class AperturePosition(TypedDict, total=False):
    f_number: Required[int]
    position: Required[int]


type ApplicationBlockInput = (
    ApplicationBlockInputVariant0
    | ApplicationBlockInputVariant1
    | ApplicationBlockInputVariant2
    | ApplicationBlockInputVariant3
    | ApplicationBlockInputVariant4
)


class ApplicationBlockInputVariant0(TypedDict, total=False):
    block_id: Required[str]
    kind: Required[Literal["adc"]]
    samples: Required[list[int]]


class ApplicationBlockInputVariant1(TypedDict, total=False):
    block_id: Required[str]
    kind: Required[Literal["gpio"]]
    levels: Required[list[int]]
    outputs: Required[list[GpioOutputWord]]
    pins: NotRequired[list[GpioPinLevel]]
    registers: NotRequired[list[GpioRegisterWord]]


class ApplicationBlockInputVariant2(TypedDict, total=False):
    block_id: Required[str]
    kind: Required[Literal["temperature"]]
    millicelsius: Required[int]


class ApplicationBlockInputVariant3(TypedDict, total=False):
    block_id: Required[str]
    idle_high: Required[bool]
    kind: Required[Literal["serial_line"]]


class ApplicationBlockInputVariant4(TypedDict, total=False):
    block_id: Required[str]
    erased_tail: NotRequired[int | None]
    kind: Required[Literal["flash"]]
    part_id: Required[str]
    presented_jedec_id: NotRequired[int | None]
    seeds: Required[list[FlashSeed]]
    source: Required[ApplicationFlashSource]


class ApplicationCommandImageInput(TypedDict, total=False):
    block_ids: Required[list[str]]
    luma: Required[int]


type ApplicationFlashSource = Literal["firmware_update"]


class ApplicationFrameInput(TypedDict, total=False):
    active_ticks: NotRequired[int | None]
    block_ids: Required[list[str]]
    luma: NotRequired[int | None]
    period_ticks: Required[int]


class ApplicationLadderInput(TypedDict, total=False):
    basis: Required[str]
    released: Required[int]
    wire: Required[LadderWire]


type ApplicationPartInput = (
    ApplicationPartInputVariant0
    | ApplicationPartInputVariant1
    | ApplicationPartInputVariant2
    | ApplicationPartInputVariant3
    | ApplicationPartInputVariant4
)


class ApplicationPartInputVariant0(TypedDict, total=False):
    kind: Required[Literal["register_words"]]
    part_id: Required[str]
    registers: Required[list[RegisterWord]]
    rtc_ticks_per_second: NotRequired[int | None]


class ApplicationPartInputVariant1(TypedDict, total=False):
    kind: Required[Literal["clock"]]
    part_id: Required[str]
    registers: NotRequired[list[RegisterWord]]
    seconds: Required[int]
    valid: Required[bool]


class ApplicationPartInputVariant2(TypedDict, total=False):
    high: Required[bool]
    kind: Required[Literal["idle_level"]]
    part_id: Required[str]


class ApplicationPartInputVariant3(TypedDict, total=False):
    block: Required[list[int]]
    idle_high: Required[bool]
    kind: Required[Literal["bis_revision"]]
    part_id: Required[str]
    revision: Required[list[int]]
    samples: Required[list[int]]


class ApplicationPartInputVariant4(TypedDict, total=False):
    acceleration: Required[list[int]]
    angular_rate: Required[list[int]]
    kind: Required[Literal["motion"]]
    part_id: Required[str]
    temperature: Required[int]


class ApplicationStatisticsInput(TypedDict, total=False):
    black_level: Required[int]
    block_id: Required[str]
    luma: Required[int]
    samples_per_cell: Required[int]
    signal_shift: Required[int]


type ApplicationTiming = (
    ApplicationTimingVariant0
    | ApplicationTimingVariant1
    | ApplicationTimingVariant2
    | ApplicationTimingVariant3
)


class ApplicationTimingVariant0(TypedDict, total=False):
    block_ids: Required[list[str]]
    kind: Required[Literal["delay"]]
    operation: Required[TimingOperation]
    ticks: Required[int]


class ApplicationTimingVariant1(TypedDict, total=False):
    block_id: Required[str]
    bytes_per_tick: Required[int]
    kind: Required[Literal["throughput"]]
    setup_ticks: Required[int]


class ApplicationTimingVariant2(TypedDict, total=False):
    block_ids: Required[list[str]]
    kind: Required[Literal["parent_clock"]]
    ticks_per_parent: Required[int]


class ApplicationTimingVariant3(TypedDict, total=False):
    erase_ticks: Required[int]
    kind: Required[Literal["storage_programming"]]
    page_ticks: Required[int]
    part_id: Required[str]


class AvcCodecOptions(TypedDict, total=False):
    backend: Required[VideoEncoderBackend]


class BankByteValue(TypedDict, total=False):
    input: Required[int]
    output: Required[int]


class BitmapLayout(TypedDict, total=False):
    data_bytes: Required[int]
    data_offset: Required[int]
    height_bytes: Required[int]
    height_offset: Required[int]
    kind_bytes: Required[int]
    kind_offset: Required[int]
    width_bytes: Required[int]
    width_offset: Required[int]


class BitmapResources(TypedDict, total=False):
    final_first_index: Required[int]
    final_table: Required[int]
    last_index: Required[int]
    last_record: Required[int]
    layout: Required[BitmapLayout]
    stride: Required[int]
    table: Required[int]


type BlockDevice = (
    BlockDeviceVariant0
    | BlockDeviceVariant1
    | BlockDeviceVariant2
    | BlockDeviceVariant3
    | BlockDeviceVariant4
    | BlockDeviceVariant5
    | BlockDeviceVariant6
    | BlockDeviceVariant7
    | BlockDeviceVariant8
    | BlockDeviceVariant9
    | BlockDeviceVariant10
    | BlockDeviceVariant11
    | BlockDeviceVariant12
    | BlockDeviceVariant13
    | BlockDeviceVariant14
    | BlockDeviceVariant15
    | BlockDeviceVariant16
    | BlockDeviceVariant17
    | BlockDeviceVariant18
    | BlockDeviceVariant19
    | BlockDeviceVariant20
    | BlockDeviceVariant21
    | BlockDeviceVariant22
    | BlockDeviceVariant23
    | BlockDeviceVariant24
    | BlockDeviceVariant25
    | BlockDeviceVariant26
    | BlockDeviceVariant27
    | BlockDeviceVariant28
    | BlockDeviceVariant29
    | BlockDeviceVariant30
    | BlockDeviceVariant31
    | BlockDeviceVariant32
    | BlockDeviceVariant33
    | BlockDeviceVariant34
    | BlockDeviceVariant35
    | BlockDeviceVariant36
    | BlockDeviceVariant37
    | BlockDeviceVariant38
    | BlockDeviceVariant39
    | BlockDeviceVariant40
    | BlockDeviceVariant41
    | BlockDeviceVariant42
    | BlockDeviceVariant43
    | BlockDeviceVariant44
    | BlockDeviceVariant45
    | BlockDeviceVariant46
    | BlockDeviceVariant47
    | BlockDeviceVariant48
    | BlockDeviceVariant49
    | BlockDeviceVariant50
    | BlockDeviceVariant51
    | BlockDeviceVariant52
    | BlockDeviceVariant53
    | BlockDeviceVariant54
    | BlockDeviceVariant55
    | BlockDeviceVariant56
    | BlockDeviceVariant57
    | BlockDeviceVariant58
    | BlockDeviceVariant59
    | BlockDeviceVariant60
    | BlockDeviceVariant61
    | BlockDeviceVariant62
    | BlockDeviceVariant63
    | BlockDeviceVariant64
    | BlockDeviceVariant65
    | BlockDeviceVariant66
    | BlockDeviceVariant67
    | BlockDeviceVariant68
    | BlockDeviceVariant69
    | BlockDeviceVariant70
    | BlockDeviceVariant71
    | BlockDeviceVariant72
    | BlockDeviceVariant73
    | BlockDeviceVariant74
    | BlockDeviceVariant75
    | BlockDeviceVariant76
    | BlockDeviceVariant77
    | BlockDeviceVariant78
    | BlockDeviceVariant79
    | BlockDeviceVariant80
    | BlockDeviceVariant81
    | BlockDeviceVariant82
    | BlockDeviceVariant83
    | BlockDeviceVariant84
    | BlockDeviceVariant85
    | BlockDeviceVariant86
    | BlockDeviceVariant87
    | BlockDeviceVariant88
    | BlockDeviceVariant89
    | BlockDeviceVariant90
    | BlockDeviceVariant91
    | BlockDeviceVariant92
    | BlockDeviceVariant93
    | BlockDeviceVariant94
    | BlockDeviceVariant95
)


class BlockDeviceVariant0(TypedDict, total=False):
    basis: NotRequired[str | None]
    channels: Required[int]
    model: Required[Literal["dma"]]
    native_dependencies: NotRequired[list[str]]
    request_mux_block_id: Required[str]
    timing: NotRequired[str | None]


class BlockDeviceVariant1(TypedDict, total=False):
    aperture_block_id: Required[str]
    aperture_size: NotRequired[int | None]
    basis: NotRequired[str | None]
    model: Required[Literal["serial_flash"]]
    native_dependencies: NotRequired[list[str]]
    timing: NotRequired[str | None]


class BlockDeviceVariant10(TypedDict, total=False):
    model: Required[Literal["control_bus"]]
    modes: Required[list[int]]
    offsets: Required[list[int]]


class BlockDeviceVariant11(TypedDict, total=False):
    model: Required[Literal["secondary_bus"]]


class BlockDeviceVariant12(TypedDict, total=False):
    model: Required[Literal["nand"]]


class BlockDeviceVariant13(TypedDict, total=False):
    model: Required[Literal["loader_dma"]]


class BlockDeviceVariant14(TypedDict, total=False):
    basis: NotRequired[str | None]
    model: Required[Literal["decompressor"]]


class BlockDeviceVariant15(TypedDict, total=False):
    basis: NotRequired[str | None]
    model: Required[Literal["usb"]]


class BlockDeviceVariant16(TypedDict, total=False):
    basis: NotRequired[str | None]
    model: Required[Literal["usb_phy"]]


class BlockDeviceVariant17(TypedDict, total=False):
    basis: Required[str]
    model: Required[Literal["sd_host_clock"]]
    settle_ticks: Required[int]


class BlockDeviceVariant18(TypedDict, total=False):
    basis: Required[str]
    model: Required[Literal["gpio"]]


class BlockDeviceVariant19(TypedDict, total=False):
    basis: Required[str]
    model: Required[Literal["adc"]]
    sample_ticks: Required[int]


class BlockDeviceVariant2(TypedDict, total=False):
    bank_bytes: Required[int]
    basis: NotRequired[str | None]
    model: Required[Literal["imaging_input_buffer"]]
    reset_ticks: NotRequired[int | None]
    sram_region_id: Required[str]


class BlockDeviceVariant20(TypedDict, total=False):
    basis: Required[str]
    model: Required[Literal["dmp"]]


class BlockDeviceVariant21(TypedDict, total=False):
    basis: NotRequired[str | None]
    model: Required[Literal["movdc"]]
    reset_ticks: Required[int]


class BlockDeviceVariant22(TypedDict, total=False):
    ack_ticks: Required[int]
    basis: Required[str]
    model: Required[Literal["control_request"]]
    request_offset: Required[int]


class BlockDeviceVariant23(TypedDict, total=False):
    ack_ticks: Required[int]
    basis: Required[str]
    model: Required[Literal["control_group"]]
    request_offset: Required[int]


class BlockDeviceVariant24(TypedDict, total=False):
    basis: Required[str]
    model: Required[Literal["io_groups"]]


class BlockDeviceVariant25(TypedDict, total=False):
    basis: Required[str]
    model: Required[Literal["spi"]]
    parent_clock_ticks: Required[int]


class BlockDeviceVariant26(TypedDict, total=False):
    basis: Required[str]
    clock: Required[str]
    layout: Required[HardwareTimerLayout]
    model: Required[Literal["interval_timer"]]


class BlockDeviceVariant27(TypedDict, total=False):
    basis: Required[str]
    commands: NotRequired[list[HardwareRegisterBlockCommand]]
    fixed_words: NotRequired[list[HardwareRegisterFixedWord]]
    frames: NotRequired[BlockRegisterBlockFrames | None]
    idle_bits: Required[list[HardwareRegisterIdleBits]]
    job: NotRequired[BlockRegisterBlockJob | None]
    model: Required[Literal["register_block"]]
    order: NotRequired[int | None]
    reads_back: NotRequired[bool]


class BlockDeviceVariant28(TypedDict, total=False):
    basis: Required[str]
    bytes_per_tick: Required[int]
    model: Required[Literal["pl330"]]
    setup_ticks: Required[int]
    timing: Required[str]


class BlockDeviceVariant29(TypedDict, total=False):
    basis: Required[str]
    model: Required[Literal["serial_port"]]


class BlockDeviceVariant3(TypedDict, total=False):
    basis: NotRequired[str | None]
    clear_bits: NotRequired[list[HardwareDisplayStatusBits]]
    frame_basis: NotRequired[str | None]
    handshake_ticks: NotRequired[int | None]
    model: Required[Literal["display"]]
    order: NotRequired[int | None]
    period_ticks: NotRequired[int | None]
    picture_layout: NotRequired[PictureLayout | None]
    pixel_clock_ticks: NotRequired[int | None]
    scanout: NotRequired[HardwareDisplayScanout | None]
    set_bits: NotRequired[list[HardwareDisplayStatusBits]]
    timing: NotRequired[HardwareDisplayTimingBanks | None]
    video_layout: NotRequired[VideoLayout | None]


class BlockDeviceVariant30(TypedDict, total=False):
    basis: Required[str]
    direction: NotRequired[JpegDirection | None]
    memory_type_block_id: NotRequired[str | None]
    model: Required[Literal["jpeg_codec"]]
    run_ticks: Required[int]


class BlockDeviceVariant31(TypedDict, total=False):
    basis: Required[str]
    model: Required[Literal["face_detection"]]
    run_ticks: Required[int]


class BlockDeviceVariant32(TypedDict, total=False):
    address_high: NotRequired[int | None]
    basis: Required[str]
    capabilities: Required[int]
    capabilities_2: Required[int]
    clock_ticks: Required[int]
    dma_windows: NotRequired[list[SdDmaWindow]]
    model: Required[Literal["sd_host"]]
    slot_and_version: Required[int]
    transfer_ticks: Required[int]
    vendor_offsets: NotRequired[list[int]]


class BlockDeviceVariant33(TypedDict, total=False):
    basis: Required[str]
    command_ticks: Required[int]
    model: Required[Literal["xpp_usb"]]
    transaction_ticks: Required[int]


class BlockDeviceVariant34(TypedDict, total=False):
    basis: Required[str]
    clear_bits: Required[list[HardwareDisplayStatusBits]]
    handshake_ticks: NotRequired[int | None]
    model: Required[Literal["display_output"]]
    order: NotRequired[int | None]
    scanout: NotRequired[HardwareDisplayScanout | None]
    set_bits: Required[list[HardwareDisplayStatusBits]]


class BlockDeviceVariant35(TypedDict, total=False):
    basis: Required[str]
    lock_ticks: Required[int]
    model: Required[Literal["pll_clocks"]]


class BlockDeviceVariant36(TypedDict, total=False):
    basis: Required[str]
    model: Required[Literal["imaging_control"]]
    readout: NotRequired[BlockSensorReadout | None]


class BlockDeviceVariant37(TypedDict, total=False):
    basis: Required[str]
    model: Required[Literal["imaging_statistics"]]
    period_ticks: Required[int]


class BlockDeviceVariant38(TypedDict, total=False):
    basis: Required[str]
    model: Required[Literal["xp5_imaging_statistics"]]


class BlockDeviceVariant39(TypedDict, total=False):
    basis: Required[str]
    model: Required[Literal["hdmi_tx"]]


class BlockDeviceVariant4(TypedDict, total=False):
    model: Required[Literal["serial_dma"]]
    receive_request: Required[int]
    transmit_request: Required[int]


class BlockDeviceVariant40(TypedDict, total=False):
    basis: Required[str]
    interrupts: Required[int]
    model: Required[Literal["frame_sync"]]
    period_ticks: Required[int]
    sources: Required[list[BlockFrameSource]]


class BlockDeviceVariant41(TypedDict, total=False):
    basis: Required[str]
    model: Required[Literal["reset_gates"]]


class BlockDeviceVariant42(TypedDict, total=False):
    basis: Required[str]
    model: Required[Literal["dram_controller"]]


class BlockDeviceVariant43(TypedDict, total=False):
    basis: Required[str]
    model: Required[Literal["power_control"]]


class BlockDeviceVariant44(TypedDict, total=False):
    basis: Required[str]
    byte_ticks: Required[int]
    model: Required[Literal["byte_i2c"]]


class BlockDeviceVariant45(TypedDict, total=False):
    basis: Required[str]
    clock_hz: Required[int]
    fifo_depth: Required[int]
    model: Required[Literal["i2c"]]
    tick_hz: Required[int]


class BlockDeviceVariant46(TypedDict, total=False):
    basis: Required[str]
    clock_ticks: Required[int]
    model: Required[Literal["xpp_sd"]]
    register_offset: Required[int]
    reset_channel: Required[int]
    reset_ticks: Required[int]
    transfer_ticks: Required[int]


class BlockDeviceVariant47(TypedDict, total=False):
    basis: Required[str]
    model: Required[Literal["h264_encoder"]]
    run_ticks: Required[int]


class BlockDeviceVariant48(TypedDict, total=False):
    basis: NotRequired[str | None]
    engine: Required[StillEngineKind]
    evidence: NotRequired[list[Evidence]]
    model: Required[Literal["still_engine"]]
    run_ticks: Required[int]


class BlockDeviceVariant49(TypedDict, total=False):
    basis: NotRequired[str | None]
    evidence: NotRequired[list[Evidence]]
    memory_types_block_id: Required[str]
    model: Required[Literal["filter_engine"]]
    run_ticks: Required[int]


class BlockDeviceVariant5(TypedDict, total=False):
    companion_irq_base: Required[int]
    model: Required[Literal["companion_dma"]]


class BlockDeviceVariant50(TypedDict, total=False):
    basis: NotRequired[str | None]
    evidence: NotRequired[list[Evidence]]
    model: Required[Literal["dsp"]]
    run_ticks: Required[int]


class BlockDeviceVariant51(TypedDict, total=False):
    basis: NotRequired[str | None]
    channels: Required[int]
    counter_block_id: Required[str]
    model: Required[Literal["countdown_timer"]]


class BlockDeviceVariant52(TypedDict, total=False):
    basis: NotRequired[str | None]
    model: Required[Literal["usb_condition"]]


class BlockDeviceVariant53(TypedDict, total=False):
    basis: Required[str]
    focus_capture: Required[bool]
    model: Required[Literal["xp5_timer"]]


class BlockDeviceVariant54(TypedDict, total=False):
    basis: Required[str]
    model: Required[Literal["xp5_timer_extended"]]


class BlockDeviceVariant55(TypedDict, total=False):
    basis: Required[str]
    model: Required[Literal["serial_flash_aperture"]]


class BlockDeviceVariant56(TypedDict, total=False):
    basis: Required[str]
    model: Required[Literal["serial_flash_controller"]]


class BlockDeviceVariant57(TypedDict, total=False):
    basis: Required[str]
    model: Required[Literal["flash_storage_dma"]]


class BlockDeviceVariant58(TypedDict, total=False):
    basis: Required[str]
    configuration_word: NotRequired[int | None]
    model: Required[Literal["gates"]]


class BlockDeviceVariant59(TypedDict, total=False):
    basis: Required[str]
    model: Required[Literal["clock_banks"]]


class BlockDeviceVariant6(TypedDict, total=False):
    basis: NotRequired[str | None]
    cpu_interface_block_id: Required[str]
    lines: Required[int]
    model: Required[Literal["gic_v2"]]


class BlockDeviceVariant60(TypedDict, total=False):
    basis: Required[str]
    model: Required[Literal["panel_pll"]]


class BlockDeviceVariant61(TypedDict, total=False):
    a53_irq_base: NotRequired[int | None]
    basis: Required[str]
    companion_irq_base: NotRequired[int | None]
    model: Required[Literal["general_dma"]]


class BlockDeviceVariant62(TypedDict, total=False):
    basis: Required[str]
    bytes_per_tick: Required[int]
    model: Required[Literal["audio_dma"]]
    setup_ticks: Required[int]


class BlockDeviceVariant63(TypedDict, total=False):
    basis: Required[str]
    model: Required[Literal["xp5_gpio"]]
    pad_model: NotRequired[GpioPadModel | None]
    port: NotRequired[int | None]


class BlockDeviceVariant64(TypedDict, total=False):
    basis: Required[str]
    model: Required[Literal["xp5_adc"]]


class BlockDeviceVariant65(TypedDict, total=False):
    basis: Required[str]
    model: Required[Literal["temperature"]]


class BlockDeviceVariant66(TypedDict, total=False):
    basis: Required[str]
    model: Required[Literal["interprocessor_mailbox"]]


class BlockDeviceVariant67(TypedDict, total=False):
    basis: Required[str]
    model: Required[Literal["audio_serial"]]
    reset_offset: Required[int]


class BlockDeviceVariant68(TypedDict, total=False):
    basis: Required[str]
    commit_ticks: Required[int]
    model: Required[Literal["memory_configuration"]]


class BlockDeviceVariant69(TypedDict, total=False):
    basis: Required[str]
    connected_level: Required[int]
    model: Required[Literal["cable_status"]]


class BlockDeviceVariant7(TypedDict, total=False):
    basis: NotRequired[str | None]
    lines: Required[int]
    model: Required[Literal["gic_v3"]]
    redistributor_block_id: Required[str]
    redistributor_stride: Required[int]


class BlockDeviceVariant70(TypedDict, total=False):
    basis: Required[str]
    model: Required[Literal["ddr_phy"]]


class BlockDeviceVariant71(TypedDict, total=False):
    basis: Required[str]
    model: Required[Literal["sd_interface"]]


class BlockDeviceVariant72(TypedDict, total=False):
    basis: Required[str]
    clock_ticks: Required[int]
    model: Required[Literal["sdhci_uhs2"]]


class BlockDeviceVariant73(TypedDict, total=False):
    basis: Required[str]
    model: Required[Literal["display_output_link"]]


class BlockDeviceVariant74(TypedDict, total=False):
    basis: Required[str]
    model: Required[Literal["renderer"]]


class BlockDeviceVariant75(TypedDict, total=False):
    basis: Required[str]
    model: Required[Literal["imaging_phy"]]


class BlockDeviceVariant76(TypedDict, total=False):
    basis: Required[str]
    model: Required[Literal["imaging_input"]]


class BlockDeviceVariant77(TypedDict, total=False):
    basis: Required[str]
    model: Required[Literal["imaging_resize"]]


class BlockDeviceVariant78(TypedDict, total=False):
    basis: Required[str]
    model: Required[Literal["imaging_paf"]]


class BlockDeviceVariant79(TypedDict, total=False):
    basis: Required[str]
    focus_statistics: NotRequired[bool]
    model: Required[Literal["imaging_preview"]]


class BlockDeviceVariant8(TypedDict, total=False):
    basis: Required[str]
    command_ticks: Required[int]
    model: Required[Literal["sensor_command"]]
    reset_ticks: NotRequired[int | None]


class BlockDeviceVariant80(TypedDict, total=False):
    basis: Required[str]
    model: Required[Literal["imaging_rifp_io"]]


class BlockDeviceVariant81(TypedDict, total=False):
    basis: Required[str]
    model: Required[Literal["imaging_rifp"]]


class BlockDeviceVariant82(TypedDict, total=False):
    basis: Required[str]
    handshake: NotRequired[CompletionHandshake | None]
    model: Required[Literal["imaging_tnr"]]


class BlockDeviceVariant83(TypedDict, total=False):
    basis: Required[str]
    handshake: NotRequired[CompletionHandshake | None]
    model: Required[Literal["imaging_fcnv"]]


class BlockDeviceVariant84(TypedDict, total=False):
    basis: Required[str]
    handshake: NotRequired[CompletionHandshake | None]
    model: Required[Literal["imaging_filter_scaler"]]


class BlockDeviceVariant85(TypedDict, total=False):
    basis: Required[str]
    handshake: NotRequired[CompletionHandshake | None]
    model: Required[Literal["imaging_rzl2"]]


class BlockDeviceVariant86(TypedDict, total=False):
    basis: Required[str]
    handshake: NotRequired[CompletionHandshake | None]
    model: Required[Literal["imaging_resz"]]


class BlockDeviceVariant87(TypedDict, total=False):
    basis: Required[str]
    model: Required[Literal["input_capture"]]
    transmit_requests: NotRequired[list[CaptureTransmit]]


class BlockDeviceVariant88(TypedDict, total=False):
    basis: Required[str]
    model: Required[Literal["pl011"]]


class BlockDeviceVariant89(TypedDict, total=False):
    basis: Required[str]
    companion_irq_line: NotRequired[int | None]
    model: Required[Literal["pl022"]]
    receive_request: NotRequired[int | None]
    source_hz: Required[int]
    ticks_per_second: Required[int]
    transmit_request: NotRequired[int | None]


class BlockDeviceVariant9(TypedDict, total=False):
    basis: Required[str]
    model: Required[Literal["imaging_receiver"]]


class BlockDeviceVariant90(TypedDict, total=False):
    ack: NotRequired[CompletionAck | None]
    basis: Required[str]
    completion: NotRequired[list[CompletionWrite]]
    delay_ticks: Required[int]
    fixed_words: NotRequired[list[HardwareRegisterFixedWord]]
    irq_mask: NotRequired[CompletionIrqMask | None]
    model: Required[Literal["completion"]]
    starts: NotRequired[list[CompletionStart]]


class BlockDeviceVariant91(TypedDict, total=False):
    basis: Required[str]
    model: Required[Literal["video_codec"]]


class BlockDeviceVariant92(TypedDict, total=False):
    basis: Required[str]
    direct_flash_straps: Required[int]
    model: Required[Literal["boot_control"]]


class BlockDeviceVariant93(TypedDict, total=False):
    basis: Required[str]
    model: Required[Literal["imaging_buffer"]]


class BlockDeviceVariant94(TypedDict, total=False):
    basis: Required[str]
    handshake: NotRequired[CompletionHandshake | None]
    model: Required[Literal["imaging_dsp"]]


class BlockDeviceVariant95(TypedDict, total=False):
    basis: Required[str]
    model: Required[Literal["unmodeled_bus_drain"]]


class BlockFrameSource(TypedDict, total=False):
    status_bit: Required[int]


class BlockOverride(TypedDict, total=False):
    block_id: Required[str]
    device: Required[BlockDevice]


class BlockRegister(TypedDict, total=False):
    access: Required[RegisterAccess]
    fields: NotRequired[list[RegisterField]]
    name: Required[str]
    offset: Required[int]
    reads: NotRequired[RegisterReads | None]
    reset: NotRequired[int | None]
    width_bytes: Required[int]
    write_mask: NotRequired[int | None]


class BlockRegisterBlockFrames(TypedDict, total=False):
    period_ticks: Required[int]
    start: Required[HardwareRegisterIdleBits]
    stopped: Required[HardwareRegisterIdleBits]


class BlockRegisterBlockJob(TypedDict, total=False):
    start: Required[HardwareRegisterIdleBits]
    status: NotRequired[HardwareRegisterIdleBits | None]
    stop: NotRequired[HardwareRegisterIdleBits | None]
    ticks: Required[int]


class BlockSensorReadout(TypedDict, total=False):
    basis: Required[str]
    offset: Required[int]
    starts_on_frame: NotRequired[bool]
    ticks: Required[int]


type BlockSignal = Literal["detect", "write_protect", "data_ready", "clock"]


Body = TypedDict(
    "Body",
    {
        "$schema": Required['Literal["https://fujimod.com/schemas/body.schema.json"]'],
        "absent_blocks": NotRequired["list[str]"],
        "card_slot": NotRequired["CardSlotAssembly | None"],
        "connections": NotRequired["list[BodyConnection]"],
        "control_bus_chips": NotRequired["list[ControlBusChip]"],
        "display": NotRequired["DisplayAssembly | None"],
        "dram_bytes": NotRequired["int | None"],
        "hardware": NotRequired["BodyHardware | None"],
        "id": Required["str"],
        "lens": NotRequired["LensAssembly | None"],
        "microcontroller": NotRequired["MicrocontrollerAssembly | None"],
        "nand": NotRequired["NandGeometry | None"],
        "parts": NotRequired["list[str]"],
        "power_control": NotRequired["PowerControlAssembly | None"],
        "power_on_ram_fill": NotRequired["PowerOnFill | None"],
        "ram_regions": NotRequired["list[BodyRamRegion]"],
        "schema": Required["int"],
        "sensor": NotRequired["SensorAssembly | None"],
        "soc_id": Required["str"],
        "wiring": NotRequired["list[BodyWiring]"],
    },
    total=False,
)


class BodyConnection(TypedDict, total=False):
    evidence: NotRequired[Evidence | None]
    id: Required[str]
    kind: Required[ConnectionKind]
    signals: NotRequired[list[ConnectionSignal]]


class BodyHardware(TypedDict, total=False):
    bus_drain: NotRequired[list[BusDrain]]
    gpio_feedback: NotRequired[list[GpioFeedback]]
    keypad_samples: NotRequired[dict[str, int]]
    lens_mount: NotRequired[LensMount | None]
    profiles: NotRequired[dict[str, BodyProfile]]
    serial_attachments: NotRequired[list[SerialAttachment]]
    stabilizer: NotRequired[StabilizerMount | None]
    thermal_connection_ids: NotRequired[list[str]]


class BodyProfile(TypedDict, total=False):
    attachments: NotRequired[HardwarePartAssemblies | None]
    block_overrides: NotRequired[list[BlockOverride]]
    card_contacts: NotRequired[HardwareCardContacts | None]
    connections: NotRequired[list[HardwareConnectionSupport] | None]
    construction: NotRequired[ConstructionProfile | None]
    control_bus_notes: NotRequired[str | None]
    control_bus_order: NotRequired[list[str] | None]
    controllers: NotRequired[HardwareControllerModels | None]
    counter_feedback: NotRequired[str | None]
    dram_basis: Required[str]
    encoders: NotRequired[list[HardwareEncoder] | None]
    face_register_mode: NotRequired[str | None]
    graph: NotRequired[DeviceGraph | None]
    imaging: NotRequired[HardwareImaging | None]
    interrupt_notes: NotRequired[str | None]
    lens_notes: NotRequired[str | None]
    limits: NotRequired[list[str] | None]
    panel_notes: NotRequired[str | None]
    pin_interrupts: NotRequired[list[HardwarePinInterrupt] | None]
    preview_activity: NotRequired[HardwarePreviewActivity | None]
    touch: NotRequired[HardwareTouch | None]


class BodyRamRegion(TypedDict, total=False):
    base: Required[int]
    bytes: Required[int]
    id: Required[str]
    kind: Required[MemoryKind]


class BodyWiring(TypedDict, total=False):
    address: NotRequired[int | None]
    signal: Required[WireSignal]
    source_id: Required[str]
    target_id: Required[str]


class BootCombination(TypedDict, total=False):
    hold: Required[list[BootHold]]
    usb: NotRequired[bool | None]


class BootFlag(TypedDict, total=False):
    address: Required[int]
    mask: Required[int]


class BootHold(TypedDict, total=False):
    id: Required[str]
    stage: NotRequired[str | None]


class BootRomIdentity(TypedDict, total=False):
    address: Required[int]
    content: Required[ContentIdentity]


class BoundPosition(TypedDict, total=False):
    asserted: NotRequired[bool | None]
    id: Required[str]
    key: NotRequired[InputBinding | None]


class BulkDma(TypedDict, total=False):
    address_alignment: Required[int]
    length_granule: Required[int]


class BulkRange(TypedDict, total=False):
    address: Required[int]
    size: Required[int]


type BulkSource = (
    BulkSourceVariant0 | BulkSourceVariant1 | BulkSourceVariant2 | BulkSourceVariant3
)


class BulkSourceVariant0(TypedDict, total=False):
    base: Required[int]
    capacities: Required[list[int]]
    data_offset: Required[int]
    mechanism: Required[Literal["direct_query"]]
    query_base: Required[int]
    tag: Required[int]
    wrapping: Required[bool]


class BulkSourceVariant1(TypedDict, total=False):
    base: Required[int]
    callback_offset: Required[int]
    callback_table: Required[int]
    capacities: Required[list[int]]
    mechanism: Required[Literal["query"]]
    source_state: Required[int]
    stock_callback: Required[int]
    tag: Required[int]
    wrapping: Required[bool]


class BulkSourceVariant2(TypedDict, total=False):
    capacity: Required[int]
    mechanism: Required[Literal["pattern"]]
    registered_buffer: Required[int]
    source_state: Required[int]


class BulkSourceVariant3(TypedDict, total=False):
    capacity: Required[int]
    configuration_bytes: Required[int]
    configuration_pointer: Required[int]
    mechanism: Required[Literal["pattern_zero_base"]]
    registered_buffer: Required[int]
    source_state: Required[int]
    system_mode_address: Required[int]


class BusDrain(TypedDict, total=False):
    address: Required[int]
    basis: Required[str]
    processor_ids: Required[list[str]]


class CacheGeometry(TypedDict, total=False):
    l1_data_bytes: Required[int]
    l1_instruction_bytes: Required[int]
    l2_bytes: Required[int]


class CacheSynchronization(TypedDict, total=False):
    data_barrier: Required[int]
    data_clean: Required[int]
    instruction_barrier: Required[int]
    instruction_invalidate: Required[int]


class CaptureScope(TypedDict, total=False):
    basis: Required[str]
    name: Required[str]
    ranges: Required[list[NamedRange]]


class CaptureTransmit(TypedDict, total=False):
    channel: Required[int]
    request: Required[int]


class CardSlotAssembly(TypedDict, total=False):
    connection_id: Required[str]
    controller_id: NotRequired[str | None]
    detect_connection_id: Required[str]
    part_id: Required[str]
    write_protect_connection_id: Required[str]


class ClockDomain(TypedDict, total=False):
    frequency_hz: NotRequired[int | None]
    id: Required[str]
    parent_id: NotRequired[str | None]


class ClockIo(TypedDict, total=False):
    absent_reason: NotRequired[str | None]
    bank_pointer: Required[int]
    live_reader: NotRequired[int | None]
    read_order: Required[list[int]]
    reader: Required[int]
    register_reader: Required[int]
    register_writer: NotRequired[int | None]
    registers: Required[ClockRegisters]
    start_value: NotRequired[int | None]
    stop_value: NotRequired[int | None]
    transaction: NotRequired[int | None]
    writer: NotRequired[int | None]


class ClockRegisters(TypedDict, total=False):
    control: NotRequired[int | None]
    high: Required[int]
    low: Required[int]


class CodeCheck(TypedDict, total=False):
    address: Required[int]
    name: Required[str]
    sha256: Required[str]
    size: Required[int]


class CompletionAck(TypedDict, total=False):
    bits: NotRequired[int | None]
    mode: Required[CompletionAckMode]
    offset: Required[int]
    status_offset: NotRequired[int | None]


type CompletionAckMode = Literal["w1c"]


class CompletionHandshake(TypedDict, total=False):
    ack: NotRequired[CompletionAck | None]
    completion: Required[list[CompletionWrite]]
    irq_mask: NotRequired[CompletionIrqMask | None]
    starts: Required[list[CompletionStart]]


class CompletionIrqMask(TypedDict, total=False):
    bit: Required[int]
    offset: Required[int]
    set_masks: NotRequired[bool]


class CompletionStart(TypedDict, total=False):
    mask: Required[int]
    offset: Required[int]


class CompletionWrite(TypedDict, total=False):
    clear: NotRequired[int | None]
    offset: Required[int]
    set: NotRequired[int | None]


type Configuration = ConfigurationVariant0 | ConfigurationVariant1


class ConfigurationCheck(TypedDict, total=False):
    offset: Required[int]
    value: Required[int]


type ConfigurationLocation = (
    ConfigurationLocationVariant0 | ConfigurationLocationVariant1
)


class ConfigurationLocationVariant0(TypedDict, total=False):
    address: Required[int]
    kind: Required[Literal["pointer"]]


class ConfigurationLocationVariant1(TypedDict, total=False):
    address: Required[int]
    kind: Required[Literal["direct"]]


class ConfigurationOverride(TypedDict, total=False):
    delay_keys: Required[list[int]]
    enable_key: Required[int]


class ConfigurationVariant0(TypedDict, total=False):
    address: Required[int]
    mechanism: Required[Literal["pointer"]]


class ConfigurationVariant1(TypedDict, total=False):
    address: Required[int]
    mechanism: Required[Literal["fixed"]]


class ConfigurationWindow(TypedDict, total=False):
    basis: Required[str]
    bytes: Required[int]
    flash_offset: Required[int]
    sha256: Required[str]


type ConnectionKind = Literal[
    "control",
    "usb",
    "hdmi",
    "sd-card",
    "remote",
    "hot-shoe",
    "external-power",
    "battery-door",
    "sensor",
    "lens",
    "display",
    "microcontroller",
    "power-control",
]


type ConnectionSignal = (
    ConnectionSignalVariant0
    | ConnectionSignalVariant1
    | ConnectionSignalVariant2
    | ConnectionSignalVariant3
    | ConnectionSignalVariant4
    | ConnectionSignalVariant5
    | ConnectionSignalVariant6
)


class ConnectionSignalVariant0(TypedDict, total=False):
    active_level: Required[bool]
    kind: Required[Literal["gpio"]]
    pin: Required[int]
    port: Required[int]


class ConnectionSignalVariant1(TypedDict, total=False):
    channel: Required[int]
    kind: Required[Literal["adc"]]
    threshold: NotRequired[int | None]


class ConnectionSignalVariant2(TypedDict, total=False):
    block_id: Required[str]
    kind: Required[Literal["block"]]
    signal: Required[BlockSignal]


class ConnectionSignalVariant3(TypedDict, total=False):
    address: Required[int]
    block_id: Required[str]
    channel: Required[int]
    kind: Required[Literal["serial"]]


class ConnectionSignalVariant4(TypedDict, total=False):
    controller_id: Required[str]
    kind: Required[Literal["interrupt"]]
    line: Required[int]


class ConnectionSignalVariant5(TypedDict, total=False):
    block_id: Required[str]
    channel: Required[int]
    kind: Required[Literal["countdown_timer"]]
    rest_phase: Required[int]


class ConnectionSignalVariant6(TypedDict, total=False):
    block_id: Required[str]
    kind: Required[Literal["serial_status"]]
    mask: Required[int]
    register: Required[int]


class ConstructionActivity(TypedDict, total=False):
    gpio: Required[str]
    selection: Required[ConstructionSelection]


type ConstructionClock = ConstructionClockVariant0 | ConstructionClockVariant1


class ConstructionClockVariant0(TypedDict, total=False):
    kind: Required[Literal["encoder_stages"]]
    ticks_per_quantum: Required[int]


class ConstructionClockVariant1(TypedDict, total=False):
    kind: Required[Literal["microstep_stages"]]
    ticks_per_quantum: Required[int]


class ConstructionCore(TypedDict, total=False):
    core: Required[int]
    line: Required[int]


class ConstructionCounters(TypedDict, total=False):
    channel: Required[int]
    divider: Required[int]


type ConstructionEmpty = dict[str, Never]


type ConstructionEndpoint = (
    ConstructionEndpointVariant0
    | ConstructionEndpointVariant1
    | ConstructionEndpointVariant2
)


class ConstructionEndpointVariant0(TypedDict, total=False):
    completion: Required[list[ConstructionIrqs]]
    focus: Required[ConstructionFocus]
    gpio: Required[str]
    kind: Required[Literal["lens-unit"]]
    optics: Required[ConstructionOptics]
    options: Required[ConstructionLensunitOptions]
    pulses: Required[str]


class ConstructionEndpointVariant1(TypedDict, total=False):
    kind: Required[Literal["panel"]]
    options: Required[ConstructionPanelOptions]


class ConstructionEndpointVariant2(TypedDict, total=False):
    kind: Required[Literal["registers"]]
    options: Required[ConstructionRegistersOptions]


class ConstructionFocus(TypedDict, total=False):
    infinity_position: Required[int]
    output: Required[str]
    steps_per_mm: Required[int]


class ConstructionHardware(TypedDict, total=False):
    base: Required[int]
    irq: Required[int]
    parts: Required[ConstructionParts]


class ConstructionHome(TypedDict, total=False):
    pin: Required[ConstructionPin]
    side: Required[str]
    threshold: Required[int]


class ConstructionIrqs(TypedDict, total=False):
    Core: Required[ConstructionCore]


class ConstructionIrqsIrqs(TypedDict, total=False):
    edge: Required[str]
    outputs: Required[list[ConstructionIrqs]]
    pin: Required[ConstructionPin]


class ConstructionLensunitOptions(TypedDict, total=False):
    encoder: NotRequired[str | None]
    motors: Required[list[ConstructionMotors]]
    protocol: Required[ConstructionProtocol]
    zoom: NotRequired[str | None]


type ConstructionModel = (
    ConstructionModelVariant0
    | ConstructionModelVariant1
    | ConstructionModelVariant2
    | ConstructionModelVariant3
    | ConstructionModelVariant4
    | ConstructionModelVariant5
    | ConstructionModelVariant6
    | ConstructionModelVariant7
    | ConstructionModelVariant8
    | ConstructionModelVariant9
    | ConstructionModelVariant10
    | ConstructionModelVariant11
    | ConstructionModelVariant12
    | ConstructionModelVariant13
    | ConstructionModelVariant14
    | ConstructionModelVariant15
    | ConstructionModelVariant16
    | ConstructionModelVariant17
    | ConstructionModelVariant18
    | ConstructionModelVariant19
    | ConstructionModelVariant20
    | ConstructionModelVariant21
    | ConstructionModelVariant22
    | ConstructionModelVariant23
    | ConstructionModelVariant24
    | ConstructionModelVariant25
    | ConstructionModelVariant26
    | ConstructionModelVariant27
    | ConstructionModelVariant28
    | ConstructionModelVariant29
    | ConstructionModelVariant30
    | ConstructionModelVariant31
    | ConstructionModelVariant32
    | ConstructionModelVariant33
    | ConstructionModelVariant34
    | ConstructionModelVariant35
    | ConstructionModelVariant36
    | ConstructionModelVariant37
    | ConstructionModelVariant38
    | ConstructionModelVariant39
    | ConstructionModelVariant40
    | ConstructionModelVariant41
)


class ConstructionModelVariant0(TypedDict, total=False):
    kind: Required[Literal["adc"]]


class ConstructionModelVariant1(TypedDict, total=False):
    kind: Required[Literal["block-98000"]]
    wiring: Required[ConstructionWiring]


class ConstructionModelVariant10(TypedDict, total=False):
    base: Required[int]
    core: Required[int]
    kind: Required[Literal["exr2-interrupt-controller"]]
    routes: Required[str]
    software: Required[list[ConstructionSoftware]]


class ConstructionModelVariant11(TypedDict, total=False):
    irqs: Required[list[ConstructionIrqs]]
    kind: Required[Literal["exr2-pulse"]]


class ConstructionModelVariant12(TypedDict, total=False):
    controllers: Required[list[str]]
    kind: Required[Literal["exr2-system-timers"]]


class ConstructionModelVariant13(TypedDict, total=False):
    activity: Required[ConstructionActivity]
    images: Required[str]
    kind: Required[Literal["external-frames"]]
    statistics: Required[str]


class ConstructionModelVariant14(TypedDict, total=False):
    kind: Required[Literal["face-detector"]]


class ConstructionModelVariant15(TypedDict, total=False):
    frames: Required[str]
    kind: Required[Literal["fe80-processing"]]
    routes: Required[str]
    transport: Required[ConstructionEmpty]


class ConstructionModelVariant16(TypedDict, total=False):
    kind: Required[Literal["formatter"]]


class ConstructionModelVariant17(TypedDict, total=False):
    irqs: Required[list[ConstructionIrqsIrqs]]
    kind: Required[Literal["gpio"]]


class ConstructionModelVariant18(TypedDict, total=False):
    hot_plug: Required[ConstructionIrqs]
    kind: Required[Literal["hdmi"]]
    sink: Required[str]


class ConstructionModelVariant19(TypedDict, total=False):
    kind: Required[Literal["hdmi-sink"]]


class ConstructionModelVariant2(TypedDict, total=False):
    kind: Required[Literal["block11000"]]


class ConstructionModelVariant20(TypedDict, total=False):
    bridge: NotRequired[str | None]
    cable: NotRequired[str | None]
    controllers: Required[list[str]]
    hardware: Required[ConstructionHardware]
    kind: Required[Literal["i2c"]]


class ConstructionModelVariant21(TypedDict, total=False):
    irq: Required[ConstructionCore]
    kind: Required[Literal["image-combination"]]


class ConstructionModelVariant22(TypedDict, total=False):
    edges: Required[str]
    frame_period: NotRequired[str | None]
    kind: Required[Literal["image-input"]]
    memory_outputs: Required[str]
    parameters: Required[str]
    phase_irqs: Required[list[ConstructionCore]]
    raw_irq: Required[ConstructionCore]
    readback_irq: Required[ConstructionCore]
    sensor: NotRequired[str | None]
    shutter_output: NotRequired[str | None]
    statistics_grid: NotRequired[str | None]


class ConstructionModelVariant23(TypedDict, total=False):
    base: Required[int]
    controllers: Required[list[str]]
    kind: Required[Literal["lz"]]


class ConstructionModelVariant24(TypedDict, total=False):
    base: Required[int]
    kind: Required[Literal["memory-dma"]]
    routes: Required[str]


class ConstructionModelVariant25(TypedDict, total=False):
    kind: Required[Literal["nand"]]
    medium: Required[str]


class ConstructionModelVariant26(TypedDict, total=False):
    base: Required[int]
    channel: Required[int]
    channels: Required[str]
    frames: Required[str]
    irq: Required[int]
    kind: Required[Literal["output-channel"]]
    memory_outputs: Required[str]
    routes: Required[str]


class ConstructionModelVariant27(TypedDict, total=False):
    irq: Required[ConstructionCore]
    kind: Required[Literal["avc-codec"]]
    options: Required[AvcCodecOptions]


class ConstructionModelVariant28(TypedDict, total=False):
    irq: Required[ConstructionCore]
    kind: Required[Literal["image-transfer"]]


class ConstructionModelVariant29(TypedDict, total=False):
    bank: Required[str]
    kind: Required[Literal["parameters"]]


class ConstructionModelVariant3(TypedDict, total=False):
    kind: Required[Literal["block86000"]]


class ConstructionModelVariant30(TypedDict, total=False):
    kind: Required[Literal["phase-detection"]]
    routes: Required[str]


class ConstructionModelVariant31(TypedDict, total=False):
    gpio: Required[str]
    kind: Required[Literal["power"]]


class ConstructionModelVariant32(TypedDict, total=False):
    kind: Required[Literal["reset"]]


class ConstructionModelVariant33(TypedDict, total=False):
    kind: Required[Literal["scanout"]]


class ConstructionModelVariant34(TypedDict, total=False):
    kind: Required[Literal["sdhci-clocks"]]
    words: Required[list[list[int]]]


class ConstructionModelVariant35(TypedDict, total=False):
    irq: Required[ConstructionIrqs]
    kind: Required[Literal["sdio-host"]]
    module: Required[ConstructionModule]


class ConstructionModelVariant36(TypedDict, total=False):
    kind: Required[Literal["serial-audio"]]
    sinks: Required[list[ConstructionEmpty]]


class ConstructionModelVariant37(TypedDict, total=False):
    kind: Required[Literal["serial-dma"]]


class ConstructionModelVariant38(TypedDict, total=False):
    kind: Required[Literal["spi-controller"]]
    routes: Required[list[ConstructionRoutes]]


class ConstructionModelVariant39(TypedDict, total=False):
    frames: Required[str]
    images: Required[str]
    kind: Required[Literal["statistics"]]
    routes: Required[str]


class ConstructionModelVariant4(TypedDict, total=False):
    kind: Required[Literal["clocks"]]


class ConstructionModelVariant40(TypedDict, total=False):
    cable: Required[str]
    endpoints: NotRequired[str | None]
    kind: Required[Literal["usb"]]


class ConstructionModelVariant41(TypedDict, total=False):
    controllers: Required[list[str]]
    kind: Required[Literal["vector-gpu"]]


class ConstructionModelVariant5(TypedDict, total=False):
    core: Required[int]
    kind: Required[Literal["core-release"]]
    request: Required[str]


class ConstructionModelVariant6(TypedDict, total=False):
    clock: Required[str]
    controllers: Required[list[str]]
    kind: Required[Literal["display-timing"]]


class ConstructionModelVariant7(TypedDict, total=False):
    kind: Required[Literal["dram"]]


class ConstructionModelVariant8(TypedDict, total=False):
    kind: Required[Literal["dram-phy"]]


class ConstructionModelVariant9(TypedDict, total=False):
    adc: NotRequired[str | None]
    counter: NotRequired[str | None]
    gpio: NotRequired[str | None]
    kind: Required[Literal["encoder"]]


class ConstructionModule(TypedDict, total=False):
    firmware_bytes: Required[int]
    protocol: Required[str]


class ConstructionMotors(TypedDict, total=False):
    counters: Required[list[ConstructionCounters]]
    home: NotRequired[ConstructionHome | None]
    initial_position: Required[int]
    settle_ticks: Required[int]
    ticks_per_step: Required[int]
    travel_limits: Required[list[int]]


class ConstructionNodes(TypedDict, total=False):
    boot_entries: Required[list[str]]
    dependencies: Required[list[str]]
    model: Required[ConstructionModel]
    name: Required[str]
    windows: Required[list[ConstructionWindows]]


class ConstructionOptics(TypedDict, total=False):
    output: Required[str]
    positions: Required[list[list[int]]]


class ConstructionOptionsClock(TypedDict, total=False):
    maximum_seconds: Required[int]
    registers: Required[list[int]]
    started: Required[int]
    ticks_per_second: Required[int]


class ConstructionPanelOptions(TypedDict, total=False):
    used_mask: Required[int]


type ConstructionParts = ConstructionPartsVariant0 | ConstructionPartsVariant1


class ConstructionPartsVariant0(TypedDict, total=False):
    commands: Required[list[str]]
    heartbeat_register: Required[int]
    kind: Required[Literal["primary"]]
    prefix_address: Required[int]
    prefix_high: Required[int]
    sensor_address: Required[int]
    sensor_fields: Required[list[int]]
    stationary: Required[bool]
    status: Required[list[str]]
    topology: Required[list[int]]
    touch: Required[ConstructionTouch]


class ConstructionPartsVariant1(TypedDict, total=False):
    connected: Required[bool]
    kind: Required[Literal["secondary"]]
    pwic_address: Required[int]
    pwic_masks: Required[list[list[int]]]
    pwic_modes: Required[list[int]]
    pwic_read_prefixes: Required[list[tuple[int, list[int]]]]
    pwic_values: Required[list[list[int]]]
    register_address: Required[int]
    register_masks: Required[list[list[int]]]
    register_modes: Required[list[int]]
    register_values: Required[list[list[int]]]
    topology: Required[list[int]]


class ConstructionPin(TypedDict, total=False):
    pin: Required[int]
    port: Required[int]


class ConstructionProfile(TypedDict, total=False):
    nodes: Required[list[ConstructionNodes]]
    schema: Required[int]
    wires: Required[dict[str, ConstructionWire]]


type ConstructionProtocol = ConstructionProtocolVariant0


class ConstructionProtocolVariant0(TypedDict, total=False):
    clock: Required[list[ConstructionClock]]
    kind: Required[Literal["banked"]]
    positive_direction_high: Required[bool]
    stop_response: Required[str]


class ConstructionRegistersOptions(TypedDict, total=False):
    clock: NotRequired[ConstructionOptionsClock | None]
    readable: Required[bool]
    registers: Required[list[list[int]]]
    status_registers: Required[list[int]]


class ConstructionRoutes(TypedDict, total=False):
    endpoint: Required[ConstructionEndpoint]
    gpio: Required[str]
    options: Required[ConstructionRoutesOptions]


class ConstructionRoutesOptions(TypedDict, total=False):
    base: Required[int]
    queue: Required[str]
    selection: Required[list[ConstructionSelection]]
    transfer_ticks: Required[int]
    transports: Required[list[ConstructionTransports]]


class ConstructionSelection(TypedDict, total=False):
    high: Required[bool]
    pin: Required[ConstructionPin]


class ConstructionSelections(TypedDict, total=False):
    choice: Required[int]
    word: Required[int]


class ConstructionSoftware(TypedDict, total=False):
    line: Required[int]
    pulse: Required[str]


class ConstructionTouch(TypedDict, total=False):
    core: Required[int]
    irq: Required[int]
    register: Required[int]
    ticks_per_second: Required[int]


class ConstructionTransports(TypedDict, total=False):
    bits: Required[int]
    configuration: Required[int]
    control: Required[int]
    receive: Required[bool]


class ConstructionWindows(TypedDict, total=False):
    base: Required[int]
    size: Required[int]


type ConstructionWire = ConstructionWireKind | ConstructionWireVariant0


type ConstructionWireKind = Literal[
    "software-pulse",
    "aperture",
    "focus",
    "irq-routes",
    "output-channels",
    "memory-outputs",
    "parameters",
    "phase-edges",
]


ConstructionWireVariant0 = TypedDict(
    "ConstructionWireVariant0",
    {
        "logic-level": Required["bool"],
    },
    total=False,
)


class ConstructionWiring(TypedDict, total=False):
    decoder: Required[int]
    selections: Required[list[ConstructionSelections]]


class ContactCamPosition(TypedDict, total=False):
    contacts: Required[list[bool]]
    run_ticks: Required[int]


class ContactPosition(TypedDict, total=False):
    contacts: Required[list[bool]]
    drive: Required[int]


class ContentIdentity(TypedDict, total=False):
    sha256: Required[str]
    size: Required[int]


type ControlBehavior = (
    ControlBehaviorVariant0
    | ControlBehaviorVariant1
    | ControlBehaviorVariant2
    | ControlBehaviorVariant3
    | ControlBehaviorVariant4
    | ControlBehaviorVariant5
    | ControlBehaviorVariant6
    | ControlBehaviorVariant7
    | ControlBehaviorVariant8
)


class ControlBehaviorVariant0(TypedDict, total=False):
    kind: Required[Literal["button"]]
    press: NotRequired[InputBinding | None]


class ControlBehaviorVariant1(TypedDict, total=False):
    kind: Required[Literal["shutter"]]
    stages: NotRequired[dict[str, list[InputBinding] | None]]


class ControlBehaviorVariant2(TypedDict, total=False):
    directions: NotRequired[dict[str, InputBinding | None]]
    kind: Required[Literal["pad"]]
    press: NotRequired[InputBinding | None]


class ControlBehaviorVariant3(TypedDict, total=False):
    decrease: NotRequired[InputBinding | None]
    increase: NotRequired[InputBinding | None]
    kind: Required[Literal["rocker"]]
    press: NotRequired[InputBinding | None]


class ControlBehaviorVariant4(TypedDict, total=False):
    endless: NotRequired[bool | None]
    kind: Required[Literal["dial"]]
    motion: Required[ControlMotion]
    press: NotRequired[InputBinding | None]


class ControlBehaviorVariant5(TypedDict, total=False):
    endless: NotRequired[bool | None]
    kind: Required[Literal["ring"]]
    motion: Required[ControlMotion]
    press: NotRequired[InputBinding | None]


class ControlBehaviorVariant6(TypedDict, total=False):
    kind: Required[Literal["switch"]]
    positions: NotRequired[list[BoundPosition]]


class ControlBehaviorVariant7(TypedDict, total=False):
    kind: Required[Literal["lever"]]
    momentary: NotRequired[bool | None]
    positions: NotRequired[list[BoundPosition]]


class ControlBehaviorVariant8(TypedDict, total=False):
    kind: Required[Literal["touch"]]


class ControlBusAttachment(TypedDict, total=False):
    address: Required[int]
    channel: Required[int]
    controller_id: Required[str]
    polarity_selector: NotRequired[int | None]


class ControlBusChip(TypedDict, total=False):
    address: Required[int]
    channel: Required[int]
    controller_id: Required[str]
    id: Required[str]
    polarity_selector: Required[int]


type ControlIcon = Literal["playback", "delete", "record", "flash"]


type ControlInterface = ControlInterfaceVariant0 | ControlInterfaceVariant1


class ControlInterfaceVariant0(TypedDict, total=False):
    connection_id: Required[str]
    endpoint: Required[int]
    kind: Required[Literal["power_switch"]]
    mask: Required[int]
    off_value: Required[int]
    on_value: Required[int]
    register: Required[int]
    saved_inputs: Required[int]


class ControlInterfaceVariant1(TypedDict, total=False):
    connection_id: Required[str]
    contacts: Required[int]
    coordinate_bits: Required[int]
    decoded: Required[DecodedTouch]
    height: Required[int]
    irq: Required[int]
    irq_mode: Required[int]
    kind: Required[Literal["prefixed_touch"]]
    mode: Required[int]
    offset_tags: Required[list[int]]
    priority: Required[int]
    register: Required[int]
    report_bytes: Required[int]
    width: Required[int]


class ControlLadder(TypedDict, total=False):
    basis: Required[str]
    levels: Required[int]
    released: NotRequired[int | None]
    rungs: Required[list[LadderRung]]
    wire: Required[LadderWire]


type ControlMotion = ControlMotionVariant0 | ControlMotionVariant1


class ControlMotionVariant0(TypedDict, total=False):
    kind: Required[Literal["positions"]]
    positions: NotRequired[list[BoundPosition]]


class ControlMotionVariant1(TypedDict, total=False):
    decrease: NotRequired[InputBinding | None]
    increase: NotRequired[InputBinding | None]
    kind: Required[Literal["steps"]]


class ControlPosition(TypedDict, total=False):
    description: NotRequired[str | None]
    id: Required[str]
    label: NotRequired[str | None]


type ControlRole = Literal["command_control"]


type ControlSurface = Literal["top", "rear", "front", "bottom", "lens", "left", "right"]


class CoprocessorWrite(TypedDict, total=False):
    core: Required[str]
    cp: Required[int]
    crm: Required[int]
    crn: Required[int]
    opc1: Required[int]
    opc2: Required[int]
    secure: Required[int]
    set_bits: Required[int]
    wide: Required[bool]


class CoreRelease(TypedDict, total=False):
    mask: Required[int]
    register: Required[int]
    source: Required[int]
    target: Required[int]


class CursorFields(TypedDict, total=False):
    cursor: Required[int]
    filtered_count: Required[int]
    pointer: Required[int]
    raw_count: Required[int]
    subcursor: Required[int]
    value_cursor: Required[int]
    visible_count: Required[int]


class CursorLayout(TypedDict, total=False):
    fields: Required[CursorFields]
    size: Required[int]
    status_offset: Required[int]
    status_stride: Required[int]
    status_width: Required[int]


class DataCacheGeometry(TypedDict, total=False):
    line_bytes: Required[int]
    sets: Required[int]
    ways: Required[int]


type DeadlineUnit = Literal["epoch-seconds"]


class DebugKey(TypedDict, total=False):
    event: Required[int]
    name: Required[str]


class DecodedTouch(TypedDict, total=False):
    address: Required[int]
    contacts: Required[list[DecodedTouchContact]]
    count: Required[int]


class DecodedTouchContact(TypedDict, total=False):
    active: Required[int]
    x: Required[int]
    y: Required[int]


class DecoderChoice(TypedDict, total=False):
    id: Required[str]
    selection: Required[DecoderSelection]


class DecoderSelection(TypedDict, total=False):
    dma_mappings: Required[list[str]]
    processors: Required[list[str]]


class DeferredTimer(TypedDict, total=False):
    callback_arguments: Required[int]
    cancel: Required[TimerRoutine]
    kind: Required[str]
    one_shot_periodic: Required[int]
    period_unit: Required[str]
    periodic_nonzero: Required[bool]
    start: Required[TimerRoutine]


class DeviceGraph(TypedDict, total=False):
    attachment_order: Required[list[str]]
    construction_order: Required[list[str]]
    devices: Required[list[DeviceNode]]
    optional_devices: NotRequired[list[DeviceKind] | None]


type DeviceKind = Literal[
    "SDCardController",
    "WiFiSDIOController",
    "AfsFrameSource",
    "JPEGDecodeControl",
    "LensHomingFixture",
    "AFExtractionControl",
    "ApproximateJPEGEncodeControl",
    "AvcCodecControl",
    "ImageTransferControl",
    "BankedLensController",
    "BootControls",
    "ControlBus",
    "DisplayFormatter",
    "DisplayScanout",
    "DisplayTiming",
    "FE80Control",
    "FaceAnalysisControl",
    "FujiDecompressor",
    "GraphicsAccelerator",
    "HDMISink",
    "HDMIStartup",
    "ImageCombination",
    "ImageParameterControl",
    "ImageScaler",
    "ImageWarpControl",
    "ImageWorkControl",
    "InputAuxiliaryControl",
    "InputProcessingControl",
    "KernelInterrupts",
    "LensBridge",
    "LensEndpoint",
    "LensRegisterMotor",
    "LoaderMemoryDMA",
    "MemoryReadback",
    "MixedWidthLensController",
    "NandController",
    "OutputControl",
    "PowerDevices",
    "PrefilterControl",
    "ProducerControl",
    "PulseInputCapture",
    "PulseOutputs",
    "PulseTimerControl",
    "QuadratureCounters",
    "RawConversionRegisters",
    "RawStatisticsControl",
    "SataHost",
    "StillProcessor",
    "SPI1SerialBus",
    "SPI2Serial",
    "SecondaryControlBus",
    "SerialAudioControlRegisters",
    "SerialAudioDMA",
    "SerialAudioTransferControlRegisters",
    "SerialAudioTransmitSink",
    "SerialAudioReceiveSource",
    "ThreeByteDCZoomLensController",
    "ThreeByteLensController",
    "USBControl",
    "WindowAnalysisControl",
    "WordStabilizer",
    "X70SensorCaptureDMA",
    "X70StillISPControl",
]


class DeviceNode(TypedDict, total=False):
    boot_entries: NotRequired[list[str] | None]
    kind: Required[DeviceKind]
    model_choices: NotRequired[HardwareDeviceChoices | None]
    name: Required[str]
    refs: Required[dict[str, str]]


class DisplayAssembly(TypedDict, total=False):
    connection_id: Required[str]
    part_id: Required[str]
    route: NotRequired[SerialRoute | None]


type DisplayOrientation = Literal["landscape", "portrait", "tilting"]


class DmaMapping(TypedDict, total=False):
    address: Required[int]
    id: Required[str]
    permissions: Required[int]
    physical: Required[int]
    size: Required[RegionSize]


class EarlyBootConstants(TypedDict, total=False):
    exr2_restart_alarm_enable: Required[int]
    exr2_restart_alarm_high: Required[int]
    exr2_restart_alarm_low: Required[int]
    exr2_restart_gpio_banks: Required[int]
    exr2_restart_gpio_base: Required[int]
    exr2_restart_gpio_config: Required[int]
    exr2_restart_gpio_inactive_mask: Required[int]
    exr2_restart_gpio_mode1: Required[int]
    exr2_restart_gpio_mode2: Required[int]
    exr2_restart_gpio_mode3: Required[int]
    exr2_restart_gpio_mode4: Required[int]
    exr2_restart_gpio_output: Required[int]
    exr2_restart_gpio_stride: Required[int]
    exr2_restart_packet_bytes: Required[int]
    exr2_restart_packet_register: Required[int]
    exr2_restart_power_command: Required[int]
    exr2_restart_power_hold: Required[int]
    exr2_restart_power_hold_mask: Required[int]
    exr2_restart_power_off_value: Required[int]
    exr2_restart_spi_base: Required[int]
    exr2_restart_spi_busy: Required[int]
    exr2_restart_spi_control: Required[int]
    exr2_restart_spi_count_mask: Required[int]
    exr2_restart_spi_enable: Required[int]
    exr2_restart_spi_fifo_control: Required[int]
    exr2_restart_spi_fifo_reset: Required[int]
    exr2_restart_spi_format: Required[int]
    exr2_restart_spi_rx: Required[int]
    exr2_restart_spi_rx_count: Required[int]
    exr2_restart_spi_rx_ready: Required[int]
    exr2_restart_spi_status: Required[int]
    exr2_restart_spi_status_clear: Required[int]
    exr2_restart_spi_tx: Required[int]
    exr2_restart_spi_tx_count: Required[int]
    exr2_restart_write: Required[int]
    restart_gpio_pin: Required[int]
    restart_panel_gpio_bank: Required[int]
    restart_power_gpio_bank: Required[int]
    restart_spi_format_value: Required[int]
    restart_spi_idle_control: Required[int]


type EarlyBootMechanism = EarlyBootMechanismVariant0


class EarlyBootMechanismVariant0(TypedDict, total=False):
    code_checks: Required[list[CodeCheck]]
    constants: Required[EarlyBootConstants]
    kind: Required[Literal["exr2_polled_alarm"]]
    spi_channel: Required[int]


type ElectricalInput = (
    ElectricalInputVariant0
    | ElectricalInputVariant1
    | ElectricalInputVariant2
    | ElectricalInputVariant3
    | ElectricalInputVariant4
    | ElectricalInputVariant5
    | ElectricalInputVariant6
    | ElectricalInputVariant7
    | ElectricalInputVariant8
)


class ElectricalInputVariant0(TypedDict, total=False):
    callback: Required[int]
    expected_result: Required[int]
    kind: Required[Literal["adc_conjunction"]]
    terms: Required[list[AdcTerm]]


class ElectricalInputVariant1(TypedDict, total=False):
    callback: NotRequired[int | None]
    connection_id: Required[str]
    expected_result: NotRequired[int | None]
    intervals: Required[list[AdcInterval]]
    kind: Required[Literal["adc"]]


class ElectricalInputVariant2(TypedDict, total=False):
    connection_id: Required[str]
    expected_level: Required[bool]
    kind: Required[Literal["gpio"]]
    scanner: Required[GpioScanner]


class ElectricalInputVariant3(TypedDict, total=False):
    callback: Required[int]
    connection_id: Required[str]
    expected_levels: Required[list[bool]]
    kind: Required[Literal["gpio_contacts"]]


class ElectricalInputVariant4(TypedDict, total=False):
    connection_id: Required[str]
    decoder_event: Required[int]
    guard: Required[int]
    kind: Required[Literal["gpio_quadrature"]]
    phase_map: Required[list[int]]
    reset: Required[list[int]]
    threshold: Required[int]
    transitions: Required[list[int]]


class ElectricalInputVariant5(TypedDict, total=False):
    connection_id: Required[str]
    decoder_event: Required[int]
    guard: Required[int]
    kind: Required[Literal["gpio_transition"]]
    phase_map: Required[list[int]]
    threshold: Required[int]
    transitions: Required[list[int]]


class ElectricalInputVariant6(TypedDict, total=False):
    calibration_offsets: Required[AdcEncoderCalibration]
    connection_id: Required[str]
    decoder_event: Required[int]
    kind: Required[Literal["adc_transition"]]
    phase_map: Required[list[int]]
    transitions: Required[list[int]]


class ElectricalInputVariant7(TypedDict, total=False):
    connection_id: Required[str]
    cycle_sign: Required[int]
    kind: Required[Literal["gpio_fixed_cycle"]]


class ElectricalInputVariant8(TypedDict, total=False):
    active_value: Required[int]
    connection_id: Required[str]
    endpoint: Required[int]
    kind: Required[Literal["serial_status"]]
    mask: Required[int]
    register: Required[int]


class EncoderLevels(TypedDict, total=False):
    high: Required[int]
    low: Required[int]


type EncoderOutputs = EncoderOutputsVariant0 | EncoderOutputsVariant1


class EncoderOutputsVariant0(TypedDict, total=False):
    kind: Required[Literal["gpio"]]


class EncoderOutputsVariant1(TypedDict, total=False):
    channels: Required[list[int]]
    kind: Required[Literal["adc"]]
    levels: Required[list[EncoderLevels]]


class EventTranslation(TypedDict, total=False):
    address: Required[int]
    rows: Required[list[TranslatedKey]]
    sha256: Required[str]
    stride: Required[int]


class Evidence(TypedDict, total=False):
    addresses: NotRequired[list[EvidenceAddress]]
    firmware_ids: NotRequired[list[str]]
    kind: Required[EvidenceKind]


class EvidenceAddress(TypedDict, total=False):
    address: Required[int]
    firmware_id: Required[str]
    space: Required[EvidenceSpace]


type EvidenceKind = Literal["measured", "located", "inferred", "stand-in"]


type EvidenceSpace = Literal["code", "data"]


type ExecutionMechanism = ExecutionMechanismVariant0


class ExecutionMechanismVariant0(TypedDict, total=False):
    allocation: Required[HeapAllocation]
    cache: Required[CacheSynchronization]
    code_checks: Required[list[CodeCheck]]
    kind: Required[Literal["exr2_query_heap"]]


Firmware = TypedDict(
    "Firmware",
    {
        "$schema": Required[
            'Literal["https://fujimod.com/schemas/firmware.schema.json"]'
        ],
        "body_id": Required["str"],
        "boot_rom": NotRequired["BootRomIdentity | None"],
        "generated_facts": NotRequired["FirmwareGeneratedFacts | None"],
        "id": Required["str"],
        "installation": NotRequired["FirmwareInstallation | None"],
        "layouts": NotRequired["FirmwareLayouts | None"],
        "reset_release": NotRequired["HardwareResetRelease | None"],
        "schema": Required["int"],
        "update": NotRequired["ContentIdentity | None"],
        "version": Required["str"],
    },
    total=False,
)


class FirmwareBootLayout(TypedDict, total=False):
    flash_slices: Required[list[FirmwareFlashSlices]]
    units: Required[list[FirmwareUnits]]


FirmwareBulk = TypedDict(
    "FirmwareBulk",
    {
        "$schema": Required["str"],
        "code_alias": Required["int"],
        "code_checks": Required["list[CodeCheck]"],
        "dma": NotRequired["BulkDma | None"],
        "firmware_id": Required["str"],
        "id": Required["str"],
        "internal_version": Required["str"],
        "mapped_probe": NotRequired["CodeCheck | None"],
        "probes": Required["list[BulkRange]"],
        "product": Required["str"],
        "schema": Required["int"],
        "scopes": Required["list[CaptureScope]"],
        "source": Required["BulkSource"],
        "upload": NotRequired["Upload | None"],
        "window": Required["BulkRange"],
    },
    total=False,
)


FirmwareCard = TypedDict(
    "FirmwareCard",
    {
        "$schema": Required["str"],
        "code_checks": Required["list[CodeCheck]"],
        "firmware_id": Required["str"],
        "internal_version": Required["str"],
        "mechanism": Required["Mechanism"],
        "product": Required["str"],
        "schema": Required["int"],
        "service": NotRequired["QueryService | None"],
    },
    total=False,
)


class FirmwareCfgdata(TypedDict, total=False):
    address: Required[int]
    defaults_sha256: Required[str]
    pointer_address: Required[int]
    remapping: NotRequired[list[FirmwareRemapping]]
    selector_address: NotRequired[int | None]
    size: Required[int]
    synthetic_byte_fields: NotRequired[list[FirmwareSyntheticByteFields] | None]
    synthetic_scalar_bytes: NotRequired[list[FirmwareSyntheticScalarBytes] | None]
    synthetic_signature: Required[FirmwareSyntheticSignature]


class FirmwareConfigurationBanks(TypedDict, total=False):
    cfg_offset: NotRequired[int | None]
    fill: NotRequired[int | None]
    row: Required[int]


class FirmwareConfigurationDefaults(TypedDict, total=False):
    builder_entry: Required[int]
    code_checks: Required[list[FirmwareUsbInterruptTables]]
    final_version_bytes: Required[list[FirmwareFinalVersionBytes]]
    limits: NotRequired[list[str]]
    max_instructions: Required[int]
    output: Required[FirmwareConfigurationdefaultsOutput]
    pointer_address: Required[int]
    sections: Required[list[str]]
    static_oracle_sha256: Required[str]
    store_limit: Required[int]
    table: Required[FirmwareTable]


class FirmwareConfigurationdefaultsOutput(TypedDict, total=False):
    address: Required[int]
    size: Required[int]


class FirmwareControlBusTables(TypedDict, total=False):
    descriptors: Required[FirmwareDescriptors]
    limits: NotRequired[list[str]]
    polarity_words: Required[FirmwareDescriptor]


FirmwareControlIds = TypedDict(
    "FirmwareControlIds",
    {
        "$schema": Required["str"],
        "debug_keys": Required["list[DebugKey]"],
        "event_translation": NotRequired["EventTranslation | None"],
        "firmware_id": Required["str"],
        "id": Required["str"],
        "input_readers": Required["InputReaders"],
        "inputs": Required["list[LocatedInput]"],
        "interfaces": Required["dict[str, ControlInterface]"],
        "native_key_names": NotRequired["NativeKeyNames | None"],
        "scanner": Required["ScannerTable"],
        "schema": Required["int"],
        "undelivered_rows": Required["list[LocatedScannerRecord]"],
    },
    total=False,
)


class FirmwareCpu(TypedDict, total=False):
    cpsr: Required[int]
    id: Required[int]
    pc: Required[int]
    regs: Required[FirmwareRegs]
    running: Required[bool]
    sysregs: Required[FirmwareSysregs]


class FirmwareCustomDefaults(TypedDict, total=False):
    entry_view: Required[str]
    output_address: Required[int]
    size: Required[int]
    slot_bytes: Required[int]
    slots: Required[int]


class FirmwareDefaultCopies(TypedDict, total=False):
    offset: Required[int]
    view: Required[str]


class FirmwareDescriptor(TypedDict, total=False):
    address: Required[int]
    sampling: Required[str]
    sha256: Required[str]
    size: Required[int]


class FirmwareDescriptors(TypedDict, total=False):
    address: Required[int]
    count: Required[int]
    format: Required[str]
    sampling: Required[str]
    sha256: Required[str]
    size: Required[int]


FirmwareExecution = TypedDict(
    "FirmwareExecution",
    {
        "$schema": Required["str"],
        "code_alias": Required["int"],
        "firmware_id": Required["str"],
        "id": Required["str"],
        "internal_version": Required["str"],
        "mechanism": Required["ExecutionMechanism"],
        "product": Required["str"],
        "schema": Required["int"],
    },
    total=False,
)


FirmwareFacts = TypedDict(
    "FirmwareFacts",
    {
        "$schema": Required["str"],
        "body_id": Required["str"],
        "configuration": Required["ConfigurationLocation"],
        "firmware_id": Required["str"],
        "firmware_sha256": Required["str"],
        "identity_checks": Required["list[AddressIdentity]"],
        "menus": Required["MenuTables"],
        "resources": Required["Resources"],
        "scanout": Required["Scanout"],
        "schema": Required["int"],
        "state": Required["FirmwareObservation"],
        "uncached_alias": Required["int"],
    },
    total=False,
)


class FirmwareFe80Programs(TypedDict, total=False):
    basis: Required[str]
    descriptor: Required[FirmwareDescriptor]
    period: Required[int]
    program: Required[FirmwareDescriptor]
    setup_phase: NotRequired[str | None]


class FirmwareFinalVersionBytes(TypedDict, total=False):
    offset: Required[int]
    value: Required[int]


type FirmwareFirmwareView = (
    FirmwareFirmwareViewVariant0
    | FirmwareFirmwareViewVariant1
    | FirmwareFirmwareViewVariant2
)


class FirmwareFirmwareViewVariant0(TypedDict, total=False):
    data: Required[FirmwareConfigurationDefaults]
    kind: Required[Literal["configuration_defaults"]]


class FirmwareFirmwareViewVariant1(TypedDict, total=False):
    data: Required[FirmwareControlBusTables]
    kind: Required[Literal["control_bus_tables"]]


class FirmwareFirmwareViewVariant2(TypedDict, total=False):
    data: Required[FirmwareStorageLayout]
    kind: Required[Literal["storage_layout"]]


class FirmwareFixtureLayouts(TypedDict, total=False):
    status: Required[str]


FirmwareFlash = TypedDict(
    "FirmwareFlash",
    {
        "$schema": Required["str"],
        "code_ranges": Required["list[FlashCodeRange]"],
        "control": Required["FlashControl"],
        "device": Required["FlashDevice"],
        "e1_window": NotRequired["FlashRange | None"],
        "firmware_id": Required["str"],
        "id": Required["str"],
        "reader": NotRequired["FlashReader | None"],
        "schema": Required["int"],
        "state_checks": Required["list[FlashStateCheck]"],
        "transport": NotRequired["str | None"],
        "update_image": Required["FlashUpdateImage"],
        "window": Required["FlashRange"],
    },
    total=False,
)


class FirmwareFlashImage(TypedDict, total=False):
    dat_origin: Required[int]
    xor: Required[int]


class FirmwareFlashSlices(TypedDict, total=False):
    address: Required[int]
    name: Required[str]
    offset: Required[int]
    sha256: Required[str]
    size: Required[int]


class FirmwareGeneratedFacts(TypedDict, total=False):
    additional_ui_modes: NotRequired[AdditionalUiModes | None]
    cfgdata_location: NotRequired[ConfigurationLocation | None]
    identity_checks: NotRequired[list[AddressIdentity]]
    menu_cursor: NotRequired[int | None]
    menu_keys: NotRequired[MenuKeys | None]
    observation: NotRequired[FirmwareObservation | None]
    remote: NotRequired[FirmwareFacts | None]
    uncached_alias: NotRequired[int | None]


class FirmwareGicRedistributorInitial(TypedDict, total=False):
    core: Required[int]
    offset: Required[int]
    size: Required[int]
    value: Required[int]


class FirmwareHeader(TypedDict, total=False):
    checksum_offset: Required[int]
    registers_view: Required[str]
    view: Required[str]


class FirmwareImageSection(TypedDict, total=False):
    address: Required[int]
    name: Required[str]
    native_flash_offset: NotRequired[int | None]
    sha256: Required[str]
    size: Required[int]


class FirmwareInstallation(TypedDict, total=False):
    entry_labels: NotRequired[dict[str, str]]
    flash_offset: Required[int]
    image_sections: Required[list[FirmwareImageSection]]
    loader_validation: NotRequired[FirmwareLoaderValidation | None]


class FirmwareJournals(TypedDict, total=False):
    bank_rows: Required[list[int]]
    check_view: Required[str]
    default_copies: NotRequired[list[FirmwareDefaultCopies]]
    kind: Required[int]
    name: Required[str]
    nondefault_edits: Required[list[FirmwareNondefaultEdits]]
    parity_context: Required[int]
    payload_size: Required[int]
    prepare_view: Required[str]
    query_view: Required[str]
    record: NotRequired[FirmwareRecord | None]
    record_copies: Required[list[FirmwareRecordCopies]]
    record_tags: NotRequired[list[FirmwareDefaultCopies]]


class FirmwareLayouts(TypedDict, total=False):
    profiles: Required[dict[str, FirmwareProfile]]
    views: NotRequired[dict[str, FirmwareFirmwareView]]


class FirmwareLoaderValidation(TypedDict, total=False):
    entry: Required[int]
    sections: Required[list[str]]


class FirmwareManaged(TypedDict, total=False):
    banks_view: Required[str]
    logical_capacity: Required[int]
    start: Required[int]


class FirmwareNondefaultEdits(TypedDict, total=False):
    mask: Required[int]
    offset: Required[int]
    values: Required[list[int]]


class FirmwareObservation(TypedDict, total=False):
    menu_active: Required[int]
    menu_node: Required[int]
    pending_mode: Required[int]
    readiness: Required[ReadinessAddresses]
    rec_mode: Required[int]
    setup_mode: Required[int]
    system_mode: Required[int]
    ui_mode: Required[int]


class FirmwarePersistentAssetLayout(TypedDict, total=False):
    bytes: Required[int]
    file_offset: Required[int]
    normalized_sha256: Required[str]
    patches: NotRequired[list[PersistentPatch]]
    sha256: Required[str]


class FirmwarePersistentInputs(TypedDict, total=False):
    assets: Required[dict[str, FirmwarePersistentAssetLayout]]
    geometry: Required[FirmwarePersistentInputsGeometry]
    sets: Required[dict[str, PersistentSet]]


class FirmwarePersistentInputsGeometry(TypedDict, total=False):
    bytes: Required[int]
    page_bytes: Required[int]
    pages_per_block: Required[int]
    spare_bytes: Required[int]


class FirmwarePreload(TypedDict, total=False):
    address: Required[int]
    dat_offset: NotRequired[int | None]
    length: NotRequired[int | None]
    sha256: NotRequired[str | None]
    size: NotRequired[int | None]
    value: NotRequired[int | None]
    word_size: NotRequired[int | None]
    words: NotRequired[list[int] | None]


class FirmwareProfile(TypedDict, total=False):
    boot_layout: NotRequired[FirmwareBootLayout | None]
    browser_configuration_id: NotRequired[str | None]
    control_bus_tables_id: NotRequired[str | None]
    cpu: NotRequired[list[FirmwareCpu] | None]
    execution_scope: NotRequired[str | None]
    fe80_programs: NotRequired[list[FirmwareFe80Programs] | None]
    fe80_setup_views: NotRequired[list[FirmwareDescriptor] | None]
    fixture_layouts: NotRequired[FirmwareFixtureLayouts | None]
    flash_image: NotRequired[FirmwareFlashImage | None]
    gic_initial: NotRequired[dict[str, int] | None]
    gic_redistributor_initial: NotRequired[list[FirmwareGicRedistributorInitial] | None]
    hdmi_configuration_id: NotRequired[str | None]
    limits: NotRequired[list[str] | None]
    persistent_inputs: NotRequired[FirmwarePersistentInputs | None]
    preload: NotRequired[list[FirmwarePreload] | None]
    reset_release: NotRequired[HardwareResetRelease | None]
    rom_basis: NotRequired[str | None]
    sdio_inputs: NotRequired[FirmwareSdioInputs | None]
    storage_layout_id: NotRequired[str | None]
    usb_interrupt_tables: NotRequired[list[FirmwareUsbInterruptTables] | None]
    viewer_abi: NotRequired[str | None]


class FirmwareRawRecords(TypedDict, total=False):
    bank_rows: Required[list[int]]
    basis: Required[str]
    consumer_view: Required[str]
    name: Required[str]
    nondefault_bytes: Required[list[FirmwareSyntheticSignature]]
    seed_sequences: Required[list[int]]
    size: Required[int]


class FirmwareRecord(TypedDict, total=False):
    protected_bytes: Required[int]
    size: Required[int]


class FirmwareRecordCopies(TypedDict, total=False):
    payload_offset: Required[int]
    record_offset: Required[int]
    size: Required[int]


class FirmwareRegs(TypedDict, total=False):
    x19: Required[int]
    x20: Required[int]
    x21: Required[int]


class FirmwareRemapping(TypedDict, total=False):
    delta: Required[int]
    selector: Required[int]
    size: Required[int]
    start: Required[int]


FirmwareRestart = TypedDict(
    "FirmwareRestart",
    {
        "$schema": Required["str"],
        "code_alias": Required["int"],
        "code_checks": Required["list[CodeCheck]"],
        "firmware_id": Required["str"],
        "id": Required["str"],
        "internal_version": Required["str"],
        "mechanism": Required["RestartMechanism"],
        "product": Required["str"],
        "schema": Required["int"],
    },
    total=False,
)


class FirmwareSdioInputs(TypedDict, total=False):
    firmware_bytes: Required[int]
    mac_address: Required[list[int]]
    ssid: Required[str]


FirmwareSettings = TypedDict(
    "FirmwareSettings",
    {
        "$schema": Required["str"],
        "bank": NotRequired["SettingsBank | None"],
        "configuration": NotRequired["ConfigurationWindow | None"],
        "defaults": NotRequired["UpdateRange | None"],
        "fields": Required["list[SettingsField]"],
        "firmware_id": Required["str"],
        "id": Required["str"],
        "profile": NotRequired["SettingsProfile | None"],
        "schema": Required["int"],
        "sections": Required["list[SettingsSection]"],
        "settings": Required["dict[str, NamedSetting]"],
        "source": Required["SettingsSource"],
    },
    total=False,
)


FirmwareState = TypedDict(
    "FirmwareState",
    {
        "$schema": Required["str"],
        "firmware_id": Required["str"],
        "id": Required["str"],
        "regions": Required["list[StateRegion]"],
        "schema": Required["int"],
    },
    total=False,
)


class FirmwareStorageLayout(TypedDict, total=False):
    cfgdata: Required[FirmwareCfgdata]
    configuration_banks: Required[list[FirmwareConfigurationBanks]]
    custom_defaults: Required[FirmwareCustomDefaults]
    geometry: Required[FirmwareStoragelayoutGeometry]
    header: Required[FirmwareHeader]
    journals: Required[list[FirmwareJournals]]
    limits: NotRequired[list[str]]
    managed: Required[FirmwareManaged]
    max_instructions: Required[int]
    raw_records: NotRequired[list[FirmwareRawRecords] | None]
    record: Required[FirmwareStoragelayoutRecord]
    scratch_record: Required[int]
    sections: Required[list[str]]
    views: Required[list[FirmwareUsbInterruptTables]]


class FirmwareStoragelayoutGeometry(TypedDict, total=False):
    blocks: Required[int]
    page_bytes: Required[int]
    pages_per_block: Required[int]
    spare_bytes: Required[int]


class FirmwareStoragelayoutRecord(TypedDict, total=False):
    empty_fill: Required[int]
    protected_bytes: Required[int]
    seed_sequences: Required[list[int]]
    sequence_offset: Required[int]
    size: Required[int]


class FirmwareSyntheticByteFields(TypedDict, total=False):
    basis: Required[str]
    field: Required[int]
    row: Required[int]
    view: Required[str]


class FirmwareSyntheticScalarBytes(TypedDict, total=False):
    basis: Required[str]
    field: Required[int]
    source_offset: Required[int]
    view: Required[str]


class FirmwareSyntheticSignature(TypedDict, total=False):
    offset: Required[int]


class FirmwareSysregs(TypedDict, total=False):
    CPACR_EL1: Required[int]
    ICC_IGRPEN0_EL1: Required[int]
    ICC_IGRPEN1_EL1: Required[int]
    ICC_PMR_EL1: Required[int]
    SCTLR_EL1: Required[int]
    SP_EL0: Required[int]
    SP_EL1: Required[int]
    VBAR_EL1: Required[int]


class FirmwareTable(TypedDict, total=False):
    address: Required[int]
    count: Required[int]
    name: Required[str]
    sampling: Required[str]
    sha256: Required[str]
    size: Required[int]


class FirmwareUnits(TypedDict, total=False):
    unit: Required[str]


class FirmwareUsbInterruptTables(TypedDict, total=False):
    address: Required[int]
    name: Required[str]
    sampling: Required[str]
    sha256: Required[str]
    size: Required[int]


class FirstStartLanguage(TypedDict, total=False):
    key: Required[int]
    name: Required[str]
    value: Required[int]


FirstStartMenu = TypedDict(
    "FirstStartMenu",
    {
        "$schema": Required["str"],
        "children_offset": Required["int"],
        "cursor": Required["int"],
        "date_editor": Required["int"],
        "date_kind": Required["int"],
        "firmware_id": Required["str"],
        "id": Required["str"],
        "languages": Required["list[FirstStartLanguage]"],
        "menu_node": Required["int"],
        "row_bytes": Required["int"],
        "schema": Required["int"],
    },
    total=False,
)


class FlashCodeRange(TypedDict, total=False):
    address: Required[int]
    name: Required[str]
    size: Required[int]


class FlashControl(TypedDict, total=False):
    offset: Required[int]
    sha256: Required[str]
    size: Required[int]


class FlashDevice(TypedDict, total=False):
    jedec_id: NotRequired[str | None]
    kind: Required[str]
    part: NotRequired[str | None]


class FlashRange(TypedDict, total=False):
    address: Required[int]
    size: Required[int]


class FlashReader(TypedDict, total=False):
    alignment: Required[int]
    nand: NotRequired[NandPageReader | None]
    symbol: Required[str]


class FlashSeed(TypedDict, total=False):
    offset: Required[int]
    value: Required[int]


class FlashStateCheck(TypedDict, total=False):
    address: Required[int]
    name: Required[str]
    size: Required[int]
    value: Required[int]


class FlashUpdateImage(TypedDict, total=False):
    dat_offset: Required[int]
    xor: Required[int]


class FocusAxis(TypedDict, total=False):
    infinity_position: Required[int]
    output: Required[OpticalQuantity]
    steps_per_mm: Required[int]


class GeneratedSceneAsset(TypedDict, total=False):
    id: Required[str]
    pattern: Required[list[int]]
    size: Required[int]


class GlyphResources(TypedDict, total=False):
    data_address_offset: Required[int]
    data_size_offset: Required[int]
    icon_index: Required[int]
    record_size: Required[int]
    registry_address: Required[int]


class GpioFeedback(TypedDict, total=False):
    input_connection_id: Required[str]
    latch_high: NotRequired[bool]
    output_connection_id: Required[str]
    rise_ns: NotRequired[int | None]


class GpioOutputWord(TypedDict, total=False):
    levels: Required[int]
    port: Required[int]


type GpioPadModel = Literal["data-latch"]


class GpioPinLevel(TypedDict, total=False):
    high: Required[bool]
    pin: Required[int]
    port: Required[int]


class GpioRegisterWord(TypedDict, total=False):
    port: Required[int]
    register: Required[int]
    value: Required[int]


type GpioScanner = GpioScannerVariant0 | GpioScannerVariant1


class GpioScannerVariant0(TypedDict, total=False):
    kind: Required[Literal["direct"]]


class GpioScannerVariant1(TypedDict, total=False):
    kind: Required[Literal["combined"]]
    lower: Required[int]
    source: Required[int]
    upper: Required[int]


class HardwareAfExtraction(TypedDict, total=False):
    profile: Required[str]


class HardwareBusSensor(TypedDict, total=False):
    address: Required[int]
    basis: Required[str]
    reply_model: Required[str]


class HardwareCaptureColor(TypedDict, total=False):
    basis: Required[str]
    black: Required[int]
    full_scale: Required[int]
    initial_rgb: Required[list[int]]
    processing_cfa: Required[list[int]]
    radiometry: NotRequired[HardwareRadiometry | None]
    reference_white: Required[list[int]]
    sensor_cfa: Required[list[int]]
    sensor_image_area: Required[list[int]]


class HardwareCardContacts(TypedDict, total=False):
    basis: Required[str]
    detect: Required[list[int]]
    write_protect: Required[list[int]]


class HardwareCommandTarget(TypedDict, total=False):
    address: Required[int]
    basis: Required[str]
    commands: Required[list[int]]
    prefix_mask: Required[int]
    prefix_value: Required[int]
    read_frames: Required[list[tuple[int, str]]]
    register_mask: Required[int]
    write_masks: Required[list[list[int]]]


class HardwareConnectionSignal(TypedDict, total=False):
    active_level: NotRequired[int | None]
    channel: NotRequired[int | None]
    label: Required[str]
    pin: NotRequired[int | None]
    port: NotRequired[int | None]
    source: Required[str]
    threshold: NotRequired[int | None]


class HardwareConnectionSupport(TypedDict, total=False):
    id: Required[str]
    label: Required[str]
    limits: Required[str]
    signals: Required[list[HardwareConnectionSignal]]
    support: Required[str]


class HardwareContrast(TypedDict, total=False):
    kind: Required[str]
    luma_bits: Required[int]


class HardwareControllerModels(TypedDict, total=False):
    control_bus0: NotRequired[HardwarePrimaryControlBus | None]
    control_bus1: NotRequired[HardwareSecondaryControlBus | None]
    display_timing: NotRequired[HardwareDisplayTiming | None]
    fe80: NotRequired[HardwareFe80 | None]
    input_processing: NotRequired[HardwareInputProcessing | None]
    spi1_serial: NotRequired[HardwareSpi1Serial | None]
    stabilizer: NotRequired[HardwareStabilizer | None]
    usb: NotRequired[HardwareUsb | None]
    window_analysis: NotRequired[HardwareWindowAnalysis | None]


class HardwareCore(TypedDict, total=False):
    core: Required[int]
    line: Required[int]


class HardwareCurtains(TypedDict, total=False):
    first_output: Required[int]
    light_offset_seconds: Required[int]
    second_output: Required[int]


class HardwareDeviceChoices(TypedDict, total=False):
    models: Required[dict[str, HardwareDeviceModel]]
    option: Required[str]


class HardwareDeviceDependency(TypedDict, total=False):
    kind: Required[DeviceKind]
    name: Required[str]
    refs: Required[dict[str, str]]


class HardwareDeviceModel(TypedDict, total=False):
    dependencies: Required[list[HardwareDeviceDependency]]
    kind: Required[DeviceKind]
    refs: Required[dict[str, str]]
    replaces: NotRequired[list[str]]


class HardwareDisplayScanout(TypedDict, total=False):
    output: Required[int]
    period_ticks: Required[int]


class HardwareDisplayStatusBits(TypedDict, total=False):
    mask: Required[int]
    offset: Required[int]


class HardwareDisplayTiming(TypedDict, total=False):
    basis: Required[str]
    frame_period_ticks: Required[int]
    phase_profile: Required[str]
    stop_delay_ticks: Required[int]


class HardwareDisplayTimingBanks(TypedDict, total=False):
    bases: Required[list[int]]
    stop_ticks: Required[int]


class HardwareEncoder(TypedDict, total=False):
    adc: NotRequired[str | None]
    counter: NotRequired[str | None]
    name: Required[str]
    options: Required[HardwareEncoderTiming]


class HardwareEncoderTiming(TypedDict, total=False):
    outputs: NotRequired[EncoderOutputs | None]
    phase_ticks: Required[int]
    pins: NotRequired[list[PartAssemblyPin] | None]
    rest_phase: Required[int]


class HardwareFaceDetector(TypedDict, total=False):
    mode: Required[str]


class HardwareFe80(TypedDict, total=False):
    basis: Required[str]
    periods: Required[list[int]]
    upload_setup_phases: Required[list[str]]


class HardwareFe80Processing(TypedDict, total=False):
    arithmetic: Required[str]
    basis: Required[str]
    memory_bytes_per_tick: NotRequired[int | None]
    optics: NotRequired[HardwareOptics | None]
    options: Required[HardwareFe80ProcessingOptions]


class HardwareFe80ProcessingOptions(TypedDict, total=False):
    irq: Required[int]
    pixels_per_tick: Required[int]


class HardwareFocusAxis(TypedDict, total=False):
    infinity_position: Required[int]
    output: Required[str]
    steps_per_mm: Required[int]


class HardwareFocusImaging(TypedDict, total=False):
    basis: Required[str]
    contrast: Required[HardwareContrast]
    face_detector: Required[HardwareFaceDetector]
    frame_phase: NotRequired[int | None]
    input_clock_mode: Required[str]
    input_command: NotRequired[str | None]
    input_period_ticks: NotRequired[int | None]
    memory_bytes_per_tick: Required[int]
    metering: Required[str]
    optics: Required[HardwareFocusImagingOptics]
    raster: Required[HardwareRaster]
    statistics_pixels_per_tick: NotRequired[int | None]


class HardwareFocusImagingOptics(TypedDict, total=False):
    maximum_radius: Required[int]
    pulses_per_radius: Required[int]
    target_position: Required[int]


class HardwareImaging(TypedDict, total=False):
    af_extraction: NotRequired[HardwareAfExtraction | None]
    capture_color: NotRequired[HardwareCaptureColor | None]
    fe80_processing: NotRequired[HardwareFe80Processing | None]
    focus_axis: NotRequired[HardwareFocusAxis | None]
    focus_imaging: NotRequired[HardwareFocusImaging | None]
    iris_mechanics: NotRequired[HardwareIrisMechanics | None]
    iris_optics: NotRequired[HardwareIrisOptics | None]
    nd_filter: NotRequired[HardwareNdFilter | None]
    sensor_capture: NotRequired[HardwareSensorCapture | None]


class HardwareInputProcessing(TypedDict, total=False):
    basis: Required[str]
    command_controls: Required[list[int]]


class HardwareInterruptOutput(TypedDict, total=False):
    Core: Required[HardwareCore]


class HardwareIrisMechanics(TypedDict, total=False):
    boundary: Required[str]
    travel_limits: Required[list[int]]


class HardwareIrisOptics(TypedDict, total=False):
    actuator: NotRequired[str | None]
    output: Required[str]
    positions: Required[list[list[int]]]


class HardwareNdFilter(TypedDict, total=False):
    basis: Required[str]
    factor: Required[int]
    insert_mask: Required[int]
    output: Required[str]
    register: Required[int]
    remove_mask: Required[int]


class HardwareOptics(TypedDict, total=False):
    maximum_radius: Required[int]
    pulses_per_radius: Required[int]
    target_position: Required[int]


class HardwarePartAssemblies(TypedDict, total=False):
    contact_actuators: NotRequired[list[str] | None]
    contact_cams: NotRequired[list[str] | None]
    serial_outputs: NotRequired[list[str] | None]
    serial_part: Required[list[str]]


class HardwarePinInterrupt(TypedDict, total=False):
    edge: Required[str]
    outputs: Required[list[HardwareInterruptOutput]]
    pin: Required[PartAssemblyPin]


class HardwarePreviewActivity(TypedDict, total=False):
    high: Required[bool]
    pin: Required[PartAssemblyPin]


class HardwarePrimaryControlBus(TypedDict, total=False):
    basis: NotRequired[str | None]
    command_targets: NotRequired[list[HardwareCommandTarget] | None]
    profile: Required[str]
    sensor: Required[HardwareBusSensor]
    status_targets: NotRequired[list[HardwareStatusTarget] | None]
    transfer_ticks: Required[int]


class HardwareProjection(TypedDict, total=False):
    focal_length_mm: Required[int]
    focus: Required[str]
    relative_illumination: Required[SensorIllumination]
    sensor_width_mm: Required[int]
    subject_distance_mm: Required[int]


class HardwareRadiometricCalibration(TypedDict, total=False):
    reference_white: Required[list[int]]
    saturation_lux_seconds: Required[int]


class HardwareRadiometry(TypedDict, total=False):
    curtains: NotRequired[HardwareCurtains | None]
    live_view: NotRequired[HardwareRadiometricCalibration | None]
    mode_change_frames: Required[int]
    pixel_clock_hz: Required[int]
    projection: NotRequired[HardwareProjection | None]
    rolling_references: Required[list[HardwareRollingReference]]
    saturation_lux_seconds: Required[int]
    shutter: NotRequired[HardwareShutter | None]
    shutter_output: Required[int]
    ticks_per_second: Required[int]
    white_luminance: Required[int]


class HardwareRaster(TypedDict, total=False):
    height: Required[int]
    stride: Required[int]
    width: Required[int]


class HardwareRegisterBlockCommand(TypedDict, total=False):
    mask: Required[int]
    state: Required[int]
    status: Required[int]
    value: Required[int]
    word: Required[int]


class HardwareRegisterFixedWord(TypedDict, total=False):
    offset: Required[int]
    value: Required[int]


class HardwareRegisterIdleBits(TypedDict, total=False):
    mask: Required[int]
    offset: Required[int]


class HardwareResetCheck(TypedDict, total=False):
    address: Required[int]
    expected_hex: Required[str]


class HardwareResetRelease(TypedDict, total=False):
    basis: NotRequired[str | None]
    checks: NotRequired[list[HardwareResetCheck] | None]
    mask: Required[int]
    register: Required[int]
    source: Required[int]
    target: Required[int]


class HardwareRollingReference(TypedDict, total=False):
    horizontal: Required[int]
    lines: Required[int]
    mode: Required[int]
    vertical: Required[int]


class HardwareSecondaryBusRegisterChip(TypedDict, total=False):
    address: Required[int]
    basis: Required[str]
    modes: Required[list[int]]
    values: Required[list[list[int]]]
    write_masks: Required[list[list[int]]]


class HardwareSecondaryBusRegisters(TypedDict, total=False):
    extra_registers: NotRequired[list[HardwareSecondaryBusRegisterChip] | None]
    pwic_address: Required[int]
    pwic_modes: Required[list[int]]
    pwic_read_prefixes: Required[list[tuple[int, str]]]
    pwic_values: Required[list[list[int]]]
    pwic_write_masks: Required[list[list[int]]]
    register_address: Required[int]
    register_modes: Required[list[int]]
    register_values: Required[list[list[int]]]
    register_write_masks: Required[list[list[int]]]
    usb_connected: NotRequired[str | None]


class HardwareSecondaryControlBus(TypedDict, total=False):
    basis: Required[str]
    profile: Required[str]
    replies: Required[HardwareSecondaryBusRegisters]
    transfer_ticks: Required[int]


class HardwareSelection(TypedDict, total=False):
    body_profile_id: Required[str]
    soc_profile_id: NotRequired[str | None]


class HardwareSensorCapture(TypedDict, total=False):
    cfa_origin: Required[list[int]]
    dma_ticks: Required[int]
    height: Required[int]
    period_ticks: Required[int]
    width: Required[int]


class HardwareShutter(TypedDict, total=False):
    closing_delay_seconds: Required[list[list[int]]]


class HardwareSpi1Serial(TypedDict, total=False):
    basis: Required[str]
    selection: Required[list[int]]
    transfer_ticks: Required[int]
    transports: Required[list[list[int]]]


class HardwareStabilizer(TypedDict, total=False):
    basis: Required[str]
    status: Required[int]
    telemetry: Required[int]


class HardwareStatusTarget(TypedDict, total=False):
    address: Required[int]
    basis: Required[str]
    status_hex: Required[str]
    writable_registers: Required[list[int]]
    write_replies: Required[list[tuple[int, int, str]]]


type HardwareTimerLayout = Literal["xp4", "xpp"]


class HardwareTouch(TypedDict, total=False):
    core: Required[int]
    irq: Required[int]
    register: Required[int]
    ticks_per_second: Required[int]


class HardwareUsb(TypedDict, total=False):
    basis: Required[str]
    in_endpoints: Required[list[list[int]]]
    out_endpoints: Required[list[list[int]]]
    packet_bytes: Required[int]
    transfer_ticks: Required[int]


class HardwareWindowAnalysis(TypedDict, total=False):
    basis: Required[str]
    profile: Required[str]
    ticks_per_window: Required[int]


class HeapAllocation(TypedDict, total=False):
    allocate: Required[int]
    configuration: Required[Configuration]
    release: Required[int]


class HomeSwitch(TypedDict, total=False):
    side: NotRequired[HomeSwitchSide | None]
    threshold: Required[int]


type HomeSwitchSide = Literal["above", "below"]


type InputBinding = InputBindingVariant0 | InputBindingVariant1 | InputBindingVariant2


class InputBindingVariant0(TypedDict, total=False):
    kind: Required[Literal["debug_key"]]
    name: Required[str]


class InputBindingVariant1(TypedDict, total=False):
    kind: Required[Literal["native_input"]]
    name: Required[str]


class InputBindingVariant2(TypedDict, total=False):
    kind: Required[Literal["scanner_row"]]
    row: Required[str]


class InputReaders(TypedDict, total=False):
    calibration: NotRequired[int | None]
    scanner: NotRequired[ScannerReader | None]


type InputTarget = InputTargetVariant0 | InputTargetVariant1 | InputTargetVariant2


class InputTargetVariant0(TypedDict, total=False):
    control_id: Required[str]
    kind: Required[Literal["control"]]


class InputTargetVariant1(TypedDict, total=False):
    connection_id: Required[str]
    kind: Required[Literal["connection"]]


class InputTargetVariant2(TypedDict, total=False):
    kind: Required[Literal["unclaimed"]]


Ip = TypedDict(
    "Ip",
    {
        "$schema": Required['Literal["https://fujimod.com/schemas/ip.schema.json"]'],
        "device": NotRequired["BlockDevice | None"],
        "evidence": Required["list[Evidence]"],
        "id": Required["str"],
        "parameters": NotRequired["list[str]"],
        "registers": NotRequired["list[BlockRegister]"],
        "schema": Required["int"],
    },
    total=False,
)


class IrisOptics(TypedDict, total=False):
    output: Required[OpticalQuantity]
    positions: Required[list[AperturePosition]]
    travel_limits: Required[list[int]]


type JpegDirection = Literal["encode", "decode"]


class LadderRung(TypedDict, total=False):
    band_high: Required[int]
    band_low: Required[int]
    basis: Required[str]
    control_id: Required[str]
    sample: Required[int]


type LadderWire = LadderWireVariant0 | LadderWireVariant1


class LadderWireVariant0(TypedDict, total=False):
    channel: Required[int]
    kind: Required[Literal["adc"]]


class LadderWireVariant1(TypedDict, total=False):
    kind: Required[Literal["power_controller_register"]]
    register: Required[int]


class LensAssembly(TypedDict, total=False):
    connection_id: Required[str]
    encoder_connection_id: NotRequired[str | None]
    focus: NotRequired[FocusAxis | None]
    home_connections: Required[list[LensHomeConnection]]
    iris: NotRequired[IrisOptics | None]
    part_id: Required[str]
    pulse: Required[PulseInterface]
    route: Required[SerialRoute]


type LensClockMode = (
    LensClockModeVariant0
    | LensClockModeVariant1
    | LensClockModeVariant2
    | LensClockModeVariant3
)


class LensClockModeVariant0(TypedDict, total=False):
    kind: Required[Literal["fixed"]]


class LensClockModeVariant1(TypedDict, total=False):
    kind: Required[Literal["divided"]]
    period_picoseconds: Required[list[int]]
    ticks_per_second: Required[int]


class LensClockModeVariant2(TypedDict, total=False):
    kind: Required[Literal["microstep_stages"]]
    ticks_per_quantum: Required[int]


class LensClockModeVariant3(TypedDict, total=False):
    kind: Required[Literal["encoder_stages"]]
    ticks_per_quantum: Required[int]


class LensHomeConnection(TypedDict, total=False):
    connection_id: Required[str]
    motor: Required[int]


class LensMotor(TypedDict, total=False):
    home: NotRequired[HomeSwitch | None]
    initial_position: Required[int]
    settle_ticks: Required[int]
    ticks_per_step: Required[int]
    travel_limits: Required[list[int]]


class LensMotorCounter(TypedDict, total=False):
    channel: Required[int]
    divider: Required[int]
    motor: Required[int]


class LensMount(TypedDict, total=False):
    controller_id: Required[str]
    detect_connection_id: Required[str]
    lens_part_id: NotRequired[str | None]
    ready_connection_id: Required[str]
    ready_function: NotRequired[int | None]
    ready_high_line: Required[int]
    ready_low_line: Required[int]
    sense_connection_ids: Required[list[str]]
    supply_connection_ids: Required[list[str]]


type LensProtocol = Literal["banked", "three-byte", "mixed-width", "dc-zoom"]


type LensStopResponse = Literal["settled"]


class LocatedInput(TypedDict, total=False):
    debug_key: NotRequired[str | None]
    electrical: NotRequired[ElectricalInput | None]
    event: Required[int]
    id: Required[str]
    scanner: NotRequired[LocatedScannerRecord | None]
    target: Required[InputTarget]


class LocatedScannerRecord(TypedDict, total=False):
    index: Required[int]
    witness: Required[ScannerRecordWitness]


class MProfileState(TypedDict, total=False):
    cpuid: NotRequired[int | None]
    vector_base: Required[int]


class ManualReference(TypedDict, total=False):
    pages: Required[list[int]]
    title: Required[str]
    url: Required[str]


type Mechanism = MechanismVariant0 | MechanismVariant1


class MechanismVariant0(TypedDict, total=False):
    address_alias: Required[int]
    build: NotRequired[int | None]
    kind: Required[Literal["mailbox"]]
    search_address: Required[int]
    search_bytes: Required[int]


class MechanismVariant1(TypedDict, total=False):
    allocate: Required[int]
    bulk_source: NotRequired[int | None]
    cache: Required[int]
    callback_offset: Required[int]
    callback_table_pointer: Required[int]
    chunk_bytes: Required[int]
    close: Required[int]
    completion: Required[int]
    directory_attribute_offset: Required[int]
    directory_context_bytes: Required[int]
    directory_first_code: Required[int]
    directory_next_code: Required[int]
    directory_size_bytes: Required[int]
    directory_size_offset: Required[int]
    dispatcher: Required[int]
    drive_map: Required[int]
    end_of_directory_status: Required[int]
    error: Required[int]
    kind: Required[Literal["stock_sdk"]]
    list_first_code: Required[int]
    list_next_code: Required[int]
    normal_ui: Required[int]
    open: Required[int]
    pool_begin: Required[int]
    pool_bytes: Required[int]
    pool_id: Required[int]
    query_output: Required[int]
    read: Required[int]
    receiver_cell: Required[int]
    release: Required[int]
    request: Required[int]
    sdk_table: Required[int]
    sdk_table_cell: Required[int]
    seek: Required[int]
    send: Required[int]
    status: Required[int]
    stock_callback: Required[int]
    table_bytes: Required[int]
    table_sha256: Required[str]
    ui_address: Required[int]
    unlink: Required[int]
    value: Required[int]
    write: Required[int]


type MemoryKind = Literal["dram", "rom", "sram", "tcm"]


class MemoryRegion(TypedDict, total=False):
    base: Required[int]
    consumers: NotRequired[list[str]]
    id: Required[str]
    kind: Required[MemoryKind]
    power_on_ram_fill: NotRequired[PowerOnFill | None]
    size: Required[RegionSize]


class MenuFields(TypedDict, total=False):
    flags_word: Required[int]
    handlers_pointer: Required[int]
    item_array: Required[int]
    menu_id: Required[int]
    parent_pointer: Required[int]
    rows_per_page: Required[int]
    setting_id: Required[int]
    type_code: Required[int]


class MenuKeys(TypedDict, total=False):
    back: Required[str]
    down: Required[str]
    menu: Required[str]
    up: Required[str]


class MenuLayout(TypedDict, total=False):
    caption_words: Required[int]
    fields: Required[MenuFields]
    max_kind: Required[int]
    nowrap_flags: Required[int]
    row_child: Required[int]
    row_key: Required[int]
    row_size: Required[int]
    sentinel: Required[int]
    size: Required[int]


class MenuNavigation(TypedDict, total=False):
    setup_first_tab: Required[int]
    shooting_first_tab: Required[int]
    shooting_last_tab: Required[int]


class MenuTables(TypedDict, total=False):
    cursor: Required[CursorLayout]
    cursor_address: Required[int]
    layout: Required[MenuLayout]
    navigation: Required[MenuNavigation]
    page_tabs: Required[MenuTablesVariant0]
    q_grid: Required[QuickGrid]
    roles: Required[dict[str, int]]
    root_count: Required[int]
    root_routes: Required[MenuTablesVariant1]
    root_titles: Required[MenuTablesVariant2]
    roots_address: Required[int]


type MenuTablesVariant0 = dict[str, Never]


type MenuTablesVariant1 = dict[str, Never]


type MenuTablesVariant2 = dict[str, Never]


class MessageQueue(TypedDict, total=False):
    enqueue: Required[int]
    id: Required[int]
    parameters: Required[MessageQueueVariant0]
    payload_bytes: Required[int]
    sender: Required[int]
    uninitialized_payload_offsets: Required[list[int]]


type MessageQueueVariant0 = dict[str, Never]


class MicrocontrollerAssembly(TypedDict, total=False):
    channel: Required[int]
    connection_id: Required[str]
    controller_id: Required[str]
    part_id: Required[str]
    select_connection_id: Required[str]
    word_bits: Required[int]


class NamedRange(TypedDict, total=False):
    address: Required[int]
    name: Required[str]
    size: Required[int]


class NamedSetting(TypedDict, total=False):
    offset: Required[int]
    store: Required[str]
    values: Required[dict[str, list[int]]]


class NandGeometry(TypedDict, total=False):
    bytes: Required[int]
    page_bytes: Required[int]
    pages_per_block: Required[int]
    spare_bytes: Required[int]


class NandPageReader(TypedDict, total=False):
    controller_mode: Required[int]
    destination_alignment: Required[int]
    event_mode: Required[int]
    lock: Required[int]
    max_pages_per_call: Required[int]
    page_bytes: Required[int]
    pages_per_block: Required[int]
    timeout_ms: Required[int]
    unlock: Required[int]


class NativeBlock11000InputsControlMode(TypedDict, total=False):
    ClearBit0Ack: Required[NativeBlock11000InputsControlModeClearbit0ack]


class NativeBlock11000InputsControlModeClearbit0ack(TypedDict, total=False):
    delay_ticks: Required[int]


type NativeControlBusProfile = Literal[
    "endpoint7-stationary-and9-heartbeat-latched-irq-unmeasured",
    "usbcharge-disconnected-pwic-startup-tagged",
]


type NativeDisplayTimingInputsMode = Literal["configured_clocked"]


class NativeEncoderInputsPinsItem(TypedDict, total=False):
    pin: Required[int]
    port: Required[int]


type NativeExr2InterruptControllerInputsArbitration = Literal["active-bitmap"]


type NativeFaceDetectorInputsMode = Literal["retained_words"]


class NativeGpioInputsOutputLevelsItem(TypedDict, total=False):
    high: Required[bool]
    pin: Required[NativeGpioInputsOutputLevelsItemPin]


class NativeGpioInputsOutputLevelsItemPin(TypedDict, total=False):
    pin: Required[int]
    port: Required[int]


type NativeImageCombinationInputsArithmetic = Literal["packed_image_unmeasured"]


class NativeImageCombinationInputsClock(TypedDict, total=False):
    bytes_per_tick: Required[int]
    kind: Required[NativeImageCombinationInputsClockKind]


type NativeImageCombinationInputsClockKind = Literal["throughput"]


class NativeImageInputInputsClock(TypedDict, total=False):
    companion_secondary: Required[bool]
    mode: Required[NativeImageInputInputsClockMode]
    secondary: Required[bool]
    source: Required[NativeImageInputInputsClockSource]


type NativeImageInputInputsClockMode = Literal["input_enable"]


class NativeImageInputInputsClockSource(TypedDict, total=False):
    first_delay_ticks: NotRequired[int | None]
    kind: Required[NativeImageInputInputsClockSourceKind]
    period_ticks: Required[int]


type NativeImageInputInputsClockSourceKind = Literal["periodic"]


type NativeImageInputInputsCommand = Literal["packed_writes"]


type NativeImageInputInputsPhaseAck = Literal["inactive"]


class NativeKeyName(TypedDict, total=False):
    event: Required[int]
    name: Required[str]
    string_address: Required[int]


class NativeKeyNames(TypedDict, total=False):
    reader: Required[int]
    rows: Required[list[NativeKeyName]]
    sha256: Required[str]
    size: Required[int]


type NativeLzInputsResize = Literal["declared_packed_pyramid"]


class NativeOutputChannelInputsClock(TypedDict, total=False):
    kind: Required[NativeOutputChannelInputsClockKind]
    ticks: Required[int]


type NativeOutputChannelInputsClockKind = Literal["fixed"]


type NativeOutputChannelInputsGeometry = Literal["skip_incomplete"]


type NativePhaseDetectionInputsArithmetic = Literal["aperture_sad"]


class NativePowerInputsSpi0(TypedDict, total=False):
    presence: Required[NativePowerInputsSpi0Presence]
    required: Required[list[int]]
    selection: Required[NativePowerInputsSpi0Selection]


class NativePowerInputsSpi0Presence(TypedDict, total=False):
    high: Required[bool]
    pin: Required[NativePowerInputsSpi0PresencePin]


class NativePowerInputsSpi0PresencePin(TypedDict, total=False):
    pin: Required[int]
    port: Required[int]


class NativePowerInputsSpi0Selection(TypedDict, total=False):
    high: Required[bool]
    pin: Required[NativePowerInputsSpi0SelectionPin]


class NativePowerInputsSpi0SelectionPin(TypedDict, total=False):
    pin: Required[int]
    port: Required[int]


type NativeScanoutInputsBackdropProfile = Literal["declared-black-canvas-unmeasured"]


type NativeScanoutInputsGraphicsProfile = Literal["mode1-abgr8888-straight"]


type NativeScanoutInputsMatrix = Literal["bt601-full"]


type NativeScanoutInputsPacking = Literal["yycbcr"]


type NativeScanoutInputsRectangleProfile = Literal[
    "exr2-signed-ycbcr-rectangle-overlay-unmeasured",
]


class NativeStatisticsInputsClock(TypedDict, total=False):
    kind: Required[NativeStatisticsInputsClockKind]
    ticks: Required[int]


type NativeStatisticsInputsClockKind = Literal["fixed"]


class NativeUsbEndpoints(TypedDict, total=False):
    dma_limit: Required[int]
    in_endpoints: Required[list[tuple[int, int]]]
    out_endpoints: Required[list[tuple[int, int]]]
    packet_bytes: Required[int]
    transfer_ticks: Required[int]


type NativeVectorGpuInputsClearProfile = Literal["declared-vg-ui-primitives-unmeasured"]


type NativeVectorGpuInputsCopyProfile = Literal["vg-32bpp-untile-unrotated"]


class NorSampling(TypedDict, total=False):
    evidence: Required[Evidence]
    miss_byte: Required[int]
    sample_first: Required[int]
    sample_last: Required[int]


type OpticalQuantity = Literal["lens_focus", "lens_aperture"]


class ParameterStorage(TypedDict, total=False):
    address: Required[int]
    bytes: Required[int]


Part = TypedDict(
    "Part",
    {
        "$schema": Required['Literal["https://fujimod.com/schemas/part.schema.json"]'],
        "attachment": NotRequired["PartAttachment | None"],
        "id": Required["str"],
        "model": Required["PartModel"],
        "schema": Required["int"],
    },
    total=False,
)


type PartAssemblyEdge = Literal["rising", "falling", "both"]


class PartAssemblyInterrupt(TypedDict, total=False):
    core: Required[int]
    line: Required[int]


class PartAssemblyPin(TypedDict, total=False):
    pin: Required[int]
    port: Required[int]


class PartAssemblyPinInterrupt(TypedDict, total=False):
    edge: Required[PartAssemblyEdge]
    outputs: Required[list[PartAssemblyInterrupt]]
    pin: Required[PartAssemblyPin]


class PartAssemblyPinLevel(TypedDict, total=False):
    high: Required[bool]
    pin: Required[PartAssemblyPin]


class PartAssemblyPositionBridge(TypedDict, total=False):
    clock_interrupt: Required[PartAssemblyInterrupt]
    initial_position: Required[int]
    position_units_per_step: Required[int]
    pulse_channel: Required[int]
    pulse_function: Required[int]
    pulse_pin: Required[PartAssemblyPin]


type PartAssemblyQueue = Literal["single", "fifo"]


class PartAssemblySerialAttachment(TypedDict, total=False):
    controller_address: Required[int]
    endpoint_part_id: Required[str]
    formats: Required[list[PartAssemblySerialFormat]]
    queue: Required[PartAssemblyQueue]
    selection: Required[list[PartAssemblyPinLevel]]
    transfer_ticks: Required[int]


class PartAssemblySerialFormat(TypedDict, total=False):
    bits: Required[int]
    configuration: Required[int]
    control: Required[int]
    receive: Required[bool]


class PartAssemblyZoomAttachment(TypedDict, total=False):
    counter_channel: Required[int]
    home_pin: Required[PartAssemblyPin]
    pulse_channel: Required[int]


class PartAttachment(TypedDict, total=False):
    basis: NotRequired[str | None]
    completion_interrupts: NotRequired[list[PartAssemblyInterrupt] | None]
    contact_pins: NotRequired[list[PartAssemblyPin] | None]
    drive_pins: NotRequired[list[PartAssemblyPin] | None]
    encoder_pins: NotRequired[list[PartAssemblyPin] | None]
    gpio_interrupts: NotRequired[list[PartAssemblyPinInterrupt] | None]
    initial_position: NotRequired[int | None]
    interrupts: NotRequired[list[PartAssemblyInterrupt] | None]
    label: NotRequired[str | None]
    limits: NotRequired[list[str]]
    motor_home_pins: NotRequired[list[PartAssemblyPin | None] | None]
    position_bridge: NotRequired[PartAssemblyPositionBridge | None]
    pulse: NotRequired[PulseInterface | None]
    serial_routes: NotRequired[list[PartAssemblySerialAttachment] | None]
    zoom: NotRequired[PartAssemblyZoomAttachment | None]


class PartEncoder(TypedDict, total=False):
    phase_ticks: NotRequired[int | None]
    rest_phase: Required[int]
    steps_per_phase: NotRequired[int | None]


type PartModel = (
    PartModelVariant0
    | PartModelVariant1
    | PartModelVariant2
    | PartModelVariant3
    | PartModelVariant4
    | PartModelVariant5
    | PartModelVariant6
    | PartModelVariant7
    | PartModelVariant8
    | PartModelVariant9
    | PartModelVariant10
)


class PartModelVariant0(TypedDict, total=False):
    clock_modes: NotRequired[list[LensClockMode] | None]
    direction_high: NotRequired[bool | None]
    encoder: NotRequired[PartEncoder | None]
    kind: Required[Literal["lens-mechanism"]]
    motor_counters: NotRequired[list[LensMotorCounter]]
    motors: Required[list[LensMotor]]
    period_ticks: NotRequired[int | None]
    protocol: Required[LensProtocol]
    stop_response: NotRequired[LensStopResponse | None]
    zoom: NotRequired[ZoomMechanism | None]


class PartModelVariant1(TypedDict, total=False):
    kind: Required[Literal["contact-actuator"]]
    positions: Required[list[ContactPosition]]
    settle_ticks: Required[int]


class PartModelVariant10(TypedDict, total=False):
    hold_drives: Required[list[int]]
    kind: Required[Literal["contact-cam"]]
    latch_overrun_ticks: Required[int]
    positions: Required[list[ContactCamPosition]]
    release_drives: Required[list[int]]
    release_ticks: Required[int]
    run_drive: Required[int]


class PartModelVariant2(TypedDict, total=False):
    endpoint: Required[SerialOutputKind]
    kind: Required[Literal["serial-output"]]


class PartModelVariant3(TypedDict, total=False):
    clock_registers: Required[RegisterRange]
    initial_registers: Required[list[int]]
    kind: Required[Literal["microcontroller"]]
    packet_registers: Required[RegisterRange]
    registers: Required[RegisterRange]
    status_register: NotRequired[int | None]


class PartModelVariant4(TypedDict, total=False):
    kind: Required[Literal["serial-register-bank"]]
    readable: Required[bool]
    registers: Required[list[int]]


class PartModelVariant5(TypedDict, total=False):
    kind: Required[Literal["power-control"]]
    status_registers: Required[list[StatusRegister]]


class PartModelVariant6(TypedDict, total=False):
    kind: Required[Literal["sd-card-slot"]]


class PartModelVariant7(TypedDict, total=False):
    black_level: Required[int]
    color_filter: Required[list[int]]
    control: Required[SensorControlInterface]
    full_scale: Required[int]
    kind: Required[Literal["image-sensor"]]
    radiometry: NotRequired[SensorRadiometry | None]
    raster: Required[SensorRaster]
    reference_white: Required[list[int]]


class PartModelVariant8(TypedDict, total=False):
    height: NotRequired[int | None]
    kind: Required[Literal["display-panel"]]
    orientation: NotRequired[DisplayOrientation | None]
    serial: Required[SerialOutputKind]
    used_mask: Required[int]
    width: NotRequired[int | None]


class PartModelVariant9(TypedDict, total=False):
    interface: Required[PeripheralPart]
    kind: Required[Literal["peripheral"]]


class PayloadDeadline(TypedDict, total=False):
    payload_offset: Required[int]
    unit: Required[DeadlineUnit]


type PeripheralPart = (
    PeripheralPartVariant0
    | PeripheralPartVariant1
    | PeripheralPartVariant2
    | PeripheralPartVariant3
    | PeripheralPartVariant4
    | PeripheralPartVariant5
    | PeripheralPartVariant6
    | PeripheralPartVariant7
    | PeripheralPartVariant8
    | PeripheralPartVariant9
    | PeripheralPartVariant10
    | PeripheralPartVariant11
    | PeripheralPartVariant12
    | PeripheralPartVariant13
    | PeripheralPartVariant14
    | PeripheralPartVariant15
    | PeripheralPartVariant16
    | PeripheralPartVariant17
    | PeripheralPartVariant18
    | PeripheralPartVariant19
    | PeripheralPartVariant20
    | PeripheralPartVariant21
    | PeripheralPartVariant22
)


class PeripheralPartVariant0(TypedDict, total=False):
    kind: Required[Literal["focus-actuator"]]


class PeripheralPartVariant1(TypedDict, total=False):
    kind: Required[Literal["iris-actuator"]]


class PeripheralPartVariant10(TypedDict, total=False):
    kind: Required[Literal["power-controller"]]
    port_detection: NotRequired[PowerPortDetection | None]
    registers: NotRequired[list[PowerControllerRegister]]
    transport: NotRequired[PowerControllerTransport | None]


class PeripheralPartVariant11(TypedDict, total=False):
    kind: Required[Literal["serial-calibration-registers"]]


class PeripheralPartVariant12(TypedDict, total=False):
    kind: Required[Literal["bis-configuration-registers"]]


class PeripheralPartVariant13(TypedDict, total=False):
    kind: Required[Literal["lsm6ds-configuration-registers"]]


class PeripheralPartVariant14(TypedDict, total=False):
    bits: Required[int]
    data_words: NotRequired[int | None]
    kind: Required[Literal["serial-command-registers"]]
    miso_high: Required[bool]
    mode: Required[int]
    read_flag: NotRequired[int | None]


class PeripheralPartVariant15(TypedDict, total=False):
    kind: Required[Literal["unidentified-serial-device"]]
    miso_high: Required[bool]


class PeripheralPartVariant16(TypedDict, total=False):
    kind: Required[Literal["fuel-gauge"]]


class PeripheralPartVariant17(TypedDict, total=False):
    kind: Required[Literal["tmd2772-proximity"]]


class PeripheralPartVariant18(TypedDict, total=False):
    kind: Required[Literal["retained-registers"]]


class PeripheralPartVariant19(TypedDict, total=False):
    commands: Required[list[StabilizerCommand]]
    kind: Required[Literal["stabilizer"]]
    status: Required[int]
    telemetry: Required[int]


class PeripheralPartVariant2(TypedDict, total=False):
    kind: Required[Literal["audio-serial-output"]]


class PeripheralPartVariant20(TypedDict, total=False):
    command_prefix: Required[int]
    data_prefix: Required[int]
    kind: Required[Literal["command-data-output"]]
    payload_bytes: Required[int]


class PeripheralPartVariant21(TypedDict, total=False):
    erase_bytes: NotRequired[int | None]
    jedec_id: NotRequired[int | None]
    kind: Required[Literal["serial-nor-flash"]]
    page_bytes: NotRequired[int | None]
    sampling: NotRequired[NorSampling | None]


class PeripheralPartVariant22(TypedDict, total=False):
    jedec_id: NotRequired[int | None]
    kind: Required[Literal["serial-nand-flash"]]
    page_bytes: NotRequired[int | None]
    pages_per_block: NotRequired[int | None]
    spare_bytes: NotRequired[int | None]


class PeripheralPartVariant3(TypedDict, total=False):
    kind: Required[Literal["imaging-serial-output"]]


class PeripheralPartVariant4(TypedDict, total=False):
    kind: Required[Literal["imaging-sensor"]]
    read_gate_mask: Required[int]
    read_gate_register: Required[int]


class PeripheralPartVariant5(TypedDict, total=False):
    fast_readout: Required[int]
    kind: Required[Literal["timed-readout-sensor"]]
    line_ns: Required[int]


class PeripheralPartVariant6(TypedDict, total=False):
    initial_frame: Required[int]
    kind: Required[Literal["framed-command-peer"]]


class PeripheralPartVariant7(TypedDict, total=False):
    center: Required[list[int]]
    drive_gain: Required[list[int]]
    kind: Required[Literal["stabilizer-stage"]]
    lag_shift: Required[int]
    latency_ns: Required[int]
    rest: Required[list[int]]


class PeripheralPartVariant8(TypedDict, total=False):
    kind: Required[Literal["display-panel-serial"]]


class PeripheralPartVariant9(TypedDict, total=False):
    busy_us: Required[int]
    kind: Required[Literal["x-mount-lens"]]
    operation_us: Required[int]
    preset: Required[str]
    sense_unpowered: Required[list[bool]]
    startup_us: Required[int]


class PersistentJournalAppend(TypedDict, total=False):
    checksum_offset: Required[int]
    parity_offset: Required[int]
    payload_asset: Required[str]
    payload_bytes: Required[int]
    payload_offset: Required[int]
    physical_offset: Required[int]
    record_asset: Required[str]
    record_payload_offset: Required[int]


type PersistentPacket = PersistentPacketVariant0 | PersistentPacketVariant1


class PersistentPacketVariant0(TypedDict, total=False):
    bytes: Required[list[int]]
    kind: Required[Literal["bytes"]]


class PersistentPacketVariant1(TypedDict, total=False):
    asset: Required[str]
    bytes: Required[int]
    kind: Required[Literal["window"]]
    offset: Required[int]


class PersistentPatch(TypedDict, total=False):
    bytes: Required[list[int]]
    offset: Required[int]


class PersistentPlacement(TypedDict, total=False):
    asset: Required[str]
    bytes: Required[int]
    managed_index: NotRequired[int | None]
    physical_offset: Required[int]
    source_offset: Required[int]


class PersistentSet(TypedDict, total=False):
    append: NotRequired[PersistentJournalAppend | None]
    mcu_packet: NotRequired[PersistentPacket | None]
    placements: NotRequired[list[PersistentPlacement]]


class PhysicalControl(TypedDict, total=False):
    assignable: Required[bool]
    behavior: NotRequired[ControlBehavior | None]
    connection_ids: NotRequired[list[str]]
    default_function: NotRequired[str | None]
    face: NotRequired[str | None]
    icon: NotRequired[ControlIcon | None]
    location: Required[ControlSurface]
    manual_page: NotRequired[int | None]
    mechanical: Required[bool]
    name: Required[str]
    positions: NotRequired[list[ControlPosition]]
    pushable: Required[bool]
    role: NotRequired[ControlRole | None]
    short_name: NotRequired[str | None]


PhysicalControls = TypedDict(
    "PhysicalControls",
    {
        "$schema": Required["str"],
        "analog_levels": NotRequired["int | None"],
        "body_id": Required["str"],
        "boot": NotRequired["PhysicalControlsVariant0"],
        "controls": Required["dict[str, PhysicalControl]"],
        "id": Required["str"],
        "ladders": NotRequired["list[ControlLadder]"],
        "manual": Required["ManualReference"],
        "schema": Required["int"],
    },
    total=False,
)


class PhysicalControlsVariant0(TypedDict, total=False):
    firmware_update: NotRequired[BootCombination]
    sd: NotRequired[BootCombination]
    service: NotRequired[BootCombination]


type PictureLayout = Literal["abgr8888-straight"]


type PixelFormat = Literal["bgra8888"]


class PowerControlAssembly(TypedDict, total=False):
    channel: Required[int]
    connection_id: Required[str]
    controller_id: Required[str]
    part_id: Required[str]
    select_connection_id: Required[str]


class PowerControllerRegister(TypedDict, total=False):
    count: NotRequired[int]
    evidence: Required[Evidence]
    first: Required[int]
    role: Required[PowerControllerRole]


type PowerControllerRole = Literal[
    "boot-reason",
    "accessory-command",
    "accessory-status",
    "battery-sample",
    "rtc-control",
    "rtc-seconds-low",
    "rtc-seconds-high",
    "rtc-stage-low",
    "rtc-stage-high",
    "settings-header",
    "port-detection-request",
    "port-detection-result",
    "usb-power-status",
]


type PowerControllerTransport = (
    PowerControllerTransportVariant0 | PowerControllerTransportVariant1
)


class PowerControllerTransportVariant0(TypedDict, total=False):
    kind: Required[Literal["direct-register"]]


class PowerControllerTransportVariant1(TypedDict, total=False):
    data_bytes: Required[int]
    kind: Required[Literal["checksum-power-spi"]]
    register_mask: Required[int]
    request_sum: Required[int]
    write_mask: Required[int]


class PowerOff(TypedDict, total=False):
    alarm: Required[AlarmRegisters]
    boot_flag: NotRequired[BootFlag | None]
    deadline_parameter: NotRequired[int | None]
    entry: Required[int]
    fixed_interval: NotRequired[AlarmInterval | None]
    halt: Required[int]
    power_cut: Required[int]
    request_deadline_offset: NotRequired[int | None]
    rollover_parameter: NotRequired[int | None]


class PowerOnFill(TypedDict, total=False):
    byte: Required[int]
    evidence: Required[Evidence]


class PowerPortDetection(TypedDict, total=False):
    accept_mask: Required[int]
    accept_value: Required[int]
    request_register: Required[int]
    request_value: Required[int]
    result_register: Required[int]


type ProcessorArchitecture = Literal["arm32", "aarch64"]


class ProcessorCore(TypedDict, total=False):
    aprofile: NotRequired[AProfileState | None]
    clock_id: NotRequired[str | None]
    evidence: NotRequired[list[Evidence]]
    id: Required[str]
    model: Required[ProcessorModel]
    mprofile: NotRequired[MProfileState | None]
    registers: NotRequired[list[RegisterValue]]
    released: NotRequired[bool | None]
    rprofile: NotRequired[RProfileState | None]


type ProcessorModel = Literal["cortex-r5f", "cortex-a7", "cortex-a53", "cortex-m4"]


type ProcessorRegister = Literal["pc", "sp", "pstate", "cpsr"]


class ProfileField(TypedDict, total=False):
    basis: Required[str]
    bytes: Required[int]
    offset: Required[int]
    source: Required[ProfileFieldSource]
    store: Required[str]


type ProfileFieldSource = (
    ProfileFieldSourceVariant0
    | ProfileFieldSourceVariant1
    | ProfileFieldSourceVariant2
    | ProfileFieldSourceVariant3
)


class ProfileFieldSourceVariant0(TypedDict, total=False):
    kind: Required[Literal["fill"]]
    value: Required[int]


class ProfileFieldSourceVariant1(TypedDict, total=False):
    kind: Required[Literal["bank_copy"]]
    offset: Required[int]


class ProfileFieldSourceVariant2(TypedDict, total=False):
    kind: Required[Literal["bank_bits"]]
    mask: Required[int]
    offset: Required[int]


class ProfileFieldSourceVariant3(TypedDict, total=False):
    kind: Required[Literal["bank_map"]]
    mask: Required[int]
    offset: Required[int]
    otherwise: Required[int]
    otherwise_high: NotRequired[int | None]
    values: Required[list[BankByteValue]]


class ProfileRecord(TypedDict, total=False):
    block: Required[int]
    bytes: Required[int]
    copies: NotRequired[int | None]
    defaults: Required[UpdateRange]
    defaults_offset: Required[int]
    fields: Required[list[SettingsField]]
    managed_index: NotRequired[int | None]
    parity_offset: Required[int]


class PulseInterface(TypedDict, total=False):
    capture_channel: NotRequired[int | None]
    counter_channels: Required[list[int]]
    initial_phase: Required[int]
    quadrature: Required[bool]
    timer_channels: Required[list[int]]


class QueryService(TypedDict, total=False):
    allocate: Required[int]
    barrier: Required[int]
    cache_clean: Required[int]
    checks: Required[list[CodeCheck]]
    configuration_pointer: Required[int]
    functions: Required[list[int]]
    instruction_barrier: Required[int]
    instruction_invalidate: Required[int]
    release: Required[int]
    worker_slots: Required[int]


class QuickGrid(TypedDict, total=False):
    columns: Required[int]
    menu_code: Required[int]
    rows: Required[int]


class RProfileState(TypedDict, total=False):
    atcm: Required[TcmState]
    btcm: Required[TcmState]
    cp14_identity: Required[int]
    data_cache: NotRequired[DataCacheGeometry | None]


class ReadinessAddresses(TypedDict, total=False):
    category_one: Required[int]
    category_two: Required[int]
    excluded_mask: Required[int]
    input_pending: Required[int]
    operation_alert_timer: Required[int]
    override_address: Required[int]


class RecIdle(TypedDict, total=False):
    menu_active_address: NotRequired[int | None]
    mode_address: Required[int]
    mode_size: Required[int]
    pending_mode_address: NotRequired[int | None]
    rec_mode: Required[int]


type RegionSize = RegionSizeVariant0 | RegionSizeVariant1


class RegionSizeVariant0(TypedDict, total=False):
    bytes: Required[int]
    kind: Required[Literal["bytes"]]


class RegionSizeVariant1(TypedDict, total=False):
    kind: Required[Literal["body_dram"]]


type RegisterAccess = Literal["read-only", "write-only", "read-write"]


class RegisterField(TypedDict, total=False):
    bit: Required[int]
    name: Required[str]
    values: NotRequired[list[RegisterFieldValue]]
    width_bits: Required[int]


class RegisterFieldValue(TypedDict, total=False):
    name: Required[str]
    value: Required[int]


class RegisterRange(TypedDict, total=False):
    count: Required[int]
    first: Required[int]
    width_bytes: Required[int]


type RegisterReads = (
    RegisterReadsVariant0
    | RegisterReadsVariant1
    | RegisterReadsVariant2
    | RegisterReadsVariant3
)


class RegisterReadsVariant0(TypedDict, total=False):
    kind: Required[Literal["fixed"]]
    value: Required[int]


class RegisterReadsVariant1(TypedDict, total=False):
    kind: Required[Literal["alternates"]]
    value: Required[int]


class RegisterReadsVariant2(TypedDict, total=False):
    kind: Required[Literal["self-clearing"]]


class RegisterReadsVariant3(TypedDict, total=False):
    kind: Required[Literal["after-write"]]
    value: Required[int]


class RegisterValue(TypedDict, total=False):
    register: Required[ProcessorRegister]
    value: Required[int]


class RegisterWord(TypedDict, total=False):
    register: Required[int]
    value: Required[int]


class RequestConversion(TypedDict, total=False):
    source: Required[int]
    target: Required[int]


type RequestTransport = RequestTransportVariant0 | RequestTransportVariant1


class RequestTransportVariant0(TypedDict, total=False):
    callback_offset: Required[int]
    callback_table: Required[int]
    command: NotRequired[int | None]
    forward: NotRequired[int | None]
    kind: Required[Literal["borrowed_query"]]
    output_bytes: Required[int]
    output_fields: Required[int]
    stock_callback: Required[int]
    tag_decoder: NotRequired[int | None]


class RequestTransportVariant1(TypedDict, total=False):
    data_cache_clean: Required[int]
    instruction_cache_sync: Required[int]
    kind: Required[Literal["console_callback"]]
    ready_words: Required[list[WordValue]]
    slot: Required[int]
    stock_callback: Required[int]
    stub_area: Required[int]
    stub_bytes: Required[int]
    translation: Required[list[WordValue]]


class ResolvedConfiguration(TypedDict, total=False):
    configuration: Required[RunConfiguration]
    reference: Required[ResolvedSelection]


class ResolvedSelection(TypedDict, total=False):
    body: Required[Body]
    bulk: NotRequired[FirmwareBulk | None]
    card_files: NotRequired[FirmwareCard | None]
    control_ids: NotRequired[FirmwareControlIds | None]
    execution: NotRequired[FirmwareExecution | None]
    firmware: Required[Firmware]
    first_start: NotRequired[FirstStartMenu | None]
    flash: NotRequired[FirmwareFlash | None]
    parts: Required[list[Part]]
    physical_controls: NotRequired[PhysicalControls | None]
    restart: NotRequired[FirmwareRestart | None]
    settings: NotRequired[FirmwareSettings | None]
    soc: Required[Soc]
    state: NotRequired[FirmwareState | None]
    ui_event_codes: NotRequired[UiEventCodes | None]
    ui_scene_names: NotRequired[UiSceneNames | None]
    ui_setting_names: NotRequired[UiSettingNames | None]


class Resources(TypedDict, total=False):
    bitmaps: Required[BitmapResources]
    glyph_first: Required[int]
    glyph_last: Required[int]
    glyphs: Required[GlyphResources]
    language: Required[ConfigurationLocation]
    last_caption: Required[int]
    private_text_bias: Required[int]
    private_text_first: Required[int]
    private_text_last: Required[int]
    supplementary_address: Required[int]
    supplementary_first: Required[int]
    supplementary_max_units: Required[int]
    supplementary_stride: Required[int]
    text_max_units: Required[int]
    text_metrics_offset: Required[int]
    text_pointer_offset: Required[int]
    text_record_size: Required[int]
    text_unit_bytes: Required[int]
    text_variants_offset: Required[int]


type RestartKind = Literal["xp4-wake-deadline"]


class RestartMechanism(TypedDict, total=False):
    clock: NotRequired[ClockIo | None]
    configuration_override: NotRequired[ConfigurationOverride | None]
    deadline: NotRequired[PayloadDeadline | None]
    deadline_offset: NotRequired[int | None]
    deferred_timer: NotRequired[DeferredTimer | None]
    early_boot: NotRequired[EarlyBootMechanism | None]
    kind: NotRequired[RestartKind | None]
    parameter_bytes: NotRequired[int | None]
    parameter_storage: NotRequired[ParameterStorage | None]
    power_off: NotRequired[PowerOff | None]
    queue: NotRequired[MessageQueue | None]
    request: NotRequired[int | None]
    request_conversion: NotRequired[RequestConversion | None]
    request_offset: NotRequired[int | None]
    runtime_qualified: NotRequired[bool | None]
    sender: NotRequired[int | None]
    transport: NotRequired[RequestTransport | None]
    ui: Required[RecIdle]


class RootRoute(TypedDict, total=False):
    entry: Required[str]
    tab: NotRequired[int | None]


class RunAdcInputs(TypedDict, total=False):
    block_id: Required[str]
    samples: Required[list[int]]


type RunAudioInputProfile = Literal["declared-silent-stereo-pcm48000"]


class RunAudioSignal(TypedDict, total=False):
    block_id: Required[str]
    sample_rate_hz: Required[int]
    tone_amplitude_thousandths: Required[int]
    tone_hz: Required[int]


type RunAuxiliaryControlProfile = Literal["stock-control3-bit0-clear-ack-unmeasured"]


type RunBackdropProfile = Literal["declared-black-canvas-unmeasured"]


class RunBinaryInput(TypedDict, total=False):
    id: Required[str]
    sha256: Required[str]
    size: Required[int]


class RunBrowserCamera(TypedDict, total=False):
    preview: Required[RunPreview]
    sensor: Required[RunBrowserSensor]
    still_capture: Required[RunStillCapture]
    virtual_card: Required[RunVirtualCard]


class RunBrowserSensor(TypedDict, total=False):
    frames: Required[str]


class RunBusByteTime(TypedDict, total=False):
    block_id: Required[str]
    ticks: Required[int]


type RunCardFilesystem = Literal["fat32"]


class RunCardFormat(TypedDict, total=False):
    label: Required[str]
    partition_start: Required[int]
    sectors_per_cluster: Required[int]


class RunCardInput(TypedDict, total=False):
    connection_id: Required[str]
    present: Required[bool]
    write_protected: Required[bool]


type RunClearProfile = Literal["declared-vg-ui-primitives-unmeasured"]


type RunClearedOption = Literal[
    "mcu_profile",
    "user_journals",
    "producer_data_profile",
    "producer_data_fixture",
    "lens_response_profile",
    "lens_response_ticks",
    "lens_continuation_profile",
]


RunConfiguration = TypedDict(
    "RunConfiguration",
    {
        "$schema": Required[
            'Literal["https://fujimod.com/schemas/emulator-run-configuration.schema.json"]'
        ],
        "audio_signals": NotRequired["list[RunAudioSignal]"],
        "body_id": Required["str"],
        "browser_camera": NotRequired["RunBrowserCamera | None"],
        "bus_byte_times": NotRequired["list[RunBusByteTime]"],
        "dram_address_space": NotRequired["RunDramAddressSpace | None"],
        "execution": NotRequired["RunExecution | None"],
        "firmware_id": Required["str"],
        "id": Required["str"],
        "idle_status_blocks": NotRequired["list[str]"],
        "inputs": NotRequired["RunInputs | None"],
        "overlays": Required["list[str]"],
        "polled_serial_blocks": NotRequired["list[str]"],
        "preset": NotRequired["RunPreset | None"],
        "purpose": Required["RunPurpose"],
        "scene": NotRequired["RunScene | None"],
        "schema": Required["int"],
        "sd_clock_steps": NotRequired["list[RunSdClockSteps]"],
        "start": Required["RunStart"],
        "unobserved_interrupt_blocks": NotRequired["list[str]"],
    },
    total=False,
)


class RunConnectionState(TypedDict, total=False):
    connected: Required[bool]
    connection_id: Required[str]


type RunControlBusProfile = Literal[
    "endpoint7-stationary-and9-heartbeat-latched-irq-unmeasured",
]


type RunCopyProfile = Literal["vg-32bpp-untile-unrotated"]


type RunCounterInputProfile = Literal["declared-lens-step-feedback-unmeasured"]


class RunDramAddressSpace(TypedDict, total=False):
    basis: Required[str]
    limit_bytes: Required[int]


class RunEncoderInput(TypedDict, total=False):
    connection_id: Required[str]
    phase: Required[int]


type RunEngine = Literal["unicorn", "hypervisor"]


type RunEntry = Literal["loader"]


class RunExecution(TypedDict, total=False):
    engine: Required[RunEngine]
    instructions_per_tick: Required[RunInstructionRate]
    mode: Required[RunExecutionMode]
    quantum: Required[int]


type RunExecutionMode = Literal["strict"]


type RunExecutionScope = Literal[
    "emulator-only",
    "isolated-driver",
    "offline-boot-development",
]


type RunFaceRegisterMode = Literal["retained_words"]


type RunFaceResultProfile = Literal["empty", "box64x20", "box172x120"]


type RunFaceStopProfile = Literal["declared-armed-face-dma-idle-abort-unmeasured"]


class RunFactoryOptions(TypedDict, total=False):
    audio_input_profile: NotRequired[RunAudioInputProfile | None]
    auxiliary_control_profile: NotRequired[RunAuxiliaryControlProfile | None]
    auxiliary_control_ticks: NotRequired[int | None]
    backdrop_profile: NotRequired[RunBackdropProfile | None]
    camera_settings: NotRequired[dict[str, str] | None]
    capability_reply: NotRequired[list[int] | None]
    clear_profile: NotRequired[RunClearProfile | None]
    clock_seconds_2000: NotRequired[int | None]
    control_bus_profile: NotRequired[RunControlBusProfile | None]
    copy_profile: NotRequired[RunCopyProfile | None]
    counter_input_profile: NotRequired[RunCounterInputProfile | None]
    display_frame_ticks: NotRequired[int | None]
    entry: NotRequired[RunEntry | None]
    face_register_mode: NotRequired[RunFaceRegisterMode | None]
    face_result_profile: NotRequired[RunFaceResultProfile | None]
    face_stop_profile: NotRequired[RunFaceStopProfile | None]
    graphics_profile: NotRequired[RunGraphicsProfile | None]
    hdmi_sink: NotRequired[RunHdmiSink | None]
    histogram_data_ticks: NotRequired[int | None]
    histogram_profile: NotRequired[RunHistogramProfile | None]
    image_work_data_profile: NotRequired[RunImageWorkDataProfile | None]
    image_work_data_ticks: NotRequired[int | None]
    image_work_stop_profile: NotRequired[RunImageWorkStopProfile | None]
    image_work_stop_ticks: NotRequired[int | None]
    input_event_profile: NotRequired[RunInputEventProfile | None]
    input_event_ticks: NotRequired[int | None]
    input_phase_profile: NotRequired[RunInputPhaseProfile | None]
    input_processing_profile: NotRequired[RunInputProcessingProfile | None]
    instructions_per_tick: NotRequired[int | None]
    irq_tie_profile: NotRequired[RunIrqTieProfile | None]
    jpeg_decode_profile: NotRequired[RunJpegDecodeProfile | None]
    lens_continuation_profile: NotRequired[RunLensContinuationProfile | None]
    lens_response_profile: NotRequired[RunLensResponseProfile | None]
    lens_response_ticks: NotRequired[int | None]
    mcu_profile: NotRequired[RunMcuProfile | None]
    movrsz_profile: NotRequired[RunMovrszProfile | None]
    movrsz_ticks: NotRequired[int | None]
    phase_profile: NotRequired[RunPhaseProfile | None]
    producer_ack_ticks: NotRequired[int | None]
    producer_data_profile: NotRequired[RunProducerDataProfile | None]
    producer_data_ticks: NotRequired[int | None]
    producer_stop_profile: NotRequired[RunProducerStopProfile | None]
    producer_stop_ticks: NotRequired[int | None]
    pulse_timer_profile: NotRequired[RunPulseTimerProfile | None]
    quantum: NotRequired[int | None]
    raw_readback_ticks: NotRequired[int | None]
    rectangle_profile: NotRequired[RunRectangleProfile | None]
    sd_card_format: NotRequired[RunCardFormat | None]
    sd_card_present: NotRequired[bool | None]
    sd_card_profile: NotRequired[RunSdCardProfile | None]
    sd_card_sectors: NotRequired[int | None]
    secondary_bus_profile: NotRequired[RunSecondaryBusProfile | None]
    sensor_frame_clock: NotRequired[RunSensorFrameClock | None]
    settings: NotRequired[RunSettings | None]
    spi0_input_profile: NotRequired[RunSpi0InputProfile | None]
    storage_profile: NotRequired[RunStorageProfile | None]
    user_journals: NotRequired[RunUserJournals | None]
    video_encoder_backend: NotRequired[VideoEncoderBackend | None]
    wifi_sdio_profile: NotRequired[RunWifiSdioProfile | None]
    wifi_sdio_transfer_ticks: NotRequired[int | None]


class RunFirstStart(TypedDict, total=False):
    hold_ticks: Required[int]
    key_id: Required[str]
    language: NotRequired[str | None]
    max_presses: Required[int]
    settle_ticks: Required[int]


class RunGpioInputs(TypedDict, total=False):
    block_id: Required[str]
    connections: Required[list[RunConnectionState]]
    keys: Required[list[RunKeyState]]
    outputs: Required[list[RunGpioOutputState]]
    unassigned_input_level: Required[bool]


class RunGpioOutputState(TypedDict, total=False):
    connection_id: Required[str]
    high: Required[bool]
    signal_index: Required[int]


type RunGraphicsProfile = Literal["mode1-abgr8888-straight"]


class RunHdmiSink(TypedDict, total=False):
    basis: Required[str]
    connected: Required[bool]
    edid: Required[RunBinaryInput]


type RunHistogramProfile = Literal["declared-postisp-histogram256-u32-unmeasured"]


class RunImage(TypedDict, total=False):
    asset: Required[str]
    height: Required[int]
    stride: Required[int]
    width: Required[int]


type RunImageWorkDataProfile = Literal[
    "declared-postisp-yycbcr-scene-unmeasured",
    "xm1-150-declared-continuous-postisp-scene-unmeasured",
]


type RunImageWorkStopProfile = Literal[
    "stock-idle-image-banks-bit0-stop-ack-unmeasured",
    "stock-initial-image-banks-bit0-stop-ack-unmeasured",
]


class RunImages(TypedDict, total=False):
    live_image: Required[RunImage]


type RunInputEventProfile = Literal[
    "declared-rec-display-card-late-source-staging-unmeasured",
    "declared-rec-display-close-reentry-plus-one-ready-source0-edge-unmeasured",
    "xm1-150-configured-phase-clock-unmeasured",
]


type RunInputPhaseProfile = Literal[
    "declared-enabled-or-inactive-phase2-stop-ack-unmeasured",
    "stock-inactive-phase2-ack",
    "xm1-150-phase2-ack-unmeasured",
]


type RunInputProcessingProfile = Literal[
    "packed-three-byte-command-words",
    "stock-mode0-81-packed-writes",
    "xm1-150-packed-command-writes",
]


type RunInputs = RunInputsVariant0 | RunInputsVariant1 | RunInputsVariant2


class RunInputsVariant0(TypedDict, total=False):
    adc: Required[RunAdcInputs]
    card: Required[RunCardInput]
    encoders: Required[list[RunEncoderInput]]
    first_start: NotRequired[RunFirstStart | None]
    gpio: Required[RunGpioInputs]
    kind: Required[Literal["exr2"]]
    mcu: Required[RunMcuInput]
    power: Required[RunPowerInput]
    serial_registers: Required[list[RunSerialRegisters]]
    settings: NotRequired[RunPowerOnSettings | None]
    usb: Required[RunUsbInput]


class RunInputsVariant1(TypedDict, total=False):
    blocks: Required[list[ApplicationBlockInput]]
    kind: Required[Literal["xp4"]]
    ladders: NotRequired[list[ApplicationLadderInput]]
    parts: Required[list[ApplicationPartInput]]
    ticks_per_second: Required[int]
    timings: Required[list[ApplicationTiming]]


class RunInputsVariant2(TypedDict, total=False):
    blocks: Required[list[ApplicationBlockInput]]
    kind: Required[Literal["xp5"]]
    ladders: NotRequired[list[ApplicationLadderInput]]
    parts: Required[list[ApplicationPartInput]]
    ticks_per_second: Required[int]
    timings: Required[list[ApplicationTiming]]


class RunInstructionRate(TypedDict, total=False):
    denominator: Required[int]
    numerator: Required[int]


type RunIrqTieProfile = Literal["kernel-active-scan-order"]


type RunJpegDecodeProfile = Literal[
    "declared-channel9-libjpeg-yycbcr-playback-unmeasured",
]


class RunKeyState(TypedDict, total=False):
    connection_id: Required[str]
    pressed: Required[bool]


type RunLensContinuationProfile = Literal[
    "declared-lens-controller-close-focus-reference-target222-unmeasured",
]


type RunLensResponseProfile = Literal["declared-lens-startup-homing-lattice-unmeasured"]


class RunMcuInput(TypedDict, total=False):
    packet: Required[list[int]]
    part_id: Required[str]
    rtc: Required[RunRtcInput]


type RunMcuProfile = Literal["disabled", "captured-backup"]


type RunMovrszProfile = Literal["declared-packed-yycbcr-pyramid-unmeasured"]


class RunNandAssetPlacement(TypedDict, total=False):
    asset_id: Required[str]
    offset: Required[int]


class RunNandGeometry(TypedDict, total=False):
    bytes: Required[int]
    page_bytes: Required[int]
    pages_per_block: Required[int]
    spare_bytes: Required[int]


type RunNandSceneInput = RunNandSceneInputVariant0


class RunNandSceneInputVariant0(TypedDict, total=False):
    base: Required[str]
    kind: Required[Literal["raw"]]
    options: Required[RunNandGeometry]
    overlays: Required[list[RunNandAssetPlacement]]
    spare: Required[list[RunNandAssetPlacement]]


type RunNativeDeviceInputs = (
    RunNativeDeviceInputsVariant0
    | RunNativeDeviceInputsVariant1
    | RunNativeDeviceInputsVariant2
    | RunNativeDeviceInputsVariant3
    | RunNativeDeviceInputsVariant4
    | RunNativeDeviceInputsVariant5
    | RunNativeDeviceInputsVariant6
    | RunNativeDeviceInputsVariant7
    | RunNativeDeviceInputsVariant8
    | RunNativeDeviceInputsVariant9
    | RunNativeDeviceInputsVariant10
    | RunNativeDeviceInputsVariant11
    | RunNativeDeviceInputsVariant12
    | RunNativeDeviceInputsVariant13
    | RunNativeDeviceInputsVariant14
    | RunNativeDeviceInputsVariant15
    | RunNativeDeviceInputsVariant16
    | RunNativeDeviceInputsVariant17
    | RunNativeDeviceInputsVariant18
    | RunNativeDeviceInputsVariant19
    | RunNativeDeviceInputsVariant20
    | RunNativeDeviceInputsVariant21
    | RunNativeDeviceInputsVariant22
    | RunNativeDeviceInputsVariant23
    | RunNativeDeviceInputsVariant24
    | RunNativeDeviceInputsVariant25
    | RunNativeDeviceInputsVariant26
    | RunNativeDeviceInputsVariant27
    | RunNativeDeviceInputsVariant28
    | RunNativeDeviceInputsVariant29
    | RunNativeDeviceInputsVariant30
    | RunNativeDeviceInputsVariant31
    | RunNativeDeviceInputsVariant32
    | RunNativeDeviceInputsVariant33
    | RunNativeDeviceInputsVariant34
    | RunNativeDeviceInputsVariant35
    | RunNativeDeviceInputsVariant36
    | RunNativeDeviceInputsVariant37
    | RunNativeDeviceInputsVariant38
    | RunNativeDeviceInputsVariant39
)


class RunNativeDeviceInputsVariant0(TypedDict, total=False):
    arbitration: Required[NativeExr2InterruptControllerInputsArbitration]
    kind: Required[Literal["exr2-interrupt-controller"]]


class RunNativeDeviceInputsVariant1(TypedDict, total=False):
    kind: Required[Literal["exr2-system-timers"]]
    main_targets: Required[list[int]]


class RunNativeDeviceInputsVariant10(TypedDict, total=False):
    kind: Required[Literal["sdhci-clocks"]]


class RunNativeDeviceInputsVariant11(TypedDict, total=False):
    conversion_ticks: Required[int]
    kind: Required[Literal["adc"]]
    samples: Required[list[int]]


class RunNativeDeviceInputsVariant12(TypedDict, total=False):
    ack_ticks: Required[int]
    clock: Required[NativeStatisticsInputsClock]
    external_statistics: Required[bool]
    histogram: Required[bool]
    kind: Required[Literal["statistics"]]
    stop_ticks: Required[int]


class RunNativeDeviceInputsVariant13(TypedDict, total=False):
    clock: Required[NativeOutputChannelInputsClock]
    geometry: Required[NativeOutputChannelInputsGeometry]
    kind: Required[Literal["output-channel"]]
    stop_ticks: Required[int]


class RunNativeDeviceInputsVariant14(TypedDict, total=False):
    kind: Required[Literal["face-detector"]]
    mode: Required[NativeFaceDetectorInputsMode]


class RunNativeDeviceInputsVariant15(TypedDict, total=False):
    arithmetic: Required[NativePhaseDetectionInputsArithmetic]
    data_ticks: Required[int]
    kind: Required[Literal["phase-detection"]]
    stop_ticks: Required[int]


class RunNativeDeviceInputsVariant16(TypedDict, total=False):
    kind: Required[Literal["external-frames"]]
    period_ticks: Required[int]


class RunNativeDeviceInputsVariant17(TypedDict, total=False):
    clock_ticks: Required[int]
    kind: Required[Literal["sdio-host"]]
    ram_bytes: Required[int]
    rca: Required[int]
    transfer_ticks: Required[int]


class RunNativeDeviceInputsVariant18(TypedDict, total=False):
    erase_ticks: Required[int]
    kind: Required[Literal["nand"]]


class RunNativeDeviceInputsVariant19(TypedDict, total=False):
    kind: Required[Literal["lz"]]
    resize: Required[NativeLzInputsResize]
    resize_ticks: Required[int]


class RunNativeDeviceInputsVariant2(TypedDict, total=False):
    kind: Required[Literal["core-release"]]
    released: Required[bool]


class RunNativeDeviceInputsVariant20(TypedDict, total=False):
    kind: Required[Literal["memory-dma"]]


class RunNativeDeviceInputsVariant21(TypedDict, total=False):
    kind: Required[Literal["serial-dma"]]
    max_transfer_bytes: Required[int]
    ram_bytes: Required[int]


class RunNativeDeviceInputsVariant22(TypedDict, total=False):
    endpoints: NotRequired[NativeUsbEndpoints | None]
    kind: Required[Literal["usb"]]


class RunNativeDeviceInputsVariant23(TypedDict, total=False):
    kind: Required[Literal["power"]]
    max_program_bytes: Required[int]
    spi0: Required[NativePowerInputsSpi0]
    transfer_ticks: Required[int]


class RunNativeDeviceInputsVariant24(TypedDict, total=False):
    kind: Required[Literal["serial-audio"]]


class RunNativeDeviceInputsVariant25(TypedDict, total=False):
    capture_channel: NotRequired[int | None]
    counter_channels: Required[list[int]]
    initial_phase: Required[int]
    kind: Required[Literal["exr2-pulse"]]
    quadrature: Required[bool]
    timer_channels: Required[list[int]]


class RunNativeDeviceInputsVariant26(TypedDict, total=False):
    irq: Required[int]
    kind: Required[Literal["fe80-processing"]]
    pixels_per_tick: Required[int]


class RunNativeDeviceInputsVariant27(TypedDict, total=False):
    cfa: Required[list[int]]
    clock: Required[NativeImageInputInputsClock]
    command: Required[NativeImageInputInputsCommand]
    command_controls: Required[list[int]]
    command_ticks: Required[int]
    kind: Required[Literal["image-input"]]
    phase_ack: Required[NativeImageInputInputsPhaseAck]
    phase_ticks: Required[int]
    raw_dma_ticks: Required[int]
    readback_ticks: Required[int]
    reset_ticks: Required[int]
    sensor_period_ticks: NotRequired[int | None]


class RunNativeDeviceInputsVariant28(TypedDict, total=False):
    control_mode: Required[NativeBlock11000InputsControlMode]
    kind: Required[Literal["block11000"]]


class RunNativeDeviceInputsVariant29(TypedDict, total=False):
    kind: Required[Literal["parameters"]]


class RunNativeDeviceInputsVariant3(TypedDict, total=False):
    input_levels: Required[list[int]]
    kind: Required[Literal["gpio"]]
    output_levels: Required[list[NativeGpioInputsOutputLevelsItem]]


class RunNativeDeviceInputsVariant30(TypedDict, total=False):
    connected: Required[bool]
    edid: Required[list[int]]
    kind: Required[Literal["hdmi-sink"]]


class RunNativeDeviceInputsVariant31(TypedDict, total=False):
    kind: Required[Literal["hdmi"]]


class RunNativeDeviceInputsVariant32(TypedDict, total=False):
    kind: Required[Literal["block86000"]]


class RunNativeDeviceInputsVariant33(TypedDict, total=False):
    bytes_per_tick: Required[int]
    clear_profile: Required[NativeVectorGpuInputsClearProfile]
    copy_bytes_per_tick: Required[int]
    copy_profile: Required[NativeVectorGpuInputsCopyProfile]
    gles_base_ticks: Required[int]
    gles_blended_pixel_microticks: Required[int]
    gles_draw_ticks: Required[int]
    gles_filled_pixel_microticks: Required[int]
    kind: Required[Literal["vector-gpu"]]
    max_clear_bytes: Required[int]
    max_command_capture_bytes: Required[int]
    max_copy_bytes: Required[int]
    max_fill_bytes: Required[int]
    ram_bytes: Required[int]


class RunNativeDeviceInputsVariant34(TypedDict, total=False):
    backdrop_profile: Required[NativeScanoutInputsBackdropProfile]
    graphics_profile: Required[NativeScanoutInputsGraphicsProfile]
    kind: Required[Literal["scanout"]]
    matrix: Required[NativeScanoutInputsMatrix]
    max_pixels: Required[int]
    packing: Required[NativeScanoutInputsPacking]
    ram_bytes: Required[int]
    rectangle_profile: Required[NativeScanoutInputsRectangleProfile]
    reset_ticks: Required[int]


class RunNativeDeviceInputsVariant35(TypedDict, total=False):
    frame_ticks: Required[int]
    kind: Required[Literal["display-timing"]]
    mode: Required[NativeDisplayTimingInputsMode]
    stop_ticks: Required[int]


class RunNativeDeviceInputsVariant36(TypedDict, total=False):
    kind: Required[Literal["formatter"]]
    latch_delay: Required[int]


class RunNativeDeviceInputsVariant37(TypedDict, total=False):
    arithmetic: Required[NativeImageCombinationInputsArithmetic]
    clock: Required[NativeImageCombinationInputsClock]
    kind: Required[Literal["image-combination"]]
    reset_ticks: Required[int]


class RunNativeDeviceInputsVariant38(TypedDict, total=False):
    kind: Required[Literal["encoder"]]
    phase_ticks: Required[int]
    pins: Required[list[NativeEncoderInputsPinsItem]]
    rest_phase: Required[int]


class RunNativeDeviceInputsVariant39(TypedDict, total=False):
    kind: Required[Literal["i2c"]]
    modes: Required[list[int]]
    profile: Required[NativeControlBusProfile]
    transfer_ticks: Required[int]


class RunNativeDeviceInputsVariant4(TypedDict, total=False):
    kind: Required[Literal["spi-controller"]]


class RunNativeDeviceInputsVariant5(TypedDict, total=False):
    kind: Required[Literal["dram"]]


class RunNativeDeviceInputsVariant6(TypedDict, total=False):
    kind: Required[Literal["dram-phy"]]


class RunNativeDeviceInputsVariant7(TypedDict, total=False):
    kind: Required[Literal["reset"]]


class RunNativeDeviceInputsVariant8(TypedDict, total=False):
    kind: Required[Literal["clocks"]]
    output_settle_ticks: Required[int]
    reset_ready: Required[bool]


class RunNativeDeviceInputsVariant9(TypedDict, total=False):
    boot_source: Required[int]
    kind: Required[Literal["block-98000"]]
    selection_reset: Required[int]


class RunObserver(TypedDict, total=False):
    instance: Required[int]
    scanout: Required[str]


RunOverlay = TypedDict(
    "RunOverlay",
    {
        "$schema": Required[
            'Literal["https://fujimod.com/schemas/emulator-run-overlay.schema.json"]'
        ],
        "change": Required["RunOverlayChange"],
        "configuration_id": Required["str"],
        "id": Required["str"],
        "schema": Required["int"],
    },
    total=False,
)


type RunOverlayChange = (
    RunOverlayChangeVariant0
    | RunOverlayChangeVariant1
    | RunOverlayChangeVariant2
    | RunOverlayChangeVariant3
    | RunOverlayChangeVariant4
    | RunOverlayChangeVariant5
    | RunOverlayChangeVariant6
)


class RunOverlayChangeVariant0(TypedDict, total=False):
    kind: Required[Literal["preset"]]
    preset: Required[RunPreset]


class RunOverlayChangeVariant1(TypedDict, total=False):
    connected: Required[bool]
    kind: Required[Literal["usb_connection"]]


class RunOverlayChangeVariant2(TypedDict, total=False):
    kind: Required[Literal["settings"]]
    requested: Required[dict[str, str]]


class RunOverlayChangeVariant3(TypedDict, total=False):
    kind: Required[Literal["start"]]
    start: Required[RunStart]


class RunOverlayChangeVariant4(TypedDict, total=False):
    kind: Required[Literal["purpose"]]
    purpose: Required[RunPurpose]


class RunOverlayChangeVariant5(TypedDict, total=False):
    execution: Required[RunExecution]
    kind: Required[Literal["execution"]]


class RunOverlayChangeVariant6(TypedDict, total=False):
    browser_camera: Required[RunBrowserCamera]
    kind: Required[Literal["browser_camera"]]


class RunPadLevel(TypedDict, total=False):
    basis: Required[str]
    level: Required[bool]
    pin: Required[int]
    port: Required[int]


type RunPhaseProfile = Literal["configured-clocked"]


class RunPowerInput(TypedDict, total=False):
    boot_reply: Required[int]
    part_id: Required[str]
    powered_on: Required[bool]
    spi_status: Required[list[RunRegisterValue]]


class RunPowerOnSettings(TypedDict, total=False):
    camera: NotRequired[str | None]
    requested: Required[dict[str, str]]


class RunPreset(TypedDict, total=False):
    browser_camera: NotRequired[RunBrowserCamera | None]
    clear_options: NotRequired[list[RunClearedOption]]
    execution: NotRequired[RunExecution | None]
    execution_scope: NotRequired[RunExecutionScope | None]
    factory_inputs_id: NotRequired[str | None]
    factory_options: Required[RunFactoryOptions]
    files: NotRequired[dict[str, dict[str, str]]]
    firmware_inputs_id: NotRequired[str | None]
    firmware_profile_id: NotRequired[str | None]
    hardware_inputs_id: NotRequired[str | None]
    hardware_notes: NotRequired[dict[str, str]]
    hardware_selection: NotRequired[HardwareSelection | None]
    initial_gpio: NotRequired[list[RunPadLevel]]
    limits: NotRequired[list[str]]
    nand_inputs: NotRequired[dict[str, RunNandSceneInput]]
    native_device_inputs: NotRequired[dict[str, RunNativeDeviceInputs]]
    observer: NotRequired[RunObserver | None]
    purpose: NotRequired[RunPurpose | None]
    start: NotRequired[RunStart | None]
    usb_connected: NotRequired[bool | None]
    virtual_card: NotRequired[RunVirtualCard | None]


class RunPreview(TypedDict, total=False):
    active_statistics_every_frames: Required[int]
    basis: Required[str]
    frames_per_second: Required[int]
    statistics_every_frames: Required[int]
    statistics_profile: Required[str]


type RunProducerDataProfile = Literal[
    "xm1-150-declared-continuous-statistics-scene-unmeasured"] | Literal["recorded-statistics",
]


type RunProducerStopProfile = Literal["stock-idle-stop-handshakes-unmeasured"]


type RunPulseTimerProfile = Literal["exr2-configured-interval-clocks-unmeasured"]


type RunPurpose = Literal[
    "rom_boot",
    "service_session",
    "rehearsal",
    "boot_access",
    "shutdown",
]


type RunRectangleProfile = Literal[
    "exr2-signed-ycbcr-rectangle-overlay-unmeasured",
    "xm1-150-declared-luma-rectangle-overlay-unmeasured",
]


class RunRegisterValue(TypedDict, total=False):
    register: Required[int]
    value: Required[int]


class RunRtcInput(TypedDict, total=False):
    maximum_seconds: Required[int]
    started: Required[int]
    ticks_per_second: Required[int]


type RunScene = RunSceneVariant0 | RunSceneVariant1 | RunSceneVariant2


class RunSceneVariant0(TypedDict, total=False):
    frame_period_ticks: Required[int]
    generated_assets: Required[list[GeneratedSceneAsset]]
    images: Required[RunImages]
    kind: Required[Literal["exr2"]]
    statistics: Required[RunStatistics]


class RunSceneVariant1(TypedDict, total=False):
    command_images: Required[list[ApplicationCommandImageInput]]
    frames: Required[list[ApplicationFrameInput]]
    kind: Required[Literal["xp4"]]
    statistics: Required[list[ApplicationStatisticsInput]]


class RunSceneVariant2(TypedDict, total=False):
    command_images: Required[list[ApplicationCommandImageInput]]
    frames: Required[list[ApplicationFrameInput]]
    kind: Required[Literal["xp5"]]
    statistics: Required[list[ApplicationStatisticsInput]]


type RunSdCardProfile = Literal[
    "declared-blank-sdhc-default-speed-unmeasured",
    "declared-blank-sdhc-high-speed-unmeasured",
]


class RunSdClockSteps(TypedDict, total=False):
    block_id: Required[str]
    default_speed: Required[int]
    high_speed: Required[int]
    interface: Required[int]


type RunSecondaryBusProfile = Literal[
    "usbcharge-disconnected-pwic-startup-tagged"] | Literal["disabled",
]


type RunSensorFrameClock = Literal["sensor", "periodic"]


class RunSerialRegisters(TypedDict, total=False):
    part_id: Required[str]
    values: Required[list[RunRegisterValue]]


type RunSettings = Literal["default", "owner"]


type RunSpi0InputProfile = Literal["absent-hot-shoe-flash-fixed-low-miso"]


type RunStart = (
    RunStartVariant0 | RunStartVariant1 | RunStartVariant2 | RunStartVariant3
)


class RunStartVariant0(TypedDict, total=False):
    kind: Required[Literal["rom"]]


class RunStartVariant1(TypedDict, total=False):
    kind: Required[Literal["loader"]]


class RunStartVariant2(TypedDict, total=False):
    kind: Required[Literal["sd_card"]]


class RunStartVariant3(TypedDict, total=False):
    kind: Required[Literal["setup"]]


class RunStatistics(TypedDict, total=False):
    live_statistics: Required[list[RunStatisticsSpan]]


class RunStatisticsSpan(TypedDict, total=False):
    asset: Required[str]
    destination: Required[int]
    offset: Required[int]


type RunStillCapture = Literal["register-capture-readonly-card-observation"]


type RunStorageProfile = Literal[
    "captured",
    "browser",
    "erased"] | Literal["settings"] | Literal["profile"] | Literal["factory",
]


class RunUsbInput(TypedDict, total=False):
    connected: Required[bool]
    connection_id: Required[str]


type RunUserJournals = Literal["disabled", "restored"]


class RunVirtualCard(TypedDict, total=False):
    filesystem: Required[RunCardFilesystem]
    label: Required[str]
    partition_start: Required[int]
    sectors: Required[int]
    sectors_per_cluster: Required[int]


type RunWifiSdioProfile = Literal[
    "mt5931-no-peers",
    "declared-wifi-sdio-one-function-enumeration-unmeasured",
]


type ScannerInputKind = Literal[
    "kind_0",
    "kind_1",
    "kind_2",
    "kind_3",
    "kind_4",
    "kind_5",
]


class ScannerReader(TypedDict, total=False):
    pressed_value: Required[int]
    released_value: Required[int]
    routine: Required[AddressIdentity]
    stable_offset: Required[int]
    stable_width: Required[int]


class ScannerRecordWitness(TypedDict, total=False):
    input_kind: Required[ScannerInputKind]
    lower: Required[int]
    physical_key: Required[int]
    source: Required[int]
    upper: Required[int]


class ScannerTable(TypedDict, total=False):
    base: Required[int]
    count: Required[int]
    lookup: Required[int]
    sha256: Required[str]
    stride: Required[int]


class Scanout(TypedDict, total=False):
    address: Required[int]
    address_mask: Required[int]
    channel: Required[int]
    height: Required[int]
    instance: Required[int]
    layer: Required[int]
    pixel_format: Required[PixelFormat]
    pointer_address: Required[int]
    stride_bytes: Required[int]
    width: Required[int]


class SdDmaWindow(TypedDict, total=False):
    base: Required[int]
    size: Required[int]


type SectionLength = (
    SectionLengthVariant0 | SectionLengthVariant1 | SectionLengthVariant2
)


class SectionLengthVariant0(TypedDict, total=False):
    bytes: Required[int]
    kind: Required[Literal["fixed"]]


class SectionLengthVariant1(TypedDict, total=False):
    bits: Required[int]
    counts: Required[int]
    kind: Required[Literal["counted"]]


class SectionLengthVariant2(TypedDict, total=False):
    bits: Required[int]
    count: Required[int]
    kind: Required[Literal["counted_list"]]


class SensorAssembly(TypedDict, total=False):
    connection_id: Required[str]
    control_bus: NotRequired[ControlBusAttachment | None]
    part_id: Required[str]


class SensorControlInterface(TypedDict, total=False):
    data_bits: Required[int]
    register_address_bits: Required[int]


type SensorIllumination = (
    Literal["uniform"] | Literal["cosine-fourth"] | SensorIlluminationVariant0
)


SensorIlluminationVariant0 = TypedDict(
    "SensorIlluminationVariant0",
    {
        "radial-polynomial": Required["SensorIlluminationVariant1"],
    },
    total=False,
)


class SensorIlluminationVariant1(TypedDict, total=False):
    r2: Required[int]
    r4: Required[int]


class SensorProjection(TypedDict, total=False):
    focal_length_mm: Required[int]
    relative_illumination: Required[SensorIllumination]
    sensor_width_mm: Required[int]
    subject_distance_mm: Required[int]


class SensorRadiometry(TypedDict, total=False):
    mode_change_frames: Required[int]
    period_ticks: Required[int]
    preview_field: NotRequired[list[int] | None]
    projection: Required[SensorProjection]
    rolling_references: Required[list[SensorRollingReference]]
    saturation_lux_seconds: Required[int]
    shutter_closing_delay_seconds: Required[list[tuple[int, int]]]
    shutter_output: Required[int]
    ticks_per_second: Required[int]
    white_luminance: Required[int]


class SensorRaster(TypedDict, total=False):
    height: Required[int]
    image_area: Required[list[int]]
    pixel_clock_hz: NotRequired[int | None]
    width: Required[int]


class SensorRollingReference(TypedDict, total=False):
    horizontal: Required[int]
    lines: Required[int]
    mode: Required[int]
    vertical: Required[int]


class SerialAttachment(TypedDict, total=False):
    connection_id: Required[str]
    controller_id: Required[str]
    part_id: Required[str]
    select_connection_id: NotRequired[str | None]


type SerialOutputKind = Literal["panel", "registers", "command_data", "fixed"]


class SerialRoute(TypedDict, total=False):
    channel: Required[int]
    clock_hz: NotRequired[int | None]
    controller_id: Required[str]
    endpoint_id: Required[str]
    select_connection_id: Required[str]
    word_bits: Required[int]


class SettingsBank(TypedDict, total=False):
    blocks: Required[int]
    bytes: Required[int]
    descriptor: Required[UpdateRange]
    first_block: Required[int]
    kind: Required[int]
    logical_first: Required[int]


class SettingsField(TypedDict, total=False):
    basis: Required[str]
    bytes: Required[list[int]]
    offset: Required[int]


class SettingsProfile(TypedDict, total=False):
    derived_fields: NotRequired[list[ProfileField]]
    header: Required[UpdateRange]
    loader_offset: NotRequired[int | None]
    marker: NotRequired[int]
    power_register: NotRequired[int | None]
    records: Required[dict[str, ProfileRecord]]


class SettingsSection(TypedDict, total=False):
    basis: Required[str]
    length: Required[SectionLength]
    name: Required[str]
    offset: Required[int]


class SettingsSource(TypedDict, total=False):
    address: Required[int]
    flash_offset: Required[int]
    sha256: Required[str]
    size: Required[int]


Soc = TypedDict(
    "Soc",
    {
        "$schema": Required['Literal["https://fujimod.com/schemas/soc.schema.json"]'],
        "architecture": Required["ProcessorArchitecture"],
        "clocks": NotRequired["list[ClockDomain]"],
        "decoders": NotRequired["list[AddressDecoder]"],
        "dma_mappings": NotRequired["list[DmaMapping]"],
        "hardware": NotRequired["SocHardware | None"],
        "id": Required["str"],
        "mappings": NotRequired["list[AddressMapping]"],
        "memory_regions": Required["list[MemoryRegion]"],
        "processors": Required["list[ProcessorCore]"],
        "reset": Required["SocReset"],
        "schema": Required["int"],
    },
    total=False,
)


class SocBlock(TypedDict, total=False):
    base: Required[int]
    clock_id: NotRequired[str | None]
    device: NotRequired[BlockDevice | None]
    evidence: NotRequired[list[Evidence]]
    id: Required[str]
    interrupts: NotRequired[list[SocBlockInterrupt]]
    ip: NotRequired[str | None]
    kind: Required[SocBlockKind]
    parent_block_id: NotRequired[str | None]
    processor_core_id: NotRequired[str | None]
    registers: NotRequired[list[BlockRegister]]
    size: Required[int]
    views: NotRequired[list[SocBlockView]]


class SocBlockInterrupt(TypedDict, total=False):
    core: NotRequired[str | None]
    line: Required[int]
    output: Required[str]


type SocBlockKind = Literal[
    "gpio",
    "adc",
    "interrupt-controller",
    "timer",
    "dma",
    "control-bus",
    "serial-controller",
    "nand-controller",
    "sd-card-controller",
    "usb-controller",
    "display-controller",
    "imaging-controller",
    "reset-controller",
    "clock-controller",
    "memory-controller",
    "audio-controller",
    "address-window",
]


class SocBlockView(TypedDict, total=False):
    address: Required[int]
    core: Required[str]
    offset: NotRequired[int | None]


class SocHardware(TypedDict, total=False):
    profiles: NotRequired[dict[str, SocProfile]]


class SocProfile(TypedDict, total=False):
    cache_basis: NotRequired[str | None]


class SocReset(TypedDict, total=False):
    coprocessor_writes: NotRequired[list[CoprocessorWrite]]
    release: NotRequired[CoreRelease | None]


class StabilizerCommand(TypedDict, total=False):
    bits: Required[int]
    code: Required[int]
    operation: Required[StabilizerCommandOperation]


type StabilizerCommandOperation = (
    StabilizerCommandOperationVariant0
    | StabilizerCommandOperationVariant1
    | StabilizerCommandOperationVariant2
)


class StabilizerCommandOperationVariant0(TypedDict, total=False):
    kind: Required[Literal["write"]]


class StabilizerCommandOperationVariant1(TypedDict, total=False):
    kind: Required[Literal["select"]]


class StabilizerCommandOperationVariant2(TypedDict, total=False):
    bank: Required[int]
    kind: Required[Literal["ram_write"]]


class StabilizerMount(TypedDict, total=False):
    capture_block_id: Required[str]
    capture_channel: Required[int]
    drive_channels: Required[list[int]]
    part_id: Required[str]


class StateRegion(TypedDict, total=False):
    address: Required[int]
    constant: Required[bool]
    name: Required[str]
    size: Required[int]


class StatusRegister(TypedDict, total=False):
    address: Required[int]
    value: Required[int]


type StillEngineKind = Literal["iifp", "recc", "resz", "reszl", "retch", "mix"]


class TcmState(TypedDict, total=False):
    bytes: Required[int]
    reset_address: Required[int]


class TimerRoutine(TypedDict, total=False):
    address: Required[int]
    size: Required[int]


type TimingOperation = Literal[
    "settle",
    "sample",
    "command",
    "reset",
    "pixel",
    "acknowledge",
    "complete",
]


class TranslatedKey(TypedDict, total=False):
    event: Required[int]
    physical_key: Required[int]


UiEventCodes = TypedDict(
    "UiEventCodes",
    {
        "$schema": Required["str"],
        "firmware_id": Required["str"],
        "firmware_sha256": Required["str"],
        "id": Required["str"],
        "schema": Required["int"],
        "ui_event_codes": Required["dict[str, int]"],
    },
    total=False,
)


UiSceneNames = TypedDict(
    "UiSceneNames",
    {
        "$schema": Required["str"],
        "firmware_id": Required["str"],
        "firmware_sha256": Required["str"],
        "id": Required["str"],
        "schema": Required["int"],
        "ui_scene_names": Required["dict[str, int]"],
    },
    total=False,
)


UiSettingNames = TypedDict(
    "UiSettingNames",
    {
        "$schema": Required["str"],
        "firmware_id": Required["str"],
        "firmware_sha256": Required["str"],
        "id": Required["str"],
        "schema": Required["int"],
        "ui_setting_names": Required["dict[str, int]"],
    },
    total=False,
)


class UpdateRange(TypedDict, total=False):
    address: Required[int]
    sha256: Required[str]
    size: Required[int]


class Upload(TypedDict, total=False):
    address_alignment: Required[int]
    capacity_address: Required[int]
    code_checks: Required[list[CodeCheck]]
    configuration_checks: Required[list[ConfigurationCheck]]
    piece_bytes: Required[int]
    pointer: Required[int]
    pointer_bytes: Required[int]


type VideoEncoderBackend = Literal["videotoolbox", "software"]


type VideoLayout = Literal["yycbcr422"]


type WireSignal = Literal[
    "gpio",
    "adc",
    "serial_data",
    "serial_clock",
    "image_data",
    "display_data",
    "microcontroller",
    "storage",
    "pulse",
    "power",
]


class WordValue(TypedDict, total=False):
    address: Required[int]
    size: Required[int]
    value: Required[int]


class ZoomMechanism(TypedDict, total=False):
    edge_scale: NotRequired[int | None]
    home: NotRequired[HomeSwitch | None]
    initial_position: Required[int]
    maximum_position: Required[int]
    minimum_position: Required[int]
    pulse_ticks: NotRequired[int | None]
