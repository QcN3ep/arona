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
.global files_metadata
files_metadata:
    addi.d $sp, $sp, -512
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
empty_path: .asciz ""
missing_path: .asciz "/etc/absent"
.section .note.GNU-stack,"",@progbits
