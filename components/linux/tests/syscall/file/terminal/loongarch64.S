# SPDX-License-Identifier: MPL-2.0

.section .text
.macro invoke number
    li.d $a7, \number
    syscall 0
.endm
.macro expect value
    li.d $s8, \@
    li.d $t0, \value
    bne $a0, $t0, terminal_failure
.endm
.global files_terminal
files_terminal:
    addi.d $sp, $sp, -512
    li.d $a0, 0
    li.d $a1, 0x5401
    move $a2, $sp
    invoke 29
    expect 0
    ld.w $t0, $sp, 12
    andi $t0, $t0, 2
    beqz $t0, terminal_failure
    li.d $a0, 0
    li.d $a1, 0x5413
    addi.d $a2, $sp, 64
    invoke 29
    expect 0
    ld.hu $t0, $sp, 64
    li.d $t1, 24
    bne $t0, $t1, terminal_failure
    li.d $t0, 32
    st.h $t0, $sp, 64
    li.d $t0, 100
    st.h $t0, $sp, 66
    li.d $a0, 0
    li.d $a1, 0x5414
    addi.d $a2, $sp, 64
    invoke 29
    expect 0
    li.d $a0, 1
    li.d $a1, 0x5413
    addi.d $a2, $sp, 80
    invoke 29
    expect 0
    ld.w $t0, $sp, 64
    ld.w $t1, $sp, 80
    bne $t0, $t1, terminal_failure
    li.d $a0, 1
    la.pcrel $a1, canonical_prompt
    li.d $a2, 25
    invoke 64
    expect 25
    li.d $a0, 0
    addi.d $a1, $sp, 128
    li.d $a2, 32
    invoke 63
    expect 4
    ld.w $t0, $sp, 128
    li.d $t1, 0x0a636261
    bne $t0, $t1, terminal_failure
    st.d $zero, $sp, 0
    st.d $zero, $sp, 8
    li.d $t0, 1
    st.b $t0, $sp, 23
    li.d $a0, 0
    li.d $a1, 0x5402
    move $a2, $sp
    invoke 29
    expect 0
    li.d $a0, 1
    la.pcrel $a1, raw_prompt
    li.d $a2, 19
    invoke 64
    expect 19
    st.w $zero, $sp, 96
    li.d $t0, 1
    st.h $t0, $sp, 100
    addi.d $a0, $sp, 96
    li.d $a1, 1
    li.d $a2, 0
    li.d $a3, 0
    li.d $a4, 8
    invoke 73
    expect 1
    ld.hu $t0, $sp, 102
    li.d $t1, 1
    bne $t0, $t1, terminal_failure
    li.d $a0, 0
    li.d $a1, 0x541b
    addi.d $a2, $sp, 112
    invoke 29
    expect 0
    ld.w $t0, $sp, 112
    beqz $t0, terminal_failure
    li.d $t1, 8
    bltu $t1, $t0, terminal_failure
    li.d $s0, 0
raw_read:
    li.d $a0, 0
    addi.d $a1, $sp, 128
    add.d $a1, $a1, $s0
    li.d $a2, 8
    sub.d $a2, $a2, $s0
    invoke 63
    blez $a0, terminal_failure
    add.d $s0, $s0, $a0
    li.d $t0, 8
    bltu $s0, $t0, raw_read
    ld.d $t0, $sp, 128
    li.d $t1, 0x617461642d776172
    bne $t0, $t1, terminal_failure
    li.d $a0, 0
    li.d $a1, 4
    li.d $a2, 2048
    invoke 25
    expect 0
    li.d $a0, 0
    addi.d $a1, $sp, 128
    li.d $a2, 1
    invoke 63
    expect -11
    li.d $a0, 0
    li.d $a1, 0x5401
    li.d $a2, 1
    invoke 29
    expect -14
    li.d $a0, 42
    invoke 94
terminal_failure:
    addi.d $a0, $s8, 100
    invoke 94
    b terminal_failure
.section .rodata
canonical_prompt: .ascii "Linux terminal canonical\n"
raw_prompt: .ascii "Linux terminal raw\n"
.section .note.GNU-stack,"",@progbits
