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
    li a0, 0
    li a1, 4
    li a2, 0
    invoke 25
    expect 0
    sb zero, 23(sp)
    li a0, 0
    li a1, 0x5402
    mv a2, sp
    invoke 29
    expect 0
    li a0, 0
    addi a1, sp, 128
    li a2, 8
    invoke 63
    expect 0
    li t0, 1
    sb t0, 22(sp)
    li a0, 0
    li a1, 0x5402
    mv a2, sp
    invoke 29
    expect 0
    li a0, 0
    addi a1, sp, 128
    li a2, 8
    invoke 63
    expect 0
    li t0, 4
    sb t0, 23(sp)
    sb zero, 22(sp)
    li a0, 0
    li a1, 0x5402
    mv a2, sp
    invoke 29
    expect 0
    li a0, 1
    la a1, minimum_prompt
    li a2, 23
    invoke 64
    expect 23
    li a0, 0
    addi a1, sp, 128
    li a2, 8
    invoke 63
    expect 4
    lw t0, 128(sp)
    li t1, 0x34333231
    bne t0, t1, terminal_failure
    li t0, 8
    sb t0, 23(sp)
    li t0, 1
    sb t0, 22(sp)
    li a0, 0
    li a1, 0x5402
    mv a2, sp
    invoke 29
    expect 0
    li a0, 1
    la a1, interbyte_prompt
    li a2, 25
    invoke 64
    expect 25
    li a0, 0
    addi a1, sp, 128
    li a2, 8
    invoke 63
    expect 2
    lhu t0, 128(sp)
    li t1, 0x7978
    bne t0, t1, terminal_failure
    li t0, 3
    sw t0, 12(sp)
    li a0, 0
    li a1, 0x5402
    mv a2, sp
    invoke 29
    expect 0
    li s0, 2
terminal_actions:
    mv a0, s0
    la a1, terminal_action
    li a2, 0
    li a3, 8
    invoke 134
    expect 0
    li t0, 2
    beq s0, t0, terminal_action_quit
    li t0, 3
    bne s0, t0, terminal_actions_done
    li s0, 20
    j terminal_actions
terminal_action_quit:
    li s0, 3
    j terminal_actions
terminal_actions_done:
    li a0, 1
    la a1, signal_prompt
    li a2, 22
    invoke 64
    expect 22
    li a0, 0
    addi a1, sp, 128
    li a2, 8
    invoke 63
    expect 5
terminal_signals_wait:
    la t0, terminal_signals
    ld t0, 0(t0)
    li t1, 0x10000c
    beq t0, t1, terminal_signals_done
    invoke 124
    j terminal_signals_wait
terminal_signals_done:
    li a0, 20
    la a1, terminal_default
    li a2, 0
    li a3, 8
    invoke 134
    expect 0
    li a0, 22
    la a1, terminal_ignore
    li a2, 0
    li a3, 8
    invoke 134
    expect 0
    li a0, 17
    li a1, 0
    li a2, 0
    li a3, 0
    li a4, 0
    invoke 220
    beqz a0, terminal_job
    bltz a0, terminal_failure
    mv s1, a0
    mv a0, s1
    mv a1, s1
    invoke 154
    expect 0
    sw s1, 64(sp)
    li a0, 0
    li a1, 0x5410
    addi a2, sp, 64
    invoke 29
    expect 0
    mv a0, s1
    li a1, 18
    invoke 129
    expect 0
    mv a0, s1
    addi a1, sp, 80
    li a2, 2
    li a3, 0
    invoke 260
    bne a0, s1, terminal_failure
    lw t0, 80(sp)
    li t1, 0x147f
    bne t0, t1, terminal_failure
    invoke 172
    sw a0, 64(sp)
    li a0, 0
    li a1, 0x5410
    addi a2, sp, 64
    invoke 29
    expect 0
    mv a0, s1
    li a1, 18
    invoke 129
    expect 0
    mv a0, s1
    addi a1, sp, 80
    li a2, 2
    li a3, 0
    invoke 260
    bne a0, s1, terminal_failure
    lw t0, 80(sp)
    li t1, 0x157f
    bne t0, t1, terminal_failure
    sw s1, 64(sp)
    li a0, 0
    li a1, 0x5410
    addi a2, sp, 64
    invoke 29
    expect 0
    mv a0, s1
    li a1, 18
    invoke 129
    expect 0
    li a0, 1
    la a1, resume_prompt
    li a2, 22
    invoke 64
    expect 22
    mv a0, s1
    addi a1, sp, 80
    li a2, 0
    li a3, 0
    invoke 260
    bne a0, s1, terminal_failure
    lw t0, 80(sp)
    li t1, 0x1a00
    bne t0, t1, terminal_failure
    li a0, 42
    invoke 94
terminal_job:
    invoke 172
    mv s0, a0
terminal_job_foreground:
    li a0, 0
    li a1, 0x540f
    addi a2, sp, 64
    invoke 29
    expect 0
    lw t0, 64(sp)
    beq t0, s0, terminal_job_read
    invoke 124
    j terminal_job_foreground
terminal_job_read:
    li a0, 1
    la a1, suspend_prompt
    li a2, 23
    invoke 64
    expect 23
    li a0, 0
    addi a1, sp, 128
    li a2, 8
    invoke 63
    expect 3
    li a0, 26
    invoke 94
terminal_signal:
    la t0, terminal_signals
    ld t1, 0(t0)
    li t2, 1
    sll t2, t2, a0
    or t1, t1, t2
    sd t1, 0(t0)
    ret

terminal_failure:
    addi a0, s11, 100
    invoke 94
    j terminal_failure
.section .rodata
canonical_prompt: .ascii "Linux terminal canonical\n"
raw_prompt: .ascii "Linux terminal raw\n"

.balign 8
terminal_action: .quad terminal_signal, 0x10000004, 0
terminal_default: .quad 0, 0, 0
terminal_ignore: .quad 1, 0, 0
minimum_prompt: .ascii "Linux terminal minimum\n"
interbyte_prompt: .ascii "Linux terminal interbyte\n"
signal_prompt: .ascii "Linux terminal signal\n"
suspend_prompt: .ascii "Linux terminal suspend\n"
resume_prompt: .ascii "Linux terminal resume\n"
.section .bss
.balign 8
terminal_signals: .skip 8
.section .note.GNU-stack,"",@progbits
