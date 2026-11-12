# SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

.section .text
.macro invoke number
    li.d $a7, \number
    syscall 0
.endm
.macro expect value
    li.d $s8, \@
    li.d $t0, \value
    bne $a0, $t0, scheduler_failure
.endm
.global scheduler
scheduler:
    # The parent scenario starts with "Wpriority"; an exec child uses
    # "Wsched-inherit" to verify that policy and nice survive execve.
    ld.d $t0, $sp, 16
    ld.bu $t1, $t0, 1
    li.d $t2, 115
    beq $t1, $t2, scheduler_inherited

    addi.d $sp, $sp, -160
    li.d $s8, 0
    st.d $zero, $sp, 0
    st.d $zero, $sp, 8
    st.d $zero, $sp, 16
    st.d $zero, $sp, 24

    # sched_getscheduler: current thread, invalid, and missing targets.
    li.d $a0, 0
    invoke 120
    expect 0
    li.d $a0, -1
    invoke 120
    expect -22
    li.d $a0, 0x7fffffff
    invoke 120
    expect -3

    # sched_getparam: output, null pointer, negative target, missing target.
    li.d $t0, -1
    st.w $t0, $sp, 0
    li.d $a0, 0
    move $a1, $sp
    invoke 121
    expect 0
    ld.w $t0, $sp, 0
    bnez $t0, scheduler_failure
    li.d $a0, 0
    li.d $a1, 0
    invoke 121
    expect -14
    li.d $a0, -1
    move $a1, $sp
    invoke 121
    expect -22
    li.d $a0, 0x7fffffff
    move $a1, $sp
    invoke 121
    expect -3

    # sched_setparam: zero priority succeeds, nonzero priority is rejected.
    li.d $a0, 0
    move $a1, $sp
    invoke 118
    expect 0
    li.d $t0, 1
    st.w $t0, $sp, 0
    li.d $a0, 0
    move $a1, $sp
    invoke 118
    expect -22
    st.w $zero, $sp, 0
    li.d $a0, 0
    li.d $a1, 0
    invoke 118
    expect -14
    li.d $a0, -1
    move $a1, $sp
    invoke 118
    expect -22

    # sched_setscheduler: batch and idle are visible through getscheduler.
    li.d $a0, 0
    li.d $a1, 3
    move $a2, $sp
    invoke 119
    expect 0
    li.d $a0, 0
    invoke 120
    expect 3
    li.d $a0, 0
    li.d $a1, 5
    move $a2, $sp
    invoke 119
    expect 0
    li.d $a0, 0
    invoke 120
    expect 5
    # FIFO and RR need real-time priority bands.
    li.d $a0, 0
    li.d $a1, 1
    move $a2, $sp
    invoke 119
    expect -22
    li.d $a0, 0
    li.d $a1, 2
    move $a2, $sp
    invoke 119
    expect -22
    # Unknown policies and reset-on-fork are not supported.
    li.d $a0, 0
    li.d $a1, 7
    move $a2, $sp
    invoke 119
    expect -22
    li.d $a0, 0
    li.d $a1, 0x40000000
    move $a2, $sp
    invoke 119
    expect -22
    # Non-real-time policies require a zero static priority.
    li.d $t0, 1
    st.w $t0, $sp, 0
    li.d $a0, 0
    li.d $a1, 0
    move $a2, $sp
    invoke 119
    expect -22
    st.w $zero, $sp, 0
    # Error paths for the policy call.
    li.d $a0, 0
    li.d $a1, 0
    li.d $a2, 0
    invoke 119
    expect -14
    li.d $a0, -1
    li.d $a1, 0
    move $a2, $sp
    invoke 119
    expect -22
    li.d $a0, 0x7fffffff
    li.d $a1, 0
    move $a2, $sp
    invoke 119
    expect -3
    # Restore the default policy.
    li.d $a0, 0
    li.d $a1, 0
    move $a2, $sp
    invoke 119
    expect 0
    li.d $a0, 0
    invoke 120
    expect 0

    # Nice values use the Linux 1..40 reporting range.
    li.d $a0, 0
    li.d $a1, 0
    invoke 141
    expect 20
    li.d $a0, 0
    li.d $a1, 0
    li.d $a2, -5
    invoke 140
    expect 0
    li.d $a0, 0
    li.d $a1, 0
    invoke 141
    expect 25
    li.d $a0, 0
    li.d $a1, 0
    li.d $a2, 19
    invoke 140
    expect 0
    li.d $a0, 0
    li.d $a1, 0
    invoke 141
    expect 1
    li.d $a0, 0
    li.d $a1, 0
    li.d $a2, -21
    invoke 140
    expect 0
    li.d $a0, 0
    li.d $a1, 0
    invoke 141
    expect 40
    li.d $a0, 0
    li.d $a1, 0
    li.d $a2, 20
    invoke 140
    expect 0
    li.d $a0, 0
    li.d $a1, 0
    invoke 141
    expect 1
    li.d $a0, 0
    li.d $a1, 0
    li.d $a2, -100
    invoke 140
    expect 0
    li.d $a0, 0
    li.d $a1, 0
    invoke 141
    expect 40
    li.d $a0, 0
    li.d $a1, 0
    li.d $a2, 100
    invoke 140
    expect 0
    li.d $a0, 0
    li.d $a1, 0
    invoke 141
    expect 1
    li.d $a0, 0
    li.d $a1, 0
    li.d $a2, 0
    invoke 140
    expect 0
    li.d $a0, 0
    li.d $a1, 0
    invoke 141
    expect 20
    # Unsupported priority targets and missing threads.
    li.d $a0, 1
    li.d $a1, 0
    li.d $a2, 0
    invoke 140
    expect -22
    li.d $a0, 0
    li.d $a1, -1
    li.d $a2, 0
    invoke 140
    expect -3
    li.d $a0, 0
    li.d $a1, 0x7fffffff
    li.d $a2, 0
    invoke 140
    expect -3
    li.d $a0, 0
    li.d $a1, 0x7fffffff
    invoke 141
    expect -3

    # CPU affinity accepts only CPU 0 in the 8-byte mask.
    li.d $t0, 1
    st.d $t0, $sp, 8
    li.d $a0, 0
    li.d $a1, 8
    addi.d $a2, $sp, 8
    invoke 122
    expect 0
    li.d $t0, -1
    st.d $t0, $sp, 8
    li.d $a0, 0
    li.d $a1, 8
    addi.d $a2, $sp, 8
    invoke 123
    expect 8
    ld.d $t0, $sp, 8
    li.d $t1, 1
    bne $t0, $t1, scheduler_failure
    # Larger masks are accepted as long as the valid member is CPU 0.
    li.d $t0, 1
    st.d $t0, $sp, 8
    li.d $t0, -1
    st.d $t0, $sp, 16
    li.d $a0, 0
    li.d $a1, 16
    addi.d $a2, $sp, 8
    invoke 122
    expect 0
    li.d $t0, -1
    st.d $t0, $sp, 8
    li.d $a0, 0
    li.d $a1, 16
    addi.d $a2, $sp, 8
    invoke 123
    expect 8
    ld.d $t0, $sp, 8
    li.d $t1, 1
    bne $t0, $t1, scheduler_failure
    # Empty masks and masks naming other processors are rejected.
    st.d $zero, $sp, 8
    li.d $a0, 0
    li.d $a1, 8
    addi.d $a2, $sp, 8
    invoke 122
    expect -22
    li.d $t0, 2
    st.d $t0, $sp, 8
    li.d $a0, 0
    li.d $a1, 8
    addi.d $a2, $sp, 8
    invoke 122
    expect -22
    li.d $t0, 3
    st.d $t0, $sp, 8
    li.d $a0, 0
    li.d $a1, 8
    addi.d $a2, $sp, 8
    invoke 122
    expect -22
    li.d $t0, 1
    slli.d $t0, $t0, 63
    st.d $t0, $sp, 8
    li.d $a0, 0
    li.d $a1, 8
    addi.d $a2, $sp, 8
    invoke 122
    expect -22
    # The mask length must cover the 8-byte processor set.
    li.d $a0, 0
    li.d $a1, 7
    addi.d $a2, $sp, 8
    invoke 122
    expect -22
    li.d $a0, 0
    li.d $a1, 4
    addi.d $a2, $sp, 8
    invoke 122
    expect -22
    li.d $a0, 0
    li.d $a1, 0
    addi.d $a2, $sp, 8
    invoke 122
    expect -22
    li.d $a0, 0
    li.d $a1, 4
    addi.d $a2, $sp, 8
    invoke 123
    expect -22
    # Invalid pointers and targets.
    li.d $a0, 0
    li.d $a1, 8
    li.d $a2, 0
    invoke 122
    expect -14
    li.d $a0, 0
    li.d $a1, 8
    li.d $a2, 0
    invoke 123
    expect -14
    li.d $a0, -1
    li.d $a1, 8
    addi.d $a2, $sp, 8
    invoke 122
    expect -22
    li.d $a0, 0x7fffffff
    li.d $a1, 8
    addi.d $a2, $sp, 8
    invoke 122
    expect -3
    li.d $a0, 0x7fffffff
    li.d $a1, 8
    addi.d $a2, $sp, 8
    invoke 123
    expect -3
    li.d $t0, 1
    st.d $t0, $sp, 8
    li.d $a0, 0
    li.d $a1, 8
    addi.d $a2, $sp, 8
    invoke 122
    expect 0

    # membarrier: query reports only the implemented commands.
    li.d $a0, 0
    li.d $a1, 0
    li.d $a2, 0
    invoke 283
    expect 543
    # Private expedited needs an explicit registration.
    li.d $a0, 8
    li.d $a1, 0
    li.d $a2, 0
    invoke 283
    expect -1
    li.d $a0, 16
    li.d $a1, 0
    li.d $a2, 0
    invoke 283
    expect 0
    li.d $a0, 8
    li.d $a1, 0
    li.d $a2, 0
    invoke 283
    expect 0
    li.d $a0, 512
    li.d $a1, 0
    li.d $a2, 0
    invoke 283
    expect 16
    # Global and global expedited barriers need no registration.
    li.d $a0, 1
    li.d $a1, 0
    li.d $a2, 0
    invoke 283
    expect 0
    li.d $a0, 2
    li.d $a1, 0
    li.d $a2, 0
    invoke 283
    expect 0
    li.d $a0, 4
    li.d $a1, 0
    li.d $a2, 0
    invoke 283
    expect 0
    li.d $a0, 512
    li.d $a1, 0
    li.d $a2, 0
    invoke 283
    expect 20
    # Unknown commands, unsupported core sync, and flags fail.
    li.d $a0, 1024
    li.d $a1, 0
    li.d $a2, 0
    invoke 283
    expect -22
    li.d $a0, 32
    li.d $a1, 0
    li.d $a2, 0
    invoke 283
    expect -95
    li.d $a0, 1
    li.d $a1, 1
    li.d $a2, 0
    invoke 283
    expect -22

    # Shared counters: 0 high, 8 low, 16 high done, 24 low done, 32 finished.
    li.d $a0, 0
    li.d $a1, 4096
    li.d $a2, 3
    li.d $a3, 0x22
    li.d $a4, -1
    li.d $a5, 0
    invoke 222
    blez $a0, scheduler_setup_failure
    move $s4, $a0
    st.d $zero, $s4, 0
    st.d $zero, $s4, 8
    st.d $zero, $s4, 16
    st.d $zero, $s4, 24
    st.d $zero, $s4, 32
    li.d $a0, 0
    li.d $a1, 8192
    li.d $a2, 3
    li.d $a3, 0x22
    li.d $a4, -1
    li.d $a5, 0
    invoke 222
    blez $a0, scheduler_setup_failure
    move $s5, $a0
    li.d $t0, 8192
    add.d $s5, $s5, $t0
    li.d $a0, 0
    li.d $a1, 8192
    li.d $a2, 3
    li.d $a3, 0x22
    li.d $a4, -1
    li.d $a5, 0
    invoke 222
    blez $a0, scheduler_setup_failure
    move $s6, $a0
    li.d $t0, 8192
    add.d $s6, $s6, $t0
    li.d $s8, 90
    li.d $a0, 0x10f00
    move $a1, $s5
    li.d $a2, 0
    li.d $a3, 0
    li.d $a4, 0
    li.d $a5, 0
    invoke 220
    beqz $a0, low_thread
    bltz $a0, scheduler_setup_failure
    li.d $a0, 0x10f00
    move $a1, $s6
    li.d $a2, 0
    li.d $a3, 0
    li.d $a4, 0
    li.d $a5, 0
    invoke 220
    beqz $a0, high_thread
    bltz $a0, scheduler_setup_failure
    # The parent waits at the lowest priority so the low-priority worker
    # cannot be starved by a busy parent after the urgent worker exits.
    li.d $a0, 0
    li.d $a1, 0
    li.d $a2, 19
    invoke 140
    expect 0
wait_children:
    ld.d $t0, $s4, 32
    li.d $t1, 2
    beq $t0, $t1, children_done
    addi.d $a0, $s4, 32
    li.d $a1, 0
    move $a2, $t0
    li.d $a3, 0
    li.d $a4, 0
    li.d $a5, 0
    invoke 98
    b wait_children
children_done:
    ld.d $t0, $s4, 0
    li.d $t1, 20
    bne $t0, $t1, scheduler_failure
    ld.d $t0, $s4, 8
    bne $t0, $t1, scheduler_failure
    ld.d $t0, $s4, 16
    li.d $t1, 1
    bne $t0, $t1, scheduler_failure
    ld.d $t0, $s4, 24
    bne $t0, $t1, scheduler_failure
    li.d $a0, 0
    li.d $a1, 0
    li.d $a2, 0
    invoke 140
    expect 0

    # Policy and nice are inherited by fork and preserved across exec.
    li.d $a0, 0
    li.d $a1, 3
    move $a2, $sp
    invoke 119
    expect 0
    li.d $a0, 0
    li.d $a1, 0
    li.d $a2, -5
    invoke 140
    expect 0
    li.d $a0, 17
    li.d $a1, 0
    li.d $a2, 0
    li.d $a3, 0
    li.d $a4, 0
    invoke 220
    beqz $a0, inherit_child
    bltz $a0, scheduler_failure
    move $s0, $a0
    move $a0, $s0
    addi.d $a1, $sp, 24
    li.d $a2, 0
    li.d $a3, 0
    invoke 260
    bne $a0, $s0, scheduler_failure
    ld.w $t0, $sp, 24
    li.d $t1, 0x2a00
    bne $t0, $t1, scheduler_failure
    li.d $a0, 17
    li.d $a1, 0
    li.d $a2, 0
    li.d $a3, 0
    li.d $a4, 0
    invoke 220
    beqz $a0, exec_child
    bltz $a0, scheduler_failure
    move $s0, $a0
    move $a0, $s0
    addi.d $a1, $sp, 24
    li.d $a2, 0
    li.d $a3, 0
    invoke 260
    bne $a0, $s0, scheduler_failure
    ld.w $t0, $sp, 24
    li.d $t1, 0x2a00
    bne $t0, $t1, scheduler_failure
    li.d $a0, 0
    li.d $a1, 0
    move $a2, $sp
    invoke 119
    expect 0
    b scheduler_success

low_thread:
    li.d $a0, 0
    li.d $a1, 0
    li.d $a2, 10
    invoke 140
    expect 0
    invoke 124
    expect 0
    li.d $s1, 20
low_loop:
    ld.d $t0, $s4, 16
    beqz $t0, scheduler_failure
    ld.d $t0, $s4, 8
    addi.d $t0, $t0, 1
    st.d $t0, $s4, 8
    invoke 124
    expect 0
    addi.d $s1, $s1, -1
    bnez $s1, low_loop
    li.d $t0, 1
    st.d $t0, $s4, 24
    ld.d $t0, $s4, 32
    addi.d $t0, $t0, 1
    st.d $t0, $s4, 32
    addi.d $a0, $s4, 32
    li.d $a1, 1
    li.d $a2, 1
    invoke 98
    li.d $a0, 0
    invoke 93
    b scheduler_failure

high_thread:
    li.d $a0, 0
    li.d $a1, 0
    li.d $a2, -10
    invoke 140
    expect 0
    li.d $a0, 0
    li.d $a1, 0
    invoke 141
    expect 30
    li.d $s0, 20
high_loop:
    ld.d $t0, $s4, 0
    addi.d $t0, $t0, 1
    st.d $t0, $s4, 0
    invoke 124
    expect 0
    addi.d $s0, $s0, -1
    bnez $s0, high_loop
    li.d $t0, 1
    st.d $t0, $s4, 16
    ld.d $t0, $s4, 32
    addi.d $t0, $t0, 1
    st.d $t0, $s4, 32
    addi.d $a0, $s4, 32
    li.d $a1, 1
    li.d $a2, 1
    invoke 98
    li.d $a0, 0
    invoke 93
    b scheduler_failure

inherit_child:
    li.d $a0, 0
    invoke 120
    expect 3
    li.d $a0, 0
    li.d $a1, 0
    invoke 141
    expect 25
    li.d $a0, 42
    invoke 93
    b scheduler_failure

exec_child:
    la.local $t0, executable
    st.d $t0, $sp, 48
    la.local $t0, inherited_mode
    st.d $t0, $sp, 56
    st.d $zero, $sp, 64
    la.local $t0, environment
    st.d $t0, $sp, 72
    st.d $zero, $sp, 80
    la.local $a0, executable
    addi.d $a1, $sp, 48
    addi.d $a2, $sp, 72
    invoke 221
    li.d $a0, 1
    invoke 93
    b scheduler_failure

scheduler_inherited:
    li.d $a0, 0
    invoke 120
    expect 3
    li.d $a0, 0
    li.d $a1, 0
    invoke 141
    expect 25
    addi.d $sp, $sp, -32
    # Clear the output mask before reading it back.
    st.d $zero, $sp, 8
    li.d $a0, 0
    li.d $a1, 8
    addi.d $a2, $sp, 8
    invoke 123
    expect 8
    ld.d $t0, $sp, 8
    li.d $t1, 1
    bne $t0, $t1, scheduler_failure
    li.d $a0, 42
    invoke 94
    b scheduler_failure

scheduler_success:
    li.d $a0, 42
    invoke 94
    b scheduler_success
scheduler_setup_failure:
    li.d $a0, 199
    invoke 94
    b scheduler_setup_failure
scheduler_failure:
    addi.d $a0, $s8, 100
    invoke 94
    b scheduler_failure

.section .rodata
executable: .asciz "/bin/linux-test"
inherited_mode: .asciz "Wsched-inherit"
environment: .asciz "ARONA_TEST=yes"
.section .note.GNU-stack,"",@progbits
