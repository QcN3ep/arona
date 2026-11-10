# SPDX-License-Identifier: MPL-2.0

.section .text
.macro invoke number
    li.d $a7, \number
    syscall 0
.endm
.macro expect value
    li.d $s8, \@
    li.d $t0, \value
    bne $a0, $t0, metadata_failure
.endm
.macro open_file path
    li.d $a0, -100
    la.pcrel $a1, \path
    li.d $a2, 0
    invoke 56
.endm
.macro statx_file path, flags=0, mask=0x7ff, directory=-100
    li.d $a0, \directory
    la.pcrel $a1, \path
    li.d $a2, \flags
    li.d $a3, \mask
    addi.d $a4, $sp, 256
    invoke 291
.endm
.global files_metadata
files_metadata:
    addi.d $sp, $sp, -528
    open_file data_path
    expect 0
    move $s0, $a0
    move $a1, $sp
    invoke 80
    expect 0
    ld.d $s1, $sp, 0
    beqz $s1, metadata_failure
    ld.d $s2, $sp, 8
    beqz $s2, metadata_failure
    ld.w $t0, $sp, 16
    li.d $t1, 0100644
    bne $t0, $t1, metadata_failure
    ld.d $t0, $sp, 48
    li.d $t1, 8193
    bne $t0, $t1, metadata_failure
    ld.w $t0, $sp, 56
    li.d $t1, 4096
    bne $t0, $t1, metadata_failure
    ld.d $t0, $sp, 120
    bnez $t0, metadata_failure
    move $a0, $s0
    li.d $a1, 0
    invoke 80
    expect -14
    move $a0, $s0
    la.pcrel $a1, empty_path
    addi.d $a2, $sp, 128
    li.d $a3, 0x1000
    invoke 79
    expect 0
    ld.d $t0, $sp, 128
    bne $t0, $s1, metadata_failure
    ld.d $t0, $sp, 136
    bne $t0, $s2, metadata_failure
    move $a0, $s0
    invoke 57
    expect 0
    li.d $a0, 999
    la.pcrel $a1, data_path
    move $a2, $sp
    li.d $a3, 0
    invoke 79
    expect 0
    ld.d $t0, $sp, 8
    bne $t0, $s2, metadata_failure
    li.d $a0, -100
    la.pcrel $a1, missing_path
    move $a2, $sp
    li.d $a3, 0
    invoke 79
    expect -2
    li.d $a0, -100
    la.pcrel $a1, empty_path
    move $a2, $sp
    li.d $a3, 0
    invoke 79
    expect -2
    li.d $a0, -100
    li.d $a1, 0
    move $a2, $sp
    li.d $a3, 0
    invoke 79
    expect -14
    li.d $a0, -100
    la.pcrel $a1, data_path
    move $a2, $sp
    li.d $a3, 1
    invoke 79
    expect -22
    open_file stat_path
    expect 0
    move $s0, $a0
    move $a1, $sp
    invoke 80
    expect 0
    ld.d $t0, $sp, 0
    beq $t0, $s1, metadata_failure
    move $s1, $t0
    ld.d $s2, $sp, 8
    ld.w $t0, $sp, 16
    li.d $t1, 0100600
    bne $t0, $t1, metadata_failure
    ld.w $t0, $sp, 20
    li.d $t1, 2
    bne $t0, $t1, metadata_failure
    ld.w $t0, $sp, 24
    li.d $t1, 123
    bne $t0, $t1, metadata_failure
    ld.w $t0, $sp, 28
    li.d $t1, 456
    bne $t0, $t1, metadata_failure
    ld.d $t0, $sp, 48
    li.d $t1, 4
    bne $t0, $t1, metadata_failure
    ld.d $t0, $sp, 72
    li.d $t1, -5
    bne $t0, $t1, metadata_failure
    ld.d $t0, $sp, 80
    li.d $t1, 99
    bne $t0, $t1, metadata_failure
    ld.d $t0, $sp, 96
    li.d $t1, 123
    bne $t0, $t1, metadata_failure
    ld.d $t0, $sp, 112
    li.d $t1, 321
    bne $t0, $t1, metadata_failure
    move $a0, $s0
    invoke 57
    expect 0
    li.d $a0, -100
    la.pcrel $a1, link_path
    move $a2, $sp
    li.d $a3, 0
    invoke 79
    expect 0
    ld.d $t0, $sp, 0
    bne $t0, $s1, metadata_failure
    ld.d $t0, $sp, 8
    bne $t0, $s2, metadata_failure
    li.d $a0, -100
    la.pcrel $a1, symlink_path
    move $a2, $sp
    li.d $a3, 0x100
    invoke 79
    expect 0
    ld.w $t0, $sp, 16
    li.d $t1, 0170000
    and $t0, $t0, $t1
    li.d $t1, 0120000
    bne $t0, $t1, metadata_failure
    li.d $a0, 1
    move $a1, $sp
    invoke 80
    expect 0
    ld.w $t0, $sp, 16
    li.d $t1, 0170000
    and $t0, $t0, $t1
    li.d $t1, 020000
    bne $t0, $t1, metadata_failure
    li.d $a0, -1
    move $a1, $sp
    invoke 80
    expect -9
    open_file stat_path
    expect 0
    move $s0, $a0
    move $a1, $sp
    invoke 80
    expect 0
    li.d $t0, -1
    addi.d $t1, $sp, 256
    addi.d $t2, $sp, 512
statx_fill:
    st.d $t0, $t1, 0
    addi.d $t1, $t1, 8
    bne $t1, $t2, statx_fill
    st.d $t0, $sp, 512
    statx_file stat_path, 0x2000, 0x7fffffff, 999
    expect 0
    ld.w $t0, $sp, 256
    li.d $t1, 0x7ff
    bne $t0, $t1, metadata_failure
    ld.w $t0, $sp, 260
    li.d $t1, 4096
    bne $t0, $t1, metadata_failure
    ld.d $t0, $sp, 264
    bnez $t0, metadata_failure
    ld.w $t0, $sp, 272
    li.d $t1, 2
    bne $t0, $t1, metadata_failure
    ld.w $t0, $sp, 276
    li.d $t1, 123
    bne $t0, $t1, metadata_failure
    ld.w $t0, $sp, 280
    li.d $t1, 456
    bne $t0, $t1, metadata_failure
    ld.hu $t0, $sp, 284
    li.d $t1, 0100600
    bne $t0, $t1, metadata_failure
    ld.hu $t0, $sp, 286
    bnez $t0, metadata_failure
    ld.d $t0, $sp, 288
    bne $t0, $s2, metadata_failure
    ld.d $t0, $sp, 296
    li.d $t1, 4
    bne $t0, $t1, metadata_failure
    ld.d $t0, $sp, 304
    ld.d $t1, $sp, 64
    bne $t0, $t1, metadata_failure
    ld.d $t0, $sp, 312
    bnez $t0, metadata_failure
    ld.d $t0, $sp, 320
    li.d $t1, -5
    bne $t0, $t1, metadata_failure
    ld.d $t0, $sp, 328
    li.d $t1, 99
    bne $t0, $t1, metadata_failure
    ld.d $t0, $sp, 336
    bnez $t0, metadata_failure
    ld.d $t0, $sp, 344
    bnez $t0, metadata_failure
    ld.d $t0, $sp, 352
    li.d $t1, 9
    bne $t0, $t1, metadata_failure
    ld.d $t0, $sp, 360
    li.d $t1, 321
    bne $t0, $t1, metadata_failure
    ld.d $t0, $sp, 368
    li.d $t1, 7
    bne $t0, $t1, metadata_failure
    ld.d $t0, $sp, 376
    li.d $t1, 123
    bne $t0, $t1, metadata_failure
    ld.d $t0, $sp, 384
    bnez $t0, metadata_failure
    ld.wu $t0, $sp, 392
    srli.d $t1, $s1, 8
    li.d $t2, 0xfff
    and $t1, $t1, $t2
    bne $t0, $t1, metadata_failure
    ld.wu $t0, $sp, 396
    andi $t1, $s1, 0xff
    srli.d $t2, $s1, 12
    li.d $t3, 0xffffff00
    and $t2, $t2, $t3
    or $t1, $t1, $t2
    bne $t0, $t1, metadata_failure
    addi.d $t0, $sp, 400
    addi.d $t1, $sp, 512
statx_zero:
    ld.d $t2, $t0, 0
    bnez $t2, metadata_failure
    addi.d $t0, $t0, 8
    bne $t0, $t1, statx_zero
    ld.d $t0, $sp, 512
    li.d $t1, -1
    bne $t0, $t1, metadata_failure
    move $a0, $s0
    li.d $a1, 0
    li.d $a2, 0x1000
    li.d $a3, 0
    addi.d $a4, $sp, 256
    invoke 291
    expect 0
    ld.d $t0, $sp, 288
    bne $t0, $s2, metadata_failure
    statx_file empty_path, 0x1000, 0x800, 0
    expect 0
    ld.w $t0, $sp, 256
    li.d $t1, 0x7ff
    bne $t0, $t1, metadata_failure
    ld.d $t0, $sp, 288
    bne $t0, $s2, metadata_failure
    statx_file relative_path, 0, 0x7ff, 0
    expect -20
    move $a0, $s0
    invoke 57
    expect 0
    open_file directory_path
    expect 0
    move $s0, $a0
    statx_file relative_path, 0x4000, 0x7ff, 0
    expect 0
    ld.d $t0, $sp, 288
    bne $t0, $s2, metadata_failure
    move $a0, $s0
    invoke 57
    expect 0
    statx_file empty_path, 0x1000
    expect 0
    ld.hu $t0, $sp, 284
    li.d $t1, 0170000
    and $t0, $t0, $t1
    li.d $t1, 040000
    bne $t0, $t1, metadata_failure
    statx_file symlink_path
    expect 0
    ld.d $t0, $sp, 288
    bne $t0, $s2, metadata_failure
    statx_file symlink_path, 0x100
    expect 0
    ld.hu $t0, $sp, 284
    li.d $t1, 0170000
    and $t0, $t0, $t1
    li.d $t1, 0120000
    bne $t0, $t1, metadata_failure
    ld.d $t0, $sp, 288
    beq $t0, $s2, metadata_failure
    statx_file link_path, 0x800
    expect 0
    ld.d $t0, $sp, 288
    bne $t0, $s2, metadata_failure
    statx_file empty_path, 0x1000, 0x7ff, 1
    expect 0
    ld.hu $t0, $sp, 284
    li.d $t1, 0170000
    and $t0, $t0, $t1
    li.d $t1, 020000
    bne $t0, $t1, metadata_failure
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
    li.d $a0, -100
    li.d $a1, 0
    li.d $a2, 0
    li.d $a3, 0x7ff
    addi.d $a4, $sp, 256
    invoke 291
    expect -14
    li.d $a0, -100
    la.pcrel $a1, data_path
    li.d $a2, 0
    li.d $a3, 0x7ff
    li.d $a4, 0
    invoke 291
    expect -14
    li.d $a0, 42
    invoke 94
metadata_failure:
    addi.d $a0, $s8, 10
    invoke 94
    b metadata_failure
.section .rodata
data_path: .asciz "/etc/linux-data"
stat_path: .asciz "/var/linux-stat"
link_path: .asciz "/var/linux-stat-link"
symlink_path: .asciz "/var/linux-stat-symlink"
directory_path: .asciz "/var"
relative_path: .asciz "linux-stat"
empty_path: .asciz ""
missing_path: .asciz "/etc/absent"
.section .note.GNU-stack,"",@progbits
