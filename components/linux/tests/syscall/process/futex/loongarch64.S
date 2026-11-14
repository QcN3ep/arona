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
    # Leave room for debug syscall overhead while distinguishing a restarted timeout.
    li.d $t0, 8
    st.d $t0, $sp, 32
    st.d $zero, $sp, 40
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
    li.d $t4, 9200000000
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
    li.d $t0, 2
    st.d $t0, $sp, 96
    st.d $zero, $sp, 104
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
    li.d $t0, 500000000
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

.section .text

.macro check value
    li.d $t0, \value
    bne $a0, $t0, futex_failure
.endm

.global robust_futex
robust_futex:
    addi.d $sp, $sp, -32
    st.d $ra, $sp, 0
    li.d $a0, 1
    la.local $a1, robust_message
    la.local $a2, robust_message_end
    la.local $t0, robust_message
    sub.d $a2, $a2, $t0
    li.d $a7, 64
    syscall 0
    la.local $s0, robust_head
    la.local $s1, robust_node
    la.local $s2, robust_lock
    la.local $s3, robust_ready
    la.local $s4, robust_owner
    la.local $s5, robust_clear
    la.local $s6, robust_get_head
    la.local $s7, robust_get_len
    st.d $s0, $s0, 0
    sub.d $t0, $s2, $s1
    st.d $t0, $s0, 8
    st.d $zero, $s0, 16
    st.d $s0, $s1, 0
    st.w $zero, $s2, 0
    st.w $zero, $s3, 0
    st.w $zero, $s4, 0
    st.w $zero, $s5, 0

    li.d $s8, 1
    move $a0, $s0
    li.d $a1, 24
    invoke 99
    check 0
    li.d $s8, 2
    li.d $a0, 0
    move $a1, $s6
    move $a2, $s7
    invoke 100
    check 0
    ld.d $t0, $s6, 0
    bne $t0, $s0, futex_failure
    ld.d $t0, $s7, 0
    li.d $t1, 24
    bne $t0, $t1, futex_failure

    li.d $s8, 3
    move $a0, $s0
    li.d $a1, 16
    invoke 99
    check -22
    move $a0, $s0
    li.d $a1, 32
    invoke 99
    check -22
    li.d $a0, -4096
    li.d $a1, 24
    invoke 99
    check -14

    li.d $s8, 4
    li.d $a0, 0
    li.d $a1, 24
    invoke 99
    check 0
    li.d $a0, 0
    move $a1, $s6
    move $a2, $s7
    invoke 100
    check 0
    ld.d $t0, $s6, 0
    bnez $t0, futex_failure
    ld.d $t0, $s7, 0
    li.d $t1, 24
    bne $t0, $t1, futex_failure
    move $a0, $s0
    li.d $a1, 24
    invoke 99
    check 0

    li.d $s8, 5
    li.d $a0, 0
    li.d $a1, -4096
    move $a2, $s7
    invoke 100
    check -14
    li.d $a0, 0
    move $a1, $s6
    li.d $a2, -4096
    invoke 100
    check -14
    li.d $a0, 0x7fffffff
    move $a1, $s6
    move $a2, $s7
    invoke 100
    check -3
    li.d $a0, -1
    move $a1, $s6
    move $a2, $s7
    invoke 100
    check -3

    li.d $s8, 6
    li.d $a0, 17
    li.d $a1, 0
    li.d $a2, 0
    li.d $a3, 0
    li.d $a4, 0
    invoke 220
    bltz $a0, futex_failure
    beqz $a0, robust_fork_child
    move $s9, $a0
    li.d $s8, 7
    move $a0, $s9
    la.local $a1, robust_status
    li.d $a2, 0
    li.d $a3, 0
    invoke 260
    bne $a0, $s9, futex_failure
    la.local $t1, robust_status
    ld.w $t0, $t1, 0
    bnez $t0, futex_failure

    li.d $s8, 9
    st.w $zero, $s3, 0
    st.w $zero, $s4, 0
    st.w $zero, $s5, 0
    st.w $zero, $s2, 0
    st.d $s1, $s0, 0
    st.d $s0, $s1, 0
    li.d $a0, 0x1210f00
    la.local $a1, robust_stack
    li.d $t0, 16384
    add.d $a1, $a1, $t0
    li.d $a2, 0
    move $a3, $s5
    li.d $a4, 0
    invoke 220
    bltz $a0, futex_failure
    beqz $a0, robust_unlock_child
    move $s9, $a0
robust_unlock_ready:
    ld.w $t0, $s3, 0
    bnez $t0, robust_unlock_ready_done
    invoke 124
    b robust_unlock_ready
robust_unlock_ready_done:
robust_unlock_wait:
    ld.w $t0, $s5, 0
    beqz $t0, robust_unlock_done
    move $a0, $s5
    li.d $a1, 0
    move $a2, $t0
    li.d $a3, 0
    li.d $a4, 0
    li.d $a5, 0
    invoke 98
    b robust_unlock_wait
robust_unlock_done:
    li.d $s8, 10
    ld.w $t0, $s2, 0
    bnez $t0, futex_failure
    li.d $s8, 11
    li.d $a0, 0
    move $a1, $s6
    move $a2, $s7
    invoke 100
    check 0
    ld.d $t0, $s6, 0
    bne $t0, $s0, futex_failure

    li.d $s8, 13
    st.w $zero, $s3, 0
    st.w $zero, $s4, 0
    st.w $zero, $s5, 0
    st.w $zero, $s2, 0
    st.d $s1, $s0, 0
    st.d $s0, $s1, 0
    li.d $a0, 0x1210f00
    la.local $a1, robust_stack
    li.d $t0, 16384
    add.d $a1, $a1, $t0
    li.d $a2, 0
    move $a3, $s5
    li.d $a4, 0
    invoke 220
    bltz $a0, futex_failure
    beqz $a0, robust_dead_child
    move $s9, $a0
robust_dead_ready:
    ld.w $t0, $s3, 0
    bnez $t0, robust_dead_ready_done
    invoke 124
    b robust_dead_ready
robust_dead_ready_done:
    li.d $s8, 14
    ld.w $t0, $s4, 0
    beqz $t0, futex_failure
    move $a0, $s2
    li.d $a1, 128
    move $a2, $t0
    li.d $a3, 0
    li.d $a4, 0
    li.d $a5, 0
    invoke 98
    check 0
    li.d $s8, 15
    ld.w $t0, $s2, 0
    li.d $t1, 0x40000000
    bne $t0, $t1, futex_failure
    li.d $s8, 16
    ld.w $t0, $s4, 0
    move $a0, $s2
    li.d $a1, 128
    move $a2, $t0
    li.d $a3, 0
    li.d $a4, 0
    li.d $a5, 0
    invoke 98
    check -11
robust_dead_wait:
    ld.w $t0, $s5, 0
    beqz $t0, robust_dead_done
    move $a0, $s5
    li.d $a1, 0
    move $a2, $t0
    li.d $a3, 0
    li.d $a4, 0
    li.d $a5, 0
    invoke 98
    b robust_dead_wait
robust_dead_done:
    st.w $zero, $s2, 0
    li.d $a0, 42
    invoke 94
    b robust_dead_done

robust_fork_child:
    la.local $s6, robust_get_head
    la.local $s7, robust_get_len
    li.d $s8, 8
    li.d $a0, 0
    move $a1, $s6
    move $a2, $s7
    invoke 100
    check 0
    ld.d $t0, $s6, 0
    bnez $t0, futex_failure
    ld.d $t0, $s7, 0
    li.d $t1, 24
    bne $t0, $t1, futex_failure
    li.d $a0, 0
    invoke 93

robust_unlock_child:
    li.d $s8, 12
    move $a0, $s0
    li.d $a1, 24
    invoke 99
    check 0
    invoke 178
    st.w $a0, $s4, 0
    st.w $a0, $s2, 0
    li.d $t0, 1
    st.w $t0, $s3, 0
    st.w $zero, $s2, 0
    st.d $s1, $s0, 16
    li.d $a0, 0
    invoke 93

robust_dead_child:
    li.d $s8, 18
    move $a0, $s0
    li.d $a1, 24
    invoke 99
    check 0
    invoke 178
    st.w $a0, $s4, 0
    st.w $a0, $s2, 0
    li.d $t0, 1
    st.w $t0, $s3, 0
    addi.d $sp, $sp, -32
    st.d $zero, $sp, 0
    li.d $t0, 200000000
    st.d $t0, $sp, 8
    move $a0, $sp
    li.d $a1, 0
    invoke 101
    li.d $a0, 0
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
