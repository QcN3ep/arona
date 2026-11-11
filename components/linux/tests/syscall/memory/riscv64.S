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
    expect -95
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
