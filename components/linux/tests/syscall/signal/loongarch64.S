# SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

.section .text
.macro invoke number
    li.d $a7, \number
    syscall 0
.endm
.macro expect value
    li.d $s8, \@
    li.d $t0, \value
    bne $a0, $t0, signal_failure
.endm
.macro action number, address
    li.d $a0, \number
    la.pcrel $a1, \address
    li.d $a2, 0
    li.d $a3, 8
    invoke 134
    expect 0
.endm
.global signals
signals:
    addi.d $sp, $sp, -128
    invoke 172
    move $s0, $a0
    invoke 178
    move $s1, $a0
    action 10, user_action
    li.d $s7, 12345
    li.d $t0, 0x3ff0000000000000
    movgr2fr.d $f31, $t0
    move $a0, $s0
    li.d $a1, 10
    invoke 129
    expect 0
    li.d $t0, 12345
    bne $s7, $t0, signal_failure
    movfr2gr.d $t0, $f31
    li.d $t1, 0x3ff0000000000000
    bne $t0, $t1, signal_failure
    la.pcrel $t0, deliveries
    ld.d $t0, $t0, 0
    li.d $t1, 1
    bne $t0, $t1, signal_failure
    li.d $a0, 0
    la.pcrel $a1, user_mask
    li.d $a2, 0
    li.d $a3, 8
    invoke 135
    expect 0
    move $a0, $s0
    li.d $a1, 10
    invoke 129
    expect 0
    la.pcrel $a0, pending_mask
    li.d $a1, 8
    invoke 136
    expect 0
    la.pcrel $t0, pending_mask
    ld.d $t0, $t0, 0
    li.d $t1, 512
    bne $t0, $t1, signal_failure
    li.d $a0, 1
    la.pcrel $a1, user_mask
    li.d $a2, 0
    li.d $a3, 8
    invoke 135
    expect 0
    la.pcrel $t0, deliveries
    ld.d $t0, $t0, 0
    li.d $t1, 2
    bne $t0, $t1, signal_failure
    la.pcrel $a0, alternate
    li.d $a1, 0
    invoke 132
    expect 0
    action 10, alternate_action
    la.pcrel $t0, handler_mode
    li.d $t1, 1
    st.d $t1, $t0, 0
    move $a0, $s0
    li.d $a1, 10
    invoke 129
    expect 0
    la.pcrel $t0, alternate_seen
    ld.d $t0, $t0, 0
    li.d $t1, 1
    bne $t0, $t1, signal_failure
    la.pcrel $t0, handler_mode
    st.d $zero, $t0, 0
    action 10, user_action
    li.d $a0, 0
    la.pcrel $a1, user_mask
    li.d $a2, 0
    li.d $a3, 8
    invoke 135
    expect 0
    li.d $a0, 0x10f00
    la.pcrel $a1, child_stack_end
    li.d $a2, 0
    li.d $a3, 0
    li.d $a4, 0
    invoke 220
    beqz $a0, signal_sender
    bltz $a0, signal_failure
    move $s2, $a0
    li.d $a0, 0
    li.d $a1, 0
    la.pcrel $a2, timeout
    la.pcrel $a3, empty_mask
    li.d $a4, 8
    invoke 73
    expect -4
    li.d $a0, 0
    li.d $a1, 0
    la.pcrel $a2, pending_mask
    li.d $a3, 8
    invoke 135
    expect 0
    la.pcrel $t0, pending_mask
    ld.d $t0, $t0, 0
    li.d $t1, 512
    bne $t0, $t1, signal_failure
    li.d $a0, 1
    la.pcrel $a1, user_mask
    li.d $a2, 0
    li.d $a3, 8
    invoke 135
    expect 0
    action 10, restart_action
    move $a0, $sp
    li.d $a1, 0
    invoke 59
    expect 0
    ld.w $s4, $sp, 0
    ld.w $s5, $sp, 4
    li.d $a0, 0x10f00
    la.pcrel $a1, restart_stack_end
    li.d $a2, 0
    li.d $a3, 0
    li.d $a4, 0
    invoke 220
    beqz $a0, restart_sender
    bltz $a0, signal_failure
    la.pcrel $t0, restart_ready
    li.d $t1, 1
    st.d $t1, $t0, 0
    move $a0, $s4
    addi.d $a1, $sp, 8
    li.d $a2, 1
    invoke 63
    expect 1
    ld.bu $t0, $sp, 8
    li.d $t1, 82
    bne $t0, $t1, signal_failure
    action 10, user_action
    li.d $a0, 0
    la.pcrel $a1, second_mask
    li.d $a2, 0
    li.d $a3, 8
    invoke 135
    expect 0
    move $a0, $s0
    li.d $a1, 12
    invoke 129
    expect 0
    la.pcrel $a0, second_mask
    la.pcrel $a1, info
    la.pcrel $a2, zero_time
    li.d $a3, 8
    invoke 137
    expect 12
    la.pcrel $t0, info
    ld.w $t0, $t0, 0
    li.d $t1, 12
    bne $t0, $t1, signal_failure
    action 4, fault_action
    .balign 4
fault_instruction:
    .word 0x00000000
    action 11, protection_action
    la.pcrel $t0, fault_instruction
    .balign 4
protection_instruction:
    st.b $zero, $t0, 0
    action 18, continue_action
    li.d $a0, 0x10f00
    la.pcrel $a1, continue_stack_end
    li.d $a2, 0
    li.d $a3, 0
    li.d $a4, 0
    invoke 220
    beqz $a0, continue_child
    bltz $a0, signal_failure
    move $s6, $a0
    move $a0, $s0
    move $a1, $s6
    li.d $a2, 18
    invoke 131
    expect 0
continue_wait:
    la.pcrel $t0, continued_tid
    ld.d $t0, $t0, 0
    bnez $t0, continue_received
    invoke 124
    b continue_wait
continue_received:
    bne $t0, $s6, signal_failure
    action 18, default_action
    li.d $a0, 17
    li.d $a1, 0
    li.d $a2, 0
    li.d $a3, 0
    li.d $a4, 0
    invoke 220
    beqz $a0, signal_child
    bltz $a0, signal_failure
    move $s2, $a0
    move $a0, $s2
    li.d $a1, 19
    invoke 129
    expect 0
    move $a0, $s2
    addi.d $a1, $sp, 32
    li.d $a2, 2
    li.d $a3, 0
    invoke 260
    bne $a0, $s2, signal_failure
    ld.w $t0, $sp, 32
    li.d $t1, 0x137f
    bne $t0, $t1, signal_failure
    move $a0, $s2
    li.d $a1, 18
    invoke 129
    expect 0
    move $a0, $s2
    addi.d $a1, $sp, 32
    li.d $a2, 8
    li.d $a3, 0
    invoke 260
    bne $a0, $s2, signal_failure
    ld.w $t0, $sp, 32
    li.d $t1, 0xffff
    bne $t0, $t1, signal_failure
    move $a0, $s2
    li.d $a1, 15
    invoke 129
    expect 0
    move $a0, $s2
    addi.d $a1, $sp, 32
    li.d $a2, 0
    li.d $a3, 0
    invoke 260
    bne $a0, $s2, signal_failure
    ld.w $t0, $sp, 32
    li.d $t1, 15
    bne $t0, $t1, signal_failure
    li.d $a0, 17
    li.d $a1, 0
    li.d $a2, 0
    li.d $a3, 0
    li.d $a4, 0
    invoke 220
    beqz $a0, invalid_return
    bltz $a0, signal_failure
    move $s2, $a0
    move $a0, $s2
    addi.d $a1, $sp, 32
    li.d $a2, 0
    li.d $a3, 0
    invoke 260
    bne $a0, $s2, signal_failure
    ld.w $t0, $sp, 32
    li.d $t1, 11
    bne $t0, $t1, signal_failure
    li.d $a0, 42
    invoke 94
invalid_return:
    li.d $sp, 1
    invoke 139
    b signal_failure
restart_sender:
    la.pcrel $t0, restart_ready
    ld.d $t0, $t0, 0
    bnez $t0, restart_send
    invoke 124
    b restart_sender
restart_send:
    move $a0, $s0
    move $a1, $s1
    li.d $a2, 10
    invoke 131
    expect 0
    li.d $t0, 82
    addi.d $sp, $sp, -16
    st.b $t0, $sp, 0
    move $a0, $s5
    move $a1, $sp
    li.d $a2, 1
    invoke 64
    expect 1
    li.d $a0, 0
    invoke 93
continue_child:
    la.pcrel $t0, continued_tid
    ld.d $t0, $t0, 0
    bnez $t0, continue_done
    invoke 124
    b continue_child
continue_done:
    li.d $a0, 0
    invoke 93
signal_sender:
    move $a0, $s0
    move $a1, $s1
    li.d $a2, 10
    invoke 131
    expect 0
    li.d $a0, 0
    invoke 93
signal_child:
    invoke 124
    b signal_child
user_handler:
    li.d $t0, 10
    bne $a0, $t0, signal_failure
    ld.w $t0, $a1, 0
    bne $a0, $t0, signal_failure
    la.pcrel $t0, deliveries
    ld.d $t1, $t0, 0
    addi.d $t1, $t1, 1
    st.d $t1, $t0, 0
    li.d $s7, 999
    movgr2fr.d $f31, $zero
    la.pcrel $t0, handler_mode
    ld.d $t0, $t0, 0
    beqz $t0, handler_done
    addi.d $sp, $sp, -32
    li.d $a0, 0
    move $a1, $sp
    invoke 132
    expect 0
    ld.w $t0, $sp, 8
    li.d $t1, 1
    bne $t0, $t1, signal_failure
    la.pcrel $t0, alternate_seen
    st.d $t1, $t0, 0
    addi.d $sp, $sp, 32
handler_done:
    jirl $zero, $ra, 0
fault_handler:
    li.d $t0, 4
    bne $a0, $t0, signal_failure
    ld.d $t0, $a2, 176
    la.pcrel $t1, fault_instruction
    bne $t0, $t1, signal_failure
    addi.d $t0, $t0, 4
    st.d $t0, $a2, 176
    jirl $zero, $ra, 0
continue_handler:
    invoke 178
    la.pcrel $t0, continued_tid
    st.d $a0, $t0, 0
    jirl $zero, $ra, 0
protection_handler:
    li.d $t0, 11
    bne $a0, $t0, signal_failure
    ld.w $t0, $a1, 8
    li.d $t1, 2
    bne $t0, $t1, signal_failure
    ld.d $t0, $a1, 16
    la.pcrel $t1, fault_instruction
    bne $t0, $t1, signal_failure
    ld.d $t0, $a2, 176
    la.pcrel $t1, protection_instruction
    bne $t0, $t1, signal_failure
    addi.d $t0, $t0, 4
    st.d $t0, $a2, 176
    jirl $zero, $ra, 0
signal_failure:
    addi.d $a0, $s8, 100
    invoke 94
    b signal_failure

.section .data
.balign 8
protection_action: .quad protection_handler, 4, 0
continue_action: .quad continue_handler, 4, 0
default_action: .quad 0, 0, 0
restart_action: .quad user_handler, 0x10000004, 0
user_action: .quad user_handler, 4, 0
alternate_action: .quad user_handler, 0x08000004, 0
fault_action: .quad fault_handler, 4, 0
user_mask: .quad 512
second_mask: .quad 2048
empty_mask: .quad 0
zero_time: .quad 0, 0
timeout: .quad 10, 0
alternate: .quad alternate_stack, 0, 8192
.section .bss
.balign 16
continued_tid: .skip 8
restart_ready: .skip 8
deliveries: .skip 8
handler_mode: .skip 8
alternate_seen: .skip 8
pending_mask: .skip 8
info: .skip 128
.balign 16
alternate_stack: .skip 8192
child_stack: .skip 8192
child_stack_end:
restart_stack: .skip 8192
restart_stack_end:
continue_stack: .skip 8192
continue_stack_end:
.section .note.GNU-stack,"",@progbits
