# SPDX-License-Identifier: MPL-2.0

.section .text
.macro invoke number
    li a7, \number
    ecall
.endm
.macro expect value
    li s8, \@
    li t0, \value
    bne a0, t0, thread_failure
.endm
.global threads
threads:
    invoke 172
    blez a0, thread_failure
    mv s0, a0
    invoke 178
    bne a0, s0, thread_failure
    invoke 173
    expect 0
    li a0, 0
    invoke 96
    bne a0, s0, thread_failure
    li a0, 0
    li a1, 8192
    li a2, 3
    li a3, 0x22
    li a4, -1
    li a5, 0
    invoke 222
    blez a0, thread_failure
    mv s1, a0
    li t0, 8192
    add s1, s1, t0
    la s2, thread_data
    sd s0, 16(s2)
    li a0, 0x13d0f00
    mv a1, s1
    mv a2, s2
    addi a3, s2, 32
    addi a4, s2, 4
    li t0, 0x3ff0000000000000
    fmv.d.x ft0, t0
    invoke 220
    beqz a0, child
    bltz a0, thread_failure
    beq a0, s0, thread_failure
    lw t0, 0(s2)
    bne a0, t0, thread_failure
    mv s3, a0
wait_child:
    lw t0, 4(s2)
    beqz t0, child_finished
    invoke 124
    expect 0
    j wait_child
child_finished:
    ld t0, 8(s2)
    bne t0, s3, thread_failure
    li a0, 42
    invoke 94
child:
    addi t0, s2, 32
    bne tp, t0, thread_failure
    bne sp, s1, thread_failure
    fmv.x.d t0, ft0
    li t1, 0x3ff0000000000000
    bne t0, t1, thread_failure
    invoke 172
    ld t0, 16(s2)
    bne a0, t0, thread_failure
    invoke 178
    ld t0, 0(s2)
    slli t0, t0, 32
    srli t0, t0, 32
    bne a0, t0, thread_failure
    sd a0, 8(s2)
    li a0, 0
    invoke 93
thread_failure:
    addi a0, s8, 100
    invoke 94
    j thread_failure
.section .bss
.balign 8
thread_data: .skip 40
.section .note.GNU-stack,"",@progbits
