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
    addi.d $sp, $sp, -256
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
    li.d $t0, 91
    st.w $t0, $s3, 8
    li.d $a0, 0x1704111
    li.d $a1, 0
    li.d $a2, 1
    addi.d $a3, $s3, 8
    li.d $a4, 0
    invoke 220
    expect -14
    ld.w $t0, $s3, 8
    li.d $t1, 91
    bne $t0, $t1, child_failure
    li.d $a0, -1
    li.d $a1, 0
    li.d $a2, 1
    li.d $a3, 0
    invoke 260
    expect -10
    st.w $zero, $s3, 8
    st.w $zero, $sp, 128
    li.d $a0, 0x1700011
    li.d $a1, 0
    addi.d $a2, $sp, 128
    addi.d $a3, $s3, 8
    li.d $a4, 0
    invoke 220
    beqz $a0, child
    bltz $a0, child_failure
    move $s5, $a0
    ld.w $t0, $sp, 128
    bne $t0, $s5, child_failure
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
wait_child_tid:
    ld.w $t0, $s3, 8
    beqz $t0, child_tid_cleared
    addi.d $a0, $s3, 8
    li.d $a1, 0
    move $a2, $t0
    li.d $a3, 0
    li.d $a4, 0
    li.d $a5, 0
    invoke 98
    beqz $a0, wait_child_tid
    expect -11
    b wait_child_tid
child_tid_cleared:
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
    li.d $t0, 91
    st.w $t0, $sp, 132
    li.d $a0, 0x1600011
    li.d $a1, 0
    li.d $a2, 0
    addi.d $a3, $sp, 132
    li.d $a4, 0
    invoke 220
    beqz $a0, busy_child
    bltz $a0, child_failure
    move $s1, $a0
    ld.w $t0, $sp, 132
    li.d $t1, 91
    bne $t0, $t1, child_failure
    move $a0, $s1
    li.d $a1, 0
    li.d $a2, 0
    li.d $a3, 1
    invoke 260
    expect -14
    move $a0, $s1
    move $a1, $sp
    li.d $a2, 0
    addi.d $a3, $sp, 80
    invoke 260
    bne $a0, $s1, child_failure
    ld.d $t0, $sp, 80
    ld.d $t1, $sp, 88
    or $t0, $t0, $t1
    beqz $t0, child_failure
    b busy_done
busy_child:
    invoke 172
    ld.w $t0, $sp, 132
    bne $a0, $t0, child_failure
    li.d $t0, 200000
1:
    addi.d $t0, $t0, -1
    bnez $t0, 1b
    li.d $a0, 0
    invoke 93
busy_done:
    li.d $a0, 42
    invoke 94
child:
    invoke 172
    beq $a0, $s0, child_failure
    ld.w $t0, $s3, 8
    bne $a0, $t0, child_failure
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
.section .text
.global vfork_process
vfork_process:
    addi.d $sp, $sp, -64
    la.pcrel $s4, private_word
    li.d $t0, 1
    st.d $t0, $s4, 0
    li.d $a0, 0x1604111
    li.d $a1, 0
    li.d $a2, 0
    addi.d $a3, $s4, 8
    li.d $a4, 0
    invoke 220
    beqz $a0, vfork_shared_child
    bltz $a0, child_failure
    move $s5, $a0
    ld.w $t0, $s4, 8
    bnez $t0, child_failure
    ld.d $t0, $s4, 0
    li.d $t1, 2
    bne $t0, $t1, child_failure
    move $a0, $s5
    addi.d $a1, $sp, 24
    li.d $a2, 0
    li.d $a3, 0
    invoke 260
    bne $a0, $s5, child_failure
    ld.w $t0, $sp, 24
    li.d $t1, 5888
    bne $t0, $t1, child_failure
    li.d $a0, 0x1604111
    li.d $a1, 0
    li.d $a2, 0
    addi.d $a3, $s4, 8
    li.d $a4, 0
    invoke 220
    beqz $a0, vfork_exec_child
    bltz $a0, child_failure
    move $s6, $a0
    ld.w $t0, $s4, 8
    bnez $t0, child_failure
    ld.d $t0, $s4, 0
    li.d $t1, 2
    bne $t0, $t1, child_failure
    move $a0, $s6
    addi.d $a1, $sp, 24
    li.d $a2, 0
    li.d $a3, 0
    invoke 260
    bne $a0, $s6, child_failure
    ld.w $t0, $sp, 24
    bnez $t0, child_failure
    li.d $a0, 42
    invoke 94
vfork_shared_child:
    la.pcrel $t0, private_word
    li.d $t1, 2
    st.d $t1, $t0, 0
    li.d $a0, 23
    invoke 94
vfork_exec_child:
    la.pcrel $t0, vfork_path
    la.pcrel $t1, vfork_argv
    st.d $t0, $t1, 0
    la.pcrel $t0, vfork_mode
    st.d $t0, $t1, 8
    st.d $zero, $t1, 16
    la.pcrel $t0, vfork_environment
    la.pcrel $t1, vfork_envp
    st.d $t0, $t1, 0
    st.d $zero, $t1, 8
    la.pcrel $a0, vfork_path
    la.pcrel $a1, vfork_argv
    la.pcrel $a2, vfork_envp
    invoke 221
    li.d $a0, 99
    invoke 94
.section .rodata
vfork_path: .asciz "/bin/linux-test"
vfork_mode: .asciz "O"
vfork_environment: .asciz "ARONA_TEST=yes"
.section .bss
.balign 8
private_word: .skip 16
vfork_argv: .skip 24
vfork_envp: .skip 16
.section .note.GNU-stack,"",@progbits
