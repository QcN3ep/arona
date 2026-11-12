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

.global futex_restart
futex_restart:
    addi.d $sp, $sp, -256
    st.d $ra, $sp, 0
    li.d $a0, 10
    la.pcrel $a1, futex_restart_action
    li.d $a2, 0
    li.d $a3, 8
    invoke 134
    bnez $a0, futex_restart_failure
    invoke 172
    move $s2, $a0
    invoke 178
    move $s0, $a0
    li.d $a0, 17
    li.d $a1, 0
    li.d $a2, 0
    li.d $a3, 0
    li.d $a4, 0
    invoke 220
    beqz $a0, futex_restart_child
    bltz $a0, futex_restart_failure
    move $s1, $a0
    st.w $zero, $sp, 16
    st.d $zero, $sp, 32
    li.d $t0, 800000000
    st.d $t0, $sp, 40
    li.d $a0, 1
    addi.d $a1, $sp, 64
    invoke 113
    bnez $a0, futex_restart_failure
    addi.d $a0, $sp, 16
    li.d $a1, 0
    li.d $a2, 0
    addi.d $a3, $sp, 32
    li.d $a4, 0
    li.d $a5, 0
    invoke 98
    li.d $t0, -110
    bne $a0, $t0, futex_restart_failure
    li.d $a0, 1
    addi.d $a1, $sp, 80
    invoke 113
    bnez $a0, futex_restart_failure
    ld.d $t0, $sp, 64
    ld.d $t1, $sp, 72
    ld.d $t2, $sp, 80
    ld.d $t3, $sp, 88
    sub.d $t2, $t2, $t0
    sub.d $t3, $t3, $t1
    li.d $t4, 1000000000
    mul.d $t2, $t2, $t4
    add.d $t2, $t2, $t3
    li.d $t4, 920000000
    bgeu $t2, $t4, futex_restart_failure
    la.pcrel $t0, futex_restart_hits
    ld.d $t0, $t0, 0
    beqz $t0, futex_restart_failure
    move $a0, $s1
    addi.d $a1, $sp, 48
    li.d $a2, 0
    li.d $a3, 0
    invoke 260
    bne $a0, $s1, futex_restart_failure
    li.d $a0, 0
    invoke 94
futex_restart_child:
    st.d $zero, $sp, 96
    li.d $t0, 200000000
    st.d $t0, $sp, 104
    addi.d $a0, $sp, 96
    li.d $a1, 0
    invoke 101
    move $a0, $s2
    move $a1, $s0
    li.d $a2, 10
    invoke 131
    li.d $a0, 0
    invoke 94
futex_restart_handler:
    la.pcrel $t0, futex_restart_hits
    ld.d $t1, $t0, 0
    addi.d $t1, $t1, 1
    st.d $t1, $t0, 0
    addi.d $sp, $sp, -32
    st.w $zero, $sp, 0
    st.d $zero, $sp, 8
    li.d $t0, 50000000
    st.d $t0, $sp, 16
    move $a0, $sp
    li.d $a1, 0
    li.d $a2, 0
    addi.d $a3, $sp, 8
    li.d $a4, 0
    li.d $a5, 0
    invoke 98
    li.d $t0, -110
    bne $a0, $t0, futex_restart_failure
    addi.d $sp, $sp, 32
    ret
futex_restart_failure:
    li.d $a0, 4
    invoke 94

.section .data
.balign 8
futex_restart_action: .quad futex_restart_handler, 0x10000004, 0
futex_restart_hits: .quad 0

.section .note.GNU-stack,"",@progbits
