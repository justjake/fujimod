@ Owned ARMv7 implementation; no SDK calls, filesystem writes or global state.
@ r0 = writable 11-byte destination; r1/r2/r3 = Gregorian year/month/day.
@ Returns 1 and writes YYYY-MM-DD<space>, or 0 without touching destination.
@ Domain 1980..2107 follows FAT timestamps. This is NOT a camera RTC adapter.
.arm
card_name:
    push {r4-r10, lr}
    mov r4, r0
    mov r5, r1
    mov r6, r2
    mov r7, r3
    movw r8, #1980
    sub r8, r5, r8
    cmp r8, #127
    bhi invalid
    sub r8, r6, #1
    cmp r8, #11
    bhi invalid
    adr r9, month_days
    ldrb r9, [r9, r8]
    cmp r6, #2
    bne check_day
    tst r5, #3
    bne check_day
    movw r8, #2100
    cmp r5, r8
    addne r9, r9, #1
check_day:
    cmp r7, #1
    blo invalid
    cmp r7, r9
    bhi invalid
    mov r0, r5
    mov r1, #1000
    bl emit_digit
    mov r1, #100
    bl emit_digit
    mov r1, #10
    bl emit_digit
    add r0, r0, #48
    strb r0, [r4], #1
    mov r0, #45
    strb r0, [r4], #1
    mov r0, r6
    mov r1, #10
    bl emit_digit
    add r0, r0, #48
    strb r0, [r4], #1
    mov r0, #45
    strb r0, [r4], #1
    mov r0, r7
    mov r1, #10
    bl emit_digit
    add r0, r0, #48
    strb r0, [r4], #1
    mov r0, #32
    strb r0, [r4]
    mov r0, #1
    pop {r4-r10, pc}
invalid:
    mov r0, #0
    pop {r4-r10, pc}
emit_digit:
    mov r2, #48
digit_loop:
    cmp r0, r1
    subhs r0, r0, r1
    addhs r2, r2, #1
    bhs digit_loop
    strb r2, [r4], #1
    bx lr
month_days:
    .byte 31,28,31,30,31,30,31,31,30,31,30,31

@ r0 = writable 11-byte destination. Uses the native camera-local date reader.
@ SDK 0x00b30148 writes six bytes: year-2000, month, day, hour, minute, second.
@ Native SDK return 0 is success. Invalid date/failure leaves output unchanged.
card_name_from_camera:
    push {r4, lr}
    mov r4, r0
    sub sp, sp, #8
    mov r0, sp
    mov r1, #0
    str r1, [sp]
    str r1, [sp, #4]
    ldr ip, =0x00b30148
    blx ip
    cmp r0, #0
    bne camera_invalid
    ldrb r1, [sp]
    add r1, r1, #2000
    ldrb r2, [sp, #1]
    ldrb r3, [sp, #2]
    mov r0, r4
    add sp, sp, #8
    pop {r4, lr}
    b card_name
camera_invalid:
    mov r0, #0
    add sp, sp, #8
    pop {r4, pc}
