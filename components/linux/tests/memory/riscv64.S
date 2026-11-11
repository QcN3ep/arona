# SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

.section .text
.option norelax

.macro memcall number
    li a7, \number
    ecall
.endm

.macro result value
    li t0, \value
    bne a0, t0, memory_failure
.endm

.global mappings
mappings:
    addi sp, sp, -16
    sd ra, 0(sp)
    li a0, 0
    li a1, 12287
    li a2, 3
    li a3, 0x22
    li a4, -1
    li a5, 0
    memcall 222
    blez a0, memory_failure
    mv s0, a0
    li t0, 4095
    and t0, s0, t0
    bnez t0, memory_failure
    li t0, 4096
    add s1, s0, t0
    add s2, s1, t0
    mv t0, s0
    li t1, 12288
    add t1, s0, t1
.Lzero:
    lbu t2, 0(t0)
    bnez t2, memory_failure
    addi t0, t0, 1
    bne t0, t1, .Lzero
    li t0, 91
    sb t0, 0(s0)
    sb t0, 0(s1)
    sb t0, 0(s2)

    addi a0, s0, 1
    li a1, 4096
    memcall 222
    blez a0, memory_failure
    beq a0, s0, memory_failure
    li a1, 4096
    memcall 215
    result 0
    mv a0, s0
    li a1, 4096
    li a2, 3
    li a3, 0x100022
    memcall 222
    result -17
    mv a0, s0
    li a1, 4096
    li a3, 0x32
    memcall 222
    result -95
    mv a0, s0
    li a1, -1
    li a3, 0x22
    memcall 222
    result -12

    mv a0, s0
    li a1, 12288
    li a2, 0
    memcall 226
    result 0
    li a0, 1
    mv a1, s0
    li a2, 1
    memcall 64
    result -14
    mv a0, s0
    li a1, 12287
    li a2, 3
    memcall 226
    result 0
    lbu t0, 0(s0)
    li t1, 91
    bne t0, t1, memory_failure
    lbu t0, 0(s1)
    bne t0, t1, memory_failure
    lbu t0, 0(s2)
    bne t0, t1, memory_failure
    mv a0, s0
    li a1, 4096
    li a2, 7
    memcall 226
    result -13
    sb zero, 0(s0)

    mv a0, s1
    li a1, 1
    memcall 215
    result 0
    li a0, 1
    mv a1, s1
    li a2, 1
    memcall 64
    result -14
    mv a0, s0
    li a1, 12288
    li a2, 1
    memcall 226
    result -12
    sb zero, 0(s0)
    sb zero, 0(s2)
    mv a0, s0
    li a1, 12288
    memcall 215
    result 0
    mv a0, s0
    memcall 215
    result 0

    mv a0, s0
    li a1, 12288
    li a2, 0
    li a3, 0x100022
    li a4, -1
    li a5, 0
    memcall 222
    bne a0, s0, memory_failure
    mv a0, s0
    li a1, 12288
    li a2, 3
    memcall 226
    result 0
    mv t0, s0
    li t1, 12288
    add t1, s0, t1
.Lreused_zero:
    lbu t2, 0(t0)
    bnez t2, memory_failure
    addi t0, t0, 1
    bne t0, t1, .Lreused_zero
    la t0, .Lcode
    ld t0, 0(t0)
    sd t0, 0(s0)
    mv a0, s0
    li a1, 12288
    li a2, 5
    memcall 226
    result 0
    jalr ra, s0, 0
    result 77
    mv a0, s0
    li a1, 12288
    memcall 215
    result 0

    li a0, 0x1000000000
    li a1, 4096
    li a2, 3
    li a3, 0x100022
    li a4, -1
    li a5, 0
    memcall 222
    result 0x1000000000
    li a0, 0x2000000000
    memcall 222
    result 0x2000000000
    li a0, 0x1000000000
    li a1, 0x1000001000
    memcall 215
    result 0
    li a0, 0x2000000000
    li a1, 4096
    memcall 222
    result 0x2000000000
    li a0, 0x2000000000
    memcall 215
    result 0
    li a0, 0x1000000000
    memcall 222
    result 0x1000000000
    li a0, 0x1000000000
    memcall 215
    result 0

    li a0, 0
    li a1, 0
    li a2, 3
    li a3, 0x22
    memcall 222
    result -22
    li a0, 0
    li a1, 4096
    li a5, 1
    memcall 222
    result -22
    li a0, 0
    li a5, 0
    li a3, 2
    memcall 222
    result -9
    li a0, 1
    li a1, 4096
    memcall 215
    result -22
    li a0, 1
    li a2, 1
    memcall 226
    result -22
    mv a0, s0
    li a1, 0
    memcall 226
    result 0
    ld ra, 0(sp)
    addi sp, sp, 16
    ret

.global memory_fault
memory_fault:
    li a0, 0
    li a1, 4096
    li a2, 3
    li a3, 0x22
    li a4, -1
    li a5, 0
    memcall 222
    blez a0, memory_failure
    mv s0, a0
    la t0, .Lcode
    ld t0, 0(t0)
    sd t0, 0(s0)
    li t0, 51
    beq s3, t0, .Lexecute
    li t0, 49
    beq s3, t0, .Lunmap
    li a2, 0
    li t0, 50
    bne s3, t0, .Lprotect
    li a2, 1
.Lprotect:
    mv a0, s0
    memcall 226
    result 0
    li t0, 53
    beq s3, t0, .Lexecute
    li t0, 48
    beq s3, t0, .Lread
    sb zero, 0(s0)
    j memory_failure
.Lunmap:
    mv a0, s0
    memcall 215
    result 0
.Lread:
    lbu t0, 0(s0)
    j memory_failure
.Lexecute:
    la ra, memory_failure
    jr s0
memory_failure:
    li a0, 2
    memcall 94
    j memory_failure

.balign 8
.option push
.option norvc
.Lcode:
    addi a0, zero, 77
    ret
.option pop

.section .note.GNU-stack,"",@progbits
