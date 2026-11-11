# SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

.section .text
.option norelax

.macro invoke number
    li a7, \number
    ecall
.endm

.macro expect value
    li t0, \value
    bne a0, t0, file_failure
.endm

.macro open_data flags=0
    li a0, -100
    la a1, data_path
    li a2, \flags
    invoke 56
.endm

.global files
files:
    addi sp, sp, -16
    sd ra, 0(sp)
    li t0, -8192
    add sp, sp, t0
    open_data 0x80000
    expect 0
    mv s0, a0
    li a1, 1
    invoke 25
    expect 1
    mv a0, s0
    li a1, 3
    invoke 25
    expect 0

    li s3, 0
.Lread_all:
    mv a0, s0
    mv a1, sp
    li a2, 8192
    invoke 63
    bltz a0, file_failure
    beqz a0, .Lread_done
    mv t0, sp
    add t1, sp, a0
.Lcheck_data:
    lbu t2, 0(t0)
    li t3, 8192
    beq s3, t3, .Llast_byte
    andi t3, s3, 15
    la t4, digits
    add t3, t4, t3
    lbu t3, 0(t3)
    j .Lcompare
.Llast_byte:
    li t3, 90
.Lcompare:
    bne t2, t3, file_failure
    addi t0, t0, 1
    addi s3, s3, 1
    bne t0, t1, .Lcheck_data
    j .Lread_all
.Lread_done:
    li t0, 8193
    bne s3, t0, file_failure
    mv a0, s0
    li a1, 0
    li a2, 0
    invoke 62
    expect 0

    mv a0, s0
    mv a1, sp
    li a2, 4
    invoke 63
    expect 4
    lw t0, 0(sp)
    li t1, 0x33323130
    bne t0, t1, file_failure
    mv a0, s0
    invoke 23
    expect 3
    mv s1, a0
    li a1, 1
    invoke 25
    expect 0
    mv a0, s1
    li a1, 2
    li a2, 1
    invoke 25
    expect 0
    mv a0, s1
    li a1, 1
    invoke 25
    expect 1
    mv a0, s0
    li a1, 2
    li a2, 0
    invoke 25
    expect 0
    mv a0, s1
    li a1, 1
    invoke 25
    expect 1

    mv a0, s1
    mv a1, sp
    li a2, 4
    invoke 63
    expect 4
    lw t0, 0(sp)
    li t1, 0x37363534
    bne t0, t1, file_failure
    mv a0, s0
    mv a1, sp
    li a2, 4
    li a3, 8
    invoke 67
    expect 4
    lw t0, 0(sp)
    li t1, 0x62613938
    bne t0, t1, file_failure
    mv a0, s1
    li a1, 0
    li a2, 1
    invoke 62
    expect 8
    mv a0, s0
    invoke 57
    expect 0
    mv a0, s0
    invoke 57
    expect -9
    open_data
    expect 0
    mv s2, a0
    mv a1, sp
    li a2, 2
    invoke 63
    expect 2
    lhu t0, 0(sp)
    li t1, 0x3130
    bne t0, t1, file_failure
    mv a0, s1
    mv a1, sp
    li a2, 2
    invoke 63
    expect 2
    lhu t0, 0(sp)
    li t1, 0x3938
    bne t0, t1, file_failure

    mv a0, s1
    li a1, -1
    li a2, 2
    invoke 62
    expect 8192
    mv a0, s1
    mv a1, sp
    li a2, 8
    invoke 63
    expect 1
    lbu t0, 0(sp)
    li t1, 90
    bne t0, t1, file_failure
    mv a0, s1
    li a1, 0
    li a2, 8
    invoke 63
    expect 0
    mv a0, s1
    li a1, 10
    li a2, 2
    invoke 62
    expect 8203
    mv a0, s1
    li a1, -1
    li a2, 0
    invoke 62
    expect -22
    mv a0, s1
    li a1, 0
    li a2, 1
    invoke 62
    expect 8203
    mv a0, s1
    li a1, 0x7fffffffffffffff
    li a2, 0
    invoke 62
    expect 0x7fffffffffffffff
    mv a0, s1
    li a1, 1
    li a2, 1
    invoke 62
    expect -75
    mv a0, s1
    li a1, 0
    li a2, 1
    invoke 62
    expect 0x7fffffffffffffff
    mv a0, s1
    li a1, 0
    li a2, 0
    invoke 62
    expect 0

    mv a0, s1
    la a1, data_path
    li a2, 4
    invoke 63
    expect -14
    mv a0, s1
    li a1, 0
    li a2, 1
    invoke 62
    expect 0
    mv a0, s1
    li a1, 0
    li a2, 4
    invoke 63
    expect -14
    mv a0, s1
    li a1, 0
    li a2, 0
    invoke 63
    expect 0
    mv a0, s1
    mv a1, sp
    li a2, 4
    li a3, -1
    invoke 67
    expect -22
    mv a0, s1
    li a1, 0
    li a2, 1
    invoke 62
    expect 0

    li a0, 0
    li a1, 8192
    li a2, 3
    li a3, 0x22
    li a4, -1
    li a5, 0
    invoke 222
    blez a0, file_failure
    mv s4, a0
    li t0, 4096
    add s5, s4, t0
    mv a0, s5
    li a1, 4096
    li a2, 0
    invoke 226
    expect 0
    mv a0, s1
    addi a1, s5, -2
    li a2, 8
    invoke 63
    expect 2
    lhu t0, -2(s5)
    li t1, 0x3130
    bne t0, t1, file_failure
    mv a0, s1
    mv a1, s5
    li a2, 8
    invoke 63
    expect -14
    mv a0, s1
    li a1, 0
    li a2, 1
    invoke 62
    expect 2
    mv a0, s4
    li a1, 8192
    invoke 215
    expect 0

    mv a0, s1
    li a1, 1000000
    li a2, 0x80000
    invoke 24
    expect 1000000
    li a0, 1000000
    li a1, 1
    invoke 25
    expect 1
    li a0, 1000000
    invoke 57
    expect 0
    mv a0, s1
    mv a1, s1
    li a2, 0
    invoke 24
    expect -22
    li a0, -1
    mv a1, s1
    li a2, 0
    invoke 24
    expect -9
    mv a0, s1
    li a1, 10
    li a2, 2
    invoke 24
    expect -22
    mv a0, s1
    li a1, 1030
    li a2, 10
    invoke 25
    expect 10
    li a0, 10
    li a1, 1
    invoke 25
    expect 1
    li a0, 10
    invoke 57
    expect 0
    mv a0, s1
    li a1, 0
    li a2, -1
    invoke 25
    expect -22
    mv a0, s1
    mv a1, s2
    li a2, 0
    invoke 24
    bne a0, s2, file_failure
    mv a0, s2
    li a1, 0
    li a2, 1
    invoke 62
    expect 2
    mv a0, s1
    invoke 57
    expect 0
    mv a0, s2
    invoke 57
    expect 0

    li a0, 999
    la a1, data_path
    li a2, 0
    invoke 56
    expect 0
    invoke 57
    expect 0
    li a0, -100
    la a1, relative_path
    li a2, 0
    invoke 56
    expect 0
    invoke 57
    expect 0
    li a0, -100
    la a1, root_path
    li a2, 0
    invoke 56
    expect 0
    mv s0, a0
    mv a1, sp
    li a2, 1
    invoke 63
    expect -21
    mv a0, s0
    la a1, relative_path
    li a2, 0
    invoke 56
    expect -2
    mv a0, s0
    invoke 57
    expect 0
    open_data 0x10000
    expect -20
    li a0, -100
    la a1, trailing_path
    li a2, 0
    invoke 56
    expect -20
    li a0, -1
    la a1, relative_path
    li a2, 0
    invoke 56
    expect -9
    li a0, 1
    la a1, relative_path
    li a2, 0
    invoke 56
    expect -20
    li a0, -100
    li a1, 0
    li a2, 0
    invoke 56
    expect -14
    li a0, -100
    la a1, empty_path
    li a2, 0
    invoke 56
    expect -2
    li a0, -100
    la a1, missing_path
    li a2, 0
    invoke 56
    expect -2
    open_data 1
    expect -30
    open_data 3
    expect -22

    mv t0, sp
    li t1, 4096
    add t1, sp, t1
    li t2, 65
.Llong_path:
    sb t2, 0(t0)
    addi t0, t0, 1
    bne t0, t1, .Llong_path
    li a0, -100
    mv a1, sp
    li a2, 0
    invoke 56
    expect -36
    li a0, 99
    mv a1, sp
    li a2, 1
    invoke 63
    expect -9
    li a0, 1
    mv a1, sp
    li a2, 1
    invoke 63
    expect -9
    li a0, 1
    li a1, 0
    li a2, 0
    invoke 62
    expect -29
    li a0, 1
    mv a1, sp
    li a2, 1
    li a3, 0
    invoke 67
    expect -29

    li a0, 1
    invoke 23
    expect 0
    li a0, 1
    invoke 57
    expect 0
    li a0, 0
    la a1, alias_message
    li a2, 6
    invoke 64
    expect 6
    li a0, 0
    li a1, 1
    li a2, 0
    invoke 24
    expect 1
    li a0, 0
    invoke 57
    expect 0
    open_data
    expect 0
    invoke 23
    expect 3
    li t0, 8192
    add sp, sp, t0
    ld ra, 0(sp)
    addi sp, sp, 16
    ret
file_failure:
    li a0, 3
    invoke 94
    j file_failure

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
