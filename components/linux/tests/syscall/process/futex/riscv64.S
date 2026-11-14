# SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

.section .text
.macro invoke number
    li a7, \number
    ecall
.endm
.macro expect value
    li s8, \@
    li t0, \value
    bne a0, t0, futex_failure
.endm
.global futex_basics
futex_basics:
    addi sp, sp, -32
    sd ra, 0(sp)
    li t0, 7
    sw t0, 16(sp)
    addi a0, sp, 16
    li a1, 128
    li a2, 8
    li a3, 0
    li a4, 0
    li a5, 0
    invoke 98
    expect -11
    addi a0, sp, 17
    li a1, 129
    li a2, 1
    invoke 98
    expect -22
    addi a0, sp, 16
    li a1, 129
    invoke 98
    expect 0
    sd zero, 8(sp)
    addi a0, sp, 16
    li a1, 128
    li a2, 7
    mv a3, sp
    sd zero, 0(sp)
    li t0, 2000000
    sd t0, 8(sp)
    invoke 98
    expect -110
    li a0, 0
    mv a1, sp
    invoke 113
    expect 0
    ld t0, 8(sp)
    li t1, 2000000
    add t0, t0, t1
    li t1, 1000000000
    bltu t0, t1, 1f
    sub t0, t0, t1
    ld t1, 0(sp)
    addi t1, t1, 1
    sd t1, 0(sp)
1:
    sd t0, 8(sp)
    li t0, 7
    sw t0, 16(sp)
    addi a0, sp, 16
    li a1, 393
    li a2, 7
    mv a3, sp
    li a4, 0
    li a5, 1
    invoke 98
    expect -110
    addi a0, sp, 16
    li a1, 137
    li a3, 0
    li a5, 0
    invoke 98
    expect -22
    li a0, 1
    li a1, 128
    invoke 98
    expect -22
    li a0, 0
    invoke 98
    expect -14
    addi sp, sp, 32
    j threads
futex_failure:
    addi a0, s8, 100
    invoke 94
    j futex_failure

.global futex_restart
futex_restart:
    addi sp, sp, -256
    sd ra, 0(sp)
    li a0, 10
    la a1, futex_restart_action
    li a2, 0
    li a3, 8
    invoke 134
    bnez a0, futex_restart_failure
    invoke 172
    mv s2, a0
    invoke 178
    mv s0, a0
    li a0, 17
    li a1, 0
    li a2, 0
    li a3, 0
    li a4, 0
    invoke 220
    beqz a0, futex_restart_child
    bltz a0, futex_restart_failure
    mv s1, a0
    sw zero, 16(sp)
    # Leave room for debug syscall overhead while distinguishing a restarted timeout.
    li t0, 8
    sd t0, 32(sp)
    sd zero, 40(sp)
    li a0, 1
    addi a1, sp, 64
    invoke 113
    bnez a0, futex_restart_failure
    addi a0, sp, 16
    li a1, 0
    li a2, 0
    addi a3, sp, 32
    li a4, 0
    li a5, 0
    invoke 98
    li t0, -110
    bne a0, t0, futex_restart_failure
    li a0, 1
    addi a1, sp, 80
    invoke 113
    bnez a0, futex_restart_failure
    ld t0, 64(sp)
    ld t1, 72(sp)
    ld t2, 80(sp)
    ld t3, 88(sp)
    sub t2, t2, t0
    sub t3, t3, t1
    li t4, 1000000000
    mul t2, t2, t4
    add t2, t2, t3
    li t4, 9200000000
    bgeu t2, t4, futex_restart_failure
    la t0, futex_restart_hits
    ld t0, 0(t0)
    beqz t0, futex_restart_failure
    mv a0, s1
    addi a1, sp, 48
    li a2, 0
    li a3, 0
    invoke 260
    bne a0, s1, futex_restart_failure
    li a0, 0
    invoke 94
futex_restart_child:
    li t0, 2
    sd t0, 96(sp)
    sd zero, 104(sp)
    addi a0, sp, 96
    li a1, 0
    invoke 101
    mv a0, s2
    mv a1, s0
    li a2, 10
    invoke 131
    li a0, 0
    invoke 94
futex_restart_handler:
    la t0, futex_restart_hits
    ld t1, 0(t0)
    addi t1, t1, 1
    sd t1, 0(t0)
    addi sp, sp, -32
    sw zero, 0(sp)
    sd zero, 8(sp)
    li t0, 500000000
    sd t0, 16(sp)
    mv a0, sp
    li a1, 0
    li a2, 0
    addi a3, sp, 8
    li a4, 0
    li a5, 0
    invoke 98
    li t0, -110
    bne a0, t0, futex_restart_failure
    addi sp, sp, 32
    ret
futex_restart_failure:
    li a0, 4
    invoke 94

.section .data
.balign 8
futex_restart_action: .quad futex_restart_handler, 0x10000004, 0
futex_restart_hits: .quad 0

.section .text

.macro check value
    li t0, \value
    bne a0, t0, futex_failure
.endm

.global robust_futex
robust_futex:
    addi sp, sp, -32
    sd ra, 0(sp)
    li a0, 1
    la a1, robust_message
    li a2, robust_message_end - robust_message
    li a7, 64
    ecall
    la s0, robust_head
    la s1, robust_node
    la s2, robust_lock
    la s3, robust_ready
    la s4, robust_owner
    la s5, robust_clear
    la s6, robust_get_head
    la s7, robust_get_len
    sd s0, 0(s0)
    sub t0, s2, s1
    sd t0, 8(s0)
    sd zero, 16(s0)
    sd s0, 0(s1)
    sw zero, 0(s2)
    sw zero, 0(s3)
    sw zero, 0(s4)
    sw zero, 0(s5)

    li s8, 1
    mv a0, s0
    li a1, 24
    invoke 99
    check 0
    li s8, 2
    li a0, 0
    mv a1, s6
    mv a2, s7
    invoke 100
    check 0
    ld t0, 0(s6)
    bne t0, s0, futex_failure
    ld t0, 0(s7)
    li t1, 24
    bne t0, t1, futex_failure

    li s8, 3
    mv a0, s0
    li a1, 16
    invoke 99
    check -22
    mv a0, s0
    li a1, 32
    invoke 99
    check -22
    li a0, -4096
    li a1, 24
    invoke 99
    check -14

    li s8, 4
    li a0, 0
    li a1, 24
    invoke 99
    check 0
    li a0, 0
    mv a1, s6
    mv a2, s7
    invoke 100
    check 0
    ld t0, 0(s6)
    bnez t0, futex_failure
    ld t0, 0(s7)
    li t1, 24
    bne t0, t1, futex_failure
    mv a0, s0
    li a1, 24
    invoke 99
    check 0

    li s8, 5
    li a0, 0
    li a1, -4096
    mv a2, s7
    invoke 100
    check -14
    li a0, 0
    mv a1, s6
    li a2, -4096
    invoke 100
    check -14
    li a0, 0x7fffffff
    mv a1, s6
    mv a2, s7
    invoke 100
    check -3
    li a0, -1
    mv a1, s6
    mv a2, s7
    invoke 100
    check -3

    li s8, 6
    li a0, 17
    li a1, 0
    li a2, 0
    li a3, 0
    li a4, 0
    invoke 220
    bltz a0, futex_failure
    beqz a0, robust_fork_child
    mv s9, a0
    li s8, 7
    mv a0, s9
    la a1, robust_status
    li a2, 0
    li a3, 0
    invoke 260
    bne a0, s9, futex_failure
    la t1, robust_status
    lw t0, 0(t1)
    bnez t0, futex_failure

    li s8, 9
    sw zero, 0(s3)
    sw zero, 0(s4)
    sw zero, 0(s5)
    sw zero, 0(s2)
    sd s1, 0(s0)
    sd s0, 0(s1)
    li a0, 0x1210f00
    la a1, robust_stack
    li t0, 16384
    add a1, a1, t0
    li a2, 0
    li a3, 0
    mv a4, s5
    invoke 220
    bltz a0, futex_failure
    beqz a0, robust_unlock_child
    mv s9, a0
robust_unlock_ready:
    lw t0, 0(s3)
    bnez t0, robust_unlock_ready_done
    invoke 124
    j robust_unlock_ready
robust_unlock_ready_done:
robust_unlock_wait:
    lw t0, 0(s5)
    beqz t0, robust_unlock_done
    mv a0, s5
    li a1, 0
    mv a2, t0
    li a3, 0
    li a4, 0
    li a5, 0
    invoke 98
    j robust_unlock_wait
robust_unlock_done:
    li s8, 10
    lw t0, 0(s2)
    bnez t0, futex_failure
    li s8, 11
    li a0, 0
    mv a1, s6
    mv a2, s7
    invoke 100
    check 0
    ld t0, 0(s6)
    bne t0, s0, futex_failure

    li s8, 13
    sw zero, 0(s3)
    sw zero, 0(s4)
    sw zero, 0(s5)
    sw zero, 0(s2)
    sd s1, 0(s0)
    sd s0, 0(s1)
    li a0, 0x1210f00
    la a1, robust_stack
    li t0, 16384
    add a1, a1, t0
    li a2, 0
    li a3, 0
    mv a4, s5
    invoke 220
    bltz a0, futex_failure
    beqz a0, robust_dead_child
    mv s9, a0
robust_dead_ready:
    lw t0, 0(s3)
    bnez t0, robust_dead_ready_done
    invoke 124
    j robust_dead_ready
robust_dead_ready_done:
    li s8, 14
    lw t0, 0(s4)
    beqz t0, futex_failure
    mv a0, s2
    li a1, 128
    mv a2, t0
    li a3, 0
    li a4, 0
    li a5, 0
    invoke 98
    check 0
    li s8, 15
    lw t0, 0(s2)
    li t1, 0x40000000
    bne t0, t1, futex_failure
    li s8, 16
    lw t0, 0(s4)
    mv a0, s2
    li a1, 128
    mv a2, t0
    li a3, 0
    li a4, 0
    li a5, 0
    invoke 98
    check -11
robust_dead_wait:
    lw t0, 0(s5)
    beqz t0, robust_dead_done
    mv a0, s5
    li a1, 0
    mv a2, t0
    li a3, 0
    li a4, 0
    li a5, 0
    invoke 98
    j robust_dead_wait
robust_dead_done:
    sw zero, 0(s2)
    li a0, 42
    invoke 94
    j robust_dead_done

robust_fork_child:
    la s6, robust_get_head
    la s7, robust_get_len
    li s8, 8
    li a0, 0
    mv a1, s6
    mv a2, s7
    invoke 100
    check 0
    ld t0, 0(s6)
    bnez t0, futex_failure
    ld t0, 0(s7)
    li t1, 24
    bne t0, t1, futex_failure
    li a0, 0
    invoke 93

robust_unlock_child:
    li s8, 12
    mv a0, s0
    li a1, 24
    invoke 99
    check 0
    invoke 178
    sw a0, 0(s4)
    sw a0, 0(s2)
    li t0, 1
    sw t0, 0(s3)
    sw zero, 0(s2)
    sd s1, 16(s0)
    li a0, 0
    invoke 93

robust_dead_child:
    li s8, 18
    mv a0, s0
    li a1, 24
    invoke 99
    check 0
    invoke 178
    sw a0, 0(s4)
    sw a0, 0(s2)
    li t0, 1
    sw t0, 0(s3)
    addi sp, sp, -32
    sd zero, 0(sp)
    li t0, 200000000
    sd t0, 8(sp)
    mv a0, sp
    li a1, 0
    invoke 101
    li a0, 0
    invoke 93

.section .rodata
robust_message: .ascii "Robust futex test\n"
robust_message_end:

.section .bss
.balign 8
robust_head: .skip 24
robust_node: .skip 8
robust_lock: .skip 4
robust_ready: .skip 4
robust_owner: .skip 4
robust_clear: .skip 4
robust_status: .skip 4
.balign 8
robust_get_head: .skip 8
robust_get_len: .skip 8
.balign 16
robust_stack: .skip 16384

.section .note.GNU-stack,"",@progbits
