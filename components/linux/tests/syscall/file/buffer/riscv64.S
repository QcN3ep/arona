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
    bne a0, t0, vector_failure
.endm

.macro iov offset, base, length
    sd \base, \offset(sp)
    li t0, \length
    sd t0, (\offset + 8)(sp)
.endm

.macro vectors number, count=3, offset=0
    mv a0, s0
    mv a1, sp
    li a2, \count
    li a3, \offset
    li a4, 0x12345678
    invoke \number
.endm

.macro position value
    mv a0, s1
    li a1, 0
    li a2, 1
    invoke 62
    expect \value
.endm

.macro rewind
    mv a0, s0
    li a1, 0
    li a2, 0
    invoke 62
    expect 0
.endm

.global files_vector
files_vector:
    addi sp, sp, -512
    li a0, -100
    la a1, vector_path
    li a2, 0x242
    li a3, 0644
    invoke 56
    expect 0
    mv s0, a0
    invoke 23
    expect 3
    mv s1, a0

    la t2, digits
    iov 0, t2, 3
    iov 16, zero, 0
    addi t2, t2, 3
    iov 32, t2, 4
    vectors 66
    expect 7
    position 7

    addi t2, sp, 128
    iov 0, t2, 2
    iov 16, zero, 0
    addi t2, sp, 192
    iov 32, t2, 5
    vectors 69
    expect 7
    position 7
    lhu t0, 128(sp)
    li t1, 0x3130
    bne t0, t1, vector_failure
    lw t0, 192(sp)
    li t1, 0x35343332
    bne t0, t1, vector_failure
    lbu t0, 196(sp)
    li t1, 54
    bne t0, t1, vector_failure

    la t2, letters
    iov 0, t2, 2
    addi t2, t2, 2
    iov 16, t2, 2
    vectors 70, 2, 2
    expect 4
    position 7
    rewind
    addi t2, sp, 128
    iov 0, t2, 2
    addi t2, sp, 192
    iov 16, t2, 5
    vectors 65, 2
    expect 7
    lw t0, 192(sp)
    li t1, 0x64636261
    bne t0, t1, vector_failure
    lbu t0, 196(sp)
    li t1, 54
    bne t0, t1, vector_failure
    position 7
    vectors 65, 2
    expect 0
    vectors 69, 2, -1
    expect -22
    vectors 70, 2, -1
    expect -22
    vectors 70, 2, 0x7fffffffffffffff
    expect -27

    vectors 66, 1025
    expect -22
    vectors 65, -1
    expect -22
    iov 0, zero, 0
    iov 16, zero, 0
    vectors 65, 2
    expect 0
    vectors 66, 2
    expect 0
    mv a0, s0
    li a1, 0
    li a2, 0
    invoke 65
    expect 0
    mv a0, s0
    li a1, 0
    li a2, 0
    invoke 66
    expect 0
    mv a0, s0
    li a1, 0
    li a2, 1
    invoke 66
    expect -14
    la t2, digits
    iov 0, t2, 0x8000000000000000
    vectors 66, 1
    expect -22
    iov 0, t2, 0x7fffffffffffffff
    iov 16, t2, 1
    vectors 65, 2
    expect -22
    iov 0, zero, 1
    iov 16, t2, 2
    vectors 66, 2
    expect -14
    position 7

    li a0, 0
    li a1, 20480
    li a2, 3
    li a3, 0x22
    li a4, -1
    li a5, 0
    invoke 222
    blez a0, vector_failure
    mv s2, a0
    li t0, 16384
    add s3, s2, t0
    mv a0, s3
    li a1, 4096
    li a2, 0
    invoke 226
    expect 0
    mv a0, s0
    mv a1, s2
    li a2, 1024
    invoke 65
    expect 0
    mv a0, s0
    mv a1, s2
    li a2, 1024
    invoke 66
    expect 0
    mv a0, s0
    addi a1, s3, -8
    li a2, 1
    invoke 66
    expect -14
    mv a0, s0
    li a1, -8
    li a2, 1
    invoke 65
    expect -14
    position 7

    la t2, letters
    iov 0, t2, 3
    iov 16, zero, 1
    vectors 66, 2
    expect 3
    position 10
    rewind
    addi t2, sp, 128
    iov 0, t2, 2
    iov 16, zero, 1
    vectors 65, 2
    expect 2
    position 2
    iov 0, zero, 1
    vectors 65, 1
    expect -14
    position 2
    addi t2, s3, -2
    iov 0, t2, 4
    vectors 65, 1
    expect 2
    position 4
    vectors 66, 1
    expect 2
    position 6

    mv a0, s0
    li a1, 4
    li a2, 0x400
    invoke 25
    expect 0
    la t2, letters
    iov 0, t2, 2
    addi t2, t2, 2
    iov 16, t2, 2
    vectors 70, 2
    expect 4
    position 6
    vectors 66, 2
    expect 4
    position 18
    addi t2, sp, 128
    iov 0, t2, 8
    vectors 69, 1, 10
    expect 8
    ld t0, 128(sp)
    li t1, 0x6463626164636261
    bne t0, t1, vector_failure
    position 18

    mv a0, s0
    li a1, 4
    li a2, 0
    invoke 25
    expect 0
    mv a0, s0
    li a1, 0
    invoke 46
    expect 0
    rewind
    iov 0, s2, 4096
    la t2, letters
    iov 16, t2, 2
    vectors 66, 2
    expect 4096
    position 4096
    rewind
    vectors 65, 2
    expect 4096
    position 4096
    iov 16, t2, 0x7fffffffffffffff
    vectors 66, 2
    expect -22
    position 4096

    mv a0, s2
    li a1, 20480
    invoke 215
    expect 0
    mv a0, s0
    invoke 82
    expect 0
    mv a0, s0
    invoke 57
    expect 0
    mv a0, s1
    invoke 57
    expect 0
    vectors 65, 0
    expect -9
    li a0, 1
    li a1, 0
    li a2, 1
    li a3, 0
    invoke 70
    expect -29
    iov 0, zero, 0
    la t2, output
    iov 16, t2, 3
    li a0, 1
    mv a1, sp
    li a2, 2
    invoke 66
    expect 3
    li a0, 42
    invoke 94
vector_failure:
    addi a0, s11, 10
    invoke 94
    j vector_failure

.section .rodata
 vector_path: .asciz "/var/linux-vector"
 digits: .ascii "0123456"
 letters: .ascii "abcd"
 output: .ascii "io\n"

.section .note.GNU-stack,"",@progbits
