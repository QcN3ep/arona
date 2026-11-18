# SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

.section .text
.global _start
.option norelax
_start:
    lla s0, mutable_data
    li t0, 73
    sd t0, 0(s0)
    lla a0, uname_buffer
    li a7, 160
    ecall
    bnez a0, failure
    lla t0, uname_buffer
    lbu t0, 0(t0)
    li t1, 76
    bne t0, t1, failure
    li a0, 0
    li a1, 4096
    li a2, 7
    li a3, 0x22
    li a4, -1
    li a5, 0
    li a7, 222
    ecall
    bltz a0, failure
    mv s1, a0
    lla t0, code
    ld t0, 0(t0)
    sd t0, 0(s1)
    fence.i
    jalr ra, s1, 0
    li t0, 77
    bne a0, t0, failure
    li a0, 17
    li a1, 0
    li a2, 0
    li a3, 0
    li a4, 0
    li a7, 220
    ecall
    bltz a0, failure
    beqz a0, child
    mv s2, a0
    addi a1, sp, -16
    li a2, 0
    li a3, 0
    li a7, 260
    ecall
    bne a0, s2, failure
    lw t0, -16(sp)
    li t1, 10752
    bne t0, t1, failure
    ld t0, 0(s0)
    li t1, 73
    bne t0, t1, failure
    j success
child:
    li t0, 91
    sd t0, 0(s0)
    jalr ra, s1, 0
    li t0, 77
    bne a0, t0, failure
success:
    li a0, 42
    li a7, 94
    ecall
failure:
    li a0, 1
    li a7, 94
    ecall
.balign 8
.option push
.option norvc
code:
    addi a0, zero, 77
    ret
.option pop
mutable_data: .quad 0
uname_buffer: .zero 392
.section .note.GNU-stack,"",@progbits
