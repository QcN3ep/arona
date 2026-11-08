# SPDX-License-Identifier: MPL-2.0

.section .text

.macro memcall number
    li.d $a7, \number
    syscall 0
.endm

.macro result value
    li.d $t0, \value
    bne $a0, $t0, memory_failure
.endm

.global mappings
mappings:
    addi.d $sp, $sp, -16
    st.d $ra, $sp, 0
    li.d $a0, 0
    li.d $a1, 12287
    li.d $a2, 3
    li.d $a3, 0x22
    li.d $a4, -1
    li.d $a5, 0
    memcall 222
    blez $a0, memory_failure
    move $s0, $a0
    andi $t0, $s0, 4095
    bnez $t0, memory_failure
    li.d $t0, 4096
    add.d $s1, $s0, $t0
    add.d $s2, $s1, $t0
    move $t0, $s0
    li.d $t1, 12288
    add.d $t1, $s0, $t1
.Lzero:
    ld.bu $t2, $t0, 0
    bnez $t2, memory_failure
    addi.d $t0, $t0, 1
    bne $t0, $t1, .Lzero
    li.d $t0, 91
    st.b $t0, $s0, 0
    st.b $t0, $s1, 0
    st.b $t0, $s2, 0

    addi.d $a0, $s0, 1
    li.d $a1, 4096
    memcall 222
    blez $a0, memory_failure
    beq $a0, $s0, memory_failure
    li.d $a1, 4096
    memcall 215
    result 0
    move $a0, $s0
    li.d $a1, 4096
    li.d $a2, 3
    li.d $a3, 0x100022
    memcall 222
    result -17
    move $a0, $s0
    li.d $a1, 4096
    li.d $a3, 0x32
    memcall 222
    result -95
    move $a0, $s0
    li.d $a1, -1
    li.d $a3, 0x22
    memcall 222
    result -12

    move $a0, $s0
    li.d $a1, 12288
    li.d $a2, 0
    memcall 226
    result 0
    li.d $a0, 1
    move $a1, $s0
    li.d $a2, 1
    memcall 64
    result -14
    move $a0, $s0
    li.d $a1, 12287
    li.d $a2, 3
    memcall 226
    result 0
    ld.bu $t0, $s0, 0
    li.d $t1, 91
    bne $t0, $t1, memory_failure
    ld.bu $t0, $s1, 0
    bne $t0, $t1, memory_failure
    ld.bu $t0, $s2, 0
    bne $t0, $t1, memory_failure
    move $a0, $s0
    li.d $a1, 4096
    li.d $a2, 7
    memcall 226
    result -13
    st.b $zero, $s0, 0

    move $a0, $s1
    li.d $a1, 1
    memcall 215
    result 0
    li.d $a0, 1
    move $a1, $s1
    li.d $a2, 1
    memcall 64
    result -14
    move $a0, $s0
    li.d $a1, 12288
    li.d $a2, 1
    memcall 226
    result -12
    st.b $zero, $s0, 0
    st.b $zero, $s2, 0
    move $a0, $s0
    li.d $a1, 12288
    memcall 215
    result 0
    move $a0, $s0
    memcall 215
    result 0

    move $a0, $s0
    li.d $a1, 12288
    li.d $a2, 0
    li.d $a3, 0x100022
    li.d $a4, -1
    li.d $a5, 0
    memcall 222
    bne $a0, $s0, memory_failure
    move $a0, $s0
    li.d $a1, 12288
    li.d $a2, 3
    memcall 226
    result 0
    move $t0, $s0
    li.d $t1, 12288
    add.d $t1, $s0, $t1
.Lreused_zero:
    ld.bu $t2, $t0, 0
    bnez $t2, memory_failure
    addi.d $t0, $t0, 1
    bne $t0, $t1, .Lreused_zero
    la.local $t0, .Lcode
    ld.d $t0, $t0, 0
    st.d $t0, $s0, 0
    move $a0, $s0
    li.d $a1, 12288
    li.d $a2, 5
    memcall 226
    result 0
    jirl $ra, $s0, 0
    result 77
    move $a0, $s0
    li.d $a1, 12288
    memcall 215
    result 0

    li.d $a0, 0x1000000000
    li.d $a1, 4096
    li.d $a2, 3
    li.d $a3, 0x100022
    li.d $a4, -1
    li.d $a5, 0
    memcall 222
    result 0x1000000000
    li.d $a0, 0x2000000000
    memcall 222
    result 0x2000000000
    li.d $a0, 0x1000000000
    li.d $a1, 0x1000001000
    memcall 215
    result 0
    li.d $a0, 0x2000000000
    li.d $a1, 4096
    memcall 222
    result 0x2000000000
    li.d $a0, 0x2000000000
    memcall 215
    result 0
    li.d $a0, 0x1000000000
    memcall 222
    result 0x1000000000
    li.d $a0, 0x1000000000
    memcall 215
    result 0

    li.d $a0, 0
    li.d $a1, 0
    li.d $a2, 3
    li.d $a3, 0x22
    memcall 222
    result -22
    li.d $a0, 0
    li.d $a1, 4096
    li.d $a5, 1
    memcall 222
    result -22
    li.d $a0, 0
    li.d $a5, 0
    li.d $a3, 2
    memcall 222
    result -95
    li.d $a0, 1
    li.d $a1, 4096
    memcall 215
    result -22
    li.d $a0, 1
    li.d $a2, 1
    memcall 226
    result -22
    move $a0, $s0
    li.d $a1, 0
    memcall 226
    result 0
    ld.d $ra, $sp, 0
    addi.d $sp, $sp, 16
    jr $ra

.global memory_fault
memory_fault:
    li.d $a0, 0
    li.d $a1, 4096
    li.d $a2, 3
    li.d $a3, 0x22
    li.d $a4, -1
    li.d $a5, 0
    memcall 222
    blez $a0, memory_failure
    move $s0, $a0
    la.local $t0, .Lcode
    ld.d $t0, $t0, 0
    st.d $t0, $s0, 0
    li.d $t0, 51
    beq $s3, $t0, .Lexecute
    li.d $t0, 49
    beq $s3, $t0, .Lunmap
    li.d $a2, 0
    li.d $t0, 50
    bne $s3, $t0, .Lprotect
    li.d $a2, 1
.Lprotect:
    move $a0, $s0
    memcall 226
    result 0
    li.d $t0, 53
    beq $s3, $t0, .Lexecute
    li.d $t0, 48
    beq $s3, $t0, .Lread
    st.b $zero, $s0, 0
    b memory_failure
.Lunmap:
    move $a0, $s0
    memcall 215
    result 0
.Lread:
    ld.bu $t0, $s0, 0
    b memory_failure
.Lexecute:
    la.local $ra, memory_failure
    jr $s0
memory_failure:
    li.d $a0, 2
    memcall 94
    b memory_failure

.balign 8
.Lcode:
    addi.d $a0, $zero, 77
    jr $ra

.section .note.GNU-stack,"",@progbits
