# SPDX-License-Identifier: MPL-2.0

.section .text
.macro invoke number
    li.d $a7, \number
    syscall 0
.endm
.macro expect value
    li.d $s8, \@
    li.d $t0, \value
    bne $a0, $t0, pipe_failure
.endm
.global files_pipe
files_pipe:
    addi.d $sp, $sp, -512
    li.d $t0, 1
    st.d $t0, $sp, 128
    st.d $zero, $sp, 136
    st.d $zero, $sp, 144
    li.d $a0, 13
    addi.d $a1, $sp, 128
    li.d $a2, 0
    li.d $a3, 8
    invoke 134
    expect 0
    move $a0, $sp
    li.d $a1, 0x80800
    invoke 59
    expect 0
    ld.w $s0, $sp, 0
    ld.w $s1, $sp, 4
    move $a0, $s0
    li.d $a1, 1
    invoke 25
    expect 1
    move $a0, $s0
    li.d $a1, 3
    invoke 25
    expect 2048
    move $a0, $s1
    addi.d $a1, $sp, 128
    invoke 80
    expect 0
    ld.w $t0, $sp, 144
    li.d $t1, 4480
    bne $t0, $t1, pipe_failure
    move $a0, $s0
    move $a1, $sp
    li.d $a2, 1
    invoke 63
    expect -11
    move $a0, $s1
    la.pcrel $a1, payload
    li.d $a2, 3
    invoke 64
    expect 3
    move $a0, $s0
    li.d $a1, 1
    li.d $a2, 3
    invoke 63
    expect -14
    move $a0, $s0
    addi.d $a1, $sp, 16
    li.d $a2, 3
    invoke 63
    expect 3
    ld.bu $t0, $sp, 16
    li.d $t1, 97
    bne $t0, $t1, pipe_failure
    move $a0, $s0
    move $a1, $sp
    li.d $a2, 0
    li.d $a3, 0
    invoke 67
    expect -29
    move $a0, $s1
    invoke 82
    expect -22
    li.d $s4, 16
fill_pipe:
    move $a0, $s1
    la.pcrel $a1, pipe_page
    li.d $a2, 4096
    invoke 64
    expect 4096
    addi.d $s4, $s4, -1
    bnez $s4, fill_pipe
    move $a0, $s1
    la.pcrel $a1, payload
    li.d $a2, 3
    invoke 64
    expect -11
    move $a0, $s0
    addi.d $a1, $sp, 16
    li.d $a2, 1
    invoke 63
    expect 1
    move $a0, $s1
    la.pcrel $a1, payload
    li.d $a2, 3
    invoke 64
    expect -11
    move $a0, $s0
    la.pcrel $a1, pipe_page
    li.d $a2, 4095
    invoke 63
    expect 4095
    move $a0, $s1
    la.pcrel $a1, pipe_page
    li.d $a2, 4096
    invoke 64
    expect 4096
    li.d $s4, 16
drain_pipe:
    move $a0, $s0
    la.pcrel $a1, pipe_page
    li.d $a2, 4096
    invoke 63
    expect 4096
    addi.d $s4, $s4, -1
    bnez $s4, drain_pipe
    move $a0, $s1
    invoke 23
    bltz $a0, pipe_failure
    move $s2, $a0
    move $a0, $s1
    invoke 57
    expect 0
    move $a0, $s0
    addi.d $a1, $sp, 16
    li.d $a2, 1
    invoke 63
    expect -11
    move $a0, $s2
    invoke 57
    expect 0
    move $a0, $s0
    addi.d $a1, $sp, 16
    li.d $a2, 1
    invoke 63
    expect 0
    move $a0, $s0
    invoke 57
    expect 0
    li.d $a0, 1
    li.d $a1, 0
    invoke 59
    expect -14
    move $a0, $sp
    li.d $a1, 0
    invoke 59
    expect 0
    ld.w $s0, $sp, 0
    ld.w $s1, $sp, 4
    move $a0, $s0
    invoke 57
    expect 0
    move $a0, $s1
    la.pcrel $a1, payload
    li.d $a2, 3
    invoke 64
    expect -32
    li.d $a0, 42
    invoke 94
pipe_failure:
    addi.d $a0, $s8, 100
    invoke 94
    b pipe_failure
.section .rodata
payload: .ascii "abc"
.section .bss
.balign 4096
pipe_page: .skip 4096
.section .note.GNU-stack,"",@progbits
