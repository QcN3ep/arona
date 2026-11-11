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
    bne a0, t0, path_failure
.endm
.global files_path
files_path:
    addi sp, sp, -512
    mv a0, sp
    li a1, 512
    invoke 17
    expect 2
    lbu t0, 0(sp)
    li t1, 47
    bne t0, t1, path_failure
    lbu t0, 1(sp)
    bnez t0, path_failure
    li a0, -100
    la a1, root_path
    li a2, 488
    invoke 34
    expect 0
    li a0, -100
    la a1, root_path
    li a2, 65536
    invoke 56
    expect 0
    mv s0, a0
    mv a0, s0
    la a1, child_name
    li a2, 493
    invoke 34
    expect 0
    mv a0, s0
    la a1, child_name
    li a2, 65536
    invoke 56
    expect 3
    mv s1, a0
    mv a0, s1
    invoke 50
    expect 0
    mv a0, sp
    li a1, 512
    invoke 17
    expect 22
    li a0, -100
    la a1, data_name
    li a2, 194
    li a3, 384
    invoke 56
    expect 4
    mv s2, a0
    mv a0, s2
    la a1, payload
    li a2, 3
    invoke 64
    expect 3
    mv a0, s0
    la a1, child_data
    mv a2, sp
    li a3, 0
    invoke 79
    expect 0
    ld s3, 8(sp)
    ld t0, 48(sp)
    li t1, 3
    bne t0, t1, path_failure
    mv a0, s0
    la a1, child_name
    mv a2, s0
    la a3, renamed_name
    invoke 38
    expect 0
    mv a0, s0
    la a1, child_name
    li a2, 493
    invoke 34
    expect 0
    mv a0, s0
    la a1, child_data
    li a2, 194
    li a3, 384
    invoke 56
    expect 5
    li a0, 5
    invoke 57
    expect 0
    mv a0, s1
    la a1, data_name
    li a2, 0
    invoke 56
    expect 5
    li a0, 5
    mv a1, sp
    invoke 80
    expect 0
    ld t0, 8(sp)
    bne t0, s3, path_failure
    li a0, 5
    invoke 57
    expect 0
    mv a0, sp
    li a1, 512
    invoke 17
    expect 24
    addi t0, sp, 16
    lbu t0, 0(t0)
    li t1, 114
    bne t0, t1, path_failure
    la a0, link_target
    mv a1, s1
    la a2, link_name
    invoke 36
    expect 0
    mv a0, s1
    la a1, link_name
    mv a2, sp
    li a3, 0
    invoke 79
    expect 0
    ld t0, 8(sp)
    bne t0, s3, path_failure
    mv a0, s1
    la a1, link_name
    mv a2, sp
    li a3, 256
    invoke 79
    expect 0
    lw t0, 16(sp)
    li t1, 0170000
    and t0, t0, t1
    li t1, 0120000
    bne t0, t1, path_failure
    mv a0, s1
    la a1, link_name
    mv a2, sp
    li a3, 4
    invoke 78
    expect 4
    lw t0, 0(sp)
    li t1, 0x722f2e2e
    bne t0, t1, path_failure
    mv a0, s1
    la a1, link_name
    li a2, 131072
    invoke 56
    expect -40
    mv a0, s1
    la a1, link_name
    li a2, 193
    li a3, 384
    invoke 56
    expect -17
    la a0, loop_name
    mv a1, s1
    la a2, loop_name
    invoke 36
    expect 0
    mv a0, s1
    la a1, loop_name
    li a2, 0
    invoke 56
    expect -40
    mv a0, s1
    la a1, not_directory
    li a2, 0
    invoke 56
    expect -20
    mv a0, s1
    la a1, data_name
    mv a2, s0
    la a3, hard_name
    li a4, 0
    invoke 37
    expect 0
    mv a0, s1
    la a1, data_name
    li a2, 0
    invoke 35
    expect 0
    mv a0, s0
    la a1, hard_name
    mv a2, sp
    li a3, 0
    invoke 79
    expect 0
    ld t0, 8(sp)
    bne t0, s3, path_failure
    mv a0, s0
    la a1, renamed_name
    li a2, 0
    invoke 35
    expect -21
    mv a0, s0
    la a1, renamed_name
    li a2, 512
    invoke 35
    expect -39
    mv a0, sp
    li a1, 1
    invoke 17
    expect -34
    li a0, 0
    li a1, 512
    invoke 17
    expect -14
    li a0, 999
    la a1, data_name
    li a2, 0
    invoke 56
    expect -9
    li a0, 999
    la a1, archive_data
    li a2, 0
    invoke 56
    expect 5
    li a0, 5
    invoke 57
    expect 0
    la a0, parent_name
    invoke 49
    expect 0
    mv a0, sp
    li a1, 512
    invoke 17
    expect 16
    mv a0, s1
    invoke 50
    expect 0
    mv a0, s1
    invoke 57
    expect 0
    mv a0, sp
    li a1, 512
    invoke 17
    expect 24
    la a0, slash
    invoke 49
    expect 0
    mv a0, s0
    la a1, renamed_link
    li a2, 0
    invoke 35
    expect 0
    mv a0, s0
    la a1, renamed_loop
    li a2, 0
    invoke 35
    expect 0
    mv a0, s2
    invoke 57
    expect 0
    mv a0, s0
    la a1, hard_name
    li a2, 0
    invoke 35
    expect 0
    mv a0, s0
    la a1, child_data
    li a2, 0
    invoke 35
    expect 0
    mv a0, s0
    la a1, child_name
    li a2, 512
    invoke 35
    expect 0
    mv a0, s0
    la a1, renamed_name
    li a2, 512
    invoke 35
    expect 0
    mv a0, s0
    invoke 57
    expect 0
    li a0, -100
    la a1, root_path
    li a2, 512
    invoke 35
    expect 0
    la a0, var_path
    invoke 49
    expect 0
    la a0, parent_name
    invoke 49
    expect 0
    mv a0, sp
    li a1, 512
    invoke 17
    expect 2
    li a0, 42
    invoke 94
path_failure:
    addi a0, s11, 100
    invoke 94
    j path_failure
.section .rodata
root_path: .asciz "/var/linux-path"
var_path: .asciz "/var"
child_name: .asciz "child"
renamed_name: .asciz "renamed"
data_name: .asciz "data"
child_data: .asciz "child/data"
link_name: .asciz "link"
link_target: .asciz "../renamed/data"
loop_name: .asciz "loop"
not_directory: .asciz "data/../missing"
hard_name: .asciz "hard"
parent_name: .asciz ".."
slash: .asciz "/"
archive_data: .asciz "/etc/linux-data"
renamed_link: .asciz "renamed/link"
renamed_loop: .asciz "renamed/loop"
payload: .asciz "xyz"
.section .note.GNU-stack,"",@progbits
