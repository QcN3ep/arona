# SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

.section .text
.macro invoke number
    li.d $a7, \number
    syscall 0
.endm
.macro expect value
    li.d $s8, \@
    li.d $t0, \value
    bne $a0, $t0, thread_failure
.endm
.global threads
threads:
    invoke 172
    blez $a0, thread_failure
    move $s0, $a0
    invoke 178
    bne $a0, $s0, thread_failure
    invoke 173
    expect 0
    li.d $a0, 0
    invoke 96
    bne $a0, $s0, thread_failure
    li.d $a0, 0
    li.d $a1, 8192
    li.d $a2, 3
    li.d $a3, 0x22
    li.d $a4, -1
    li.d $a5, 0
    invoke 222
    blez $a0, thread_failure
    move $s1, $a0
    li.d $t0, 8192
    add.d $s1, $s1, $t0
    la.pcrel $s2, thread_data
    st.d $s0, $s2, 16
    li.d $a0, 0x13d0f00
    move $a1, $s1
    move $a2, $s2
    addi.d $a3, $s2, 4
    addi.d $a4, $s2, 32
    li.d $t0, 0x3ff0000000000000
    movgr2fr.d $ft0, $t0
    invoke 220
    beqz $a0, child
    bltz $a0, thread_failure
    beq $a0, $s0, thread_failure
    ld.w $t0, $s2, 0
    bne $a0, $t0, thread_failure
    move $s3, $a0
    move $a0, $s3
    invoke 155
    bne $a0, $s0, thread_failure
    move $a0, $s3
    invoke 156
    bne $a0, $s0, thread_failure
    move $a0, $s3
    li.d $a1, 0
    invoke 154
    expect -22
    addi.d $s5, $s1, -2048
requeue_child:
    addi.d $a0, $s2, 24
    li.d $a1, 132
    li.d $a2, 0
    li.d $a3, 1
    move $a4, $s5
    li.d $a5, 0
    invoke 98
    bnez $a0, wake_requeued
    invoke 124
    expect 0
    b requeue_child
wake_requeued:
    expect 1
    move $a0, $s5
    li.d $a1, 138
    li.d $a2, 1
    li.d $a3, 0
    li.d $a4, 0
    li.d $a5, 2
    invoke 98
    expect 0
    move $a0, $s5
    li.d $a1, 138
    li.d $a5, 4
    invoke 98
    expect 1
wait_child:
    ld.w $t0, $s2, 4
    beqz $t0, child_finished
    addi.d $a0, $s2, 4
    li.d $a1, 0
    move $a2, $t0
    li.d $a3, 0
    li.d $a4, 0
    li.d $a5, 0
    invoke 98
    beqz $a0, wait_child
    expect -11
    b wait_child
child_finished:
    ld.d $t0, $s2, 8
    bne $t0, $s3, thread_failure
    li.d $a0, 42
    invoke 94
child:
    addi.d $t0, $s2, 32
    bne $tp, $t0, thread_failure
    bne $sp, $s1, thread_failure
    movfr2gr.d $t0, $ft0
    li.d $t1, 0x3ff0000000000000
    bne $t0, $t1, thread_failure
    invoke 172
    ld.d $t0, $s2, 16
    bne $a0, $t0, thread_failure
    invoke 178
    ld.d $t0, $s2, 0
    slli.d $t0, $t0, 32
    srli.d $t0, $t0, 32
    bne $a0, $t0, thread_failure
    st.d $a0, $s2, 8
    addi.d $a0, $s2, 24
    li.d $a1, 137
    li.d $a2, 0
    li.d $a3, 0
    li.d $a4, 0
    li.d $a5, 4
    invoke 98
    expect 0
    li.d $a0, 0
    invoke 93
thread_failure:
    addi.d $a0, $s8, 100
    invoke 94
    b thread_failure
.section .bss
.balign 8
thread_data: .skip 40
.section .note.GNU-stack,"",@progbits
