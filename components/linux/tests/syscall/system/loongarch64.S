# SPDX-License-Identifier: MPL-2.0

.section .text
.macro invoke number
    li.d $a7, \number
    syscall 0
.endm
.macro expect value
    li.d $s8, \@
    li.d $t0, \value
    bne $a0, $t0, system_failure
.endm
.global system_basics
system_basics:
    addi.d $s7, $sp, 48
    addi.d $sp, $sp, -512
.Laux_random:
    ld.d $t0, $s7, 0
    beqz $t0, system_failure
    li.d $t1, 25
    beq $t0, $t1, .Lrandom_found
    addi.d $s7, $s7, 16
    b .Laux_random
.Lrandom_found:
    ld.d $t0, $s7, 8
    ld.d $t1, $t0, 0
    ld.d $t2, $t0, 8
    or $t1, $t1, $t2
    beqz $t1, system_failure
    li.d $a0, 1
    move $a1, $sp
    invoke 113
    expect 0
    ld.d $s0, $sp, 0
    ld.d $s1, $sp, 8
    li.d $t0, 1000000000
    bgeu $s1, $t0, system_failure
    st.d $zero, $sp, 16
    li.d $t0, 2000000
    st.d $t0, $sp, 24
    addi.d $a0, $sp, 16
    li.d $a1, 0
    invoke 101
    expect 0
    li.d $a0, 1
    move $a1, $sp
    invoke 113
    expect 0
    ld.d $t0, $sp, 0
    bltu $t0, $s0, system_failure
    bne $t0, $s0, .Lelapsed
    ld.d $t0, $sp, 8
    sub.d $t0, $t0, $s1
    li.d $t1, 2000000
    bltu $t0, $t1, system_failure
.Lelapsed:
    li.d $a0, 1
    addi.d $a1, $sp, 16
    invoke 114
    expect 0
    ld.d $t0, $sp, 16
    bnez $t0, system_failure
    ld.d $t0, $sp, 24
    beqz $t0, system_failure
    li.d $a0, -1
    move $a1, $sp
    invoke 113
    expect -22
    li.d $a0, 1
    li.d $a1, 1
    invoke 113
    expect -14
    li.d $a0, 0
    move $a1, $sp
    invoke 113
    expect 0
    ld.d $t0, $sp, 0
    li.d $t1, 946684800
    bltu $t0, $t1, system_failure
    move $a0, $sp
    addi.d $a1, $sp, 32
    invoke 169
    expect 0
    ld.d $t0, $sp, 8
    li.d $t1, 1000000
    bgeu $t0, $t1, system_failure
    ld.d $t0, $sp, 32
    bnez $t0, system_failure
    li.d $a0, 0
    li.d $a1, 0
    invoke 169
    expect 0
    st.d $zero, $sp, 16
    st.d $zero, $sp, 24
    li.d $a0, 1
    li.d $a1, 1
    addi.d $a2, $sp, 16
    li.d $a3, 0
    invoke 115
    expect 0
    li.d $a0, 1
    li.d $a1, 2
    addi.d $a2, $sp, 16
    invoke 115
    expect -22
    move $a0, $sp
    invoke 160
    expect 0
    ld.w $t0, $sp, 0
    li.d $t1, 0x756e694c
    bne $t0, $t1, system_failure
    li.d $a0, 1
    invoke 160
    expect -14
    move $a0, $sp
    li.d $a1, 32
    li.d $a2, 0
    invoke 278
    blez $a0, system_failure
    li.d $t0, 32
    bltu $t0, $a0, system_failure
    move $t0, $sp
    add.d $t1, $sp, $a0
    li.d $t2, 0
.Lrandom_bytes:
    ld.bu $t3, $t0, 0
    or $t2, $t2, $t3
    addi.d $t0, $t0, 1
    bltu $t0, $t1, .Lrandom_bytes
    beqz $t2, system_failure
    li.d $a0, 1
    li.d $a1, 0
    invoke 278
    expect 0
    li.d $a0, 1
    li.d $a1, 1
    invoke 278
    expect -14
    move $a0, $sp
    li.d $a1, 1
    li.d $a2, 8
    invoke 278
    expect -22
    li.d $a0, 42
    invoke 94
system_failure:
    addi.d $a0, $s8, 100
    invoke 94
    b system_failure
.section .note.GNU-stack,"",@progbits
