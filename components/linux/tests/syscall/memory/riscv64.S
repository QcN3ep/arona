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
    bne a0, t0, mapping_failure
.endm
.global file_mappings
file_mappings:
    addi sp, sp, -512
    li a0, -100
    la a1, data_path
    li a2, 0
    invoke 56
    expect 0
    mv s0, a0
    li a0, 0
    li a1, 12288
    li a2, 3
    li a3, 2
    mv a4, s0
    li a5, 0
    invoke 222
    bltz a0, mapping_failure
    mv s1, a0
    lbu t0, 0(s1)
    li t1, 48
    bne t0, t1, mapping_failure
    li t0, 8192
    add t0, s1, t0
    lbu t1, 0(t0)
    li t2, 90
    bne t1, t2, mapping_failure
    lbu t1, 1(t0)
    bnez t1, mapping_failure
    li t1, 120
    sb t1, 0(s1)
    mv a0, s1
    li a1, 12288
    li a2, 4
    invoke 227
    expect 0
    mv a0, s1
    li a1, 4096
    li a2, 1
    invoke 227
    expect 0
    mv a0, s1
    li a1, 4096
    li a2, 2
    invoke 227
    expect 0
    mv a0, s1
    li a1, 0
    li a2, 4
    invoke 227
    expect 0
    mv a0, s1
    li a1, 4096
    li a2, 8
    invoke 227
    expect -22
    addi a0, s1, 1
    li a1, 4096
    li a2, 4
    invoke 227
    expect -22
    mv a0, s1
    li a1, 4096
    li a2, 5
    invoke 227
    expect -22
    li a0, 0x40000000
    li a1, 4096
    li a2, 4
    invoke 227
    expect -12
    mv a0, s0
    li a1, 0
    li a2, 1
    invoke 62
    expect 0
    mv a0, s0
    mv a1, sp
    li a2, 1
    li a3, 0
    invoke 67
    expect 1
    lbu t0, 0(sp)
    li t1, 48
    bne t0, t1, mapping_failure
    mv a0, s0
    invoke 57
    expect 0
    mv a0, s1
    li a1, 12288
    li a2, 0
    invoke 226
    expect 0
    mv a0, s1
    li a1, 12288
    li a2, 3
    invoke 226
    expect 0
    lbu t0, 0(s1)
    li t1, 120
    bne t0, t1, mapping_failure
    mv a0, s1
    li a1, 12288
    invoke 215
    expect 0
    li a0, -100
    la a1, data_path
    li a2, 0
    invoke 56
    expect 0
    mv s0, a0
    li a0, 0
    li a1, 4096
    li a2, 1
    li a3, 2
    mv a4, s0
    li a5, 4096
    invoke 222
    bltz a0, mapping_failure
    mv s1, a0
    lbu t0, 0(s1)
    li t1, 48
    bne t0, t1, mapping_failure
    mv a0, s1
    li a1, 4096
    invoke 215
    expect 0
    li a0, 0
    li a1, 4096
    li a2, 3
    li a3, 2
    li a4, 999
    li a5, 0
    invoke 222
    expect -9
    li a0, 0
    li a1, 4096
    li a2, 3
    li a3, 2
    li a4, 1
    li a5, 0
    invoke 222
    expect -19
    li a0, 0
    li a1, 4096
    li a2, 3
    li a3, 1
    mv a4, s0
    li a5, 0
    invoke 222
    expect -13
    li a0, 0
    li a1, 4096
    li a2, 3
    li a3, 2
    mv a4, s0
    li a5, 1
    invoke 222
    expect -22
    mv a0, s0
    invoke 57
    expect 0
    li a0, 0
    li a1, 8192
    li a2, 3
    li a3, 34
    li a4, -1
    li a5, 0
    invoke 222
    bltz a0, mapping_failure
    mv s1, a0
    li t0, 77
    sb t0, 0(s1)
    mv a0, s1
    li a1, 8192
    li a2, 4
    invoke 233
    expect 0
    lbu t0, 0(s1)
    li t1, 0
    bne t0, t1, mapping_failure
    mv a0, s1
    li a1, 8192
    invoke 215
    expect 0
    li a0, 0
    li a1, 8192
    li a2, 3
    li a3, 33
    li a4, -1
    li a5, 0
    invoke 222
    bltz a0, mapping_failure
    mv s1, a0
    li t0, 77
    sb t0, 0(s1)
    mv a0, s1
    li a1, 8192
    li a2, 4
    invoke 233
    expect 0
    lbu t0, 0(s1)
    li t1, 77
    bne t0, t1, mapping_failure
    mv a0, s1
    li a1, 8192
    invoke 215
    expect 0
    li a0, 0
    li a1, 4096
    li a2, 3
    li a3, 34
    li a4, -1
    li a5, 0
    invoke 222
    bltz a0, mapping_failure
    mv s2, a0
    li t0, 85
    sb t0, 0(s2)
    mv a0, s2
    li a1, 4096
    li a2, 3
    li a3, 50
    li a4, -1
    li a5, 0
    invoke 222
    bne a0, s2, mapping_failure
    lbu t0, 0(s2)
    bnez t0, mapping_failure
    li a0, 0
    li a1, 8192
    li a2, 3
    li a3, 34
    li a4, -1
    li a5, 0
    invoke 222
    bltz a0, mapping_failure
    mv s3, a0
    li t0, 66
    sb t0, 0(s3)
    li t0, 4096
    add t0, s3, t0
    li t1, 77
    sb t1, 0(t0)
    li a0, 4096
    add a0, s3, a0
    li a1, 4096
    li a2, 3
    li a3, 50
    li a4, -1
    li a5, 0
    invoke 222
    li t0, 4096
    add t0, s3, t0
    bne a0, t0, mapping_failure
    lbu t0, 0(s3)
    li t1, 66
    bne t0, t1, mapping_failure
    li t0, 4096
    add t0, s3, t0
    lbu t1, 0(t0)
    bnez t1, mapping_failure
    li a0, 16384
    add a0, s3, a0
    li a1, 4096
    li a2, 3
    li a3, 50
    li a4, -1
    li a5, 0
    invoke 222
    li t0, 16384
    add t0, s3, t0
    bne a0, t0, mapping_failure
    lbu t0, 0(a0)
    bnez t0, mapping_failure
    mv a0, s2
    li a1, 4096
    li a2, 3
    li a3, 0x100032
    li a4, -1
    li a5, 0
    invoke 222
    expect -22
    addi a0, s2, 1
    li a1, 4096
    li a2, 3
    li a3, 50
    li a4, -1
    li a5, 0
    invoke 222
    expect -22
    li a0, -100
    la a1, data_path
    li a2, 0
    invoke 56
    expect 0
    mv s4, a0
    li a0, 0
    li a1, 4096
    li a2, 1
    li a3, 2
    mv a4, s4
    li a5, 0
    invoke 222
    bltz a0, mapping_failure
    mv s5, a0
    lbu t0, 0(s5)
    li t1, 48
    bne t0, t1, mapping_failure
    mv a0, s5
    li a1, 4096
    li a2, 1
    li a3, 50
    li a4, -1
    li a5, 0
    invoke 222
    bne a0, s5, mapping_failure
    lbu t0, 0(s5)
    bnez t0, mapping_failure
    mv a0, s4
    invoke 57
    expect 0
    li a0, 42
    invoke 94
.global remap_mappings
remap_mappings:
    addi sp, sp, -64

    # Clear the anonymous test window.
    li a0, 0x1000000000
    li a1, 65536
    invoke 215
    li a0, 0x1000000000
    li a1, 8192
    li a2, 3
    li a3, 0x32
    li a4, -1
    li a5, 0
    invoke 222
    li t0, 0x1000000000
    bne a0, t0, mapping_failure
    mv s0, a0
    li t1, 0x11
    sb t1, 0(s0)
    li t0, 4096
    add t2, s0, t0
    li t1, 0x22
    sb t1, 0(t2)

    # Grow in place and observe zeroed pages.
    mv a0, s0
    li a1, 8192
    li a2, 16384
    li a3, 0
    li a4, 0
    invoke 216
    bne a0, s0, mapping_failure
    lbu t0, 0(s0)
    li t1, 0x11
    bne t0, t1, mapping_failure
    li t0, 4096
    add t2, s0, t0
    lbu t0, 0(t2)
    li t1, 0x22
    bne t0, t1, mapping_failure
    li t0, 8192
    add t2, s0, t0
    lbu t0, 0(t2)
    bnez t0, mapping_failure
    li t1, 0x33
    sb t1, 0(t2)

    # Shrink in place and check the tail is unmapped.
    mv a0, s0
    li a1, 16384
    li a2, 8192
    li a3, 0
    li a4, 0
    invoke 216
    bne a0, s0, mapping_failure
    lbu t0, 0(s0)
    li t1, 0x11
    bne t0, t1, mapping_failure
    li t0, 8192
    add a0, s0, t0
    li a1, 4096
    li a2, 4
    invoke 227
    expect -12
    mv a0, s0
    li a1, 16384
    li a2, 8192
    li a3, 0
    li a4, 0
    invoke 216
    expect -14

    # Shrink to one page and block in-place growth.
    mv a0, s0
    li a1, 8192
    li a2, 4096
    li a3, 0
    li a4, 0
    invoke 216
    bne a0, s0, mapping_failure
    li a0, 0x1000001000
    li a1, 4096
    li a2, 3
    li a3, 0x32
    li a4, -1
    li a5, 0
    invoke 222
    li t0, 0x1000001000
    bne a0, t0, mapping_failure
    li t1, 0x44
    sb t1, 0(a0)
    mv a0, s0
    li a1, 4096
    li a2, 8192
    li a3, 0
    li a4, 0
    invoke 216
    expect -12
    lbu t0, 0(s0)
    li t1, 0x11
    bne t0, t1, mapping_failure
    li t0, 0x1000001000
    lbu t0, 0(t0)
    li t1, 0x44
    bne t0, t1, mapping_failure

    # MAYMOVE relocates and preserves the data.
    mv a0, s0
    li a1, 4096
    li a2, 8192
    li a3, 1
    li a4, 0
    invoke 216
    bltz a0, mapping_failure
    beq a0, s0, mapping_failure
    mv s1, a0
    lbu t0, 0(s1)
    li t1, 0x11
    bne t0, t1, mapping_failure
    li t0, 4096
    add t2, s1, t0
    lbu t0, 0(t2)
    bnez t0, mapping_failure
    mv a0, s0
    li a1, 4096
    li a2, 4
    invoke 227
    expect -12
    mv a0, s1
    li a1, 8192
    li a2, 8192
    li a3, 0
    li a4, 0
    invoke 216
    bne a0, s1, mapping_failure
    li t0, 0x1000001000
    lbu t0, 0(t0)
    li t1, 0x44
    bne t0, t1, mapping_failure

    # MREMAP_FIXED moves over an existing destination mapping.
    li a0, 0x1000100000
    li a1, 8192
    invoke 215
    li a0, 0x1000100000
    li a1, 4096
    li a2, 3
    li a3, 0x32
    li a4, -1
    li a5, 0
    invoke 222
    li t0, 0x1000100000
    bne a0, t0, mapping_failure
    li t1, 0x55
    sb t1, 0(a0)
    mv a0, s1
    li a1, 8192
    li a2, 8192
    li a3, 3
    li a4, 0x1000100000
    invoke 216
    li t0, 0x1000100000
    bne a0, t0, mapping_failure
    mv s2, a0
    lbu t0, 0(s2)
    li t1, 0x11
    bne t0, t1, mapping_failure
    li t0, 4096
    add t2, s2, t0
    lbu t0, 0(t2)
    bnez t0, mapping_failure
    mv a0, s1
    li a1, 8192
    li a2, 4
    invoke 227
    expect -12

    # Reject invalid flags, sizes, addresses, and overlapping destinations.
    mv a0, s2
    li a1, 8192
    li a2, 8192
    li a3, 8
    li a4, 0
    invoke 216
    expect -22
    mv a0, s2
    li a1, 8192
    li a2, 8192
    li a3, 2
    li a4, 0
    invoke 216
    expect -22
    mv a0, s2
    li a1, 8192
    li a2, 8192
    li a3, 3
    mv a4, s2
    invoke 216
    expect -22
    mv a0, s2
    li a1, 8192
    li a2, 8192
    li a3, 3
    li t0, 4096
    add a4, s2, t0
    invoke 216
    expect -22
    mv a0, s2
    li a1, 8192
    li a2, 8192
    li a3, 3
    li t0, 1
    add a4, s2, t0
    invoke 216
    expect -22
    mv a0, s2
    li a1, 8192
    li a2, 8192
    li a3, 3
    li a4, 0
    invoke 216
    expect -22
    li t0, 1
    add a0, s2, t0
    li a1, 8192
    li a2, 8192
    li a3, 1
    li a4, 0
    invoke 216
    expect -22
    mv a0, s2
    li a1, 8192
    li a2, 0
    li a3, 1
    li a4, 0
    invoke 216
    expect -22
    mv a0, s2
    li a1, 0
    li a2, 8192
    li a3, 1
    li a4, 0
    invoke 216
    expect -22
    mv a0, s2
    li a1, 8192
    li a2, 8192
    li a3, 5
    li a4, 0
    invoke 216
    expect -95
    li a0, 0x1000400000
    li a1, 4096
    li a2, 8192
    li a3, 1
    li a4, 0
    invoke 216
    expect -14
    mv a0, s2
    li a1, 8192
    li a2, 8192
    li a3, 3
    li a4, 0x4000000000
    invoke 216
    expect -22
    lbu t0, 0(s2)
    li t1, 0x11
    bne t0, t1, mapping_failure
    mv a0, s2
    li a1, 8192
    li a2, 4
    invoke 227
    expect 0

    # MREMAP_FIXED growth zero-fills the extension.
    li a0, 0x1000050000
    li a1, 65536
    invoke 215
    mv a0, s2
    li a1, 8192
    li a2, 16384
    li a3, 3
    li a4, 0x1000050000
    invoke 216
    li t0, 0x1000050000
    bne a0, t0, mapping_failure
    mv s3, a0
    lbu t0, 0(s3)
    li t1, 0x11
    bne t0, t1, mapping_failure
    li t0, 4096
    add t2, s3, t0
    lbu t0, 0(t2)
    bnez t0, mapping_failure
    li t0, 8192
    add t2, s3, t0
    lbu t0, 0(t2)
    bnez t0, mapping_failure
    li t0, 12288
    add t2, s3, t0
    lbu t0, 0(t2)
    bnez t0, mapping_failure
    mv a0, s2
    li a1, 8192
    li a2, 4
    invoke 227
    expect -12

    # MREMAP_FIXED shrink moves only the retained prefix.
    li a0, 0x1000060000
    li a1, 65536
    invoke 215
    mv a0, s3
    li a1, 16384
    li a2, 8192
    li a3, 3
    li a4, 0x1000060000
    invoke 216
    li t0, 0x1000060000
    bne a0, t0, mapping_failure
    mv s4, a0
    lbu t0, 0(s4)
    li t1, 0x11
    bne t0, t1, mapping_failure
    li t0, 4096
    add t2, s4, t0
    lbu t0, 0(t2)
    bnez t0, mapping_failure
    mv a0, s3
    li a1, 16384
    li a2, 4
    invoke 227
    expect -12
    mv a0, s4
    li a1, 8192
    invoke 215
    expect 0

    # Shrink, move, and reject growth for private file mappings.
    li a0, 0x1000200000
    li a1, 65536
    invoke 215
    li a0, -100
    la a1, data_path
    li a2, 0
    invoke 56
    bltz a0, mapping_failure
    mv s5, a0
    li a0, 0x1000200000
    li a1, 12288
    li a2, 3
    li a3, 0x12
    mv a4, s5
    li a5, 0
    invoke 222
    li t0, 0x1000200000
    bne a0, t0, mapping_failure
    mv s6, a0
    lbu t0, 0(s6)
    li t1, 48
    bne t0, t1, mapping_failure
    li t0, 4096
    add t2, s6, t0
    lbu t0, 0(t2)
    li t1, 48
    bne t0, t1, mapping_failure
    li t0, 8192
    add t2, s6, t0
    lbu t0, 0(t2)
    li t1, 90
    bne t0, t1, mapping_failure
    lbu t0, 1(t2)
    bnez t0, mapping_failure
    mv a0, s6
    li a1, 8193
    li a2, 8192
    li a3, 0
    li a4, 0
    invoke 216
    bne a0, s6, mapping_failure
    lbu t0, 0(s6)
    li t1, 48
    bne t0, t1, mapping_failure
    li t0, 8192
    add a0, s6, t0
    li a1, 4096
    li a2, 4
    invoke 227
    expect -12
    mv a0, s6
    li a1, 8192
    li a2, 12288
    li a3, 0
    li a4, 0
    invoke 216
    expect -12
    lbu t0, 0(s6)
    li t1, 48
    bne t0, t1, mapping_failure
    # A file mapping cannot grow through MREMAP_FIXED either.
    li a0, 0x1000350000
    li a1, 4096
    li a2, 3
    li a3, 0x32
    li a4, -1
    li a5, 0
    invoke 222
    li t0, 0x1000350000
    bne a0, t0, mapping_failure
    li t1, 0x66
    sb t1, 0(a0)
    mv a0, s6
    li a1, 8192
    li a2, 12288
    li a3, 3
    li a4, 0x1000350000
    invoke 216
    expect -12
    li t0, 0x1000350000
    lbu t0, 0(t0)
    li t1, 0x66
    bne t0, t1, mapping_failure
    lbu t0, 0(s6)
    li t1, 48
    bne t0, t1, mapping_failure

    # Moving a file mapping preserves its contents.
    li a0, 0x1000300000
    li a1, 8192
    invoke 215
    mv a0, s6
    li a1, 8192
    li a2, 8192
    li a3, 3
    li a4, 0x1000300000
    invoke 216
    li t0, 0x1000300000
    bne a0, t0, mapping_failure
    mv s7, a0
    lbu t0, 0(s7)
    li t1, 48
    bne t0, t1, mapping_failure
    li t0, 4096
    add t2, s7, t0
    lbu t0, 0(t2)
    li t1, 48
    bne t0, t1, mapping_failure
    mv a0, s6
    li a1, 8192
    li a2, 4
    invoke 227
    expect -12

    mv a0, s5
    invoke 57
    expect 0
    mv a0, s7
    li a1, 8192
    invoke 215
    expect 0

    li a0, 42
    invoke 94

.global mapping_bus_fault
mapping_bus_fault:
    addi sp, sp, -512
    li a0, -100
    la a1, data_path
    li a2, 0
    invoke 56
    expect 0
    mv s0, a0
    li a0, 0
    li a1, 16384
    li a2, 3
    li a3, 2
    mv a4, s0
    li a5, 0
    invoke 222
    bltz a0, mapping_failure
    mv s1, a0
    li a0, 0x1000500000
    li a1, 16384
    invoke 215
    mv a0, s1
    li a1, 16384
    li a2, 16384
    li a3, 3
    li a4, 0x1000500000
    invoke 216
    li t0, 0x1000500000
    bne a0, t0, mapping_failure
    li t0, 12288
    add t0, a0, t0
    lbu t0, 0(t0)
    j mapping_failure
mapping_failure:
    addi a0, s11, 100
    invoke 94
    j mapping_failure
.section .rodata
data_path: .asciz "/etc/linux-data"
.section .note.GNU-stack,"",@progbits
