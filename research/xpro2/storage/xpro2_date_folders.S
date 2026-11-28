@ Link after xpro2_card_name.s. Read the camera-local date once.
@ r0 points to six writable bytes for _MMDD\0. On success r0=1,
@ r1=0xYYYYMMDD, and r2 is the 16-bit FAT date for metadata comparisons.
@ Failure returns r0=r1=r2=0 and leaves the destination intact.
.arm
date_folder_from_camera:
    push {r4, lr}
    sub sp, sp, #16
    mov r4, r0
    mov r0, sp
    bl card_name_from_camera
    mov r1, #0
    mov r2, #0
    cmp r0, #1
    bne folder_date_return

    mov r2, #0
folder_date_digits:
    ldrb r3, [sp, r2]
    add r2, r2, #1
    cmp r3, #45
    beq folder_date_digits
    sub r3, r3, #48
    orr r1, r3, r1, lsl #4
    cmp r2, #10
    blo folder_date_digits

    mov r3, #95
    strb r3, [r4]
    ldrb r3, [sp, #5]
    strb r3, [r4, #1]
    ldrb r3, [sp, #6]
    strb r3, [r4, #2]
    ldrb r3, [sp, #8]
    strb r3, [r4, #3]
    ldrb r3, [sp, #9]
    strb r3, [r4, #4]
    mov r3, #0
    strb r3, [r4, #5]

    @ The validated year is 2000..2107. FAT stores the offset from 1980.
    ubfx r2, r1, #24, #4
    mov r3, #100
    mul r2, r2, r3
    ubfx r3, r1, #20, #4
    add r3, r3, r3, lsl #2
    add r2, r2, r3, lsl #1
    ubfx r3, r1, #16, #4
    add r2, r2, r3
    add r2, r2, #20
    lsl r2, r2, #9

    ubfx r3, r1, #12, #4
    add r3, r3, r3, lsl #2
    ubfx ip, r1, #8, #4
    add r3, ip, r3, lsl #1
    orr r2, r2, r3, lsl #5
    ubfx r3, r1, #4, #4
    add r3, r3, r3, lsl #2
    and ip, r1, #15
    add r3, ip, r3, lsl #1
    orr r2, r2, r3
folder_date_return:
    add sp, sp, #16
    pop {r4, pc}
