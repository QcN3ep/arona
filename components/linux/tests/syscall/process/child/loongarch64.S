# SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

.section .text
.macro invoke number
    li.d $a7, \number
    syscall 0
.endm
.macro expect value
    li.d $s8, \@
    li.d $t0, \value
    bne $a0, $t0, child_failure
.endm
.global fork_process
fork_process:
    addi.d $sp, $sp, -64
    invoke 172
    move $s0, $a0
    move $a0, $sp
    li.d $a1, 0
    invoke 59
    expect 0
    ld.w $s1, $sp, 0
    ld.w $s2, $sp, 4
    li.d $a0, 0
    li.d $a1, 4096
    li.d $a2, 3
    li.d $a3, 0x21
    li.d $a4, -1
    li.d $a5, 0
    invoke 222
    blez $a0, child_failure
    move $s3, $a0
    li.d $t0, 1
    st.d $t0, $s3, 0
    la.pcrel $s4, private_word
    st.d $t0, $s4, 0
    li.d $a0, 17
    li.d $a1, 0
    li.d $a2, 0
    li.d $a3, 0
    li.d $a4, 0
    invoke 220
    beqz $a0, child
    bltz $a0, child_failure
    move $s5, $a0
    move $a0, $s2
    invoke 57
    expect 0
    move $a0, $s1
    addi.d $a1, $sp, 16
    li.d $a2, 1
    invoke 63
    expect 1
    ld.bu $t0, $sp, 16
    li.d $t1, 67
    bne $t0, $t1, child_failure
    move $a0, $s5
    li.d $a1, 1
    li.d $a2, 0
    li.d $a3, 0
    invoke 260
    expect -14
    move $a0, $s5
    addi.d $a1, $sp, 24
    li.d $a2, 0
    li.d $a3, 0
    invoke 260
    bne $a0, $s5, child_failure
    ld.w $t0, $sp, 24
    li.d $t1, 5888
    bne $t0, $t1, child_failure
    ld.d $t0, $s3, 0
    li.d $t1, 2
    bne $t0, $t1, child_failure
    ld.d $t0, $s4, 0
    li.d $t1, 1
    bne $t0, $t1, child_failure
    move $a0, $s5
    li.d $a1, 0
    li.d $a2, 1
    li.d $a3, 0
    invoke 260
    expect -10
    li.d $a0, 42
    invoke 94
child:
    invoke 172
    beq $a0, $s0, child_failure
    invoke 173
    bne $a0, $s0, child_failure
    li.d $t0, 2
    st.d $t0, $s3, 0
    st.d $t0, $s4, 0
    move $a0, $s1
    invoke 57
    expect 0
    li.d $t0, 67
    st.b $t0, $sp, 16
    move $a0, $s2
    addi.d $a1, $sp, 16
    li.d $a2, 1
    invoke 64
    expect 1
    li.d $a0, 23
    invoke 94
child_failure:
    addi.d $a0, $s8, 100
    invoke 94
    b child_failure
.section .bss
.balign 8
private_word: .skip 8
.section .note.GNU-stack,"",@progbits
