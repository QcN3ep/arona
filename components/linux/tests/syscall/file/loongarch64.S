# SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

.section .text

.macro invoke number
    li.d $a7, \number
    syscall 0
.endm

.macro expect value
    li.d $t0, \value
    bne $a0, $t0, file_failure
.endm

.macro open_data flags=0
    li.d $a0, -100
    la.local $a1, data_path
    li.d $a2, \flags
    invoke 56
.endm

.global files
files:
    addi.d $sp, $sp, -16
    st.d $ra, $sp, 0
    li.d $t0, -8192
    add.d $sp, $sp, $t0
    open_data 0x80000
    expect 0
    move $s0, $a0
    li.d $a1, 1
    invoke 25
    expect 1
    move $a0, $s0
    li.d $a1, 3
    invoke 25
    expect 0

    li.d $s3, 0
.Lread_all:
    move $a0, $s0
    move $a1, $sp
    li.d $a2, 8192
    invoke 63
    bltz $a0, file_failure
    beqz $a0, .Lread_done
    move $t0, $sp
    add.d $t1, $sp, $a0
.Lcheck_data:
    ld.bu $t2, $t0, 0
    li.d $t3, 8192
    beq $s3, $t3, .Llast_byte
    andi $t3, $s3, 15
    la.local $t4, digits
    add.d $t3, $t4, $t3
    ld.bu $t3, $t3, 0
    b .Lcompare
.Llast_byte:
    li.d $t3, 90
.Lcompare:
    bne $t2, $t3, file_failure
    addi.d $t0, $t0, 1
    addi.d $s3, $s3, 1
    bne $t0, $t1, .Lcheck_data
    b .Lread_all
.Lread_done:
    li.d $t0, 8193
    bne $s3, $t0, file_failure
    move $a0, $s0
    li.d $a1, 0
    li.d $a2, 0
    invoke 62
    expect 0

    move $a0, $s0
    move $a1, $sp
    li.d $a2, 4
    invoke 63
    expect 4
    ld.w $t0, $sp, 0
    li.d $t1, 0x33323130
    bne $t0, $t1, file_failure
    move $a0, $s0
    invoke 23
    expect 3
    move $s1, $a0
    li.d $a1, 1
    invoke 25
    expect 0
    move $a0, $s1
    li.d $a1, 2
    li.d $a2, 1
    invoke 25
    expect 0
    move $a0, $s1
    li.d $a1, 1
    invoke 25
    expect 1
    move $a0, $s0
    li.d $a1, 2
    li.d $a2, 0
    invoke 25
    expect 0
    move $a0, $s1
    li.d $a1, 1
    invoke 25
    expect 1

    move $a0, $s1
    move $a1, $sp
    li.d $a2, 4
    invoke 63
    expect 4
    ld.w $t0, $sp, 0
    li.d $t1, 0x37363534
    bne $t0, $t1, file_failure
    move $a0, $s0
    move $a1, $sp
    li.d $a2, 4
    li.d $a3, 8
    invoke 67
    expect 4
    ld.w $t0, $sp, 0
    li.d $t1, 0x62613938
    bne $t0, $t1, file_failure
    move $a0, $s1
    li.d $a1, 0
    li.d $a2, 1
    invoke 62
    expect 8
    move $a0, $s0
    invoke 57
    expect 0
    move $a0, $s0
    invoke 57
    expect -9
    open_data
    expect 0
    move $s2, $a0
    move $a1, $sp
    li.d $a2, 2
    invoke 63
    expect 2
    ld.hu $t0, $sp, 0
    li.d $t1, 0x3130
    bne $t0, $t1, file_failure
    move $a0, $s1
    move $a1, $sp
    li.d $a2, 2
    invoke 63
    expect 2
    ld.hu $t0, $sp, 0
    li.d $t1, 0x3938
    bne $t0, $t1, file_failure

    move $a0, $s1
    li.d $a1, -1
    li.d $a2, 2
    invoke 62
    expect 8192
    move $a0, $s1
    move $a1, $sp
    li.d $a2, 8
    invoke 63
    expect 1
    ld.bu $t0, $sp, 0
    li.d $t1, 90
    bne $t0, $t1, file_failure
    move $a0, $s1
    li.d $a1, 0
    li.d $a2, 8
    invoke 63
    expect 0
    move $a0, $s1
    li.d $a1, 10
    li.d $a2, 2
    invoke 62
    expect 8203
    move $a0, $s1
    li.d $a1, -1
    li.d $a2, 0
    invoke 62
    expect -22
    move $a0, $s1
    li.d $a1, 0
    li.d $a2, 1
    invoke 62
    expect 8203
    move $a0, $s1
    li.d $a1, 0x7fffffffffffffff
    li.d $a2, 0
    invoke 62
    expect 0x7fffffffffffffff
    move $a0, $s1
    li.d $a1, 1
    li.d $a2, 1
    invoke 62
    expect -75
    move $a0, $s1
    li.d $a1, 0
    li.d $a2, 1
    invoke 62
    expect 0x7fffffffffffffff
    move $a0, $s1
    li.d $a1, 0
    li.d $a2, 0
    invoke 62
    expect 0

    move $a0, $s1
    la.local $a1, data_path
    li.d $a2, 4
    invoke 63
    expect -14
    move $a0, $s1
    li.d $a1, 0
    li.d $a2, 1
    invoke 62
    expect 0
    move $a0, $s1
    li.d $a1, 0
    li.d $a2, 4
    invoke 63
    expect -14
    move $a0, $s1
    li.d $a1, 0
    li.d $a2, 0
    invoke 63
    expect 0
    move $a0, $s1
    move $a1, $sp
    li.d $a2, 4
    li.d $a3, -1
    invoke 67
    expect -22
    move $a0, $s1
    li.d $a1, 0
    li.d $a2, 1
    invoke 62
    expect 0

    li.d $a0, 0
    li.d $a1, 8192
    li.d $a2, 3
    li.d $a3, 0x22
    li.d $a4, -1
    li.d $a5, 0
    invoke 222
    blez $a0, file_failure
    move $s4, $a0
    li.d $t0, 4096
    add.d $s5, $s4, $t0
    move $a0, $s5
    li.d $a1, 4096
    li.d $a2, 0
    invoke 226
    expect 0
    move $a0, $s1
    addi.d $a1, $s5, -2
    li.d $a2, 8
    invoke 63
    expect 2
    ld.hu $t0, $s5, -2
    li.d $t1, 0x3130
    bne $t0, $t1, file_failure
    move $a0, $s1
    move $a1, $s5
    li.d $a2, 8
    invoke 63
    expect -14
    move $a0, $s1
    li.d $a1, 0
    li.d $a2, 1
    invoke 62
    expect 2
    move $a0, $s4
    li.d $a1, 8192
    invoke 215
    expect 0

    move $a0, $s1
    li.d $a1, 1000000
    li.d $a2, 0x80000
    invoke 24
    expect 1000000
    li.d $a0, 1000000
    li.d $a1, 1
    invoke 25
    expect 1
    li.d $a0, 1000000
    invoke 57
    expect 0
    move $a0, $s1
    move $a1, $s1
    li.d $a2, 0
    invoke 24
    expect -22
    li.d $a0, -1
    move $a1, $s1
    li.d $a2, 0
    invoke 24
    expect -9
    move $a0, $s1
    li.d $a1, 10
    li.d $a2, 2
    invoke 24
    expect -22
    move $a0, $s1
    li.d $a1, 1030
    li.d $a2, 10
    invoke 25
    expect 10
    li.d $a0, 10
    li.d $a1, 1
    invoke 25
    expect 1
    li.d $a0, 10
    invoke 57
    expect 0
    move $a0, $s1
    li.d $a1, 0
    li.d $a2, -1
    invoke 25
    expect -22
    move $a0, $s1
    move $a1, $s2
    li.d $a2, 0
    invoke 24
    bne $a0, $s2, file_failure
    move $a0, $s2
    li.d $a1, 0
    li.d $a2, 1
    invoke 62
    expect 2
    move $a0, $s1
    invoke 57
    expect 0
    move $a0, $s2
    invoke 57
    expect 0

    li.d $a0, 999
    la.local $a1, data_path
    li.d $a2, 0
    invoke 56
    expect 0
    invoke 57
    expect 0
    li.d $a0, -100
    la.local $a1, relative_path
    li.d $a2, 0
    invoke 56
    expect 0
    invoke 57
    expect 0
    li.d $a0, -100
    la.local $a1, root_path
    li.d $a2, 0
    invoke 56
    expect 0
    move $s0, $a0
    move $a1, $sp
    li.d $a2, 1
    invoke 63
    expect -21
    move $a0, $s0
    la.local $a1, relative_path
    li.d $a2, 0
    invoke 56
    expect -2
    move $a0, $s0
    invoke 57
    expect 0
    open_data 0x10000
    expect -20
    li.d $a0, -100
    la.local $a1, trailing_path
    li.d $a2, 0
    invoke 56
    expect -20
    li.d $a0, -1
    la.local $a1, relative_path
    li.d $a2, 0
    invoke 56
    expect -9
    li.d $a0, 1
    la.local $a1, relative_path
    li.d $a2, 0
    invoke 56
    expect -20
    li.d $a0, -100
    li.d $a1, 0
    li.d $a2, 0
    invoke 56
    expect -14
    li.d $a0, -100
    la.local $a1, empty_path
    li.d $a2, 0
    invoke 56
    expect -2
    li.d $a0, -100
    la.local $a1, missing_path
    li.d $a2, 0
    invoke 56
    expect -2
    open_data 1
    expect -30
    open_data 3
    expect -22

    move $t0, $sp
    li.d $t1, 4096
    add.d $t1, $sp, $t1
    li.d $t2, 65
.Llong_path:
    st.b $t2, $t0, 0
    addi.d $t0, $t0, 1
    bne $t0, $t1, .Llong_path
    li.d $a0, -100
    move $a1, $sp
    li.d $a2, 0
    invoke 56
    expect -36
    li.d $a0, 99
    move $a1, $sp
    li.d $a2, 1
    invoke 63
    expect -9
    li.d $a0, 1
    move $a1, $sp
    li.d $a2, 1
    invoke 63
    expect -9
    li.d $a0, 1
    li.d $a1, 0
    li.d $a2, 0
    invoke 62
    expect -29
    li.d $a0, 1
    move $a1, $sp
    li.d $a2, 1
    li.d $a3, 0
    invoke 67
    expect -29

    li.d $a0, 1
    invoke 23
    expect 0
    li.d $a0, 1
    invoke 57
    expect 0
    li.d $a0, 0
    la.local $a1, alias_message
    li.d $a2, 6
    invoke 64
    expect 6
    li.d $a0, 0
    li.d $a1, 1
    li.d $a2, 0
    invoke 24
    expect 1
    li.d $a0, 0
    invoke 57
    expect 0
    open_data
    expect 0
    invoke 23
    expect 3
    invoke 174
    expect 0
    invoke 175
    expect 0
    invoke 176
    expect 0
    invoke 177
    expect 0
    move $a0, $sp
    addi.d $a1, $sp, 4
    addi.d $a2, $sp, 8
    invoke 148
    expect 0
    ld.w $t0, $sp, 0
    bnez $t0, file_failure
    ld.w $t0, $sp, 4
    bnez $t0, file_failure
    ld.w $t0, $sp, 8
    bnez $t0, file_failure
    li.d $a0, -1
    li.d $a1, 1234
    invoke 145
    expect 0
    invoke 175
    expect 1234
    li.d $a0, 0
    li.d $a1, 0
    invoke 145
    expect 0
    invoke 175
    expect 0
    li.d $a0, 5
    li.d $a1, 0
    li.d $a2, 7
    invoke 147
    expect 0
    move $a0, $sp
    addi.d $a1, $sp, 4
    addi.d $a2, $sp, 8
    invoke 148
    expect 0
    ld.w $t0, $sp, 0
    li.d $t1, 5
    bne $t0, $t1, file_failure
    ld.w $t0, $sp, 4
    bnez $t0, file_failure
    ld.w $t0, $sp, 8
    li.d $t1, 7
    bne $t0, $t1, file_failure
    li.d $a0, 0
    li.d $a1, 0
    li.d $a2, 0
    invoke 147
    expect 0
    li.d $a0, 2000
    invoke 144
    expect 0
    invoke 176
    expect 2000
    li.d $a0, 1000
    invoke 146
    expect 0
    invoke 174
    expect 1000
    invoke 175
    expect 1000
    li.d $a0, 0
    invoke 146
    expect -1
    li.d $a0, 0
    invoke 144
    expect -1
    li.d $t0, 8192
    add.d $sp, $sp, $t0
    ld.d $ra, $sp, 0
    addi.d $sp, $sp, 16
    jr $ra
file_failure:
    li.d $a0, 3
    invoke 94
    b file_failure

.section .rodata
 data_path: .asciz "/etc/linux-data"
 relative_path: .asciz "etc/linux-data"
 trailing_path: .asciz "/etc/linux-data/"
 root_path: .asciz "/bin"
 missing_path: .asciz "/etc/absent"
 empty_path: .asciz ""
 alias_message: .ascii "alias\n"
 digits: .ascii "0123456789abcdef"

.section .note.GNU-stack,"",@progbits
