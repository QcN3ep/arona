# SPDX-License-Identifier: MPL-2.0

.section .text
.macro invoke number
    li.d $a7, \number
    syscall 0
.endm
.macro expect value
    li.d $s8, \@
    li.d $t0, \value
    bne $a0, $t0, directory_failure
.endm
.macro seek offset, whence=0
    move $a0, $s0
    li.d $a1, \offset
    li.d $a2, \whence
    invoke 62
    expect \offset
.endm
.macro read_dir fd, size
    move $a0, \fd
    move $a1, $sp
    li.d $a2, \size
    invoke 61
.endm
.global files_directory
files_directory:
    addi.d $sp, $sp, -512
    li.d $a0, -100
    la.pcrel $a1, root_path
    li.d $a2, 0x10000
    invoke 56
    expect 0
    move $s0, $a0
    invoke 23
    expect 3
    move $s1, $a0
    move $a0, $s0
    move $a1, $sp
    invoke 80
    expect 0
    ld.d $s2, $sp, 8
    read_dir $s0, 1
    expect -22
    seek 0, 1
    move $a0, $s0
    li.d $a1, 0
    li.d $a2, 24
    invoke 61
    expect -14
    seek 0, 1
    read_dir $s0, 24
    expect 24
    ld.d $t0, $sp, 0
    bne $t0, $s2, directory_failure
    ld.d $t0, $sp, 8
    li.d $t1, 1
    bne $t0, $t1, directory_failure
    ld.hu $t0, $sp, 16
    li.d $t1, 24
    bne $t0, $t1, directory_failure
    ld.bu $t0, $sp, 18
    li.d $t1, 4
    bne $t0, $t1, directory_failure
    ld.bu $t0, $sp, 19
    li.d $t1, 46
    bne $t0, $t1, directory_failure
    ld.bu $t0, $sp, 20
    bnez $t0, directory_failure
    read_dir $s1, 24
    expect 24
    ld.d $t0, $sp, 8
    li.d $t1, 2
    bne $t0, $t1, directory_failure
    ld.bu $t0, $sp, 19
    li.d $t1, 46
    bne $t0, $t1, directory_failure
    ld.bu $t0, $sp, 20
    bne $t0, $t1, directory_failure
    move $a0, $s0
    li.d $a1, 0
    li.d $a2, 1
    invoke 62
    expect 2
    li.d $s3, 0
.Ldirectory_loop:
    read_dir $s0, 512
    bltz $a0, directory_failure
    beqz $a0, .Ldirectory_done
    move $t2, $sp
    add.d $t3, $sp, $a0
.Ldirectory_entry:
    ld.d $t0, $t2, 0
    beqz $t0, directory_failure
    ld.hu $t0, $t2, 16
    andi $t1, $t0, 7
    bnez $t1, directory_failure
    li.d $t1, 24
    bltu $t0, $t1, directory_failure
    ld.d $t1, $t2, 8
    bgeu $s3, $t1, directory_failure
    move $s3, $t1
    add.d $t2, $t2, $t0
    bltu $t2, $t3, .Ldirectory_entry
    bne $t2, $t3, directory_failure
    b .Ldirectory_loop
.Ldirectory_done:
    li.d $t0, 5
    bltu $s3, $t0, directory_failure
    read_dir $s1, 512
    expect 0
    seek 0
    read_dir $s1, 24
    expect 24
    ld.d $t0, $sp, 8
    li.d $t1, 1
    bne $t0, $t1, directory_failure
    move $a0, $s0
    li.d $a1, -1
    li.d $a2, 0
    invoke 62
    expect -22
    move $a0, $s0
    li.d $a1, 0
    li.d $a2, 2
    invoke 62
    expect -22
    li.d $a0, 1
    move $a1, $sp
    li.d $a2, 512
    invoke 61
    expect -20
    li.d $a0, -1
    move $a1, $sp
    li.d $a2, 512
    invoke 61
    expect -9
    move $a0, $s0
    invoke 57
    expect 0
    move $a0, $s1
    invoke 57
    expect 0
    li.d $a0, 42
    invoke 94
directory_failure:
    addi.d $a0, $s8, 10
    invoke 94
    b directory_failure
.section .rodata
root_path: .asciz "/var"
.section .note.GNU-stack,"",@progbits
