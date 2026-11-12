# SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

.section .text
.macro invoke number
    li a7, \number
    ecall
.endm
.macro expect value
    li s8, \@
    li t0, \value
    bne a0, t0, scheduler_failure
.endm
.global scheduler
scheduler:
    # The parent scenario starts with "Wpriority"; an exec child uses
    # "Wsched-inherit" to verify that policy and nice survive execve.
    ld t0, 16(sp)
    lbu t1, 1(t0)
    li t2, 115
    beq t1, t2, scheduler_inherited

    addi sp, sp, -160
    li s8, 0
    sd zero, 0(sp)
    sd zero, 8(sp)
    sd zero, 16(sp)
    sd zero, 24(sp)

    # sched_getscheduler: current thread, invalid, and missing targets.
    li a0, 0
    invoke 120
    expect 0
    li a0, -1
    invoke 120
    expect -22
    li a0, 0x7fffffff
    invoke 120
    expect -3

    # sched_getparam: output, null pointer, negative target, missing target.
    li t0, -1
    sw t0, 0(sp)
    li a0, 0
    move a1, sp
    invoke 121
    expect 0
    lw t0, 0(sp)
    bnez t0, scheduler_failure
    li a0, 0
    li a1, 0
    invoke 121
    expect -14
    li a0, -1
    move a1, sp
    invoke 121
    expect -22
    li a0, 0x7fffffff
    move a1, sp
    invoke 121
    expect -3

    # sched_setparam: zero priority succeeds, nonzero priority is rejected.
    li a0, 0
    move a1, sp
    invoke 118
    expect 0
    li t0, 1
    sw t0, 0(sp)
    li a0, 0
    move a1, sp
    invoke 118
    expect -22
    sw zero, 0(sp)
    li a0, 0
    li a1, 0
    invoke 118
    expect -14
    li a0, -1
    move a1, sp
    invoke 118
    expect -22

    # sched_setscheduler: batch and idle are visible through getscheduler.
    li a0, 0
    li a1, 3
    move a2, sp
    invoke 119
    expect 0
    li a0, 0
    invoke 120
    expect 3
    li a0, 0
    li a1, 5
    move a2, sp
    invoke 119
    expect 0
    li a0, 0
    invoke 120
    expect 5
    # FIFO and RR need real-time priority bands.
    li a0, 0
    li a1, 1
    move a2, sp
    invoke 119
    expect -22
    li a0, 0
    li a1, 2
    move a2, sp
    invoke 119
    expect -22
    # Unknown policies and reset-on-fork are not supported.
    li a0, 0
    li a1, 7
    move a2, sp
    invoke 119
    expect -22
    li a0, 0
    li a1, 0x40000000
    move a2, sp
    invoke 119
    expect -22
    # Non-real-time policies require a zero static priority.
    li t0, 1
    sw t0, 0(sp)
    li a0, 0
    li a1, 0
    move a2, sp
    invoke 119
    expect -22
    sw zero, 0(sp)
    # Error paths for the policy call.
    li a0, 0
    li a1, 0
    li a2, 0
    invoke 119
    expect -14
    li a0, -1
    li a1, 0
    move a2, sp
    invoke 119
    expect -22
    li a0, 0x7fffffff
    li a1, 0
    move a2, sp
    invoke 119
    expect -3
    # Restore the default policy.
    li a0, 0
    li a1, 0
    move a2, sp
    invoke 119
    expect 0
    li a0, 0
    invoke 120
    expect 0

    # Nice values use the Linux 1..40 reporting range.
    li a0, 0
    li a1, 0
    invoke 141
    expect 20
    li a0, 0
    li a1, 0
    li a2, -5
    invoke 140
    expect 0
    li a0, 0
    li a1, 0
    invoke 141
    expect 25
    li a0, 0
    li a1, 0
    li a2, 19
    invoke 140
    expect 0
    li a0, 0
    li a1, 0
    invoke 141
    expect 1
    li a0, 0
    li a1, 0
    li a2, -21
    invoke 140
    expect 0
    li a0, 0
    li a1, 0
    invoke 141
    expect 40
    li a0, 0
    li a1, 0
    li a2, 20
    invoke 140
    expect 0
    li a0, 0
    li a1, 0
    invoke 141
    expect 1
    li a0, 0
    li a1, 0
    li a2, -100
    invoke 140
    expect 0
    li a0, 0
    li a1, 0
    invoke 141
    expect 40
    li a0, 0
    li a1, 0
    li a2, 100
    invoke 140
    expect 0
    li a0, 0
    li a1, 0
    invoke 141
    expect 1
    li a0, 0
    li a1, 0
    li a2, 0
    invoke 140
    expect 0
    li a0, 0
    li a1, 0
    invoke 141
    expect 20
    # Unsupported priority targets and missing threads.
    li a0, 1
    li a1, 0
    li a2, 0
    invoke 140
    expect -22
    li a0, 0
    li a1, -1
    li a2, 0
    invoke 140
    expect -3
    li a0, 0
    li a1, 0x7fffffff
    li a2, 0
    invoke 140
    expect -3
    li a0, 0
    li a1, 0x7fffffff
    invoke 141
    expect -3

    # CPU affinity accepts only CPU 0 in the 8-byte mask.
    li t0, 1
    sd t0, 8(sp)
    li a0, 0
    li a1, 8
    addi a2, sp, 8
    invoke 122
    expect 0
    li t0, -1
    sd t0, 8(sp)
    li a0, 0
    li a1, 8
    addi a2, sp, 8
    invoke 123
    expect 8
    ld t0, 8(sp)
    li t1, 1
    bne t0, t1, scheduler_failure
    # Larger masks are accepted as long as the valid member is CPU 0.
    li t0, 1
    sd t0, 8(sp)
    li t0, -1
    sd t0, 16(sp)
    li a0, 0
    li a1, 16
    addi a2, sp, 8
    invoke 122
    expect 0
    li t0, -1
    sd t0, 8(sp)
    li a0, 0
    li a1, 16
    addi a2, sp, 8
    invoke 123
    expect 8
    ld t0, 8(sp)
    li t1, 1
    bne t0, t1, scheduler_failure
    # Empty masks and masks naming other processors are rejected.
    sd zero, 8(sp)
    li a0, 0
    li a1, 8
    addi a2, sp, 8
    invoke 122
    expect -22
    li t0, 2
    sd t0, 8(sp)
    li a0, 0
    li a1, 8
    addi a2, sp, 8
    invoke 122
    expect -22
    li t0, 3
    sd t0, 8(sp)
    li a0, 0
    li a1, 8
    addi a2, sp, 8
    invoke 122
    expect -22
    li t0, 1
    slli t0, t0, 63
    sd t0, 8(sp)
    li a0, 0
    li a1, 8
    addi a2, sp, 8
    invoke 122
    expect -22
    # The mask length must cover the 8-byte processor set.
    li a0, 0
    li a1, 7
    addi a2, sp, 8
    invoke 122
    expect -22
    li a0, 0
    li a1, 4
    addi a2, sp, 8
    invoke 122
    expect -22
    li a0, 0
    li a1, 0
    addi a2, sp, 8
    invoke 122
    expect -22
    li a0, 0
    li a1, 4
    addi a2, sp, 8
    invoke 123
    expect -22
    # Invalid pointers and targets.
    li a0, 0
    li a1, 8
    li a2, 0
    invoke 122
    expect -14
    li a0, 0
    li a1, 8
    li a2, 0
    invoke 123
    expect -14
    li a0, -1
    li a1, 8
    addi a2, sp, 8
    invoke 122
    expect -22
    li a0, 0x7fffffff
    li a1, 8
    addi a2, sp, 8
    invoke 122
    expect -3
    li a0, 0x7fffffff
    li a1, 8
    addi a2, sp, 8
    invoke 123
    expect -3
    li t0, 1
    sd t0, 8(sp)
    li a0, 0
    li a1, 8
    addi a2, sp, 8
    invoke 122
    expect 0

    # membarrier: query reports only the implemented commands.
    li a0, 0
    li a1, 0
    li a2, 0
    invoke 283
    expect 543
    # Private expedited needs an explicit registration.
    li a0, 8
    li a1, 0
    li a2, 0
    invoke 283
    expect -1
    li a0, 16
    li a1, 0
    li a2, 0
    invoke 283
    expect 0
    li a0, 8
    li a1, 0
    li a2, 0
    invoke 283
    expect 0
    li a0, 512
    li a1, 0
    li a2, 0
    invoke 283
    expect 16
    # Global and global expedited barriers need no registration.
    li a0, 1
    li a1, 0
    li a2, 0
    invoke 283
    expect 0
    li a0, 2
    li a1, 0
    li a2, 0
    invoke 283
    expect 0
    li a0, 4
    li a1, 0
    li a2, 0
    invoke 283
    expect 0
    li a0, 512
    li a1, 0
    li a2, 0
    invoke 283
    expect 20
    # Unknown commands, unsupported core sync, and flags fail.
    li a0, 1024
    li a1, 0
    li a2, 0
    invoke 283
    expect -22
    li a0, 32
    li a1, 0
    li a2, 0
    invoke 283
    expect -95
    li a0, 1
    li a1, 1
    li a2, 0
    invoke 283
    expect -22

    # Shared counters: 0 high, 8 low, 16 high done, 24 low done, 32 finished.
    li a0, 0
    li a1, 4096
    li a2, 3
    li a3, 0x22
    li a4, -1
    li a5, 0
    invoke 222
    blez a0, scheduler_setup_failure
    mv s4, a0
    sd zero, 0(s4)
    sd zero, 8(s4)
    sd zero, 16(s4)
    sd zero, 24(s4)
    sd zero, 32(s4)
    li a0, 0
    li a1, 8192
    li a2, 3
    li a3, 0x22
    li a4, -1
    li a5, 0
    invoke 222
    blez a0, scheduler_setup_failure
    mv s5, a0
    li t0, 8192
    add s5, s5, t0
    li a0, 0
    li a1, 8192
    li a2, 3
    li a3, 0x22
    li a4, -1
    li a5, 0
    invoke 222
    blez a0, scheduler_setup_failure
    mv s6, a0
    li t0, 8192
    add s6, s6, t0
    li s8, 90
    li a0, 0x10f00
    mv a1, s5
    li a2, 0
    li a3, 0
    li a4, 0
    li a5, 0
    invoke 220
    beqz a0, low_thread
    bltz a0, scheduler_setup_failure
    li a0, 0x10f00
    mv a1, s6
    li a2, 0
    li a3, 0
    li a4, 0
    li a5, 0
    invoke 220
    beqz a0, high_thread
    bltz a0, scheduler_setup_failure
    # The parent waits at the lowest priority so the low-priority worker
    # cannot be starved by a busy parent after the urgent worker exits.
    li a0, 0
    li a1, 0
    li a2, 19
    invoke 140
    expect 0
wait_children:
    ld t0, 32(s4)
    li t1, 2
    beq t0, t1, children_done
    addi a0, s4, 32
    li a1, 0
    mv a2, t0
    li a3, 0
    li a4, 0
    li a5, 0
    invoke 98
    j wait_children
children_done:
    ld t0, 0(s4)
    li t1, 20
    bne t0, t1, scheduler_failure
    ld t0, 8(s4)
    bne t0, t1, scheduler_failure
    ld t0, 16(s4)
    li t1, 1
    bne t0, t1, scheduler_failure
    ld t0, 24(s4)
    bne t0, t1, scheduler_failure
    li a0, 0
    li a1, 0
    li a2, 0
    invoke 140
    expect 0

    # Policy and nice are inherited by fork and preserved across exec.
    li a0, 0
    li a1, 3
    move a2, sp
    invoke 119
    expect 0
    li a0, 0
    li a1, 0
    li a2, -5
    invoke 140
    expect 0
    li a0, 17
    li a1, 0
    li a2, 0
    li a3, 0
    li a4, 0
    invoke 220
    beqz a0, inherit_child
    bltz a0, scheduler_failure
    mv s0, a0
    mv a0, s0
    addi a1, sp, 24
    li a2, 0
    li a3, 0
    invoke 260
    bne a0, s0, scheduler_failure
    lw t0, 24(sp)
    li t1, 0x2a00
    bne t0, t1, scheduler_failure
    li a0, 17
    li a1, 0
    li a2, 0
    li a3, 0
    li a4, 0
    invoke 220
    beqz a0, exec_child
    bltz a0, scheduler_failure
    mv s0, a0
    mv a0, s0
    addi a1, sp, 24
    li a2, 0
    li a3, 0
    invoke 260
    bne a0, s0, scheduler_failure
    lw t0, 24(sp)
    li t1, 0x2a00
    bne t0, t1, scheduler_failure
    li a0, 0
    li a1, 0
    move a2, sp
    invoke 119
    expect 0
    j scheduler_success

low_thread:
    li a0, 0
    li a1, 0
    li a2, 10
    invoke 140
    expect 0
    invoke 124
    expect 0
    li s1, 20
low_loop:
    ld t0, 16(s4)
    beqz t0, scheduler_failure
    ld t0, 8(s4)
    addi t0, t0, 1
    sd t0, 8(s4)
    invoke 124
    expect 0
    addi s1, s1, -1
    bnez s1, low_loop
    li t0, 1
    sd t0, 24(s4)
    ld t0, 32(s4)
    addi t0, t0, 1
    sd t0, 32(s4)
    addi a0, s4, 32
    li a1, 1
    li a2, 1
    invoke 98
    li a0, 0
    invoke 93
    j scheduler_failure

high_thread:
    li a0, 0
    li a1, 0
    li a2, -10
    invoke 140
    expect 0
    li a0, 0
    li a1, 0
    invoke 141
    expect 30
    li s0, 20
high_loop:
    ld t0, 0(s4)
    addi t0, t0, 1
    sd t0, 0(s4)
    invoke 124
    expect 0
    addi s0, s0, -1
    bnez s0, high_loop
    li t0, 1
    sd t0, 16(s4)
    ld t0, 32(s4)
    addi t0, t0, 1
    sd t0, 32(s4)
    addi a0, s4, 32
    li a1, 1
    li a2, 1
    invoke 98
    li a0, 0
    invoke 93
    j scheduler_failure

inherit_child:
    li a0, 0
    invoke 120
    expect 3
    li a0, 0
    li a1, 0
    invoke 141
    expect 25
    li a0, 42
    invoke 93
    j scheduler_failure

exec_child:
    la t0, executable
    sd t0, 48(sp)
    la t0, inherited_mode
    sd t0, 56(sp)
    sd zero, 64(sp)
    la t0, environment
    sd t0, 72(sp)
    sd zero, 80(sp)
    la a0, executable
    addi a1, sp, 48
    addi a2, sp, 72
    invoke 221
    li a0, 1
    invoke 93
    j scheduler_failure

scheduler_inherited:
    li a0, 0
    invoke 120
    expect 3
    li a0, 0
    li a1, 0
    invoke 141
    expect 25
    addi sp, sp, -32
    # Clear the output mask before reading it back.
    sd zero, 8(sp)
    li a0, 0
    li a1, 8
    addi a2, sp, 8
    invoke 123
    expect 8
    ld t0, 8(sp)
    li t1, 1
    bne t0, t1, scheduler_failure
    li a0, 42
    invoke 94
    j scheduler_failure

scheduler_success:
    li a0, 42
    invoke 94
    j scheduler_success
scheduler_setup_failure:
    li a0, 199
    invoke 94
    j scheduler_setup_failure
scheduler_failure:
    addi a0, s8, 100
    invoke 94
    j scheduler_failure

.section .rodata
executable: .asciz "/bin/linux-test"
inherited_mode: .asciz "Wsched-inherit"
environment: .asciz "ARONA_TEST=yes"
.section .note.GNU-stack,"",@progbits
