# SPDX-License-Identifier: MPL-2.0

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
.section .note.GNU-stack,"",@progbits
