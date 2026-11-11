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
    bne a0, t0, directory_failure
.endm
.macro seek offset, whence=0
    mv a0, s0
    li a1, \offset
    li a2, \whence
    invoke 62
    expect \offset
.endm
.macro read_dir fd, size
    mv a0, \fd
    mv a1, sp
    li a2, \size
    invoke 61
.endm
.global files_directory
files_directory:
    addi sp, sp, -512
    li a0, -100
    la a1, root_path
    li a2, 0x10000
    invoke 56
    expect 0
    mv s0, a0
    invoke 23
    expect 3
    mv s1, a0
    mv a0, s0
    mv a1, sp
    invoke 80
    expect 0
    ld s2, 8(sp)
    read_dir s0, 1
    expect -22
    seek 0, 1
    mv a0, s0
    li a1, 0
    li a2, 24
    invoke 61
    expect -14
    seek 0, 1
    read_dir s0, 24
    expect 24
    ld t0, 0(sp)
    bne t0, s2, directory_failure
    ld t0, 8(sp)
    li t1, 1
    bne t0, t1, directory_failure
    lhu t0, 16(sp)
    li t1, 24
    bne t0, t1, directory_failure
    lbu t0, 18(sp)
    li t1, 4
    bne t0, t1, directory_failure
    lbu t0, 19(sp)
    li t1, 46
    bne t0, t1, directory_failure
    lbu t0, 20(sp)
    bnez t0, directory_failure
    read_dir s1, 24
    expect 24
    ld t0, 8(sp)
    li t1, 2
    bne t0, t1, directory_failure
    lbu t0, 19(sp)
    li t1, 46
    bne t0, t1, directory_failure
    lbu t0, 20(sp)
    bne t0, t1, directory_failure
    mv a0, s0
    li a1, 0
    li a2, 1
    invoke 62
    expect 2
    li s3, 0
.Ldirectory_loop:
    read_dir s0, 512
    bltz a0, directory_failure
    beqz a0, .Ldirectory_done
    mv t2, sp
    add t3, sp, a0
.Ldirectory_entry:
    ld t0, 0(t2)
    beqz t0, directory_failure
    lhu t0, 16(t2)
    andi t1, t0, 7
    bnez t1, directory_failure
    li t1, 24
    bltu t0, t1, directory_failure
    ld t1, 8(t2)
    bgeu s3, t1, directory_failure
    mv s3, t1
    add t2, t2, t0
    bltu t2, t3, .Ldirectory_entry
    bne t2, t3, directory_failure
    j .Ldirectory_loop
.Ldirectory_done:
    li t0, 5
    bltu s3, t0, directory_failure
    read_dir s1, 512
    expect 0
    seek 0
    read_dir s1, 24
    expect 24
    ld t0, 8(sp)
    li t1, 1
    bne t0, t1, directory_failure
    mv a0, s0
    li a1, -1
    li a2, 0
    invoke 62
    expect -22
    mv a0, s0
    li a1, 0
    li a2, 2
    invoke 62
    expect -22
    li a0, 1
    mv a1, sp
    li a2, 512
    invoke 61
    expect -20
    li a0, -1
    mv a1, sp
    li a2, 512
    invoke 61
    expect -9
    mv a0, s0
    invoke 57
    expect 0
    mv a0, s1
    invoke 57
    expect 0
    li a0, 42
    invoke 94
directory_failure:
    addi a0, s11, 10
    invoke 94
    j directory_failure
.section .rodata
root_path: .asciz "/var"
.section .note.GNU-stack,"",@progbits
