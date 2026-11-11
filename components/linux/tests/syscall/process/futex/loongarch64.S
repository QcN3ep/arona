# SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

.section .text
.macro invoke number
    li.d $a7, \number
    syscall 0
.endm
.macro expect value
    li.d $s8, \@
    li.d $t0, \value
    bne $a0, $t0, futex_failure
.endm
.global futex_basics
futex_basics:
    addi.d $sp, $sp, -32
    st.d $ra, $sp, 0
    li.d $t0, 7
    st.w $t0, $sp, 16
    addi.d $a0, $sp, 16
    li.d $a1, 128
    li.d $a2, 8
    li.d $a3, 0
    li.d $a4, 0
    li.d $a5, 0
    invoke 98
    expect -11
    addi.d $a0, $sp, 17
    li.d $a1, 129
    li.d $a2, 1
    invoke 98
    expect -22
    addi.d $a0, $sp, 16
    li.d $a1, 129
    invoke 98
    expect 0
    st.d $zero, $sp, 8
    addi.d $a0, $sp, 16
    li.d $a1, 128
    li.d $a2, 7
    move $a3, $sp
    st.d $zero, $sp, 0
    li.d $t0, 2000000
    st.d $t0, $sp, 8
    invoke 98
    expect -110
    li.d $a0, 0
    move $a1, $sp
    invoke 113
    expect 0
    ld.d $t0, $sp, 8
    li.d $t1, 2000000
    add.d $t0, $t0, $t1
    li.d $t1, 1000000000
    bltu $t0, $t1, 1f
    sub.d $t0, $t0, $t1
    ld.d $t1, $sp, 0
    addi.d $t1, $t1, 1
    st.d $t1, $sp, 0
1:
    st.d $t0, $sp, 8
    li.d $t0, 7
    st.w $t0, $sp, 16
    addi.d $a0, $sp, 16
    li.d $a1, 393
    li.d $a2, 7
    move $a3, $sp
    li.d $a4, 0
    li.d $a5, 1
    invoke 98
    expect -110
    addi.d $a0, $sp, 16
    li.d $a1, 137
    li.d $a3, 0
    li.d $a5, 0
    invoke 98
    expect -22
    li.d $a0, 1
    li.d $a1, 128
    invoke 98
    expect -22
    li.d $a0, 0
    invoke 98
    expect -14
    addi.d $sp, $sp, 32
    b threads
futex_failure:
    addi.d $a0, $s8, 100
    invoke 94
    b futex_failure
.section .note.GNU-stack,"",@progbits
