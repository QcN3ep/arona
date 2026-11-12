# SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

.section .text
.macro invoke number
    li a7, \number
    ecall
.endm
.macro expect value
    li s8, \@
    li t0, \value
    bne a0, t0, futex_failure
.endm
.global futex_basics
futex_basics:
    addi sp, sp, -32
    sd ra, 0(sp)
    li t0, 7
    sw t0, 16(sp)
    addi a0, sp, 16
    li a1, 128
    li a2, 8
    li a3, 0
    li a4, 0
    li a5, 0
    invoke 98
    expect -11
    addi a0, sp, 17
    li a1, 129
    li a2, 1
    invoke 98
    expect -22
    addi a0, sp, 16
    li a1, 129
    invoke 98
    expect 0
    sd zero, 8(sp)
    addi a0, sp, 16
    li a1, 128
    li a2, 7
    mv a3, sp
    sd zero, 0(sp)
    li t0, 2000000
    sd t0, 8(sp)
    invoke 98
    expect -110
    li a0, 0
    mv a1, sp
    invoke 113
    expect 0
    ld t0, 8(sp)
    li t1, 2000000
    add t0, t0, t1
    li t1, 1000000000
    bltu t0, t1, 1f
    sub t0, t0, t1
    ld t1, 0(sp)
    addi t1, t1, 1
    sd t1, 0(sp)
1:
    sd t0, 8(sp)
    li t0, 7
    sw t0, 16(sp)
    addi a0, sp, 16
    li a1, 393
    li a2, 7
    mv a3, sp
    li a4, 0
    li a5, 1
    invoke 98
    expect -110
    addi a0, sp, 16
    li a1, 137
    li a3, 0
    li a5, 0
    invoke 98
    expect -22
    li a0, 1
    li a1, 128
    invoke 98
    expect -22
    li a0, 0
    invoke 98
    expect -14
    addi sp, sp, 32
    j threads
futex_failure:
    addi a0, s8, 100
    invoke 94
    j futex_failure

.global futex_restart
futex_restart:
    addi sp, sp, -256
    sd ra, 0(sp)
    li a0, 10
    la a1, futex_restart_action
    li a2, 0
    li a3, 8
    invoke 134
    bnez a0, futex_restart_failure
    invoke 172
    mv s2, a0
    invoke 178
    mv s0, a0
    li a0, 17
    li a1, 0
    li a2, 0
    li a3, 0
    li a4, 0
    invoke 220
    beqz a0, futex_restart_child
    bltz a0, futex_restart_failure
    mv s1, a0
    sw zero, 16(sp)
    sd zero, 32(sp)
    li t0, 800000000
    sd t0, 40(sp)
    li a0, 1
    addi a1, sp, 64
    invoke 113
    bnez a0, futex_restart_failure
    addi a0, sp, 16
    li a1, 0
    li a2, 0
    addi a3, sp, 32
    li a4, 0
    li a5, 0
    invoke 98
    li t0, -110
    bne a0, t0, futex_restart_failure
    li a0, 1
    addi a1, sp, 80
    invoke 113
    bnez a0, futex_restart_failure
    ld t0, 64(sp)
    ld t1, 72(sp)
    ld t2, 80(sp)
    ld t3, 88(sp)
    sub t2, t2, t0
    sub t3, t3, t1
    li t4, 1000000000
    mul t2, t2, t4
    add t2, t2, t3
    li t4, 920000000
    bgeu t2, t4, futex_restart_failure
    la t0, futex_restart_hits
    ld t0, 0(t0)
    beqz t0, futex_restart_failure
    mv a0, s1
    addi a1, sp, 48
    li a2, 0
    li a3, 0
    invoke 260
    bne a0, s1, futex_restart_failure
    li a0, 0
    invoke 94
futex_restart_child:
    sd zero, 96(sp)
    li t0, 200000000
    sd t0, 104(sp)
    addi a0, sp, 96
    li a1, 0
    invoke 101
    mv a0, s2
    mv a1, s0
    li a2, 10
    invoke 131
    li a0, 0
    invoke 94
futex_restart_handler:
    la t0, futex_restart_hits
    ld t1, 0(t0)
    addi t1, t1, 1
    sd t1, 0(t0)
    addi sp, sp, -32
    sw zero, 0(sp)
    sd zero, 8(sp)
    li t0, 50000000
    sd t0, 16(sp)
    mv a0, sp
    li a1, 0
    li a2, 0
    addi a3, sp, 8
    li a4, 0
    li a5, 0
    invoke 98
    li t0, -110
    bne a0, t0, futex_restart_failure
    addi sp, sp, 32
    ret
futex_restart_failure:
    li a0, 4
    invoke 94

.section .data
.balign 8
futex_restart_action: .quad futex_restart_handler, 0x10000004, 0
futex_restart_hits: .quad 0

.section .note.GNU-stack,"",@progbits
