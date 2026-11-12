# SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

.section .text
.macro invoke number
    li a7, \number
    ecall
.endm
.macro expect value
    li s8, \@
    li t0, \value
    bne a0, t0, exec_failure
.endm
.global exec_process
exec_process:
    addi sp, sp, -128
    li a0, 0
    li a1, 0
    li a2, 0
    invoke 221
    expect -14
    la a0, noexec
    li a1, 0
    li a2, 0
    invoke 221
    expect -13
    la a0, malformed_exec
    li a1, 0
    li a2, 0
    invoke 221
    expect -8
    li a0, -100
    la a1, exec_data
    li a2, 0x80000
    invoke 56
    expect 0
    invoke 172
    sd a0, 64(sp)
    addi a0, sp, 72
    li a1, 0
    invoke 59
    expect 0
    lw a0, 76(sp)
    addi a1, sp, 64
    li a2, 8
    invoke 64
    expect 8
    lw a0, 76(sp)
    invoke 57
    expect 0
    la t0, executable
    sd t0, 0(sp)
    la t0, replaced
    sd t0, 8(sp)
    sd zero, 16(sp)
    la t0, environment
    sd t0, 24(sp)
    sd zero, 32(sp)
    la a0, executable
    mv a1, sp
    addi a2, sp, 24
    invoke 221
    j exec_failure
.global exec_replaced
exec_replaced:
    addi sp, sp, -32
    invoke 172
    mv s0, a0
    li a0, 3
    mv a1, sp
    li a2, 8
    invoke 63
    expect 8
    ld t0, 0(sp)
    bne t0, s0, exec_failure
    li a0, 3
    invoke 57
    expect 0
    li a0, 0
    li a1, 1
    invoke 25
    expect -9
    li a0, -100
    la a1, exec_data
    li a2, 0
    invoke 56
    expect 0
    li a0, 42
    invoke 94
exec_failure:
    addi a0, s8, 100
    invoke 94
    j exec_failure
.global exec_pie
exec_pie:
    la a0, pie
    addi a1, sp, 8
    li a2, 0
    invoke 221
    li a0, 100
    invoke 94
.global exec_interp
exec_interp:
    la a0, dynamic
    addi a1, sp, 8
    li a2, 0
    invoke 221
    li a0, 100
    invoke 94
.section .rodata
executable: .asciz "/bin/linux-test"
replaced: .asciz "Z"
environment: .asciz "ARONA_TEST=yes"
noexec: .asciz "/bin/linux-noexec"
malformed_exec: .asciz "/bin/linux-truncated"
exec_data: .asciz "/etc/linux-data"
pie: .asciz "/bin/linux-pie"
dynamic: .asciz "/bin/linux-dynamic"
.section .note.GNU-stack,"",@progbits
