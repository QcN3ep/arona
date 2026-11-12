# SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

.section .text

.macro invoke number
    li.d $a7, \number
    syscall 0
.endm

.macro expect value
    li.d $s8, \@
    li.d $t0, \value
    bne $a0, $t0, pty_failure
.endm

.global files_pty
files_pty:
    addi.d $sp, $sp, -512
    la.pcrel $t0, pts_prefix
    addi.d $t1, $sp, 128
    li.d $t2, 9
.Lcopy_prefix:
    ld.bu $t3, $t0, 0
    st.b $t3, $t1, 0
    addi.d $t0, $t0, 1
    addi.d $t1, $t1, 1
    addi.d $t2, $t2, -1
    bnez $t2, .Lcopy_prefix

    li.d $a0, -100
    la.pcrel $a1, ptmx_path
    li.d $a2, 0x802
    invoke 56
    bltz $a0, pty_failure
    move $s0, $a0
    move $a0, $s0
    li.d $a1, 0x80045430
    addi.d $a2, $sp, 0
    invoke 29
    expect 0
    ld.w $s2, $sp, 0

    # A non-blocking master with no peer data reports EAGAIN.
    move $a0, $s0
    addi.d $a1, $sp, 192
    li.d $a2, 8
    invoke 63
    expect -11

    move $a0, $s2
    addi.d $a1, $sp, 128
    bl pty_make_path
    move $a1, $a0
    li.d $a0, -100
    li.d $a2, 2
    invoke 56
    expect -5

    st.w $zero, $sp, 8
    move $a0, $s0
    li.d $a1, 0x40045431
    addi.d $a2, $sp, 8
    invoke 29
    expect 0

    # TIOCGPTPEER opens the unlocked slave without a path lookup.
    move $a0, $s0
    li.d $a1, 0x5441
    li.d $a2, 0x80102
    invoke 29
    bltz $a0, pty_failure
    move $s7, $a0
    move $a0, $s7
    li.d $a1, 0x541b
    addi.d $a2, $sp, 16
    invoke 29
    expect 0
    ld.w $t0, $sp, 16
    bnez $t0, pty_failure
    move $a0, $s7
    invoke 57
    expect 0

    move $a0, $s2
    addi.d $a1, $sp, 128
    bl pty_make_path
    move $a1, $a0
    li.d $a0, -100
    li.d $a2, 0x802
    invoke 56
    bltz $a0, pty_failure
    move $s1, $a0

    move $a0, $s1
    li.d $a1, 0x80045430
    addi.d $a2, $sp, 0
    invoke 29
    expect -25
    move $a0, $s1
    li.d $a1, 0x40045431
    addi.d $a2, $sp, 8
    invoke 29
    expect -25
    move $a0, $s1
    li.d $a1, 0x5441
    li.d $a2, 0
    invoke 29
    expect -25

    move $a0, $s1
    li.d $a1, 0x5401
    addi.d $a2, $sp, 64
    invoke 29
    expect 0
    ld.w $t0, $sp, 76
    andi $t0, $t0, 0xa
    li.d $t1, 0xa
    bne $t0, $t1, pty_failure

    move $a0, $s0
    la.pcrel $a1, ping
    li.d $a2, 5
    invoke 64
    expect 5
    move $a0, $s1
    addi.d $a1, $sp, 192
    li.d $a2, 8
    invoke 63
    expect 5
    ld.w $t0, $sp, 192
    li.d $t1, 0x676e6970
    bne $t0, $t1, pty_failure
    ld.bu $t0, $sp, 196
    li.d $t1, 10
    bne $t0, $t1, pty_failure

    move $a0, $s0
    addi.d $a1, $sp, 192
    li.d $a2, 8
    invoke 63
    expect 6
    ld.w $t0, $sp, 192
    li.d $t1, 0x676e6970
    bne $t0, $t1, pty_failure
    ld.hu $t0, $sp, 196
    li.d $t1, 0x0a0d
    bne $t0, $t1, pty_failure

    move $a0, $s1
    la.pcrel $a1, pong
    li.d $a2, 5
    invoke 64
    expect 5
    move $a0, $s0
    addi.d $a1, $sp, 192
    li.d $a2, 8
    invoke 63
    expect 6
    ld.w $t0, $sp, 192
    li.d $t1, 0x676e6f70
    bne $t0, $t1, pty_failure
    ld.hu $t0, $sp, 196
    li.d $t1, 0x0a0d
    bne $t0, $t1, pty_failure

    move $a0, $s1
    li.d $a1, 0x541b
    addi.d $a2, $sp, 16
    invoke 29
    expect 0
    ld.w $t0, $sp, 16
    bnez $t0, pty_failure

    li.d $a0, -100
    la.pcrel $a1, ptmx_path
    li.d $a2, 2
    invoke 56
    bltz $a0, pty_failure
    move $s3, $a0
    move $a0, $s3
    li.d $a1, 0x80045430
    addi.d $a2, $sp, 4
    invoke 29
    expect 0
    ld.w $s5, $sp, 4
    beq $s2, $s5, pty_failure
    st.w $zero, $sp, 8
    move $a0, $s3
    li.d $a1, 0x40045431
    addi.d $a2, $sp, 8
    invoke 29
    expect 0
    move $a0, $s5
    addi.d $a1, $sp, 128
    bl pty_make_path
    move $a1, $a0
    li.d $a0, -100
    li.d $a2, 2
    invoke 56
    bltz $a0, pty_failure
    move $s4, $a0

    move $a0, $s3
    la.pcrel $a1, two
    li.d $a2, 4
    invoke 64
    expect 4

    move $a0, $s1
    li.d $a1, 0x541b
    addi.d $a2, $sp, 16
    invoke 29
    expect 0
    ld.w $t0, $sp, 16
    bnez $t0, pty_failure
    move $a0, $s0
    li.d $a1, 0x541b
    addi.d $a2, $sp, 16
    invoke 29
    expect 0
    ld.w $t0, $sp, 16
    bnez $t0, pty_failure

    move $a0, $s4
    addi.d $a1, $sp, 192
    li.d $a2, 8
    invoke 63
    expect 4
    ld.w $t0, $sp, 192
    li.d $t1, 0x0a6f7774
    bne $t0, $t1, pty_failure
    move $a0, $s3
    addi.d $a1, $sp, 192
    li.d $a2, 8
    invoke 63
    expect 5
    ld.w $t0, $sp, 192
    li.d $t1, 0x0d6f7774
    bne $t0, $t1, pty_failure
    ld.bu $t0, $sp, 196
    li.d $t1, 10
    bne $t0, $t1, pty_failure

    move $a0, $s4
    invoke 57
    expect 0
    move $a0, $s3
    addi.d $a1, $sp, 192
    li.d $a2, 8
    invoke 63
    expect -5
    move $a0, $s3
    invoke 57
    expect 0

    li.d $a0, -100
    la.pcrel $a1, ptmx_path
    li.d $a2, 2
    invoke 56
    bltz $a0, pty_failure
    move $s6, $a0
    move $a0, $s6
    li.d $a1, 0x80045430
    addi.d $a2, $sp, 4
    invoke 29
    expect 0
    ld.w $t0, $sp, 4
    beq $t0, $s5, pty_reused
    b pty_failure
pty_reused:
    move $a0, $s6
    invoke 57
    expect 0

    move $a0, $s1
    invoke 57
    expect 0
    move $a0, $s0
    invoke 57
    expect 0

    move $a0, $s2
    addi.d $a1, $sp, 128
    bl pty_make_path
    move $a1, $a0
    li.d $a0, -100
    li.d $a2, 2
    invoke 56
    expect -2
    li.d $a0, -100
    la.pcrel $a1, missing_path
    li.d $a2, 2
    invoke 56
    expect -2

    li.d $a0, 42
    invoke 94

pty_make_path:
    addi.d $t0, $a1, 9
    addi.d $t1, $a1, 32
    move $t2, $a0
    li.d $t3, 10
.Ldigits:
    mod.du $t4, $t2, $t3
    div.du $t2, $t2, $t3
    addi.d $t4, $t4, 48
    addi.d $t1, $t1, -1
    st.b $t4, $t1, 0
    bnez $t2, .Ldigits
    addi.d $t5, $a1, 32
.Lcopy_digits:
    ld.bu $t4, $t1, 0
    st.b $t4, $t0, 0
    addi.d $t1, $t1, 1
    addi.d $t0, $t0, 1
    bne $t1, $t5, .Lcopy_digits
    st.b $zero, $t0, 0
    move $a0, $a1
    ret

pty_failure:
    addi.d $a0, $s8, 100
    invoke 94
    b pty_failure

.section .rodata
ptmx_path: .asciz "/dev/ptmx"
pts_prefix: .ascii "/dev/pts/"
missing_path: .asciz "/dev/pts/9999"
ping: .ascii "ping\n"
pong: .ascii "pong\n"
two: .ascii "two\n"
.section .note.GNU-stack,"",@progbits
