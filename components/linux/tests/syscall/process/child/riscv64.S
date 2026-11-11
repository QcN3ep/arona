# SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

.section .text
.macro invoke number
    li a7, \number
    ecall
.endm
.macro expect value
    li s8, \@
    li t0, \value
    bne a0, t0, child_failure
.endm
.global fork_process
fork_process:
    addi sp, sp, -64
    invoke 172
    mv s0, a0
    mv a0, sp
    li a1, 0
    invoke 59
    expect 0
    lw s1, 0(sp)
    lw s2, 4(sp)
    li a0, 0
    li a1, 4096
    li a2, 3
    li a3, 0x21
    li a4, -1
    li a5, 0
    invoke 222
    blez a0, child_failure
    mv s3, a0
    li t0, 1
    sd t0, 0(s3)
    la s4, private_word
    sd t0, 0(s4)
    li a0, 17
    li a1, 0
    li a2, 0
    li a3, 0
    li a4, 0
    invoke 220
    beqz a0, child
    bltz a0, child_failure
    mv s5, a0
    mv a0, s2
    invoke 57
    expect 0
    mv a0, s1
    addi a1, sp, 16
    li a2, 1
    invoke 63
    expect 1
    lbu t0, 16(sp)
    li t1, 67
    bne t0, t1, child_failure
    mv a0, s5
    li a1, 1
    li a2, 0
    li a3, 0
    invoke 260
    expect -14
    mv a0, s5
    addi a1, sp, 24
    li a2, 0
    li a3, 0
    invoke 260
    bne a0, s5, child_failure
    lw t0, 24(sp)
    li t1, 5888
    bne t0, t1, child_failure
    ld t0, 0(s3)
    li t1, 2
    bne t0, t1, child_failure
    ld t0, 0(s4)
    li t1, 1
    bne t0, t1, child_failure
    mv a0, s5
    li a1, 0
    li a2, 1
    li a3, 0
    invoke 260
    expect -10
    li a0, 42
    invoke 94
child:
    invoke 172
    beq a0, s0, child_failure
    invoke 173
    bne a0, s0, child_failure
    li t0, 2
    sd t0, 0(s3)
    sd t0, 0(s4)
    mv a0, s1
    invoke 57
    expect 0
    li t0, 67
    sb t0, 16(sp)
    mv a0, s2
    addi a1, sp, 16
    li a2, 1
    invoke 64
    expect 1
    li a0, 23
    invoke 94
child_failure:
    addi a0, s8, 100
    invoke 94
    j child_failure
.section .bss
.balign 8
private_word: .skip 8
.section .note.GNU-stack,"",@progbits
