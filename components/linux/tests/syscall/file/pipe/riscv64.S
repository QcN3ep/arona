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
    bne a0, t0, pipe_failure
.endm
.global files_pipe
files_pipe:
    addi sp, sp, -512
    mv a0, sp
    li a1, 0x80800
    invoke 59
    expect 0
    lw s0, 0(sp)
    lw s1, 4(sp)
    mv a0, s0
    li a1, 1
    invoke 25
    expect 1
    mv a0, s0
    li a1, 3
    invoke 25
    expect 2048
    mv a0, s1
    addi a1, sp, 128
    invoke 80
    expect 0
    lw t0, 144(sp)
    li t1, 4480
    bne t0, t1, pipe_failure
    mv a0, s0
    mv a1, sp
    li a2, 1
    invoke 63
    expect -11
    mv a0, s1
    la a1, payload
    li a2, 3
    invoke 64
    expect 3
    mv a0, s0
    li a1, 1
    li a2, 3
    invoke 63
    expect -14
    mv a0, s0
    addi a1, sp, 16
    li a2, 3
    invoke 63
    expect 3
    lbu t0, 16(sp)
    li t1, 97
    bne t0, t1, pipe_failure
    mv a0, s0
    mv a1, sp
    li a2, 0
    li a3, 0
    invoke 67
    expect -29
    mv a0, s1
    invoke 82
    expect -22
    li s4, 16
fill_pipe:
    mv a0, s1
    la a1, pipe_page
    li a2, 4096
    invoke 64
    expect 4096
    addi s4, s4, -1
    bnez s4, fill_pipe
    mv a0, s1
    la a1, payload
    li a2, 3
    invoke 64
    expect -11
    mv a0, s0
    addi a1, sp, 16
    li a2, 1
    invoke 63
    expect 1
    mv a0, s1
    la a1, payload
    li a2, 3
    invoke 64
    expect -11
    mv a0, s0
    la a1, pipe_page
    li a2, 4095
    invoke 63
    expect 4095
    mv a0, s1
    la a1, pipe_page
    li a2, 4096
    invoke 64
    expect 4096
    li s4, 16
drain_pipe:
    mv a0, s0
    la a1, pipe_page
    li a2, 4096
    invoke 63
    expect 4096
    addi s4, s4, -1
    bnez s4, drain_pipe
    mv a0, s1
    invoke 23
    bltz a0, pipe_failure
    mv s2, a0
    mv a0, s1
    invoke 57
    expect 0
    mv a0, s0
    addi a1, sp, 16
    li a2, 1
    invoke 63
    expect -11
    mv a0, s2
    invoke 57
    expect 0
    mv a0, s0
    addi a1, sp, 16
    li a2, 1
    invoke 63
    expect 0
    mv a0, s0
    invoke 57
    expect 0
    li a0, 1
    li a1, 0
    invoke 59
    expect -14
    mv a0, sp
    li a1, 0
    invoke 59
    expect 0
    lw s0, 0(sp)
    lw s1, 4(sp)
    mv a0, s0
    invoke 57
    expect 0
    mv a0, s1
    la a1, payload
    li a2, 3
    invoke 64
    expect -32
    li a0, 42
    invoke 94
pipe_failure:
    addi a0, s11, 100
    invoke 94
    j pipe_failure
.section .rodata
payload: .ascii "abc"
.section .bss
.balign 4096
pipe_page: .skip 4096
.section .note.GNU-stack,"",@progbits
