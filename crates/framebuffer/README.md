# Framebuffer capture

The `fujimod-framebuffer` crate contains runtime-independent pixels, register
snapshots, packed color conversion, raster sampling/composition, and BMP output.
The processor crates `framebuffer-exr2`, `framebuffer-xpp`, `framebuffer-xp4`, and
`framebuffer-xp5` own register interpretation. They depend on neither an emulator
nor USB. Display device models retain programming, resets, clocks, interrupts,
and publication, and delegate pixel interpretation to their processor crate.
The camera filesystem uses those same crates with a read-only USB memory reader.
The catalog selects the SoC and explicitly selected pixel transports; firmware
RAM shadow pointers are unnecessary.

This is Rust code, not a second rendering language in JSON. The catalog supplies
hardware selection and parameters. Diagnostic register snapshots can still be
serialized as JSON without embedding pixel payloads.

EXR II preserves its checked group/channel ordering, pixel/channel alpha,
placement, declared sampling and rectangle interpretation. XPP preserves the
implemented packed plane and four premultiplied layers. XP4 preserves the two
selected plane transports and Q12 sampling. Unsupported control bits and formats
remain errors. Modeled nearest sampling and color/blending approximations retain
their original limits; extraction does not establish new hardware validation.

XP5's `Encoding` distinguishes `Linear`, `Unresolved`, and `EmulatorConverted`.
`emulator_codec` describes the explicit converter-to-display replacement
encoding. Physical capture always selects `Unresolved` for converted formats;
it cannot accept the emulator header. Known packed planes can be captured
individually even when another enabled plane prevents full composition.

Run the processor and display-device tests together when changing an algorithm.
The device tests continue exercising registers, completion timing, and actual
composed pixels through the extracted implementation.
