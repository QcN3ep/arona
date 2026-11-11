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
    li a0, 42
    invoke 94
system_failure:
    addi a0, s8, 100
    invoke 94
    j system_failure
.section .note.GNU-stack,"",@progbits
