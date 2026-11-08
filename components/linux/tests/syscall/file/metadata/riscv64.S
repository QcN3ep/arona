# SPDX-License-Identifier: MPL-2.0

.section .text
.option norelax
.macro invoke number
    li a7, \number
    ecall
.endm
.macro expect value
    li s11, \@
    li t0, \value
    bne a0, t0, metadata_failure
.endm
.macro open_file path
    li a0, -100
    la a1, \path
    li a2, 0
    invoke 56
.endm
.global files_metadata
files_metadata:
    addi sp, sp, -512
    open_file data_path
    expect 0
    mv s0, a0
    mv a1, sp
    invoke 80
    expect 0
    ld s1, 0(sp)
    beqz s1, metadata_failure
    ld s2, 8(sp)
    beqz s2, metadata_failure
    lw t0, 16(sp)
    li t1, 0100644
    bne t0, t1, metadata_failure
    ld t0, 48(sp)
    li t1, 8193
    bne t0, t1, metadata_failure
    lw t0, 56(sp)
    li t1, 4096
    bne t0, t1, metadata_failure
    ld t0, 120(sp)
    bnez t0, metadata_failure
    mv a0, s0
    li a1, 0
    invoke 80
    expect -14
    mv a0, s0
    la a1, empty_path
    addi a2, sp, 128
    li a3, 0x1000
    invoke 79
    expect 0
    ld t0, 128(sp)
    bne t0, s1, metadata_failure
    ld t0, 136(sp)
    bne t0, s2, metadata_failure
    mv a0, s0
    invoke 57
    expect 0
    li a0, 999
    la a1, data_path
    mv a2, sp
    li a3, 0
    invoke 79
    expect 0
    ld t0, 8(sp)
    bne t0, s2, metadata_failure
    li a0, -100
    la a1, missing_path
    mv a2, sp
    li a3, 0
    invoke 79
    expect -2
    li a0, -100
    la a1, empty_path
    mv a2, sp
    li a3, 0
    invoke 79
    expect -2
    li a0, -100
    li a1, 0
    mv a2, sp
    li a3, 0
    invoke 79
    expect -14
    li a0, -100
    la a1, data_path
    mv a2, sp
    li a3, 1
    invoke 79
    expect -22
    open_file stat_path
    expect 0
    mv s0, a0
    mv a1, sp
    invoke 80
    expect 0
    ld t0, 0(sp)
    beq t0, s1, metadata_failure
    mv s1, t0
    ld s2, 8(sp)
    lw t0, 16(sp)
    li t1, 0100600
    bne t0, t1, metadata_failure
    lw t0, 20(sp)
    li t1, 2
    bne t0, t1, metadata_failure
    lw t0, 24(sp)
    li t1, 123
    bne t0, t1, metadata_failure
    lw t0, 28(sp)
    li t1, 456
    bne t0, t1, metadata_failure
    ld t0, 48(sp)
    li t1, 4
    bne t0, t1, metadata_failure
    ld t0, 72(sp)
    li t1, -5
    bne t0, t1, metadata_failure
    ld t0, 80(sp)
    li t1, 99
    bne t0, t1, metadata_failure
    ld t0, 96(sp)
    li t1, 123
    bne t0, t1, metadata_failure
    ld t0, 112(sp)
    li t1, 321
    bne t0, t1, metadata_failure
    mv a0, s0
    invoke 57
    expect 0
    li a0, -100
    la a1, link_path
    mv a2, sp
    li a3, 0
    invoke 79
    expect 0
    ld t0, 0(sp)
    bne t0, s1, metadata_failure
    ld t0, 8(sp)
    bne t0, s2, metadata_failure
    li a0, -100
    la a1, symlink_path
    mv a2, sp
    li a3, 0x100
    invoke 79
    expect 0
    lw t0, 16(sp)
    li t1, 0170000
    and t0, t0, t1
    li t1, 0120000
    bne t0, t1, metadata_failure
    li a0, 1
    mv a1, sp
    invoke 80
    expect 0
    lw t0, 16(sp)
    li t1, 0170000
    and t0, t0, t1
    li t1, 020000
    bne t0, t1, metadata_failure
    li a0, -1
    mv a1, sp
    invoke 80
    expect -9
    li a0, 42
    invoke 94
metadata_failure:
    addi a0, s11, 10
    invoke 94
    j metadata_failure
.section .rodata
data_path: .asciz "/etc/linux-data"
stat_path: .asciz "/var/linux-stat"
link_path: .asciz "/var/linux-stat-link"
symlink_path: .asciz "/var/linux-stat-symlink"
empty_path: .asciz ""
missing_path: .asciz "/etc/absent"
.section .note.GNU-stack,"",@progbits
