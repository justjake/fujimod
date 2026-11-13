// XP5 internal-flash startup stub: the secondary-core release the RAM-image reset stage needs,
// and an optional load-failure fallback.
//
// Hand-written A64, assembled by build-sd-boot-v1-xp5-flash-stub.sh into
// sd-boot-v1-xp5-flash-stub.generated.bin. The packager appends it to the compiled loader and
// binds its entries after checking the loader words it patches (see xp5/flash_stub.rs).
//
//   release_entry  replaces the `sev` that follows the loader's `a64_boot_ready = 1` store
//                  (the path without a card-resident setup). Before it, the loader never
//                  wakes the cores the boot ROM parks in WFI. The RAM image's reset stage
//                  (preloaded selector) expects them to be awake and waiting for its own
//                  SGI0, so the stub sends SGI0 to cores 1..3 as the card-resident loader
//                  does, then the `sev` it replaced, then returns to the loader.
//
//   fail_entry     (only with a fallback) replaces the body of the loader's `fail()`.
//                  `fail()` is only called from setup_sd_initialize, so x29 is that
//                  function's frame (`stp x29, x30, [sp, #-0x60]!; mov x29, sp`, callee-saved
//                  x19..x26 stored at +0x10..+0x50 before any call of fail()). On a load
//                  failure it
//                  - waits params.wait_seconds seconds with the stock setup timer,
//                  - puts the stock reset image back at RAM 0 if the loader had already
//                    streamed any of FFBOOT over it,
//                  - publishes a64_boot_ready = 2 (a successful load publishes 1; the loader's
//                    zero gate releases on either),
//                  - unwinds setup_sd_initialize, so the stock bootstrap goes on.
//                  A failure that is not a load failure (the request check before the stock
//                  initializer ran, or an unexpected exception level or MMU state) halts as
//                  `fail()` always did.
//
//   There is no gate of its own: the loader's a64_boot_zero_gate releases for any nonzero
//   a64_boot_ready, and the stock continuation it replaces is `ldr x30, =0; ret` (the literal at
//   flash 0x27140 of the X100VI 1.31 update is 0), so a fallback reaches RAM 0, holding the
//   restored stock reset image, exactly as the stock bootstrap does.
//
// params is filled by the packager.

    .text
    .balign 16
    .global fail_entry
fail_entry:
    adr     x9, params
    ldr     x10, [x9, #0x00]            // &setup_sd_progress
    sxtw    x0, w0
    str     x0, [x10, #0x10]            // progress.error, as fail() records it
    ldr     x21, [x10, #0x08]           // progress.stage at the failure
    cmp     x21, #2
    b.lo    halt                        // before the stock initializer ran
    cmn     x0, #101
    b.eq    halt                        // unexpected exception level
    cmn     x0, #102
    b.eq    halt                        // unexpected MMU or cache state
    mov     x19, x9
    ldr     x20, [x19, #0x30]           // wait_seconds
1:  cbz     x20, 2f
    ldr     x0, [x19, #0x38]            // setup-timer units per second
    ldr     x1, [x19, #0x10]            // boot_setup_delay
    blr     x1
    sub     x20, x20, #1
    b       1b
2:  ldr     x10, [x19, #0x00]
    ldr     x0, [x10, #0x18]            // progress.loaded
    cbz     x0, 4f                      // RAM still holds the stock reset image
    ldr     x1, [x19, #0x20]            // restore_source (flash aperture)
    ldr     x2, [x19, #0x28]            // restore_bytes
    mov     x3, xzr                     // RAM 0
3:  ldr     x4, [x1], #8
    str     x4, [x3], #8
    subs    x2, x2, #8
    b.hi    3b
    dsb     sy
    isb
4:  ldr     x0, [x19, #0x08]            // &a64_boot_ready
    mov     w1, #2
    stlr    w1, [x0]
    dsb     sy
    sev
    ldp     x19, x20, [x29, #0x10]      // the compiled function stores x19..x26 in its frame
    ldp     x21, x22, [x29, #0x20]      // before every call of fail() (checked by the packager)
    ldp     x23, x24, [x29, #0x30]
    ldp     x25, x26, [x29, #0x40]
    mov     sp, x29
    ldp     x29, x30, [sp], #0x60
    ret
halt:
    dsb     sy
6:  wfi
    b       6b

    .balign 4
    .global release_entry
release_entry:
    mov     x0, #0xe                    // Aff0 1..3 of the boot cluster
    msr     S3_0_C12_C11_7, x0          // ICC_SGI0R_EL1, INTID 0
    isb
    sev                                 // the instruction this entry replaced
    adr     x1, params
    ldr     x1, [x1, #0x40]             // the loader instruction after it
    br      x1

    .balign 8
    // Local, so the assembler resolves the adr instructions above; exported as stub_params.
params:
    .skip   0x48
    .global stub_params
    .set    stub_params, params
