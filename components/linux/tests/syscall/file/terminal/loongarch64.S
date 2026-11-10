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
    li.d $a0, 0
    li.d $a1, 4
    li.d $a2, 0
    invoke 25
    expect 0
    st.b $zero, $sp, 23
    li.d $a0, 0
    li.d $a1, 0x5402
    move $a2, $sp
    invoke 29
    expect 0
    li.d $a0, 0
    addi.d $a1, $sp, 128
    li.d $a2, 8
    invoke 63
    expect 0
    li.d $t0, 1
    st.b $t0, $sp, 22
    li.d $a0, 0
    li.d $a1, 0x5402
    move $a2, $sp
    invoke 29
    expect 0
    li.d $a0, 0
    addi.d $a1, $sp, 128
    li.d $a2, 8
    invoke 63
    expect 0
    li.d $t0, 4
    st.b $t0, $sp, 23
    st.b $zero, $sp, 22
    li.d $a0, 0
    li.d $a1, 0x5402
    move $a2, $sp
    invoke 29
    expect 0
    li.d $a0, 1
    la.pcrel $a1, minimum_prompt
    li.d $a2, 23
    invoke 64
    expect 23
    li.d $a0, 0
    addi.d $a1, $sp, 128
    li.d $a2, 8
    invoke 63
    expect 4
    ld.w $t0, $sp, 128
    li.d $t1, 0x34333231
    bne $t0, $t1, terminal_failure
    li.d $t0, 8
    st.b $t0, $sp, 23
    li.d $t0, 1
    st.b $t0, $sp, 22
    li.d $a0, 0
    li.d $a1, 0x5402
    move $a2, $sp
    invoke 29
    expect 0
    li.d $a0, 1
    la.pcrel $a1, interbyte_prompt
    li.d $a2, 25
    invoke 64
    expect 25
    li.d $a0, 0
    addi.d $a1, $sp, 128
    li.d $a2, 8
    invoke 63
    expect 2
    ld.hu $t0, $sp, 128
    li.d $t1, 0x7978
    bne $t0, $t1, terminal_failure
    li.d $t0, 3
    st.w $t0, $sp, 12
    li.d $a0, 0
    li.d $a1, 0x5402
    move $a2, $sp
    invoke 29
    expect 0
    li.d $s0, 2
terminal_actions:
    move $a0, $s0
    la.pcrel $a1, terminal_action
    li.d $a2, 0
    li.d $a3, 8
    invoke 134
    expect 0
    li.d $t0, 2
    beq $s0, $t0, terminal_action_quit
    li.d $t0, 3
    bne $s0, $t0, terminal_actions_done
    li.d $s0, 20
    b terminal_actions
terminal_action_quit:
    li.d $s0, 3
    b terminal_actions
terminal_actions_done:
    li.d $a0, 1
    la.pcrel $a1, signal_prompt
    li.d $a2, 22
    invoke 64
    expect 22
    li.d $a0, 0
    addi.d $a1, $sp, 128
    li.d $a2, 8
    invoke 63
    expect 5
terminal_signals_wait:
    la.pcrel $t0, terminal_signals
    ld.d $t0, $t0, 0
    li.d $t1, 0x10000c
    beq $t0, $t1, terminal_signals_done
    invoke 124
    b terminal_signals_wait
terminal_signals_done:
    li.d $a0, 20
    la.pcrel $a1, terminal_default
    li.d $a2, 0
    li.d $a3, 8
    invoke 134
    expect 0
    li.d $a0, 22
    la.pcrel $a1, terminal_ignore
    li.d $a2, 0
    li.d $a3, 8
    invoke 134
    expect 0
    li.d $a0, 17
    li.d $a1, 0
    li.d $a2, 0
    li.d $a3, 0
    li.d $a4, 0
    invoke 220
    beqz $a0, terminal_job
    bltz $a0, terminal_failure
    move $s1, $a0
    move $a0, $s1
    move $a1, $s1
    invoke 154
    expect 0
    st.w $s1, $sp, 64
    li.d $a0, 0
    li.d $a1, 0x5410
    addi.d $a2, $sp, 64
    invoke 29
    expect 0
    move $a0, $s1
    li.d $a1, 18
    invoke 129
    expect 0
    move $a0, $s1
    addi.d $a1, $sp, 80
    li.d $a2, 2
    li.d $a3, 0
    invoke 260
    bne $a0, $s1, terminal_failure
    ld.w $t0, $sp, 80
    li.d $t1, 0x147f
    bne $t0, $t1, terminal_failure
    invoke 172
    st.w $a0, $sp, 64
    li.d $a0, 0
    li.d $a1, 0x5410
    addi.d $a2, $sp, 64
    invoke 29
    expect 0
    move $a0, $s1
    li.d $a1, 18
    invoke 129
    expect 0
    move $a0, $s1
    addi.d $a1, $sp, 80
    li.d $a2, 2
    li.d $a3, 0
    invoke 260
    bne $a0, $s1, terminal_failure
    ld.w $t0, $sp, 80
    li.d $t1, 0x157f
    bne $t0, $t1, terminal_failure
    st.w $s1, $sp, 64
    li.d $a0, 0
    li.d $a1, 0x5410
    addi.d $a2, $sp, 64
    invoke 29
    expect 0
    move $a0, $s1
    li.d $a1, 18
    invoke 129
    expect 0
    li.d $a0, 1
    la.pcrel $a1, resume_prompt
    li.d $a2, 22
    invoke 64
    expect 22
    move $a0, $s1
    addi.d $a1, $sp, 80
    li.d $a2, 0
    li.d $a3, 0
    invoke 260
    bne $a0, $s1, terminal_failure
    ld.w $t0, $sp, 80
    li.d $t1, 0x1a00
    bne $t0, $t1, terminal_failure
    li.d $a0, 42
    invoke 94
terminal_job:
    invoke 172
    move $s0, $a0
terminal_job_foreground:
    li.d $a0, 0
    li.d $a1, 0x540f
    addi.d $a2, $sp, 64
    invoke 29
    expect 0
    ld.w $t0, $sp, 64
    beq $t0, $s0, terminal_job_read
    invoke 124
    b terminal_job_foreground
terminal_job_read:
    li.d $a0, 1
    la.pcrel $a1, suspend_prompt
    li.d $a2, 23
    invoke 64
    expect 23
    li.d $a0, 0
    addi.d $a1, $sp, 128
    li.d $a2, 8
    invoke 63
    expect 3
    li.d $a0, 26
    invoke 94
terminal_signal:
    la.pcrel $t0, terminal_signals
    ld.d $t1, $t0, 0
    li.d $t2, 1
    sll.d $t2, $t2, $a0
    or $t1, $t1, $t2
    st.d $t1, $t0, 0
    ret

terminal_failure:
    addi.d $a0, $s8, 100
    invoke 94
    b terminal_failure
.section .rodata
canonical_prompt: .ascii "Linux terminal canonical\n"
raw_prompt: .ascii "Linux terminal raw\n"

.balign 8
terminal_action: .quad terminal_signal, 0x10000004, 0
terminal_default: .quad 0, 0, 0
terminal_ignore: .quad 1, 0, 0
minimum_prompt: .ascii "Linux terminal minimum\n"
interbyte_prompt: .ascii "Linux terminal interbyte\n"
signal_prompt: .ascii "Linux terminal signal\n"
suspend_prompt: .ascii "Linux terminal suspend\n"
resume_prompt: .ascii "Linux terminal resume\n"
.section .bss
.balign 8
terminal_signals: .skip 8
.section .note.GNU-stack,"",@progbits
