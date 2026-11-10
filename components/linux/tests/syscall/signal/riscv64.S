# SPDX-License-Identifier: MPL-2.0

.section .text
.macro invoke number
    li a7, \number
    ecall
.endm
.macro expect value
    li s8, \@
    li t0, \value
    bne a0, t0, signal_failure
.endm
.macro action number, address
    li a0, \number
    la a1, \address
    li a2, 0
    li a3, 8
    invoke 134
    expect 0
.endm
.global signals
signals:
    addi sp, sp, -128
    invoke 172
    mv s0, a0
    invoke 178
    mv s1, a0
    action 10, user_action
    li s7, 12345
    li t0, 0x3ff0000000000000
    fmv.d.x f31, t0
    mv a0, s0
    li a1, 10
    invoke 129
    expect 0
    li t0, 12345
    bne s7, t0, signal_failure
    fmv.x.d t0, f31
    li t1, 0x3ff0000000000000
    bne t0, t1, signal_failure
    la t0, deliveries
    ld t0, 0(t0)
    li t1, 1
    bne t0, t1, signal_failure
    li a0, 0
    la a1, user_mask
    li a2, 0
    li a3, 8
    invoke 135
    expect 0
    mv a0, s0
    li a1, 10
    invoke 129
    expect 0
    la a0, pending_mask
    li a1, 8
    invoke 136
    expect 0
    la t0, pending_mask
    ld t0, 0(t0)
    li t1, 512
    bne t0, t1, signal_failure
    li a0, 1
    la a1, user_mask
    li a2, 0
    li a3, 8
    invoke 135
    expect 0
    la t0, deliveries
    ld t0, 0(t0)
    li t1, 2
    bne t0, t1, signal_failure
    la a0, alternate
    li a1, 0
    invoke 132
    expect 0
    action 10, alternate_action
    la t0, handler_mode
    li t1, 1
    sd t1, 0(t0)
    mv a0, s0
    li a1, 10
    invoke 129
    expect 0
    la t0, alternate_seen
    ld t0, 0(t0)
    li t1, 1
    bne t0, t1, signal_failure
    la t0, handler_mode
    sd zero, 0(t0)
    action 10, user_action
    li a0, 0
    la a1, user_mask
    li a2, 0
    li a3, 8
    invoke 135
    expect 0
    li a0, 0x10f00
    la a1, child_stack_end
    li a2, 0
    li a3, 0
    li a4, 0
    invoke 220
    beqz a0, signal_sender
    bltz a0, signal_failure
    mv s2, a0
    li a0, 0
    li a1, 0
    la a2, timeout
    la a3, empty_mask
    li a4, 8
    invoke 73
    expect -4
    li a0, 0
    li a1, 0
    la a2, pending_mask
    li a3, 8
    invoke 135
    expect 0
    la t0, pending_mask
    ld t0, 0(t0)
    li t1, 512
    bne t0, t1, signal_failure
    li a0, 1
    la a1, user_mask
    li a2, 0
    li a3, 8
    invoke 135
    expect 0
    action 10, restart_action
    mv a0, sp
    li a1, 0
    invoke 59
    expect 0
    lw s4, 0(sp)
    lw s5, 4(sp)
    li a0, 0x10f00
    la a1, restart_stack_end
    li a2, 0
    li a3, 0
    li a4, 0
    invoke 220
    beqz a0, restart_sender
    bltz a0, signal_failure
    la t0, restart_ready
    li t1, 1
    sd t1, 0(t0)
    mv a0, s4
    addi a1, sp, 8
    li a2, 1
    invoke 63
    expect 1
    lbu t0, 8(sp)
    li t1, 82
    bne t0, t1, signal_failure
    action 10, user_action
    li a0, 0
    la a1, second_mask
    li a2, 0
    li a3, 8
    invoke 135
    expect 0
    mv a0, s0
    li a1, 12
    invoke 129
    expect 0
    la a0, second_mask
    la a1, info
    la a2, zero_time
    li a3, 8
    invoke 137
    expect 12
    la t0, info
    lw t0, 0(t0)
    li t1, 12
    bne t0, t1, signal_failure
    action 4, fault_action
    .balign 4
fault_instruction:
    .word 0xffffffff
    action 11, protection_action
    la t0, fault_instruction
    .balign 4
    .option push
    .option norvc
protection_instruction:
    sb zero, 0(t0)
    .option pop
    action 18, continue_action
    li a0, 0x10f00
    la a1, continue_stack_end
    li a2, 0
    li a3, 0
    li a4, 0
    invoke 220
    beqz a0, continue_child
    bltz a0, signal_failure
    mv s6, a0
    mv a0, s0
    mv a1, s6
    li a2, 18
    invoke 131
    expect 0
continue_wait:
    la t0, continued_tid
    ld t0, 0(t0)
    bnez t0, continue_received
    invoke 124
    j continue_wait
continue_received:
    bne t0, s6, signal_failure
    action 18, default_action
    li a0, 17
    li a1, 0
    li a2, 0
    li a3, 0
    li a4, 0
    invoke 220
    beqz a0, signal_child
    bltz a0, signal_failure
    mv s2, a0
    mv a0, s2
    li a1, 19
    invoke 129
    expect 0
    mv a0, s2
    addi a1, sp, 32
    li a2, 2
    li a3, 0
    invoke 260
    bne a0, s2, signal_failure
    lw t0, 32(sp)
    li t1, 0x137f
    bne t0, t1, signal_failure
    mv a0, s2
    li a1, 18
    invoke 129
    expect 0
    mv a0, s2
    addi a1, sp, 32
    li a2, 8
    li a3, 0
    invoke 260
    bne a0, s2, signal_failure
    lw t0, 32(sp)
    li t1, 0xffff
    bne t0, t1, signal_failure
    mv a0, s2
    li a1, 15
    invoke 129
    expect 0
    mv a0, s2
    addi a1, sp, 32
    li a2, 0
    li a3, 0
    invoke 260
    bne a0, s2, signal_failure
    lw t0, 32(sp)
    li t1, 15
    bne t0, t1, signal_failure
    li a0, 17
    li a1, 0
    li a2, 0
    li a3, 0
    li a4, 0
    invoke 220
    beqz a0, invalid_return
    bltz a0, signal_failure
    mv s2, a0
    mv a0, s2
    addi a1, sp, 32
    li a2, 0
    li a3, 0
    invoke 260
    bne a0, s2, signal_failure
    lw t0, 32(sp)
    li t1, 11
    bne t0, t1, signal_failure
    li a0, 42
    invoke 94
invalid_return:
    li sp, 1
    invoke 139
    j signal_failure
restart_sender:
    la t0, restart_ready
    ld t0, 0(t0)
    bnez t0, restart_send
    invoke 124
    j restart_sender
restart_send:
    mv a0, s0
    mv a1, s1
    li a2, 10
    invoke 131
    expect 0
    li t0, 82
    addi sp, sp, -16
    sb t0, 0(sp)
    mv a0, s5
    mv a1, sp
    li a2, 1
    invoke 64
    expect 1
    li a0, 0
    invoke 93
continue_child:
    la t0, continued_tid
    ld t0, 0(t0)
    bnez t0, continue_done
    invoke 124
    j continue_child
continue_done:
    li a0, 0
    invoke 93
signal_sender:
    mv a0, s0
    mv a1, s1
    li a2, 10
    invoke 131
    expect 0
    li a0, 0
    invoke 93
signal_child:
    invoke 124
    j signal_child
user_handler:
    li t0, 10
    bne a0, t0, signal_failure
    lw t0, 0(a1)
    bne a0, t0, signal_failure
    la t0, deliveries
    ld t1, 0(t0)
    addi t1, t1, 1
    sd t1, 0(t0)
    li s7, 999
    fmv.d.x f31, zero
    la t0, handler_mode
    ld t0, 0(t0)
    beqz t0, handler_done
    addi sp, sp, -32
    li a0, 0
    mv a1, sp
    invoke 132
    expect 0
    lw t0, 8(sp)
    li t1, 1
    bne t0, t1, signal_failure
    la t0, alternate_seen
    sd t1, 0(t0)
    addi sp, sp, 32
handler_done:
    ret
fault_handler:
    li t0, 4
    bne a0, t0, signal_failure
    ld t0, 176(a2)
    la t1, fault_instruction
    bne t0, t1, signal_failure
    addi t0, t0, 4
    sd t0, 176(a2)
    ret
continue_handler:
    invoke 178
    la t0, continued_tid
    sd a0, 0(t0)
    ret
protection_handler:
    li t0, 11
    bne a0, t0, signal_failure
    lw t0, 8(a1)
    li t1, 2
    bne t0, t1, signal_failure
    ld t0, 16(a1)
    la t1, fault_instruction
    bne t0, t1, signal_failure
    ld t0, 176(a2)
    la t1, protection_instruction
    bne t0, t1, signal_failure
    addi t0, t0, 4
    sd t0, 176(a2)
    ret
signal_failure:
    addi a0, s8, 100
    invoke 94
    j signal_failure

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
