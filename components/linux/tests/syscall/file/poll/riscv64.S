# SPDX-License-Identifier: MPL-2.0

.section .text
.option norelax
.macro invoke number
    li a7, \number
    ecall
.endm
.macro expect value
    li s11, \@
    li t0, \value
    bne a0, t0, poll_failure
.endm
.global files_poll
files_poll:
    addi sp, sp, -512
    mv a0, sp
    li a1, 0
    invoke 59
    expect 0
    lw s0, 0(sp)
    lw s1, 4(sp)
    sw s0, 16(sp)
    li t0, 1
    sh t0, 20(sp)
    sw s1, 24(sp)
    li t0, 4
    sh t0, 28(sp)
    sd zero, 64(sp)
    sd zero, 72(sp)
    addi a0, sp, 16
    li a1, 2
    addi a2, sp, 64
    li a3, 0
    li a4, 8
    invoke 73
    expect 1
    lhu t0, 22(sp)
    bnez t0, poll_failure
    lhu t0, 30(sp)
    li t1, 4
    bne t0, t1, poll_failure
    mv a0, s1
    la a1, poll_payload
    li a2, 1
    invoke 64
    expect 1
    addi a0, sp, 16
    li a1, 2
    addi a2, sp, 64
    li a3, 0
    li a4, 8
    invoke 73
    expect 2
    lhu t0, 22(sp)
    li t1, 1
    bne t0, t1, poll_failure
    li t0, 1
    sll t0, t0, s0
    sd t0, 80(sp)
    li t0, 1
    sll t0, t0, s1
    sd t0, 88(sp)
    addi a0, s1, 1
    addi a1, sp, 80
    addi a2, sp, 88
    li a3, 0
    addi a4, sp, 64
    li a5, 0
    invoke 72
    expect 2
    mv a0, s0
    addi a1, sp, 100
    li a2, 1
    invoke 63
    expect 1
    mv a0, s1
    invoke 57
    expect 0
    addi a0, sp, 16
    li a1, 1
    li a2, 0
    li a3, 0
    li a4, 8
    invoke 73
    expect 1
    lhu t0, 22(sp)
    li t1, 16
    bne t0, t1, poll_failure
    li t0, 999
    sw t0, 16(sp)
    addi a0, sp, 16
    li a1, 1
    addi a2, sp, 64
    li a3, 0
    li a4, 8
    invoke 73
    expect 1
    lhu t0, 22(sp)
    li t1, 32
    bne t0, t1, poll_failure
    li a0, 0
    li a1, 0
    addi a2, sp, 64
    li a3, 0
    li a4, 8
    li t0, 2000000
    sd t0, 72(sp)
    invoke 73
    expect 0
    ld t0, 64(sp)
    ld t1, 72(sp)
    or t0, t0, t1
    bnez t0, poll_failure
    mv a0, s0
    invoke 57
    expect 0
    li a0, 42
    invoke 94
poll_failure:
    addi a0, s11, 100
    invoke 94
    j poll_failure
.section .rodata
poll_payload: .ascii "x"
.section .note.GNU-stack,"",@progbits
