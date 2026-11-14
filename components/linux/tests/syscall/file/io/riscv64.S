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
    bne a0, t0, io_failure
.endm

.macro open_file flags, path=write_path, mode=0644
    li a0, -100
    la a1, \path
    li a2, \flags
    li a3, \mode
    invoke 56
.endm

.macro seek fd, offset=0, whence=1
    mv a0, \fd
    li a1, \offset
    li a2, \whence
    invoke 62
.endm

.macro put fd, bytes=pair, length=2
    mv a0, \fd
    la a1, \bytes
    li a2, \length
    invoke 64
.endm

.global files_write
files_write:
    li t0, -8192
    add sp, sp, t0
    open_file 0xc2
    expect 0
    mv s0, a0
    open_file 0xc2
    expect -17
    mv a0, s0
    invoke 23
    expect 3
    mv s1, a0
    put s0, digits, 10
    expect 10
    seek s1
    expect 10
    mv a0, s0
    la a1, pair
    li a2, 2
    li a3, 2
    invoke 68
    expect 2
    seek s1
    expect 10
    mv a0, s1
    mv a1, sp
    li a2, 10
    li a3, 0
    invoke 67
    expect 10
    ld t0, 0(sp)
    li t1, 0x3736353442413130
    bne t0, t1, io_failure
    seek s0
    expect 10
    mv a0, s0
    li a1, 0
    li a2, 4
    invoke 64
    expect -14
    seek s0
    expect 10
    mv a0, s0
    li a1, 0
    li a2, 2
    li a3, 0
    invoke 68
    expect -14
    mv a0, s0
    li a1, 0
    li a2, 0
    invoke 64
    expect 0
    mv a0, s0
    la a1, pair
    li a2, 2
    li a3, -1
    invoke 68
    expect -22
    mv a0, s0
    la a1, pair
    li a2, 2
    li a3, 0x7fffffffffffffff
    invoke 68
    expect -27
    mv a0, s0
    li a1, -1
    invoke 46
    expect -22
    mv a0, s0
    li a1, 2147483649
    invoke 46
    expect -27
    mv a0, s0
    li a1, 4
    invoke 46
    expect 0
    seek s1
    expect 10
    mv a0, s0
    li a1, 16
    invoke 46
    expect 0
    mv a0, s1
    mv a1, sp
    li a2, 16
    li a3, 0
    invoke 67
    expect 16
    lw t0, 0(sp)
    li t1, 0x42413130
    bne t0, t1, io_failure
    lw t0, 4(sp)
    bnez t0, io_failure
    ld t0, 8(sp)
    bnez t0, io_failure

    open_file 0x401
    expect 4
    mv s2, a0
    seek s2, 0, 0
    expect 0
    put s2, suffix
    expect 2
    seek s2
    expect 18
    mv a0, s2
    la a1, pair
    li a2, 2
    li a3, 0
    invoke 68
    expect 2
    seek s2
    expect 18
    open_file 0x401
    expect 5
    mv s3, a0
    put s3, suffix
    expect 2
    seek s2, 0, 2
    expect 22
    mv a0, s3
    invoke 57
    expect 0
    mv a0, s2
    invoke 57
    expect 0

    mv a0, s1
    li a1, 4
    li a2, 0x400
    invoke 25
    expect 0
    mv a0, s0
    li a1, 3
    invoke 25
    expect 0x402
    put s1, suffix
    expect 2
    seek s0
    expect 24
    mv a0, s0
    li a1, 4
    li a2, 0
    invoke 25
    expect 0
    mv a0, s1
    li a1, 3
    invoke 25
    expect 2
    put s1
    expect 2
    seek s0
    expect 26
    open_file 0
    expect 4
    mv s2, a0
    put s2
    expect -9
    mv a0, s2
    li a1, 0
    invoke 46
    expect -22
    mv a0, s2
    invoke 82
    expect 0
    mv a0, s2
    invoke 57
    expect 0
    open_file 0x42
    expect 4
    mv s2, a0
    seek s2, 0, 2
    expect 26
    mv a0, s2
    invoke 57
    expect 0
    open_file 0x202
    expect 4
    mv s2, a0
    seek s2, 0, 2
    expect 0
    seek s0
    expect 26
    mv a0, s2
    invoke 57
    expect 0

    li a0, 077
    invoke 166
    expect 022
    open_file 0xc2, new_path, 0666
    expect 4
    mv s2, a0
    li a0, 022
    invoke 166
    expect 077
    mv a0, s2
    invoke 57
    expect 0
    open_file 0x101002
    expect 4
    mv s2, a0
    put s2
    expect 2
    mv a0, s2
    invoke 83
    expect 0
    mv a0, s2
    invoke 57
    expect 0
    open_file 0, directory
    expect 4
    mv s2, a0
    invoke 82
    expect 0
    mv a0, s2
    invoke 57
    expect 0
    open_file 1, directory
    expect -21
    li a0, 1
    invoke 82
    expect -22
    li a0, -1
    invoke 83
    expect -9

    li a0, 0
    li a1, 8192
    li a2, 3
    li a3, 0x22
    li a4, -1
    li a5, 0
    invoke 222
    blez a0, io_failure
    mv s4, a0
    li t0, 4096
    add s5, s4, t0
    li t0, 0x7978
    sh t0, -2(s5)
    mv a0, s5
    li a1, 4096
    li a2, 0
    invoke 226
    expect 0
    mv a0, s0
    addi a1, s5, -2
    li a2, 8
    invoke 64
    expect 2
    seek s0
    expect 28
    mv a0, s0
    mv a1, s5
    li a2, 8
    invoke 64
    expect -14
    seek s0
    expect 28
    mv a0, s0
    mv a1, sp
    li a2, 28
    li a3, 0
    invoke 67
    expect 28
    lhu t0, 0(sp)
    li t1, 0x4241
    bne t0, t1, io_failure
    lhu t0, 26(sp)
    li t1, 0x7978
    bne t0, t1, io_failure
    mv a0, s0
    invoke 82
    expect 0
    open_file 0xc2, temporary
    bltz a0, io_failure
    mv s0, a0
    put s0
    expect 2
    mv a0, s0
    mv a1, sp
    invoke 80
    expect 0
    ld t0, 64(sp)
    li t1, 8
    bne t0, t1, io_failure
    mv a0, s0
    li a1, 65536
    invoke 46
    expect 0
    mv a0, s0
    la a1, pair
    li a2, 2
    li a3, 65536
    invoke 68
    expect -28
    mv a0, s0
    mv a1, sp
    invoke 80
    expect 0
    ld t0, 48(sp)
    li t1, 65536
    bne t0, t1, io_failure
    mv a0, s0
    invoke 57
    expect 0
    li a0, -100
    la a1, temporary
    li a2, 0
    invoke 35
    expect 0
    li a0, 42
    invoke 94
    j io_failure

.global files_append
files_append:
    addi sp, sp, -128
    addi s0, sp, 64
    addi t0, sp, 128
.Lfill:
    sb s3, 0(s0)
    addi s0, s0, 1
    bne s0, t0, .Lfill
    addi t0, sp, 64
    li t1, 16
    sd t0, 0(sp)
    sd t1, 8(sp)
    addi t0, t0, 16
    sd t0, 16(sp)
    sd t1, 24(sp)
    addi t0, t0, 16
    sd t0, 32(sp)
    sd t1, 40(sp)
    addi t0, t0, 16
    sd t0, 48(sp)
    sd t1, 56(sp)
    open_file 0x401, append_path
    expect 0
    mv s0, a0
    li s1, 8
.Lappend:
    mv a0, s0
    mv a1, sp
    li a2, 4
    invoke 66
    expect 64
    addi s1, s1, -1
    bnez s1, .Lappend
    li a0, 42
    invoke 94
io_failure:
    addi a0, s11, 10
    invoke 94
    j io_failure

.section .rodata
 write_path: .asciz "/var/linux-write"
 new_path: .asciz "/var/linux-new"
 append_path: .asciz "/var/linux-append"
 temporary: .asciz "/tmp/linux-temporary"
 directory: .asciz "/var"
 digits: .ascii "0123456789"
 pair: .ascii "AB"
 suffix: .ascii "xy"

.section .note.GNU-stack,"",@progbits
