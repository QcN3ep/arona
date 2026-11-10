# SPDX-License-Identifier: MPL-2.0

.section .text
.macro invoke number
    li.d $a7, \number
    syscall 0
.endm
.macro expect value
    li.d $s8, \@
    li.d $t0, \value
    bne $a0, $t0, control_failure
.endm
.macro control command, offset
    li.d $a0, 1
    li.d $a1, \command
    addi.d $a2, $sp, \offset
    invoke 29
.endm
.macro spawn target
    li.d $a0, 17
    li.d $a1, 0
    li.d $a2, 0
    li.d $a3, 0
    li.d $a4, 0
    invoke 220
    beqz $a0, \target
    bltz $a0, control_failure
    move $s1, $a0
.endm
.macro reap options, status
    move $a0, $s1
    addi.d $a1, $sp, 64
    li.d $a2, \options
    li.d $a3, 0
    invoke 260
    bne $a0, $s1, control_failure
    ld.w $t0, $sp, 64
    li.d $t1, \status
    bne $t0, $t1, control_failure
.endm
.macro resume
    move $a0, $s1
    li.d $a1, 18
    invoke 129
    expect 0
.endm
.global process_terminal
process_terminal:
    addi.d $sp, $sp, -256
    invoke 172
    move $s0, $a0
    control 0x540f, 0
    expect -25
    control 0x540e, 0
    expect 0
    control 0x5429, 0
    expect 0
    ld.w $t0, $sp, 0
    bne $t0, $s0, control_failure
    control 0x540f, 0
    expect 0
    ld.w $t0, $sp, 0
    bne $t0, $s0, control_failure
    li.d $a0, -100
    la.pcrel $a1, tty_path
    li.d $a2, 2
    li.d $a3, 0
    invoke 56
    bltz $a0, control_failure
    move $s2, $a0
    spawn control_child
    reap 2, 0x167f
    st.w $s1, $sp, 0
    control 0x5410, 0
    expect 0
    resume
    reap 0, 0x1800
    li.d $a0, 22
    la.pcrel $a1, control_ignore
    li.d $a2, 0
    li.d $a3, 8
    invoke 134
    expect 0
    st.w $s0, $sp, 0
    control 0x5410, 0
    expect 0
    control 0x5401, 80
    expect 0
    ld.w $t0, $sp, 92
    ori $t0, $t0, 0x100
    st.w $t0, $sp, 92
    control 0x5402, 80
    expect 0
    spawn background_write
    reap 2, 0x167f
    st.w $s1, $sp, 0
    control 0x5410, 0
    expect 0
    resume
    reap 0, 0x1900
    st.w $s0, $sp, 0
    control 0x5410, 0
    expect 0
    spawn background_read
    reap 2, 0x157f
    move $a0, $s1
    li.d $a1, 9
    invoke 129
    expect 0
    reap 0, 9
    li.d $a0, 1
    la.pcrel $a1, control_ignore
    li.d $a2, 0
    li.d $a3, 8
    invoke 134
    expect 0
    control 0x5422, 0
    expect 0
    control 0x540f, 0
    expect -25
    li.d $a0, -100
    la.pcrel $a1, tty_path
    li.d $a2, 2
    li.d $a3, 0
    invoke 56
    expect -6
    control 0x540e, 0
    expect 0
    li.d $a0, 42
    invoke 94
control_child:
    li.d $a0, 0
    li.d $a1, 0
    invoke 154
    expect 0
    invoke 172
    st.w $a0, $sp, 0
    control 0x5410, 0
    expect 0
    control 0x5422, 0
    expect 0
    control 0x5429, 0
    expect -25
    li.d $a0, 24
    invoke 94
background_write:
    li.d $a0, 22
    la.pcrel $a1, control_default
    li.d $a2, 0
    li.d $a3, 8
    invoke 134
    expect 0
    li.d $a0, 0
    li.d $a1, 0
    invoke 154
    expect 0
    li.d $a0, 1
    la.pcrel $a1, control_text
    li.d $a2, 1
    invoke 64
    expect 1
    li.d $a0, 25
    invoke 94
background_read:
    li.d $a0, 0
    li.d $a1, 0
    invoke 154
    expect 0
    move $a0, $s2
    addi.d $a1, $sp, 128
    li.d $a2, 1
    invoke 63
    b control_failure
control_failure:
    addi.d $a0, $s8, 100
    invoke 94
    b control_failure
.section .rodata
.balign 8
control_ignore: .quad 1, 0, 0
control_default: .quad 0, 0, 0
tty_path: .asciz "/dev/tty"
control_text: .ascii "!"
.section .note.GNU-stack,"",@progbits
