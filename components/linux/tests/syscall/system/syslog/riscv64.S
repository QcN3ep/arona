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
    bne a0, t0, syslog_failure
.endm

.global system_syslog
system_syslog:
    li t0, 8192
    sub sp, sp, t0

    # SYSLOG_ACTION_SIZE_BUFFER reports the kernel ring capacity.
    li a0, 10
    li a1, 0
    li a2, 0
    invoke 116
    expect 65536

    # SYSLOG_ACTION_SIZE_UNREAD reports the formatted backlog.
    li a0, 9
    li a1, 0
    li a2, 0
    invoke 116
    blez a0, syslog_failure
    mv s0, a0

    # SYSLOG_ACTION_READ_ALL returns the newest whole lines.
    li a0, 3
    mv a1, sp
    li a2, 8192
    invoke 116
    blez a0, syslog_failure
    mv s1, a0
    add t0, sp, s1
    lbu t0, -1(t0)
    li t1, 10
    bne t0, t1, syslog_failure
    # A full buffer returns the newest bytes, which may start mid-record.
    bne s1, s0, scan_started
    lbu t0, 0(sp)
    li t1, 60
    bne t0, t1, syslog_failure
scan_started:

    # The log carries the kernel record emitted by the test harness.
    mv t0, sp
    add t1, sp, s1
    la t3, needle
scan:
    beq t0, t1, syslog_failure
    lbu t2, 0(t0)
    lbu t4, 0(t3)
    bne t2, t4, scan_next
    mv t4, t0
    la t5, needle
compare:
    lbu t6, 0(t5)
    beqz t6, scan_found
    beq t4, t1, syslog_failure
    lbu t2, 0(t4)
    bne t2, t6, scan_next
    addi t4, t4, 1
    addi t5, t5, 1
    j compare
scan_next:
    addi t0, t0, 1
    j scan
scan_found:

    # READ_ALL does not move the read cursor.
    li a0, 9
    invoke 116
    bne a0, s0, syslog_failure

    # A failed destination copy leaves the consuming cursor unchanged.
    li a0, 2
    li a1, 1
    li a2, 32
    invoke 116
    expect -14
    li a0, 9
    invoke 116
    bne a0, s0, syslog_failure

    # SYSLOG_ACTION_READ consumes unread bytes from the cursor.
    li a0, 2
    mv a1, sp
    li a2, 32
    invoke 116
    expect 32
    lbu t0, 0(sp)
    li t1, 60
    bne t0, t1, syslog_failure
    li a0, 9
    invoke 116
    addi t0, s0, -32
    bne a0, t0, syslog_failure

    # READ_CLEAR hides the snapshot while preserving the consuming reader.
    li a0, 4
    mv a1, sp
    li a2, 8192
    invoke 116
    bne a0, s1, syslog_failure
    li a0, 9
    invoke 116
    addi t0, s0, -32
    bne a0, t0, syslog_failure
    li a0, 2
    mv a1, sp
    li a2, 32
    invoke 116
    expect 32

    # CLEAR discards records for syslog readers.
    li a0, 5
    invoke 116
    expect 0
    li a0, 3
    mv a1, sp
    li a2, 8192
    invoke 116
    expect 0

    # Invalid actions, lengths, and buffers report their errno.
    li a0, 2
    mv a1, sp
    li a2, -1
    invoke 116
    expect -22
    li a0, 2
    li a1, 0
    li a2, 8
    invoke 116
    expect -14
    li a0, 11
    invoke 116
    expect -22
    li a0, -1
    invoke 116
    expect -22
    li a0, 8
    li a2, 8
    invoke 116
    expect -22

    # OPEN and CLOSE need no privilege.
    li a0, 1
    invoke 116
    expect 0
    li a0, 0
    invoke 116
    expect 0

    # Drop privilege for the final checks: OPEN and CLOSE stay available while
    # every control and read action is denied without CAP_SYSLOG.
    li a0, 146
    li a1, 1000
    invoke 146
    expect 0
    li a0, 9
    invoke 116
    expect -1
    li a0, 10
    invoke 116
    expect -1
    li a0, 2
    mv a1, sp
    li a2, 16
    invoke 116
    expect -1
    li a0, 3
    mv a1, sp
    li a2, 16
    invoke 116
    expect -1
    li a0, 4
    mv a1, sp
    li a2, 16
    invoke 116
    expect -1
    li a0, 5
    invoke 116
    expect -1
    li a0, 1
    invoke 116
    expect 0
    li a0, 0
    invoke 116
    expect 0

    li a0, 42
    invoke 94
    j syslog_failure

syslog_failure:
    addi a0, s8, 100
    invoke 94
    j syslog_failure

.section .rodata
needle: .asciz "kernel"
.section .note.GNU-stack,"",@progbits
