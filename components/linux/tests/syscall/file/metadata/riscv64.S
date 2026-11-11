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
    bne a0, t0, metadata_failure
.endm
.macro open_file path
    li a0, -100
    la a1, \path
    li a2, 0
    invoke 56
.endm
.macro statx_file path, flags=0, mask=0x7ff, directory=-100
    li a0, \directory
    la a1, \path
    li a2, \flags
    li a3, \mask
    addi a4, sp, 256
    invoke 291
.endm
.global files_metadata
files_metadata:
    addi sp, sp, -528
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
    open_file stat_path
    expect 0
    mv s0, a0
    mv a1, sp
    invoke 80
    expect 0
    li t0, -1
    addi t1, sp, 256
    addi t2, sp, 512
statx_fill:
    sd t0, 0(t1)
    addi t1, t1, 8
    bne t1, t2, statx_fill
    sd t0, 512(sp)
    statx_file stat_path, 0x2000, 0x7fffffff, 999
    expect 0
    lw t0, 256(sp)
    li t1, 0x17ff
    bne t0, t1, metadata_failure
    lw t0, 260(sp)
    li t1, 4096
    bne t0, t1, metadata_failure
    ld t0, 264(sp)
    bnez t0, metadata_failure
    lw t0, 272(sp)
    li t1, 2
    bne t0, t1, metadata_failure
    lw t0, 276(sp)
    li t1, 123
    bne t0, t1, metadata_failure
    lw t0, 280(sp)
    li t1, 456
    bne t0, t1, metadata_failure
    lhu t0, 284(sp)
    li t1, 0100600
    bne t0, t1, metadata_failure
    lhu t0, 286(sp)
    bnez t0, metadata_failure
    ld t0, 288(sp)
    bne t0, s2, metadata_failure
    ld t0, 296(sp)
    li t1, 4
    bne t0, t1, metadata_failure
    ld t0, 304(sp)
    ld t1, 64(sp)
    bne t0, t1, metadata_failure
    ld t0, 312(sp)
    bnez t0, metadata_failure
    ld t0, 320(sp)
    li t1, -5
    bne t0, t1, metadata_failure
    ld t0, 328(sp)
    li t1, 99
    bne t0, t1, metadata_failure
    ld t0, 336(sp)
    bnez t0, metadata_failure
    ld t0, 344(sp)
    bnez t0, metadata_failure
    ld t0, 352(sp)
    li t1, 9
    bne t0, t1, metadata_failure
    ld t0, 360(sp)
    li t1, 321
    bne t0, t1, metadata_failure
    ld t0, 368(sp)
    li t1, 7
    bne t0, t1, metadata_failure
    ld t0, 376(sp)
    li t1, 123
    bne t0, t1, metadata_failure
    ld t0, 384(sp)
    bnez t0, metadata_failure
    lwu t0, 392(sp)
    srli t1, s1, 8
    li t2, 0xfff
    and t1, t1, t2
    bne t0, t1, metadata_failure
    lwu t0, 396(sp)
    andi t1, s1, 0xff
    srli t2, s1, 12
    li t3, 0xffffff00
    and t2, t2, t3
    or t1, t1, t2
    bne t0, t1, metadata_failure
    ld t0, 400(sp)
    beqz t0, metadata_failure
    addi t0, sp, 408
    addi t1, sp, 512
statx_zero:
    ld t2, 0(t0)
    bnez t2, metadata_failure
    addi t0, t0, 8
    bne t0, t1, statx_zero
    ld t0, 512(sp)
    li t1, -1
    bne t0, t1, metadata_failure
    mv a0, s0
    li a1, 0
    li a2, 0x1000
    li a3, 0
    addi a4, sp, 256
    invoke 291
    expect 0
    ld t0, 288(sp)
    bne t0, s2, metadata_failure
    statx_file empty_path, 0x1000, 0x800, 0
    expect 0
    lw t0, 256(sp)
    li t1, 0x17ff
    bne t0, t1, metadata_failure
    ld t0, 288(sp)
    bne t0, s2, metadata_failure
    statx_file relative_path, 0, 0x7ff, 0
    expect -20
    mv a0, s0
    invoke 57
    expect 0
    open_file directory_path
    expect 0
    mv s0, a0
    statx_file relative_path, 0x4000, 0x7ff, 0
    expect 0
    ld t0, 288(sp)
    bne t0, s2, metadata_failure
    mv a0, s0
    invoke 57
    expect 0
    statx_file empty_path, 0x1000
    expect 0
    lhu t0, 284(sp)
    li t1, 0170000
    and t0, t0, t1
    li t1, 040000
    bne t0, t1, metadata_failure
    statx_file symlink_path
    expect 0
    ld t0, 288(sp)
    bne t0, s2, metadata_failure
    statx_file symlink_path, 0x100
    expect 0
    lhu t0, 284(sp)
    li t1, 0170000
    and t0, t0, t1
    li t1, 0120000
    bne t0, t1, metadata_failure
    ld t0, 288(sp)
    beq t0, s2, metadata_failure
    statx_file link_path, 0x800
    expect 0
    ld t0, 288(sp)
    bne t0, s2, metadata_failure
    statx_file empty_path, 0x1000, 0x7ff, 1
    expect 0
    lhu t0, 284(sp)
    li t1, 0170000
    and t0, t0, t1
    li t1, 020000
    bne t0, t1, metadata_failure
    statx_file empty_path, 0x1000, 0x7ff, 999
    expect -9
    statx_file data_path, 1
    expect -22
    statx_file data_path, 0x6000
    expect -22
    statx_file data_path, 0, 0x80000000
    expect -22
    statx_file missing_path
    expect -2
    statx_file empty_path
    expect -2
    li a0, -100
    li a1, 0
    li a2, 0
    li a3, 0x7ff
    addi a4, sp, 256
    invoke 291
    expect -14
    li a0, -100
    la a1, data_path
    li a2, 0
    li a3, 0x7ff
    li a4, 0
    invoke 291
    expect -14
    la a0, missing_path
    mv a1, sp
    invoke 43
    expect -2
    la a0, data_path
    mv a1, sp
    invoke 43
    expect 0
    ld t0, 0(sp)
    li t1, 0x858458f6
    bne t0, t1, metadata_failure
    ld s4, 56(sp)
    beqz s4, metadata_failure
    ld t0, 80(sp)
    andi t0, t0, 1
    beqz t0, metadata_failure
    open_file data_path
    bltz a0, metadata_failure
    mv s0, a0
    mv a1, sp
    invoke 44
    expect 0
    ld t0, 56(sp)
    bne t0, s4, metadata_failure
    mv a0, s0
    invoke 57
    expect 0
    la a0, directory_path
    mv a1, sp
    invoke 43
    expect 0
    ld t0, 0(sp)
    li t1, 0xef53
    bne t0, t1, metadata_failure
    ld s5, 56(sp)
    beq s5, s4, metadata_failure
    ld s6, 16(sp)
    beqz s6, metadata_failure
    ld t0, 24(sp)
    bltu s6, t0, metadata_failure
    ld t1, 32(sp)
    bltu t0, t1, metadata_failure
    ld t0, 40(sp)
    ld t1, 48(sp)
    bltu t0, t1, metadata_failure
    li a0, -100
    la a1, directory_path
    li a2, 0x10000
    invoke 56
    bltz a0, metadata_failure
    mv s0, a0
    mv a1, sp
    invoke 44
    expect 0
    ld t0, 56(sp)
    bne t0, s5, metadata_failure
    mv a0, s0
    invoke 57
    expect 0
    la a0, data_path
    li a1, -100
    la a2, statfs_link
    invoke 36
    expect 0
    la a0, statfs_link
    mv a1, sp
    invoke 43
    expect 0
    ld t0, 56(sp)
    bne t0, s4, metadata_failure
    li a0, -100
    la a1, statfs_link
    li a2, 0
    invoke 35
    expect 0
    la a0, directory_path
    invoke 49
    expect 0
    la a0, relative_path
    mv a1, sp
    invoke 43
    expect 0
    ld t0, 56(sp)
    bne t0, s5, metadata_failure
    la a0, statfs_root
    invoke 49
    expect 0
    li a0, -100
    la a1, statfs_path
    li a2, 0x42
    li a3, 0600
    invoke 56
    bltz a0, metadata_failure
    mv s0, a0
    li a0, -100
    la a1, statfs_path
    li a2, 0
    invoke 35
    expect 0
    mv a0, s0
    mv a1, sp
    invoke 44
    expect 0
    ld t0, 56(sp)
    bne t0, s5, metadata_failure
    ld t0, 16(sp)
    bne t0, s6, metadata_failure
    la a0, statfs_path
    mv a1, sp
    invoke 43
    expect -2
    mv a0, s0
    invoke 57
    expect 0
    li a0, 999
    mv a1, sp
    invoke 44
    expect -9
    la a0, data_path
    li a1, 0
    invoke 43
    expect -14
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
directory_path: .asciz "/var"
relative_path: .asciz "linux-stat"
empty_path: .asciz ""
missing_path: .asciz "/etc/absent"
statfs_link: .asciz "/var/linux-statfs-link"
statfs_root: .asciz "/"
statfs_path: .asciz "/var/linux-statfs"
.section .note.GNU-stack,"",@progbits
