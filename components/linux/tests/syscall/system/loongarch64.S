# SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

.section .text
.macro invoke number
    li.d $a7, \number
    syscall 0
.endm
.macro expect value
    li.d $s8, \@
    li.d $t0, \value
    bne $a0, $t0, system_failure
.endm
.global system_basics
system_basics:
    addi.d $s7, $sp, 48
    addi.d $sp, $sp, -512
.Laux_random:
    ld.d $t0, $s7, 0
    beqz $t0, system_failure
    li.d $t1, 25
    beq $t0, $t1, .Lrandom_found
    addi.d $s7, $s7, 16
    b .Laux_random
.Lrandom_found:
    ld.d $t0, $s7, 8
    ld.d $t1, $t0, 0
    ld.d $t2, $t0, 8
    or $t1, $t1, $t2
    beqz $t1, system_failure
    li.d $a0, 1
    move $a1, $sp
    invoke 113
    expect 0
    ld.d $s0, $sp, 0
    ld.d $s1, $sp, 8
    li.d $t0, 1000000000
    bgeu $s1, $t0, system_failure
    st.d $zero, $sp, 16
    li.d $t0, 2000000
    st.d $t0, $sp, 24
    addi.d $a0, $sp, 16
    li.d $a1, 0
    invoke 101
    expect 0
    li.d $a0, 1
    move $a1, $sp
    invoke 113
    expect 0
    ld.d $t0, $sp, 0
    bltu $t0, $s0, system_failure
    bne $t0, $s0, .Lelapsed
    ld.d $t0, $sp, 8
    sub.d $t0, $t0, $s1
    li.d $t1, 2000000
    bltu $t0, $t1, system_failure
.Lelapsed:
    li.d $a0, 1
    addi.d $a1, $sp, 16
    invoke 114
    expect 0
    ld.d $t0, $sp, 16
    bnez $t0, system_failure
    ld.d $t0, $sp, 24
    beqz $t0, system_failure
    li.d $a0, -1
    move $a1, $sp
    invoke 113
    expect -22
    li.d $a0, 1
    li.d $a1, 1
    invoke 113
    expect -14
    li.d $a0, 0
    move $a1, $sp
    invoke 113
    expect 0
    ld.d $t0, $sp, 0
    li.d $t1, 946684800
    bltu $t0, $t1, system_failure
    move $a0, $sp
    addi.d $a1, $sp, 32
    invoke 169
    expect 0
    ld.d $t0, $sp, 8
    li.d $t1, 1000000
    bgeu $t0, $t1, system_failure
    ld.d $t0, $sp, 32
    bnez $t0, system_failure
    li.d $a0, 0
    li.d $a1, 0
    invoke 169
    expect 0
    st.d $zero, $sp, 16
    st.d $zero, $sp, 24
    li.d $a0, 1
    li.d $a1, 1
    addi.d $a2, $sp, 16
    li.d $a3, 0
    invoke 115
    expect 0
    li.d $a0, 1
    li.d $a1, 2
    addi.d $a2, $sp, 16
    invoke 115
    expect -22
    move $a0, $sp
    invoke 160
    expect 0
    ld.w $t0, $sp, 0
    li.d $t1, 0x756e694c
    bne $t0, $t1, system_failure
    li.d $a0, 1
    invoke 160
    expect -14
    move $a0, $sp
    li.d $a1, 32
    li.d $a2, 0
    invoke 278
    blez $a0, system_failure
    li.d $t0, 32
    bltu $t0, $a0, system_failure
    move $t0, $sp
    add.d $t1, $sp, $a0
    li.d $t2, 0
.Lrandom_bytes:
    ld.bu $t3, $t0, 0
    or $t2, $t2, $t3
    addi.d $t0, $t0, 1
    bltu $t0, $t1, .Lrandom_bytes
    beqz $t2, system_failure
    li.d $a0, 1
    li.d $a1, 0
    invoke 278
    expect 0
    li.d $a0, 1
    li.d $a1, 1
    invoke 278
    expect -14
    move $a0, $sp
    li.d $a1, 1
    li.d $a2, 8
    invoke 278
    expect -22
    bl timer_selftest
    # sysinfo reports real memory, process, and uptime data.
    move $a0, $sp
    invoke 179
    expect 0
    ld.d $t0, $sp, 32
    beqz $t0, system_failure
    ld.d $t0, $sp, 40
    beqz $t0, system_failure
    ld.d $t1, $sp, 32
    bltu $t1, $t0, system_failure
    ld.wu $t0, $sp, 104
    li.d $t1, 1
    bne $t0, $t1, system_failure
    ld.hu $t0, $sp, 80
    beqz $t0, system_failure
    li.d $a0, 0
    invoke 179
    expect -14

    # times reports clock ticks for the process and its children.
    addi.d $a0, $sp, 128
    invoke 153
    blt $a0, $zero, system_failure
    ld.d $t0, $sp, 128
    blt $t0, $zero, system_failure
    li.d $a0, 0
    invoke 153
    expect -14

    # getrusage rejects unknown targets and faults on a null buffer.
    li.d $a0, 2
    addi.d $a1, $sp, 192
    invoke 165
    expect -22
    li.d $a0, 0
    li.d $a1, 0
    invoke 165
    expect -14

    # A terminated child contributes to children CPU time and usage.
    li.d $a0, 17
    li.d $a1, 0
    li.d $a2, 0
    li.d $a3, 0
    li.d $a4, 0
    invoke 220
    beqz $a0, .Lchild_burn
    blt $a0, $zero, system_failure
    move $s2, $a0
    move $a0, $s2
    addi.d $a1, $sp, 160
    li.d $a2, 0
    li.d $a3, 0
    invoke 260
    bne $a0, $s2, system_failure
    ld.w $t0, $sp, 160
    bnez $t0, system_failure
    addi.d $a0, $sp, 128
    invoke 153
    blt $a0, $zero, system_failure
    ld.d $t0, $sp, 144
    beqz $t0, system_failure
    li.d $a0, -1
    addi.d $a1, $sp, 192
    invoke 165
    expect 0
    ld.d $t0, $sp, 192
    ld.d $t1, $sp, 200
    or $t0, $t0, $t1
    beqz $t0, system_failure
    li.d $a0, 0
    addi.d $a1, $sp, 192
    invoke 165
    expect 0
    ld.d $t0, $sp, 224
    beqz $t0, system_failure
    ld.d $t0, $sp, 256
    bnez $t0, system_failure
    ld.d $t0, $sp, 264
    bnez $t0, system_failure
    ld.d $t0, $sp, 320
    bnez $t0, system_failure
    ld.d $t0, $sp, 328
    bnez $t0, system_failure
    b .Lsystem_done
.Lchild_burn:
    li.d $t0, 200000000
1:
    addi.d $t0, $t0, -1
    bnez $t0, 1b
    li.d $a0, 0
    invoke 93
.Lsystem_done:
    li.d $a0, 42
    invoke 94
system_failure:
    addi.d $a0, $s8, 100
    invoke 94
    b system_failure
.globl timer_syscall
timer_syscall:
    move $a7, $a0
    move $t0, $a1
    ld.d $a0, $t0, 0
    ld.d $a1, $t0, 8
    ld.d $a2, $t0, 16
    ld.d $a3, $t0, 24
    ld.d $a4, $t0, 32
    ld.d $a5, $t0, 40
    syscall 0
    jr $ra

# RTC device paths, rtc_time conversion, and permission checks.
.global hwclock_test
hwclock_test:
    addi.d $sp, $sp, -512
    la.local $a1, hwclock_rtc
    li.d $a0, -100
    li.d $a2, 2
    li.d $a3, 0
    invoke 56
    li.d $s8, 1
    blt $a0, $zero, system_failure
    move $s6, $a0

    # Save the current time so the suite can be restored afterwards.
    move $a0, $s6
    li.d $a1, 0x80247009
    move $a2, $sp
    invoke 29
    expect 0
    move $s0, $sp
    bl hwclock_check_fields
    ld.d $t0, $sp, 0
    st.d $t0, $sp, 440
    ld.d $t0, $sp, 8
    st.d $t0, $sp, 448
    ld.d $t0, $sp, 16
    st.d $t0, $sp, 456
    ld.d $t0, $sp, 24
    st.d $t0, $sp, 464
    ld.w $t0, $sp, 32
    st.w $t0, $sp, 472

    # A known time: 2025-03-09 12:34:56 UTC.
    li.d $t0, 56
    st.w $t0, $sp, 40
    li.d $t0, 34
    st.w $t0, $sp, 44
    li.d $t0, 12
    st.w $t0, $sp, 48
    li.d $t0, 9
    st.w $t0, $sp, 52
    li.d $t0, 2
    st.w $t0, $sp, 56
    li.d $t0, 125
    st.w $t0, $sp, 60
    st.w $zero, $sp, 64
    li.d $t0, 67
    st.w $t0, $sp, 68
    st.w $zero, $sp, 72

    # Months are zero-based, so month 12 is invalid.
    li.d $t0, 12
    st.w $t0, $sp, 56
    move $a0, $s6
    li.d $a1, 0x4024700a
    addi.d $a2, $sp, 40
    invoke 29
    expect -22
    li.d $t0, 2
    st.w $t0, $sp, 56

    # Days outside the month and February 30 are invalid.
    li.d $t0, 32
    st.w $t0, $sp, 52
    move $a0, $s6
    li.d $a1, 0x4024700a
    addi.d $a2, $sp, 40
    invoke 29
    expect -22
    li.d $t0, 1
    st.w $t0, $sp, 56
    li.d $t0, 30
    st.w $t0, $sp, 52
    move $a0, $s6
    li.d $a1, 0x4024700a
    addi.d $a2, $sp, 40
    invoke 29
    expect -22
    li.d $t0, 2
    st.w $t0, $sp, 56
    li.d $t0, 9
    st.w $t0, $sp, 52

    # The valid value is written and read back.
    move $a0, $s6
    li.d $a1, 0x4024700a
    addi.d $a2, $sp, 40
    invoke 29
    expect 0
    move $a0, $s6
    li.d $a1, 0x80247009
    addi.d $a2, $sp, 96
    invoke 29
    expect 0
    addi.d $s0, $sp, 96
    bl hwclock_check_target

    # Alarm controls are unsupported and unknown commands are not RTC ioctls.
    move $a0, $s6
    li.d $a1, 0x00007001
    move $a2, $sp
    invoke 29
    expect -22
    move $a0, $s6
    li.d $a1, 0
    li.d $a2, 0
    invoke 29
    expect -25

    # Writing needs root credentials; the saved identity allows returning to root.
    li.d $a0, 1
    li.d $a1, 1
    li.d $a2, 0
    invoke 147
    expect 0
    move $a0, $s6
    li.d $a1, 0x4024700a
    addi.d $a2, $sp, 40
    invoke 29
    expect -1
    move $a0, $s6
    li.d $a1, 0x80247009
    addi.d $a2, $sp, 96
    invoke 29
    expect 0
    li.d $a0, 1
    li.d $a1, 0
    li.d $a2, 0
    invoke 147
    expect 0
    li.d $a0, 0
    li.d $a1, 0
    li.d $a2, 0
    invoke 147
    expect 0

    # read(2) returns one rtc_time and rejects short buffers.
    move $a0, $s6
    addi.d $a1, $sp, 96
    li.d $a2, 36
    invoke 63
    li.d $s8, 2
    li.d $t0, 36
    bne $a0, $t0, system_failure
    addi.d $s0, $sp, 96
    bl hwclock_check_target
    move $a0, $s6
    addi.d $a1, $sp, 136
    li.d $a2, 8
    invoke 63
    expect -22
    move $a0, $s6
    invoke 57
    expect 0

    # All three device paths support reads and writes.
    la.local $s4, hwclock_paths
    li.d $s5, 3
.Lhwclock_path:
    ld.d $a1, $s4, 0
    li.d $a0, -100
    li.d $a2, 2
    li.d $a3, 0
    invoke 56
    li.d $s8, 3
    blt $a0, $zero, system_failure
    move $s6, $a0
    move $a0, $s6
    li.d $a1, 0x80247009
    addi.d $a2, $sp, 96
    invoke 29
    expect 0
    addi.d $s0, $sp, 96
    bl hwclock_check_fields
    move $a0, $s6
    li.d $a1, 0x4024700a
    addi.d $a2, $sp, 40
    invoke 29
    expect 0
    move $a0, $s6
    li.d $a1, 0x80247009
    addi.d $a2, $sp, 96
    invoke 29
    expect 0
    addi.d $s0, $sp, 96
    bl hwclock_check_target
    move $a0, $s6
    invoke 57
    expect 0
    addi.d $s4, $s4, 8
    addi.d $s5, $s5, -1
    bne $s5, $zero, .Lhwclock_path

    # Restore the original time.
    la.local $a1, hwclock_rtc
    li.d $a0, -100
    li.d $a2, 1
    li.d $a3, 0
    invoke 56
    li.d $s8, 4
    blt $a0, $zero, system_failure
    move $s6, $a0
    move $a0, $s6
    li.d $a1, 0x4024700a
    addi.d $a2, $sp, 440
    invoke 29
    expect 0
    move $a0, $s6
    li.d $a1, 0x80247009
    addi.d $a2, $sp, 96
    invoke 29
    expect 0
    ld.w $t0, $sp, 96
    ld.w $t1, $sp, 440
    beq $t0, $t1, .Lhwclock_restore_second
    addi.d $t1, $t1, 1
    bne $t0, $t1, .Lhwclock_bad_restore
.Lhwclock_restore_second:
    ld.w $t0, $sp, 100
    ld.w $t1, $sp, 444
    bne $t0, $t1, .Lhwclock_bad_restore
    ld.w $t0, $sp, 104
    ld.w $t1, $sp, 448
    bne $t0, $t1, .Lhwclock_bad_restore
    ld.w $t0, $sp, 108
    ld.w $t1, $sp, 452
    bne $t0, $t1, .Lhwclock_bad_restore
    ld.w $t0, $sp, 112
    ld.w $t1, $sp, 456
    bne $t0, $t1, .Lhwclock_bad_restore
    ld.w $t0, $sp, 116
    ld.w $t1, $sp, 460
    bne $t0, $t1, .Lhwclock_bad_restore
    b .Lhwclock_restored
.Lhwclock_bad_restore:
    li.d $s8, 5
    b system_failure
.Lhwclock_restored:
    move $a0, $s6
    invoke 57
    expect 0
    li.d $a0, 42
    invoke 94
    b hwclock_test

# s0 points at an rtc_time; s8 holds the caller line marker.
hwclock_check_fields:
    ld.w $t0, $s0, 20
    li.d $t1, 70
    blt $t0, $t1, system_failure
    li.d $t1, 201
    bge $t0, $t1, system_failure
    ld.w $t0, $s0, 16
    li.d $t1, 12
    bgeu $t0, $t1, system_failure
    ld.w $t0, $s0, 12
    beqz $t0, system_failure
    li.d $t1, 32
    bgeu $t0, $t1, system_failure
    ld.w $t0, $s0, 8
    li.d $t1, 24
    bgeu $t0, $t1, system_failure
    ld.w $t0, $s0, 4
    li.d $t1, 60
    bgeu $t0, $t1, system_failure
    ld.w $t0, $s0, 0
    li.d $t1, 60
    bgeu $t0, $t1, system_failure
    ld.w $t0, $s0, 24
    li.d $t1, 7
    bgeu $t0, $t1, system_failure
    ld.w $t0, $s0, 28
    li.d $t1, 366
    bgeu $t0, $t1, system_failure
    jr $ra

hwclock_check_target:
    ld.w $t0, $s0, 0
    li.d $t1, 56
    bltu $t0, $t1, system_failure
    li.d $t1, 59
    bgeu $t0, $t1, system_failure
    ld.w $t0, $s0, 4
    li.d $t1, 34
    bne $t0, $t1, system_failure
    ld.w $t0, $s0, 8
    li.d $t1, 12
    bne $t0, $t1, system_failure
    ld.w $t0, $s0, 12
    li.d $t1, 9
    bne $t0, $t1, system_failure
    ld.w $t0, $s0, 16
    li.d $t1, 2
    bne $t0, $t1, system_failure
    ld.w $t0, $s0, 20
    li.d $t1, 125
    bne $t0, $t1, system_failure
    jr $ra

.section .rodata
hwclock_rtc:
    .asciz "/dev/rtc"
hwclock_rtc0:
    .asciz "/dev/rtc0"
hwclock_misc:
    .asciz "/dev/misc/rtc"
.balign 8
hwclock_paths:
    .dword hwclock_rtc
    .dword hwclock_rtc0
    .dword hwclock_misc
.section .note.GNU-stack,"",@progbits
