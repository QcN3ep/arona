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
    bne a0, t0, identity_failure
.endm
.macro fork_child target
    li a0, 17
    li a1, 0
    li a2, 0
    li a3, 0
    li a4, 0
    invoke 220
    beqz a0, \target
    bltz a0, identity_failure
    mv s6, a0
.endm
.macro send_word descriptor, value
    mv a0, \descriptor
    mv t0, \value
    sd t0, 48(sp)
    addi a1, sp, 48
    li a2, 8
    invoke 64
    expect 8
.endm
.macro receive_word descriptor
    mv a0, \descriptor
    addi a1, sp, 48
    li a2, 8
    invoke 63
    expect 8
.endm
.macro group_of pid, expected
    mv a0, \pid
    invoke 155
    bne a0, \expected, identity_failure
.endm
.macro session_of pid, expected
    mv a0, \pid
    invoke 156
    bne a0, \expected, identity_failure
.endm
.macro reap pid, options, status
    mv a0, \pid
    addi a1, sp, 56
    li a2, \options
    li a3, 0
    invoke 260
    bne a0, s6, identity_failure
    lw t0, 56(sp)
    li t1, \status
    bne t0, t1, identity_failure
.endm
.global process_identity
process_identity:
    addi sp, sp, -128
    invoke 172
    mv s0, a0
    group_of zero, s0
    session_of zero, s0
    invoke 157
    expect -1
    li a0, 0
    li a1, 0
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
    li t0, -1
    sd t0, 24(sp)
    sd t0, 32(sp)
    addi a0, sp, 24
    addi a1, sp, 28
    addi a2, sp, 32
    invoke 148
    expect 0
    ld t0, 24(sp)
    bnez t0, identity_failure
    lw t0, 32(sp)
    bnez t0, identity_failure
    li a0, 0
    invoke 148
    expect -14
    addi a0, sp, 24
    addi a1, sp, 28
    addi a2, sp, 32
    invoke 150
    expect 0
    li a2, 0
    invoke 150
    expect -14
    li a0, 0
    li a1, 0
    invoke 158
    expect 0
    li a0, -1
    invoke 158
    expect -22
    li a0, -1
    invoke 155
    expect -3
    li a0, 0x7fffffff
    invoke 156
    expect -3
    li a0, 0x7fffffff
    li a1, 0
    invoke 154
    expect -3
    li a0, -1
    li a1, 0
    invoke 154
    expect -22
    li a0, 0
    li a1, -1
    invoke 154
    expect -22
    mv a0, s0
    li a1, 20
    invoke 129
    expect 0
    mv a0, sp
    li a1, 0
    invoke 59
    expect 0
    lw s1, 0(sp)
    lw s2, 4(sp)
    addi a0, sp, 8
    li a1, 0
    invoke 59
    expect 0
    lw s3, 8(sp)
    lw s4, 12(sp)
    fork_child session_child
    receive_word s1
    mv a0, s6
    li a1, 0
    invoke 154
    expect 0
    group_of s6, s6
    session_of s6, s0
    mv a0, s6
    li a1, 0x7fffffff
    invoke 154
    expect -1
    send_word s4, s0
    receive_word s1
    group_of s6, s6
    session_of s6, s6
    mv a0, s6
    mv a1, s0
    invoke 154
    expect -1
    send_word s4, s0
    neg s7, s6
    reap s7, 0, 0x1700
    mv a0, s6
    invoke 155
    expect -3
    fork_child exec_child
    mv a0, s6
    li a1, 0
    invoke 154
    expect 0
    send_word s4, s0
    mv a0, s1
    addi a1, sp, 24
    li a2, 24
    invoke 63
    expect 24
    ld t0, 24(sp)
    bne t0, s6, identity_failure
    ld t0, 32(sp)
    bne t0, s6, identity_failure
    ld t0, 40(sp)
    bne t0, s0, identity_failure
    mv a0, s6
    mv a1, s0
    invoke 154
    expect -13
    group_of s6, s6
    neg s7, s6
    mv a0, s7
    li a1, 19
    invoke 129
    expect 0
    reap s7, 2, 0x137f
    mv a0, s7
    li a1, 18
    invoke 129
    expect 0
    reap s7, 8, 0xffff
    send_word s4, s0
    reap s6, 0, 0x1800
    fork_child orphan_parent
    receive_word s1
    ld s7, 48(sp)
    group_of s7, s7
    session_of s7, s6
    send_word s4, s0
    reap s6, 0, 0x1a00
    receive_word s1
    ld t0, 48(sp)
    li t1, 42
    bne t0, t1, identity_failure
orphan_exit:
    mv a0, s7
    li a1, 0
    invoke 129
    li t0, -3
    beq a0, t0, identity_done
    expect 0
    invoke 124
    j orphan_exit
identity_done:
    mv a0, s7
    li a1, 0
    li a2, 1
    li a3, 0
    invoke 260
    expect -10
    li a0, 42
    invoke 94
session_child:
    invoke 172
    mv s5, a0
    group_of zero, s0
    session_of zero, s0
    mv a0, s0
    mv a1, s5
    invoke 154
    expect -3
    la a0, invalid_executable
    li a1, 0
    li a2, 0
    invoke 221
    expect -8
    send_word s2, s5
    receive_word s3
    group_of zero, s5
    invoke 157
    expect -1
    li a0, 0
    mv a1, s0
    invoke 154
    expect 0
    invoke 157
    bne a0, s5, identity_failure
    group_of zero, s5
    session_of zero, s5
    invoke 157
    expect -1
    li a0, 0
    li a1, 0
    invoke 154
    expect -1
    invoke 174
    expect 0
    send_word s2, s5
    receive_word s3
    li a0, 23
    invoke 94
exec_child:
    receive_word s3
    mv a0, s2
    li a1, 40
    li a2, 0
    invoke 24
    expect 40
    mv a0, s3
    li a1, 41
    li a2, 0
    invoke 24
    expect 41
    la t0, identity_executable
    sd t0, 64(sp)
    la t0, identity_mode
    sd t0, 72(sp)
    sd zero, 80(sp)
    la t0, identity_environment
    sd t0, 88(sp)
    sd zero, 96(sp)
    la a0, identity_executable
    addi a1, sp, 64
    addi a2, sp, 88
    invoke 221
    j identity_failure
.global identity_replaced
identity_replaced:
    addi sp, sp, -128
    invoke 172
    sd a0, 24(sp)
    li a0, 0
    invoke 155
    sd a0, 32(sp)
    li a0, 0
    invoke 156
    sd a0, 40(sp)
    invoke 175
    expect 0
    li a0, 40
    addi a1, sp, 24
    li a2, 24
    invoke 64
    expect 24
    li a0, 41
    addi a1, sp, 48
    li a2, 8
    invoke 63
    expect 8
    li a0, 0
    ld a1, 40(sp)
    invoke 154
    expect 0
    invoke 157
    ld t0, 24(sp)
    bne a0, t0, identity_failure
    li a0, 24
    invoke 94
orphan_parent:
    invoke 157
    blez a0, identity_failure
    mv s5, a0
    addi a0, sp, 16
    li a1, 0
    invoke 59
    expect 0
    fork_child orphan_child
    lw a0, 16(sp)
    addi a1, sp, 48
    li a2, 8
    invoke 63
    expect 8
    mv a0, s6
    mv a1, s6
    invoke 154
    expect 0
    mv a0, s6
    li a1, 19
    invoke 129
    expect 0
    reap s6, 2, 0x137f
    send_word s2, s6
    receive_word s3
    li a0, 26
    invoke 94
orphan_child:
    li a0, 1
    la a1, orphan_hup_action
    li a2, 0
    li a3, 8
    invoke 134
    expect 0
    li a0, 18
    la a1, orphan_continue_action
    li a2, 0
    li a3, 8
    invoke 134
    expect 0
    lw a0, 20(sp)
    addi a1, sp, 48
    li a2, 8
    invoke 64
    expect 8
orphan_wait:
    la t0, orphan_signals
    ld t0, 0(t0)
    li t1, 3
    beq t0, t1, orphan_continued
    invoke 124
    j orphan_wait
orphan_continued:
    session_of zero, s5
    invoke 173
    expect 0
    invoke 172
    mv s5, a0
    group_of zero, s5
    li s5, 42
    send_word s2, s5
    li a0, 25
    invoke 94
orphan_hup:
    la t0, orphan_signals
    ld t1, 0(t0)
    ori t1, t1, 1
    sd t1, 0(t0)
    ret
orphan_continue:
    la t0, orphan_signals
    ld t1, 0(t0)
    ori t1, t1, 2
    sd t1, 0(t0)
    ret
identity_failure:
    addi a0, s8, 100
    invoke 94
    j identity_failure
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
