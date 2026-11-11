# SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

.section .text
.macro invoke number
    li.d $a7, \number
    syscall 0
.endm
.macro expect value
    li.d $s8, \@
    li.d $t0, \value
    bne $a0, $t0, identity_failure
.endm
.macro fork_child target
    li.d $a0, 17
    li.d $a1, 0
    li.d $a2, 0
    li.d $a3, 0
    li.d $a4, 0
    invoke 220
    beqz $a0, \target
    bltz $a0, identity_failure
    move $s6, $a0
.endm
.macro send_word descriptor, value
    move $a0, \descriptor
    move $t0, \value
    st.d $t0, $sp, 48
    addi.d $a1, $sp, 48
    li.d $a2, 8
    invoke 64
    expect 8
.endm
.macro receive_word descriptor
    move $a0, \descriptor
    addi.d $a1, $sp, 48
    li.d $a2, 8
    invoke 63
    expect 8
.endm
.macro group_of pid, expected
    move $a0, \pid
    invoke 155
    bne $a0, \expected, identity_failure
.endm
.macro session_of pid, expected
    move $a0, \pid
    invoke 156
    bne $a0, \expected, identity_failure
.endm
.macro reap pid, options, status
    move $a0, \pid
    addi.d $a1, $sp, 56
    li.d $a2, \options
    li.d $a3, 0
    invoke 260
    bne $a0, $s6, identity_failure
    ld.w $t0, $sp, 56
    li.d $t1, \status
    bne $t0, $t1, identity_failure
.endm
.global process_identity
process_identity:
    addi.d $sp, $sp, -128
    invoke 172
    move $s0, $a0
    group_of $zero, $s0
    session_of $zero, $s0
    invoke 157
    expect -1
    li.d $a0, 0
    li.d $a1, 0
    invoke 154
    expect -1
    invoke 174
    expect 0
    invoke 175
    expect 0
    invoke 176
    expect 0
    invoke 177
    expect 0
    li.d $t0, -1
    st.d $t0, $sp, 24
    st.d $t0, $sp, 32
    addi.d $a0, $sp, 24
    addi.d $a1, $sp, 28
    addi.d $a2, $sp, 32
    invoke 148
    expect 0
    ld.d $t0, $sp, 24
    bnez $t0, identity_failure
    ld.w $t0, $sp, 32
    bnez $t0, identity_failure
    li.d $a0, 0
    invoke 148
    expect -14
    addi.d $a0, $sp, 24
    addi.d $a1, $sp, 28
    addi.d $a2, $sp, 32
    invoke 150
    expect 0
    li.d $a2, 0
    invoke 150
    expect -14
    li.d $a0, 0
    li.d $a1, 0
    invoke 158
    expect 0
    li.d $a0, -1
    invoke 158
    expect -22
    li.d $a0, -1
    invoke 155
    expect -3
    li.d $a0, 0x7fffffff
    invoke 156
    expect -3
    li.d $a0, 0x7fffffff
    li.d $a1, 0
    invoke 154
    expect -3
    li.d $a0, -1
    li.d $a1, 0
    invoke 154
    expect -22
    li.d $a0, 0
    li.d $a1, -1
    invoke 154
    expect -22
    move $a0, $s0
    li.d $a1, 20
    invoke 129
    expect 0
    move $a0, $sp
    li.d $a1, 0
    invoke 59
    expect 0
    ld.w $s1, $sp, 0
    ld.w $s2, $sp, 4
    addi.d $a0, $sp, 8
    li.d $a1, 0
    invoke 59
    expect 0
    ld.w $s3, $sp, 8
    ld.w $s4, $sp, 12
    fork_child session_child
    receive_word $s1
    move $a0, $s6
    li.d $a1, 0
    invoke 154
    expect 0
    group_of $s6, $s6
    session_of $s6, $s0
    move $a0, $s6
    li.d $a1, 0x7fffffff
    invoke 154
    expect -1
    send_word $s4, $s0
    receive_word $s1
    group_of $s6, $s6
    session_of $s6, $s6
    move $a0, $s6
    move $a1, $s0
    invoke 154
    expect -1
    send_word $s4, $s0
    sub.d $s7, $zero, $s6
    reap $s7, 0, 0x1700
    move $a0, $s6
    invoke 155
    expect -3
    fork_child exec_child
    move $a0, $s6
    li.d $a1, 0
    invoke 154
    expect 0
    send_word $s4, $s0
    move $a0, $s1
    addi.d $a1, $sp, 24
    li.d $a2, 24
    invoke 63
    expect 24
    ld.d $t0, $sp, 24
    bne $t0, $s6, identity_failure
    ld.d $t0, $sp, 32
    bne $t0, $s6, identity_failure
    ld.d $t0, $sp, 40
    bne $t0, $s0, identity_failure
    move $a0, $s6
    move $a1, $s0
    invoke 154
    expect -13
    group_of $s6, $s6
    sub.d $s7, $zero, $s6
    move $a0, $s7
    li.d $a1, 19
    invoke 129
    expect 0
    reap $s7, 2, 0x137f
    move $a0, $s7
    li.d $a1, 18
    invoke 129
    expect 0
    reap $s7, 8, 0xffff
    send_word $s4, $s0
    reap $s6, 0, 0x1800
    fork_child orphan_parent
    receive_word $s1
    ld.d $s7, $sp, 48
    group_of $s7, $s7
    session_of $s7, $s6
    send_word $s4, $s0
    reap $s6, 0, 0x1a00
    receive_word $s1
    ld.d $t0, $sp, 48
    li.d $t1, 42
    bne $t0, $t1, identity_failure
orphan_exit:
    move $a0, $s7
    li.d $a1, 0
    invoke 129
    li.d $t0, -3
    beq $a0, $t0, identity_done
    expect 0
    invoke 124
    b orphan_exit
identity_done:
    move $a0, $s7
    li.d $a1, 0
    li.d $a2, 1
    li.d $a3, 0
    invoke 260
    expect -10
    fork_child credential_drop
    reap $s6, 0, 0x2a00
    fork_child credential_saved
    reap $s6, 0, 0x2a00
    li.d $a0, 42
    invoke 94
credential_drop:
    li.d $a0, 1000
    li.d $a1, 1000
    invoke 143
    expect 0
    li.d $a0, 1000
    li.d $a1, 1000
    invoke 145
    expect 0
    li.d $a0, -1
    li.d $a1, 0
    invoke 145
    expect -1
    li.d $a0, -1
    li.d $a1, 0
    invoke 143
    expect -1
    li.d $a0, 0
    invoke 146
    expect -1
    addi.d $a0, $sp, 24
    addi.d $a1, $sp, 28
    addi.d $a2, $sp, 32
    invoke 148
    expect 0
    ld.w $t0, $sp, 24
    li.d $t1, 1000
    bne $t0, $t1, identity_failure
    ld.w $t0, $sp, 28
    bne $t0, $t1, identity_failure
    ld.w $t0, $sp, 32
    bne $t0, $t1, identity_failure
    fork_child credential_inherited
    reap $s6, 0, 0x2a00
    li.d $a0, 42
    invoke 94
credential_inherited:
    invoke 174
    expect 1000
    invoke 175
    expect 1000
    invoke 176
    expect 1000
    invoke 177
    expect 1000
    li.d $a0, 42
    invoke 94
credential_saved:
    li.d $a0, 1000
    li.d $a1, 1000
    li.d $a2, 2000
    invoke 149
    expect 0
    li.d $a0, 1000
    li.d $a1, 1000
    li.d $a2, 2000
    invoke 147
    expect 0
    li.d $a0, 2000
    li.d $a1, -1
    invoke 145
    expect -1
    li.d $a0, 2000
    li.d $a1, -1
    invoke 143
    expect -1
    addi.d $a0, $sp, 24
    addi.d $a1, $sp, 28
    addi.d $a2, $sp, 32
    invoke 148
    expect 0
    ld.w $t0, $sp, 24
    li.d $t1, 1000
    bne $t0, $t1, identity_failure
    ld.w $t0, $sp, 28
    bne $t0, $t1, identity_failure
    ld.w $t0, $sp, 32
    li.d $t1, 2000
    bne $t0, $t1, identity_failure
    li.d $a0, -1
    li.d $a1, 2000
    invoke 145
    expect 0
    invoke 175
    expect 2000
    li.d $a0, -1
    li.d $a1, 2000
    invoke 143
    expect 0
    invoke 177
    expect 2000
    li.d $a0, 42
    invoke 94
session_child:
    invoke 172
    move $s5, $a0
    group_of $zero, $s0
    session_of $zero, $s0
    move $a0, $s0
    move $a1, $s5
    invoke 154
    expect -3
    la.pcrel $a0, invalid_executable
    li.d $a1, 0
    li.d $a2, 0
    invoke 221
    expect -8
    send_word $s2, $s5
    receive_word $s3
    group_of $zero, $s5
    invoke 157
    expect -1
    li.d $a0, 0
    move $a1, $s0
    invoke 154
    expect 0
    invoke 157
    bne $a0, $s5, identity_failure
    group_of $zero, $s5
    session_of $zero, $s5
    invoke 157
    expect -1
    li.d $a0, 0
    li.d $a1, 0
    invoke 154
    expect -1
    invoke 174
    expect 0
    send_word $s2, $s5
    receive_word $s3
    li.d $a0, 23
    invoke 94
exec_child:
    receive_word $s3
    move $a0, $s2
    li.d $a1, 40
    li.d $a2, 0
    invoke 24
    expect 40
    move $a0, $s3
    li.d $a1, 41
    li.d $a2, 0
    invoke 24
    expect 41
    la.pcrel $t0, identity_executable
    st.d $t0, $sp, 64
    la.pcrel $t0, identity_mode
    st.d $t0, $sp, 72
    st.d $zero, $sp, 80
    la.pcrel $t0, identity_environment
    st.d $t0, $sp, 88
    st.d $zero, $sp, 96
    la.pcrel $a0, identity_executable
    addi.d $a1, $sp, 64
    addi.d $a2, $sp, 88
    invoke 221
    b identity_failure
.global identity_replaced
identity_replaced:
    addi.d $sp, $sp, -128
    invoke 172
    st.d $a0, $sp, 24
    li.d $a0, 0
    invoke 155
    st.d $a0, $sp, 32
    li.d $a0, 0
    invoke 156
    st.d $a0, $sp, 40
    invoke 175
    expect 0
    li.d $a0, 40
    addi.d $a1, $sp, 24
    li.d $a2, 24
    invoke 64
    expect 24
    li.d $a0, 41
    addi.d $a1, $sp, 48
    li.d $a2, 8
    invoke 63
    expect 8
    li.d $a0, 0
    ld.d $a1, $sp, 40
    invoke 154
    expect 0
    invoke 157
    ld.d $t0, $sp, 24
    bne $a0, $t0, identity_failure
    li.d $a0, 24
    invoke 94
orphan_parent:
    invoke 157
    blez $a0, identity_failure
    move $s5, $a0
    addi.d $a0, $sp, 16
    li.d $a1, 0
    invoke 59
    expect 0
    fork_child orphan_child
    ld.w $a0, $sp, 16
    addi.d $a1, $sp, 48
    li.d $a2, 8
    invoke 63
    expect 8
    move $a0, $s6
    move $a1, $s6
    invoke 154
    expect 0
    move $a0, $s6
    li.d $a1, 19
    invoke 129
    expect 0
    reap $s6, 2, 0x137f
    send_word $s2, $s6
    receive_word $s3
    li.d $a0, 26
    invoke 94
orphan_child:
    li.d $a0, 1
    la.pcrel $a1, orphan_hup_action
    li.d $a2, 0
    li.d $a3, 8
    invoke 134
    expect 0
    li.d $a0, 18
    la.pcrel $a1, orphan_continue_action
    li.d $a2, 0
    li.d $a3, 8
    invoke 134
    expect 0
    ld.w $a0, $sp, 20
    addi.d $a1, $sp, 48
    li.d $a2, 8
    invoke 64
    expect 8
orphan_wait:
    la.pcrel $t0, orphan_signals
    ld.d $t0, $t0, 0
    li.d $t1, 3
    beq $t0, $t1, orphan_continued
    invoke 124
    b orphan_wait
orphan_continued:
    session_of $zero, $s5
    invoke 173
    expect 0
    invoke 172
    move $s5, $a0
    group_of $zero, $s5
    li.d $s5, 42
    send_word $s2, $s5
    li.d $a0, 25
    invoke 94
orphan_hup:
    la.pcrel $t0, orphan_signals
    ld.d $t1, $t0, 0
    ori $t1, $t1, 1
    st.d $t1, $t0, 0
    ret
orphan_continue:
    la.pcrel $t0, orphan_signals
    ld.d $t1, $t0, 0
    ori $t1, $t1, 2
    st.d $t1, $t0, 0
    ret
identity_failure:
    addi.d $a0, $s8, 100
    invoke 94
    b identity_failure
.section .data
.balign 8
orphan_hup_action: .quad orphan_hup, 4, 0
orphan_continue_action: .quad orphan_continue, 4, 0
.section .rodata
identity_executable: .asciz "/bin/linux-test"
invalid_executable: .asciz "/bin/linux-truncated"
identity_mode: .asciz "U"
identity_environment: .asciz "ARONA_TEST=yes"
.section .bss
.balign 8
orphan_signals: .skip 8
.section .note.GNU-stack,"",@progbits
