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
    bne a0, t0, terminal_failure
.endm
.global files_terminal
files_terminal:
    addi sp, sp, -512
    li a0, 0
    li a1, 0x5401
    mv a2, sp
    invoke 29
    expect 0
    lw t0, 12(sp)
    andi t0, t0, 2
    beqz t0, terminal_failure
    li a0, 0
    li a1, 0x5413
    addi a2, sp, 64
    invoke 29
    expect 0
    lhu t0, 64(sp)
    li t1, 24
    bne t0, t1, terminal_failure
    li t0, 32
    sh t0, 64(sp)
    li t0, 100
    sh t0, 66(sp)
    li a0, 0
    li a1, 0x5414
    addi a2, sp, 64
    invoke 29
    expect 0
    li a0, 1
    li a1, 0x5413
    addi a2, sp, 80
    invoke 29
    expect 0
    lw t0, 64(sp)
    lw t1, 80(sp)
    bne t0, t1, terminal_failure
    li a0, 1
    la a1, canonical_prompt
    li a2, 25
    invoke 64
    expect 25
    li a0, 0
    addi a1, sp, 128
    li a2, 32
    invoke 63
    expect 4
    lw t0, 128(sp)
    li t1, 0x0a636261
    bne t0, t1, terminal_failure
    sd zero, 0(sp)
    sd zero, 8(sp)
    li t0, 1
    sb t0, 23(sp)
    li a0, 0
    li a1, 0x5402
    mv a2, sp
    invoke 29
    expect 0
    li a0, 1
    la a1, raw_prompt
    li a2, 19
    invoke 64
    expect 19
    sw zero, 96(sp)
    li t0, 1
    sh t0, 100(sp)
    addi a0, sp, 96
    li a1, 1
    li a2, 0
    li a3, 0
    li a4, 8
    invoke 73
    expect 1
    lhu t0, 102(sp)
    li t1, 1
    bne t0, t1, terminal_failure
    li a0, 0
    li a1, 0x541b
    addi a2, sp, 112
    invoke 29
    expect 0
    lw t0, 112(sp)
    beqz t0, terminal_failure
    li t1, 8
    bltu t1, t0, terminal_failure
    li s0, 0
raw_read:
    li a0, 0
    addi a1, sp, 128
    add a1, a1, s0
    li a2, 8
    sub a2, a2, s0
    invoke 63
    blez a0, terminal_failure
    add s0, s0, a0
    li t0, 8
    bltu s0, t0, raw_read
    ld t0, 128(sp)
    li t1, 0x617461642d776172
    bne t0, t1, terminal_failure
    li a0, 0
    li a1, 4
    li a2, 2048
    invoke 25
    expect 0
    li a0, 0
    addi a1, sp, 128
    li a2, 1
    invoke 63
    expect -11
    li a0, 0
    li a1, 0x5401
    li a2, 1
    invoke 29
    expect -14
    li a0, 42
    invoke 94
terminal_failure:
    addi a0, s11, 100
    invoke 94
    j terminal_failure
.section .rodata
canonical_prompt: .ascii "Linux terminal canonical\n"
raw_prompt: .ascii "Linux terminal raw\n"
.section .note.GNU-stack,"",@progbits
