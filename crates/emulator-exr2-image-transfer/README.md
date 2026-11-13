# Image sample conversion

`ImageTransfer` owns aligned 32-bit registers at `FFF64000..FFF64020`.
Reset (`00=0`) cancels pending work, clears status and returns ready bit 16
after two declared virtual ticks. The configuration uses control bits 4–5,
low 30 source/destination fields at 04/08, low 15 output pitch in luma samples
at 0C, paired 12-bit dimensions at 10, status 14, W1C 18 and mask 1C.

Control `10101 | alignment<<4` expands I420 input into packed Y0,Y1,Cb,Cr
4:2:2 output. The declared I420 transport stores Y, then Cb, then Cr planes;
its footprint is `width*height*3/2`. This explicit transport is a software model,
not a measurement of physical plane ordering or tiling. Each chroma row repeats
on two adjacent luma rows, preserving luma and performing no resizing.
Alignment is 3/2/1/0 for widths divisible by 128/64/32/other. Only positive
even dimensions and tightly packed output rows (`pitch=width`) are supported.
Overlapping source/destination footprints, unmapped DMA, unknown register bits
and other layouts refuse before DMA.

Source planes and destination geometry latch at START. Output remains unchanged
until the virtual deadline, when all packed bytes write before success status
bit 0 and the configured physical interrupt. The declared clock is 256 output
bytes per tick with a two-tick minimum, independent of host elapsed time.
Status and mask are independent; W1C lowers the interrupt only after all
enabled status bits are acknowledged. Reset explicitly cancels pending work.

The transfer counter and `last_output` receipt publish atomically at completion.
The receipt contains physical addresses, dimensions, source/output packing,
byte pitch/count, separate source/output hashes and start/completion ticks.
Pending metadata cannot relabel the prior completed receipt. Checkpoints retain
the deadline and immutable binary input/output payloads; restore checks their
shape, hashes and conversion relationship. Diagnostics expose metadata and
hashes without embedding pixel buffers.
