# SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

.section .text
.option norelax

.macro invoke number
    li a7, \number
    ecall
.endm

.macro expect value
    li s8, \@
    li t0, \value
    bne a0, t0, pty_failure
.endm

.global files_pty
files_pty:
    addi sp, sp, -512
    la t0, pts_prefix
    addi t1, sp, 128
    li t2, 9
.Lcopy_prefix:
    lbu t3, 0(t0)
    sb t3, 0(t1)
    addi t0, t0, 1
    addi t1, t1, 1
    addi t2, t2, -1
    bnez t2, .Lcopy_prefix

    li a0, -100
    la a1, ptmx_path
    li a2, 0x802
    invoke 56
    bltz a0, pty_failure
    mv s0, a0
    mv a0, s0
    li a1, 0x80045430
    addi a2, sp, 0
    invoke 29
    expect 0
    lw s2, 0(sp)

    # A non-blocking master with no peer data reports EAGAIN.
    mv a0, s0
    addi a1, sp, 192
    li a2, 8
    invoke 63
    expect -11

    mv a0, s2
    addi a1, sp, 128
    jal ra, pty_make_path
    mv a1, a0
    li a0, -100
    li a2, 2
    invoke 56
    expect -5

    sw zero, 8(sp)
    mv a0, s0
    li a1, 0x40045431
    addi a2, sp, 8
    invoke 29
    expect 0

    # TIOCGPTPEER opens the unlocked slave without a path lookup.
    mv a0, s0
    li a1, 0x5441
    li a2, 0x80102
    invoke 29
    bltz a0, pty_failure
    mv s7, a0
    mv a0, s7
    li a1, 0x541b
    addi a2, sp, 16
    invoke 29
    expect 0
    lw t0, 16(sp)
    bnez t0, pty_failure
    mv a0, s7
    invoke 57
    expect 0

    mv a0, s2
    addi a1, sp, 128
    jal ra, pty_make_path
    mv a1, a0
    li a0, -100
    li a2, 0x802
    invoke 56
    bltz a0, pty_failure
    mv s1, a0

    mv a0, s1
    li a1, 0x80045430
    addi a2, sp, 0
    invoke 29
    expect -25
    mv a0, s1
    li a1, 0x40045431
    addi a2, sp, 8
    invoke 29
    expect -25
    mv a0, s1
    li a1, 0x5441
    li a2, 0
    invoke 29
    expect -25

    mv a0, s1
    li a1, 0x5401
    addi a2, sp, 64
    invoke 29
    expect 0
    lw t0, 76(sp)
    andi t0, t0, 0xa
    li t1, 0xa
    bne t0, t1, pty_failure

    mv a0, s0
    la a1, ping
    li a2, 5
    invoke 64
    expect 5
    mv a0, s1
    addi a1, sp, 192
    li a2, 8
    invoke 63
    expect 5
    lw t0, 192(sp)
    li t1, 0x676e6970
    bne t0, t1, pty_failure
    lbu t0, 196(sp)
    li t1, 10
    bne t0, t1, pty_failure

    mv a0, s0
    addi a1, sp, 192
    li a2, 8
    invoke 63
    expect 6
    lw t0, 192(sp)
    li t1, 0x676e6970
    bne t0, t1, pty_failure
    lhu t0, 196(sp)
    li t1, 0x0a0d
    bne t0, t1, pty_failure

    mv a0, s1
    la a1, pong
    li a2, 5
    invoke 64
    expect 5
    mv a0, s0
    addi a1, sp, 192
    li a2, 8
    invoke 63
    expect 6
    lw t0, 192(sp)
    li t1, 0x676e6f70
    bne t0, t1, pty_failure
    lhu t0, 196(sp)
    li t1, 0x0a0d
    bne t0, t1, pty_failure

    mv a0, s1
    li a1, 0x541b
    addi a2, sp, 16
    invoke 29
    expect 0
    lw t0, 16(sp)
    bnez t0, pty_failure

    li a0, -100
    la a1, ptmx_path
    li a2, 2
    invoke 56
    bltz a0, pty_failure
    mv s3, a0
    mv a0, s3
    li a1, 0x80045430
    addi a2, sp, 4
    invoke 29
    expect 0
    lw s5, 4(sp)
    beq s2, s5, pty_failure
    sw zero, 8(sp)
    mv a0, s3
    li a1, 0x40045431
    addi a2, sp, 8
    invoke 29
    expect 0
    mv a0, s5
    addi a1, sp, 128
    jal ra, pty_make_path
    mv a1, a0
    li a0, -100
    li a2, 2
    invoke 56
    bltz a0, pty_failure
    mv s4, a0

    mv a0, s3
    la a1, two
    li a2, 4
    invoke 64
    expect 4

    mv a0, s1
    li a1, 0x541b
    addi a2, sp, 16
    invoke 29
    expect 0
    lw t0, 16(sp)
    bnez t0, pty_failure
    mv a0, s0
    li a1, 0x541b
    addi a2, sp, 16
    invoke 29
    expect 0
    lw t0, 16(sp)
    bnez t0, pty_failure

    mv a0, s4
    addi a1, sp, 192
    li a2, 8
    invoke 63
    expect 4
    lw t0, 192(sp)
    li t1, 0x0a6f7774
    bne t0, t1, pty_failure
    mv a0, s3
    addi a1, sp, 192
    li a2, 8
    invoke 63
    expect 5
    lw t0, 192(sp)
    li t1, 0x0d6f7774
    bne t0, t1, pty_failure
    lbu t0, 196(sp)
    li t1, 10
    bne t0, t1, pty_failure

    mv a0, s4
    invoke 57
    expect 0
    mv a0, s3
    addi a1, sp, 192
    li a2, 8
    invoke 63
    expect -5
    mv a0, s3
    invoke 57
    expect 0

    li a0, -100
    la a1, ptmx_path
    li a2, 2
    invoke 56
    bltz a0, pty_failure
    mv s6, a0
    mv a0, s6
    li a1, 0x80045430
    addi a2, sp, 4
    invoke 29
    expect 0
    lw t0, 4(sp)
    beq t0, s5, pty_reused
    j pty_failure
pty_reused:
    mv a0, s6
    invoke 57
    expect 0

    mv a0, s1
    invoke 57
    expect 0
    mv a0, s0
    invoke 57
    expect 0

    mv a0, s2
    addi a1, sp, 128
    jal ra, pty_make_path
    mv a1, a0
    li a0, -100
    li a2, 2
    invoke 56
    expect -2
    li a0, -100
    la a1, missing_path
    li a2, 2
    invoke 56
    expect -2

    li a0, 42
    invoke 94

pty_make_path:
    addi t0, a1, 9
    addi t1, a1, 32
    mv t2, a0
    li t3, 10
.Ldigits:
    remu t4, t2, t3
    divu t2, t2, t3
    addi t4, t4, 48
    addi t1, t1, -1
    sb t4, 0(t1)
    bnez t2, .Ldigits
    addi t5, a1, 32
.Lcopy_digits:
    lbu t4, 0(t1)
    sb t4, 0(t0)
    addi t1, t1, 1
    addi t0, t0, 1
    bne t1, t5, .Lcopy_digits
    sb zero, 0(t0)
    mv a0, a1
    ret

pty_failure:
    addi a0, s8, 100
    invoke 94
    j pty_failure

.section .rodata
ptmx_path: .asciz "/dev/ptmx"
pts_prefix: .ascii "/dev/pts/"
missing_path: .asciz "/dev/pts/9999"
ping: .ascii "ping\n"
pong: .ascii "pong\n"
two: .ascii "two\n"
.section .note.GNU-stack,"",@progbits
