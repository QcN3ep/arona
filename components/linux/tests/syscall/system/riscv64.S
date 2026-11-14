# SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

.section .text
.macro invoke number
    li a7, \number
    ecall
.endm
.macro expect value
    li s8, \@
    li t0, \value
    bne a0, t0, system_failure
.endm
.global system_basics
system_basics:
    addi s7, sp, 48
    addi sp, sp, -512
.Laux_random:
    ld t0, 0(s7)
    beqz t0, system_failure
    li t1, 25
    beq t0, t1, .Lrandom_found
    addi s7, s7, 16
    j .Laux_random
.Lrandom_found:
    ld t0, 8(s7)
    ld t1, 0(t0)
    ld t2, 8(t0)
    or t1, t1, t2
    beqz t1, system_failure
    li a0, 1
    mv a1, sp
    invoke 113
    expect 0
    ld s0, 0(sp)
    ld s1, 8(sp)
    li t0, 1000000000
    bgeu s1, t0, system_failure
    sd zero, 16(sp)
    li t0, 2000000
    sd t0, 24(sp)
    addi a0, sp, 16
    li a1, 0
    invoke 101
    expect 0
    li a0, 1
    mv a1, sp
    invoke 113
    expect 0
    ld t0, 0(sp)
    bltu t0, s0, system_failure
    bne t0, s0, .Lelapsed
    ld t0, 8(sp)
    sub t0, t0, s1
    li t1, 2000000
    bltu t0, t1, system_failure
.Lelapsed:
    li a0, 1
    addi a1, sp, 16
    invoke 114
    expect 0
    ld t0, 16(sp)
    bnez t0, system_failure
    ld t0, 24(sp)
    beqz t0, system_failure
    li a0, -1
    mv a1, sp
    invoke 113
    expect -22
    li a0, 1
    li a1, 1
    invoke 113
    expect -14
    li a0, 0
    mv a1, sp
    invoke 113
    expect 0
    ld t0, 0(sp)
    li t1, 946684800
    bltu t0, t1, system_failure
    mv a0, sp
    addi a1, sp, 32
    invoke 169
    expect 0
    ld t0, 8(sp)
    li t1, 1000000
    bgeu t0, t1, system_failure
    ld t0, 32(sp)
    bnez t0, system_failure
    li a0, 0
    li a1, 0
    invoke 169
    expect 0
    sd zero, 16(sp)
    sd zero, 24(sp)
    li a0, 1
    li a1, 1
    addi a2, sp, 16
    li a3, 0
    invoke 115
    expect 0
    li a0, 1
    li a1, 2
    addi a2, sp, 16
    invoke 115
    expect -22
    mv a0, sp
    invoke 160
    expect 0
    lw t0, 0(sp)
    li t1, 0x756e694c
    bne t0, t1, system_failure
    li a0, 1
    invoke 160
    expect -14
    mv a0, sp
    li a1, 32
    li a2, 0
    invoke 278
    blez a0, system_failure
    li t0, 32
    bltu t0, a0, system_failure
    mv t0, sp
    add t1, sp, a0
    li t2, 0
.Lrandom_bytes:
    lbu t3, 0(t0)
    or t2, t2, t3
    addi t0, t0, 1
    bltu t0, t1, .Lrandom_bytes
    beqz t2, system_failure
    li a0, 1
    li a1, 0
    invoke 278
    expect 0
    li a0, 1
    li a1, 1
    invoke 278
    expect -14
    mv a0, sp
    li a1, 1
    li a2, 8
    invoke 278
    expect -22
    jal ra, timer_selftest
    # sysinfo reports real memory, process, and uptime data.
    mv a0, sp
    invoke 179
    expect 0
    ld t0, 32(sp)
    beqz t0, system_failure
    ld t0, 40(sp)
    beqz t0, system_failure
    ld t1, 32(sp)
    bltu t1, t0, system_failure
    lwu t0, 104(sp)
    li t1, 1
    bne t0, t1, system_failure
    lhu t0, 80(sp)
    beqz t0, system_failure
    li a0, 0
    invoke 179
    expect -14

    # times reports clock ticks for the process and its children.
    addi a0, sp, 128
    invoke 153
    bltz a0, system_failure
    ld t0, 128(sp)
    bltz t0, system_failure
    li a0, 0
    invoke 153
    expect -14

    # getrusage rejects unknown targets and faults on a null buffer.
    li a0, 2
    addi a1, sp, 192
    invoke 165
    expect -22
    li a0, 0
    li a1, 0
    invoke 165
    expect -14

    # A terminated child contributes to children CPU time and usage.
    li a0, 17
    li a1, 0
    li a2, 0
    li a3, 0
    li a4, 0
    invoke 220
    beqz a0, .Lchild_burn
    bltz a0, system_failure
    mv s2, a0
    mv a0, s2
    addi a1, sp, 160
    li a2, 0
    li a3, 0
    invoke 260
    bne a0, s2, system_failure
    lw t0, 160(sp)
    bnez t0, system_failure
    addi a0, sp, 128
    invoke 153
    bltz a0, system_failure
    ld t0, 144(sp)
    beqz t0, system_failure
    li a0, -1
    addi a1, sp, 192
    invoke 165
    expect 0
    ld t0, 192(sp)
    ld t1, 200(sp)
    or t0, t0, t1
    beqz t0, system_failure
    li a0, 0
    addi a1, sp, 192
    invoke 165
    expect 0
    ld t0, 224(sp)
    beqz t0, system_failure
    ld t0, 256(sp)
    bnez t0, system_failure
    ld t0, 264(sp)
    bnez t0, system_failure
    ld t0, 320(sp)
    bnez t0, system_failure
    ld t0, 328(sp)
    bnez t0, system_failure
    j .Lsystem_done
.Lchild_burn:
    li t0, 200000000
1:
    addi t0, t0, -1
    bnez t0, 1b
    li a0, 0
    invoke 93
.Lsystem_done:
    li a0, 42
    invoke 94
system_failure:
    addi a0, s8, 100
    invoke 94
    j system_failure
.globl timer_syscall
timer_syscall:
    mv a7, a0
    mv t0, a1
    ld a0, 0(t0)
    ld a1, 8(t0)
    ld a2, 16(t0)
    ld a3, 24(t0)
    ld a4, 32(t0)
    ld a5, 40(t0)
    ecall
    ret

# RTC device paths, rtc_time conversion, and permission checks.
.global hwclock_test
hwclock_test:
    addi sp, sp, -512
    la a1, hwclock_rtc
    li a0, -100
    li a2, 2
    li a3, 0
    li a7, 56
    ecall
    li s8, 1
    bltz a0, system_failure
    mv s6, a0

    # Save the current time so the suite can be restored afterwards.
    mv a0, s6
    li a1, 0x80247009
    mv a2, sp
    li a7, 29
    ecall
    expect 0
    mv s0, sp
    jal ra, hwclock_check_fields
    ld t0, 0(sp)
    sd t0, 440(sp)
    ld t0, 8(sp)
    sd t0, 448(sp)
    ld t0, 16(sp)
    sd t0, 456(sp)
    ld t0, 24(sp)
    sd t0, 464(sp)
    lw t0, 32(sp)
    sw t0, 472(sp)

    # A known time: 2025-03-09 12:34:56 UTC.
    li t0, 56
    sw t0, 40(sp)
    li t0, 34
    sw t0, 44(sp)
    li t0, 12
    sw t0, 48(sp)
    li t0, 9
    sw t0, 52(sp)
    li t0, 2
    sw t0, 56(sp)
    li t0, 125
    sw t0, 60(sp)
    sw zero, 64(sp)
    li t0, 67
    sw t0, 68(sp)
    sw zero, 72(sp)

    # Months are zero-based, so month 12 is invalid.
    li t0, 12
    sw t0, 56(sp)
    mv a0, s6
    li a1, 0x4024700a
    addi a2, sp, 40
    li a7, 29
    ecall
    expect -22
    li t0, 2
    sw t0, 56(sp)

    # Days outside the month and February 30 are invalid.
    li t0, 32
    sw t0, 52(sp)
    mv a0, s6
    li a1, 0x4024700a
    addi a2, sp, 40
    li a7, 29
    ecall
    expect -22
    li t0, 1
    sw t0, 56(sp)
    li t0, 30
    sw t0, 52(sp)
    mv a0, s6
    li a1, 0x4024700a
    addi a2, sp, 40
    li a7, 29
    ecall
    expect -22
    li t0, 2
    sw t0, 56(sp)
    li t0, 9
    sw t0, 52(sp)

    # The valid value is written and read back.
    mv a0, s6
    li a1, 0x4024700a
    addi a2, sp, 40
    li a7, 29
    ecall
    expect 0
    mv a0, s6
    li a1, 0x80247009
    addi a2, sp, 96
    li a7, 29
    ecall
    expect 0
    addi s0, sp, 96
    jal ra, hwclock_check_target

    # Alarm controls are unsupported and unknown commands are not RTC ioctls.
    mv a0, s6
    li a1, 0x00007001
    mv a2, sp
    li a7, 29
    ecall
    expect -22
    mv a0, s6
    li a1, 0
    li a2, 0
    li a7, 29
    ecall
    expect -25

    # Writing needs root credentials; the saved identity allows returning to root.
    li a0, 1
    li a1, 1
    li a2, 0
    li a7, 147
    ecall
    expect 0
    mv a0, s6
    li a1, 0x4024700a
    addi a2, sp, 40
    li a7, 29
    ecall
    expect -1
    mv a0, s6
    li a1, 0x80247009
    addi a2, sp, 96
    li a7, 29
    ecall
    expect 0
    li a0, 1
    li a1, 0
    li a2, 0
    li a7, 147
    ecall
    expect 0
    li a0, 0
    li a1, 0
    li a2, 0
    li a7, 147
    ecall
    expect 0

    # read(2) returns one rtc_time and rejects short buffers.
    mv a0, s6
    addi a1, sp, 96
    li a2, 36
    li a7, 63
    ecall
    li s8, 2
    li t0, 36
    bne a0, t0, system_failure
    addi s0, sp, 96
    jal ra, hwclock_check_target
    mv a0, s6
    addi a1, sp, 136
    li a2, 8
    li a7, 63
    ecall
    expect -22
    mv a0, s6
    li a7, 57
    ecall
    expect 0

    # All three device paths support reads and writes.
    la s4, hwclock_paths
    li s5, 3
.Lhwclock_path:
    ld a1, 0(s4)
    li a0, -100
    li a2, 2
    li a3, 0
    li a7, 56
    ecall
    li s8, 3
    bltz a0, system_failure
    mv s6, a0
    mv a0, s6
    li a1, 0x80247009
    addi a2, sp, 96
    li a7, 29
    ecall
    expect 0
    addi s0, sp, 96
    jal ra, hwclock_check_fields
    mv a0, s6
    li a1, 0x4024700a
    addi a2, sp, 40
    li a7, 29
    ecall
    expect 0
    mv a0, s6
    li a1, 0x80247009
    addi a2, sp, 96
    li a7, 29
    ecall
    expect 0
    addi s0, sp, 96
    jal ra, hwclock_check_target
    mv a0, s6
    li a7, 57
    ecall
    expect 0
    addi s4, s4, 8
    addi s5, s5, -1
    bnez s5, .Lhwclock_path

    # Restore the original time.
    la a1, hwclock_rtc
    li a0, -100
    li a2, 1
    li a3, 0
    li a7, 56
    ecall
    li s8, 4
    bltz a0, system_failure
    mv s6, a0
    mv a0, s6
    li a1, 0x4024700a
    addi a2, sp, 440
    li a7, 29
    ecall
    expect 0
    mv a0, s6
    li a1, 0x80247009
    addi a2, sp, 96
    li a7, 29
    ecall
    expect 0
    lw t0, 96(sp)
    lw t1, 440(sp)
    beq t0, t1, .Lhwclock_restore_second
    addi t1, t1, 1
    bne t0, t1, .Lhwclock_bad_restore
.Lhwclock_restore_second:
    lw t0, 100(sp)
    lw t1, 444(sp)
    bne t0, t1, .Lhwclock_bad_restore
    lw t0, 104(sp)
    lw t1, 448(sp)
    bne t0, t1, .Lhwclock_bad_restore
    lw t0, 108(sp)
    lw t1, 452(sp)
    bne t0, t1, .Lhwclock_bad_restore
    lw t0, 112(sp)
    lw t1, 456(sp)
    bne t0, t1, .Lhwclock_bad_restore
    lw t0, 116(sp)
    lw t1, 460(sp)
    bne t0, t1, .Lhwclock_bad_restore
    j .Lhwclock_restored
.Lhwclock_bad_restore:
    li s8, 5
    j system_failure
.Lhwclock_restored:
    mv a0, s6
    li a7, 57
    ecall
    expect 0
    li a0, 42
    li a7, 94
    ecall
    j hwclock_test

# s0 points at an rtc_time; s8 holds the caller line marker.
hwclock_check_fields:
    lw t0, 20(s0)
    li t1, 70
    blt t0, t1, system_failure
    li t1, 201
    bge t0, t1, system_failure
    lw t0, 16(s0)
    li t1, 12
    bgeu t0, t1, system_failure
    lw t0, 12(s0)
    beqz t0, system_failure
    li t1, 32
    bgeu t0, t1, system_failure
    lw t0, 8(s0)
    li t1, 24
    bgeu t0, t1, system_failure
    lw t0, 4(s0)
    li t1, 60
    bgeu t0, t1, system_failure
    lw t0, 0(s0)
    li t1, 60
    bgeu t0, t1, system_failure
    lw t0, 24(s0)
    li t1, 7
    bgeu t0, t1, system_failure
    lw t0, 28(s0)
    li t1, 366
    bgeu t0, t1, system_failure
    ret

hwclock_check_target:
    lw t0, 0(s0)
    li t1, 56
    bltu t0, t1, system_failure
    li t1, 59
    bgeu t0, t1, system_failure
    lw t0, 4(s0)
    li t1, 34
    bne t0, t1, system_failure
    lw t0, 8(s0)
    li t1, 12
    bne t0, t1, system_failure
    lw t0, 12(s0)
    li t1, 9
    bne t0, t1, system_failure
    lw t0, 16(s0)
    li t1, 2
    bne t0, t1, system_failure
    lw t0, 20(s0)
    li t1, 125
    bne t0, t1, system_failure
    ret

.section .rodata
hwclock_rtc:
    .asciz "/dev/rtc"
hwclock_rtc0:
    .asciz "/dev/rtc0"
hwclock_misc:
    .asciz "/dev/misc/rtc"
.balign 8
hwclock_paths:
    .quad hwclock_rtc
    .quad hwclock_rtc0
    .quad hwclock_misc

.section .note.GNU-stack,"",@progbits
