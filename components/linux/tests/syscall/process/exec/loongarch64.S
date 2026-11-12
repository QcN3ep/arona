# SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

.section .text
.macro invoke number
    li.d $a7, \number
    syscall 0
.endm
.macro expect value
    li.d $s8, \@
    li.d $t0, \value
    bne $a0, $t0, exec_failure
.endm
.global exec_process
exec_process:
    addi.d $sp, $sp, -128
    li.d $a0, 0
    li.d $a1, 0
    li.d $a2, 0
    invoke 221
    expect -14
    la.pcrel $a0, noexec
    li.d $a1, 0
    li.d $a2, 0
    invoke 221
    expect -13
    la.pcrel $a0, malformed_exec
    li.d $a1, 0
    li.d $a2, 0
    invoke 221
    expect -8
    la.pcrel $a0, empty_script
    li.d $a1, 0
    li.d $a2, 0
    invoke 221
    expect -8
    la.pcrel $a0, loop_script
    li.d $a1, 0
    li.d $a2, 0
    invoke 221
    expect -40
    li.d $a0, -100
    la.pcrel $a1, exec_data
    li.d $a2, 0x80000
    invoke 56
    expect 0
    invoke 172
    st.d $a0, $sp, 64
    addi.d $a0, $sp, 72
    li.d $a1, 0
    invoke 59
    expect 0
    ld.w $a0, $sp, 76
    addi.d $a1, $sp, 64
    li.d $a2, 8
    invoke 64
    expect 8
    ld.w $a0, $sp, 76
    invoke 57
    expect 0
    la.pcrel $t0, executable
    st.d $t0, $sp, 0
    la.pcrel $t0, replaced
    st.d $t0, $sp, 8
    st.d $zero, $sp, 16
    la.pcrel $t0, environment
    st.d $t0, $sp, 24
    st.d $zero, $sp, 32
    la.pcrel $a0, executable
    move $a1, $sp
    addi.d $a2, $sp, 24
    invoke 221
    b exec_failure
.global exec_replaced
exec_replaced:
    addi.d $sp, $sp, -32
    invoke 172
    move $s0, $a0
    li.d $a0, 3
    move $a1, $sp
    li.d $a2, 8
    invoke 63
    expect 8
    ld.d $t0, $sp, 0
    bne $t0, $s0, exec_failure
    li.d $a0, 3
    invoke 57
    expect 0
    li.d $a0, 0
    li.d $a1, 1
    invoke 25
    expect -9
    li.d $a0, -100
    la.pcrel $a1, exec_data
    li.d $a2, 0
    invoke 56
    expect 0
    li.d $a0, 42
    invoke 94
exec_failure:
    addi.d $a0, $s8, 100
    invoke 94
    b exec_failure
.global exec_pie
exec_pie:
    la.pcrel $a0, pie
    addi.d $a1, $sp, 8
    li.d $a2, 0
    invoke 221
    li.d $a0, 100
    invoke 94
.global exec_interp
exec_interp:
    la.pcrel $a0, dynamic
    addi.d $a1, $sp, 8
    li.d $a2, 0
    invoke 221
    li.d $a0, 100
    invoke 94

.global script_exec
script_exec:
    addi.d $sp, $sp, -32
    la.pcrel $t0, script_path
    st.d $t0, $sp, 0
    la.pcrel $t0, script_tail
    st.d $t0, $sp, 8
    st.d $zero, $sp, 16
    la.pcrel $a0, script_path
    move $a1, $sp
    li.d $a2, 0
    invoke 221
    b exec_failure
.section .rodata
script_path: .asciz "/bin/linux-script"
script_tail: .asciz "tail"
empty_script: .asciz "/bin/linux-empty-script"
loop_script: .asciz "/bin/linux-script-loop"

executable: .asciz "/bin/linux-test"
replaced: .asciz "Z"
environment: .asciz "ARONA_TEST=yes"
noexec: .asciz "/bin/linux-noexec"
malformed_exec: .asciz "/bin/linux-truncated"
exec_data: .asciz "/etc/linux-data"
pie: .asciz "/bin/linux-pie"
dynamic: .asciz "/bin/linux-dynamic"
.section .note.GNU-stack,"",@progbits
