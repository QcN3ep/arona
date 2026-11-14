# SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

.section .text
.option norelax

.macro invoke number
    li a7, \number
    ecall
.endm

.macro expect value
    li s11, \@
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
    li a0, 1
    li a1, 0x5401
    mv a2, sp
    invoke 29
    expect 0
    li a0, 1
    li a1, 0x5403
    mv a2, sp
    invoke 29
    expect 0
    li a0, 1
    li a1, 0x5404
    mv a2, sp
    invoke 29
    expect 0
    li a0, 1
    li a1, 0x540b
    li a2, 2
    invoke 29
    expect 0
    li a0, 1
    li a1, 0x540b
    li a2, 1
    invoke 29
    expect 0
    li a0, -100
    la a1, data_path
    li a2, 0x40000
    invoke 56
    bltz a0, file_failure
    invoke 57
    expect 0
    li a0, -100
    la a1, data_path
    li a2, 0x4000
    invoke 56
    bltz a0, file_failure
    invoke 57
    expect 0
    li a0, -100
    la a1, data_path
    li a2, 0x200000
    invoke 56
    bltz a0, file_failure
    invoke 57
    expect 0
    li a0, -100
    la a1, null_path
    li a2, 2
    invoke 56
    bltz a0, file_failure
    mv s0, a0
    mv a0, s0
    la a1, null_data
    li a2, 3
    invoke 64
    expect 3
    mv a0, s0
    mv a1, sp
    li a2, 4
    invoke 63
    expect 0
    mv a0, s0
    li a1, 37
    li a2, 0
    invoke 62
    expect 0
    mv a0, s0
    li a1, -1
    li a2, 1
    invoke 62
    expect 0
    mv a0, s0
    li a1, 0
    li a2, 3
    invoke 62
    expect -22
    mv a0, s0
    mv a1, sp
    li a2, 16
    invoke 80
    expect 0
    lw t0, 16(sp)
    li t1, 0x2000
    and t0, t0, t1
    beqz t0, file_failure
    mv a0, s0
    invoke 57
    expect 0
    li a0, -100
    la a1, zero_path
    li a2, 2
    invoke 56
    bltz a0, file_failure
    mv s0, a0
    mv a0, s0
    mv a1, sp
    li a2, 8
    invoke 63
    expect 8
    ld t0, 0(sp)
    bnez t0, file_failure
    mv a0, s0
    mv a1, sp
    li a2, 5
    invoke 64
    expect 5
    mv a0, s0
    invoke 57
    expect 0
    li a0, -100
    la a1, random_path
    li a2, 0
    invoke 56
    bltz a0, file_failure
    mv s0, a0
    mv a0, s0
    mv a1, sp
    li a2, 8
    invoke 63
    expect 8
    ld t0, 0(sp)
    beqz t0, file_failure
    mv a0, s0
    la a1, null_data
    li a2, 3
    invoke 64
    expect -9
    mv a0, s0
    invoke 57
    expect 0
    li a0, -100
    la a1, urandom_path
    li a2, 0
    invoke 56
    bltz a0, file_failure
    mv s0, a0
    mv a0, s0
    mv a1, sp
    li a2, 4
    invoke 63
    expect 4
    mv a0, s0
    invoke 57
    expect 0
    li a0, -100
    la a1, null_path
    li a2, 0x10000
    invoke 56
    expect -20
    # An O_PATH descriptor reports status but rejects byte I/O and positioning.
    open_data 0x200000
    bltz a0, file_failure
    mv s0, a0
    mv a0, s0
    mv a1, sp
    invoke 80
    expect 0
    lw t0, 16(sp)
    li t1, 0100644
    bne t0, t1, file_failure
    ld t0, 48(sp)
    li t1, 8193
    bne t0, t1, file_failure
    mv a0, s0
    mv a1, sp
    li a2, 4
    invoke 63
    expect -9
    mv a0, s0
    la a1, null_data
    li a2, 3
    invoke 64
    expect -9
    mv a0, s0
    li a1, 0
    li a2, 0
    invoke 62
    expect -9
    mv a0, s0
    li a1, 0
    invoke 46
    expect -9
    mv a0, s0
    invoke 82
    expect -9
    mv a0, s0
    li a1, 4
    li a2, 2048
    invoke 25
    expect -9
    mv a0, s0
    li a1, 3
    invoke 25
    expect 0x200000
    mv a0, s0
    li a1, 0x5403
    mv a2, sp
    invoke 29
    expect -9
    mv a0, s0
    li a1, 0x5421
    mv a2, sp
    invoke 29
    expect -9
    li a0, 0
    li a1, 4096
    li a2, 1
    li a3, 2
    mv a4, s0
    li a5, 0
    invoke 222
    expect -9
    sw s0, 0(sp)
    li t0, 1
    sh t0, 4(sp)
    sh zero, 6(sp)
    sd zero, 16(sp)
    sd zero, 24(sp)
    mv a0, sp
    li a1, 1
    addi a2, sp, 16
    li a3, 0
    invoke 73
    expect 1
    lhu t0, 6(sp)
    li t1, 32
    bne t0, t1, file_failure
    li t0, 1
    sll t0, t0, s0
    sd t0, 0(sp)
    addi a0, s0, 1
    mv a1, sp
    li a2, 0
    li a3, 0
    addi a4, sp, 16
    li a5, 0
    invoke 72
    expect -9
    mv a0, s0
    invoke 57
    expect 0
    # An O_PATH directory anchors relative opens but rejects directory reads.
    li a0, -100
    la a1, path_root
    li a2, 0x210000
    invoke 56
    bltz a0, file_failure
    mv s0, a0
    mv a0, s0
    mv a1, sp
    li a2, 8
    invoke 61
    expect -9
    mv a0, s0
    la a1, relative_path
    li a2, 0
    invoke 56
    bltz a0, file_failure
    mv s1, a0
    mv a0, s1
    invoke 57
    expect 0
    mv a0, s0
    invoke 57
    expect 0
    # O_PATH ignores the access mode, and O_PATH|O_DIRECTORY still rejects files.
    open_data 0x80200003
    bltz a0, file_failure
    mv s0, a0
    mv a0, s0
    mv a1, sp
    li a2, 4
    invoke 63
    expect -9
    mv a0, s0
    invoke 57
    expect 0
    open_data 0x210000
    expect -20
    li t0, 8192
    add sp, sp, t0
    ld ra, 0(sp)
    addi sp, sp, 16
    ret
file_failure:
    addi a0, s11, 3
    invoke 94
    j file_failure

.section .rodata
 data_path: .asciz "/etc/linux-data"
 null_path: .asciz "/dev/null"
 zero_path: .asciz "/dev/zero"
 random_path: .asciz "/dev/random"
 urandom_path: .asciz "/dev/urandom"
 null_data: .ascii "abc"
 relative_path: .asciz "etc/linux-data"
 trailing_path: .asciz "/etc/linux-data/"
 path_root: .asciz "/"
 root_path: .asciz "/bin"
 missing_path: .asciz "/etc/absent"
 empty_path: .asciz ""
 alias_message: .ascii "alias\n"
 digits: .ascii "0123456789abcdef"

.section .note.GNU-stack,"",@progbits
