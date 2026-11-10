# SPDX-License-Identifier: MPL-2.0

.section .text
.option norelax
.macro invoke number
    li a7, \number
    ecall
.endm
.macro expect value
    li s8, \@
    li t0, \value
    bne a0, t0, control_failure
.endm
.macro control command, offset
    li a0, 1
    li a1, \command
    addi a2, sp, \offset
    invoke 29
.endm
.macro spawn target
    li a0, 17
    li a1, 0
    li a2, 0
    li a3, 0
    li a4, 0
    invoke 220
    beqz a0, \target
    bltz a0, control_failure
    mv s1, a0
.endm
.macro reap options, status
    mv a0, s1
    addi a1, sp, 64
    li a2, \options
    li a3, 0
    invoke 260
    bne a0, s1, control_failure
    lw t0, 64(sp)
    li t1, \status
    bne t0, t1, control_failure
.endm
.macro resume
    mv a0, s1
    li a1, 18
    invoke 129
    expect 0
.endm
.global process_terminal
process_terminal:
    addi sp, sp, -256
    invoke 172
    mv s0, a0
    control 0x540f, 0
    expect -25
    control 0x540e, 0
    expect 0
    control 0x5429, 0
    expect 0
    lw t0, 0(sp)
    bne t0, s0, control_failure
    control 0x540f, 0
    expect 0
    lw t0, 0(sp)
    bne t0, s0, control_failure
    li a0, -100
    la a1, tty_path
    li a2, 2
    li a3, 0
    invoke 56
    bltz a0, control_failure
    mv s2, a0
    spawn control_child
    reap 2, 0x167f
    sw s1, 0(sp)
    control 0x5410, 0
    expect 0
    resume
    reap 0, 0x1800
    li a0, 22
    la a1, control_ignore
    li a2, 0
    li a3, 8
    invoke 134
    expect 0
    sw s0, 0(sp)
    control 0x5410, 0
    expect 0
    control 0x5401, 80
    expect 0
    lw t0, 92(sp)
    ori t0, t0, 0x100
    sw t0, 92(sp)
    control 0x5402, 80
    expect 0
    spawn background_write
    reap 2, 0x167f
    sw s1, 0(sp)
    control 0x5410, 0
    expect 0
    resume
    reap 0, 0x1900
    sw s0, 0(sp)
    control 0x5410, 0
    expect 0
    spawn background_read
    reap 2, 0x157f
    mv a0, s1
    li a1, 9
    invoke 129
    expect 0
    reap 0, 9
    li a0, 1
    la a1, control_ignore
    li a2, 0
    li a3, 8
    invoke 134
    expect 0
    control 0x5422, 0
    expect 0
    control 0x540f, 0
    expect -25
    li a0, -100
    la a1, tty_path
    li a2, 2
    li a3, 0
    invoke 56
    expect -6
    control 0x540e, 0
    expect 0
    li a0, 42
    invoke 94
control_child:
    li a0, 0
    li a1, 0
    invoke 154
    expect 0
    invoke 172
    sw a0, 0(sp)
    control 0x5410, 0
    expect 0
    control 0x5422, 0
    expect 0
    control 0x5429, 0
    expect -25
    li a0, 24
    invoke 94
background_write:
    li a0, 22
    la a1, control_default
    li a2, 0
    li a3, 8
    invoke 134
    expect 0
    li a0, 0
    li a1, 0
    invoke 154
    expect 0
    li a0, 1
    la a1, control_text
    li a2, 1
    invoke 64
    expect 1
    li a0, 25
    invoke 94
background_read:
    li a0, 0
    li a1, 0
    invoke 154
    expect 0
    mv a0, s2
    addi a1, sp, 128
    li a2, 1
    invoke 63
    j control_failure
control_failure:
    addi a0, s8, 100
    invoke 94
    j control_failure
.section .rodata
.balign 8
control_ignore: .quad 1, 0, 0
control_default: .quad 0, 0, 0
tty_path: .asciz "/dev/tty"
control_text: .ascii "!"
.section .note.GNU-stack,"",@progbits
