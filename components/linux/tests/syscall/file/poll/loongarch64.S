# SPDX-License-Identifier: MPL-2.0

.section .text
.macro invoke number
    li.d $a7, \number
    syscall 0
.endm
.macro expect value
    li.d $s8, \@
    li.d $t0, \value
    bne $a0, $t0, poll_failure
.endm
.global files_poll
files_poll:
    addi.d $sp, $sp, -512
    move $a0, $sp
    li.d $a1, 0
    invoke 59
    expect 0
    ld.w $s0, $sp, 0
    ld.w $s1, $sp, 4
    st.w $s0, $sp, 16
    li.d $t0, 1
    st.h $t0, $sp, 20
    st.w $s1, $sp, 24
    li.d $t0, 4
    st.h $t0, $sp, 28
    st.d $zero, $sp, 64
    st.d $zero, $sp, 72
    addi.d $a0, $sp, 16
    li.d $a1, 2
    addi.d $a2, $sp, 64
    li.d $a3, 0
    li.d $a4, 8
    invoke 73
    expect 1
    ld.hu $t0, $sp, 22
    bnez $t0, poll_failure
    ld.hu $t0, $sp, 30
    li.d $t1, 4
    bne $t0, $t1, poll_failure
    move $a0, $s1
    la.pcrel $a1, poll_payload
    li.d $a2, 1
    invoke 64
    expect 1
    addi.d $a0, $sp, 16
    li.d $a1, 2
    addi.d $a2, $sp, 64
    li.d $a3, 0
    li.d $a4, 8
    invoke 73
    expect 2
    ld.hu $t0, $sp, 22
    li.d $t1, 1
    bne $t0, $t1, poll_failure
    li.d $t0, 1
    sll.d $t0, $t0, $s0
    st.d $t0, $sp, 80
    li.d $t0, 1
    sll.d $t0, $t0, $s1
    st.d $t0, $sp, 88
    addi.d $a0, $s1, 1
    addi.d $a1, $sp, 80
    addi.d $a2, $sp, 88
    li.d $a3, 0
    addi.d $a4, $sp, 64
    li.d $a5, 0
    invoke 72
    expect 2
    move $a0, $s0
    addi.d $a1, $sp, 100
    li.d $a2, 1
    invoke 63
    expect 1
    move $a0, $s1
    invoke 57
    expect 0
    addi.d $a0, $sp, 16
    li.d $a1, 1
    li.d $a2, 0
    li.d $a3, 0
    li.d $a4, 8
    invoke 73
    expect 1
    ld.hu $t0, $sp, 22
    li.d $t1, 16
    bne $t0, $t1, poll_failure
    li.d $t0, 999
    st.w $t0, $sp, 16
    addi.d $a0, $sp, 16
    li.d $a1, 1
    addi.d $a2, $sp, 64
    li.d $a3, 0
    li.d $a4, 8
    invoke 73
    expect 1
    ld.hu $t0, $sp, 22
    li.d $t1, 32
    bne $t0, $t1, poll_failure
    li.d $a0, 0
    li.d $a1, 0
    addi.d $a2, $sp, 64
    li.d $a3, 0
    li.d $a4, 8
    li.d $t0, 2000000
    st.d $t0, $sp, 72
    invoke 73
    expect 0
    ld.d $t0, $sp, 64
    ld.d $t1, $sp, 72
    or $t0, $t0, $t1
    bnez $t0, poll_failure
    move $a0, $s0
    invoke 57
    expect 0
    li.d $a0, 42
    invoke 94
poll_failure:
    addi.d $a0, $s8, 100
    invoke 94
    b poll_failure
.section .rodata
poll_payload: .ascii "x"
.section .note.GNU-stack,"",@progbits
