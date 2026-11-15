# SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

.section .text

.macro invoke number
    li.d $a7, \number
    syscall 0
.endm

.macro expect value
    li.d $s8, \@
    li.d $t0, \value
    bne $a0, $t0, syslog_failure
.endm

.global system_syslog
system_syslog:
    li.d $t0, 8192
    sub.d $sp, $sp, $t0

    # SYSLOG_ACTION_SIZE_BUFFER reports the kernel ring capacity.
    li.d $a0, 10
    li.d $a1, 0
    li.d $a2, 0
    invoke 116
    expect 65536

    # SYSLOG_ACTION_SIZE_UNREAD reports the formatted backlog.
    li.d $a0, 9
    li.d $a1, 0
    li.d $a2, 0
    invoke 116
    blez $a0, syslog_failure
    move $s0, $a0

    # SYSLOG_ACTION_READ_ALL returns the newest whole lines.
    li.d $a0, 3
    move $a1, $sp
    li.d $a2, 8192
    invoke 116
    blez $a0, syslog_failure
    move $s1, $a0
    add.d $t0, $sp, $s1
    ld.bu $t0, $t0, -1
    li.d $t1, 10
    bne $t0, $t1, syslog_failure
    # A full buffer returns the newest bytes, which may start mid-record.
    bne $s1, $s0, scan_started
    ld.bu $t0, $sp, 0
    li.d $t1, 60
    bne $t0, $t1, syslog_failure
scan_started:

    # The log carries the kernel record emitted by the test harness.
    move $t0, $sp
    add.d $t1, $sp, $s1
    la.pcrel $t3, needle
scan:
    beq $t0, $t1, syslog_failure
    ld.bu $t2, $t0, 0
    ld.bu $t4, $t3, 0
    bne $t2, $t4, scan_next
    move $t4, $t0
    la.pcrel $t5, needle
compare:
    ld.bu $t6, $t5, 0
    beqz $t6, scan_found
    beq $t4, $t1, syslog_failure
    ld.bu $t2, $t4, 0
    bne $t2, $t6, scan_next
    addi.d $t4, $t4, 1
    addi.d $t5, $t5, 1
    b compare
scan_next:
    addi.d $t0, $t0, 1
    b scan
scan_found:

    # READ_ALL does not move the read cursor.
    li.d $a0, 9
    invoke 116
    bne $a0, $s0, syslog_failure

    # A failed destination copy leaves the consuming cursor unchanged.
    li.d $a0, 2
    li.d $a1, 1
    li.d $a2, 32
    invoke 116
    expect -14
    li.d $a0, 9
    invoke 116
    bne $a0, $s0, syslog_failure

    # SYSLOG_ACTION_READ consumes unread bytes from the cursor.
    li.d $a0, 2
    move $a1, $sp
    li.d $a2, 32
    invoke 116
    expect 32
    ld.bu $t0, $sp, 0
    li.d $t1, 60
    bne $t0, $t1, syslog_failure
    li.d $a0, 9
    invoke 116
    addi.d $t0, $s0, -32
    bne $a0, $t0, syslog_failure

    # READ_CLEAR hides the snapshot while preserving the consuming reader.
    li.d $a0, 4
    move $a1, $sp
    li.d $a2, 8192
    invoke 116
    bne $a0, $s1, syslog_failure
    li.d $a0, 9
    invoke 116
    addi.d $t0, $s0, -32
    bne $a0, $t0, syslog_failure
    li.d $a0, 2
    move $a1, $sp
    li.d $a2, 32
    invoke 116
    expect 32

    # CLEAR discards records for syslog readers.
    li.d $a0, 5
    invoke 116
    expect 0
    li.d $a0, 3
    move $a1, $sp
    li.d $a2, 8192
    invoke 116
    expect 0

    # Invalid actions, lengths, and buffers report their errno.
    li.d $a0, 2
    move $a1, $sp
    li.d $a2, -1
    invoke 116
    expect -22
    li.d $a0, 2
    li.d $a1, 0
    li.d $a2, 8
    invoke 116
    expect -14
    li.d $a0, 11
    invoke 116
    expect -22
    li.d $a0, -1
    invoke 116
    expect -22
    li.d $a0, 8
    li.d $a2, 8
    invoke 116
    expect -22

    # OPEN and CLOSE need no privilege.
    li.d $a0, 1
    invoke 116
    expect 0
    li.d $a0, 0
    invoke 116
    expect 0

    # Drop privilege for the final checks: OPEN and CLOSE stay available while
    # every control and read action is denied without CAP_SYSLOG.
    li.d $a0, 146
    li.d $a1, 1000
    invoke 146
    expect 0
    li.d $a0, 9
    invoke 116
    expect -1
    li.d $a0, 10
    invoke 116
    expect -1
    li.d $a0, 2
    move $a1, $sp
    li.d $a2, 16
    invoke 116
    expect -1
    li.d $a0, 3
    move $a1, $sp
    li.d $a2, 16
    invoke 116
    expect -1
    li.d $a0, 4
    move $a1, $sp
    li.d $a2, 16
    invoke 116
    expect -1
    li.d $a0, 5
    invoke 116
    expect -1
    li.d $a0, 1
    invoke 116
    expect 0
    li.d $a0, 0
    invoke 116
    expect 0

    li.d $a0, 42
    invoke 94
    b syslog_failure

syslog_failure:
    addi.d $a0, $s8, 100
    invoke 94
    b syslog_failure

.section .rodata
needle: .asciz "kernel"
.section .note.GNU-stack,"",@progbits
