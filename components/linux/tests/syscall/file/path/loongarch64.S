# SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

.section .text
.macro invoke number
    li.d $a7, \number
    syscall 0
.endm
.macro expect value
    li.d $s8, \@
    li.d $t0, \value
    bne $a0, $t0, path_failure
.endm
.global files_path
files_path:
    addi.d $sp, $sp, -512
    move $a0, $sp
    li.d $a1, 512
    invoke 17
    expect 2
    ld.bu $t0, $sp, 0
    li.d $t1, 47
    bne $t0, $t1, path_failure
    ld.bu $t0, $sp, 1
    bnez $t0, path_failure
    li.d $a0, -100
    la.pcrel $a1, root_path
    li.d $a2, 488
    invoke 34
    expect 0
    li.d $a0, -100
    la.pcrel $a1, root_path
    li.d $a2, 65536
    invoke 56
    expect 0
    move $s0, $a0
    move $a0, $s0
    la.pcrel $a1, child_name
    li.d $a2, 493
    invoke 34
    expect 0
    move $a0, $s0
    la.pcrel $a1, child_name
    li.d $a2, 65536
    invoke 56
    expect 3
    move $s1, $a0
    move $a0, $s1
    invoke 50
    expect 0
    move $a0, $sp
    li.d $a1, 512
    invoke 17
    expect 22
    li.d $a0, -100
    la.pcrel $a1, data_name
    li.d $a2, 194
    li.d $a3, 384
    invoke 56
    expect 4
    move $s2, $a0
    move $a0, $s2
    la.pcrel $a1, payload
    li.d $a2, 3
    invoke 64
    expect 3
    move $a0, $s0
    la.pcrel $a1, child_data
    move $a2, $sp
    li.d $a3, 0
    invoke 79
    expect 0
    ld.d $s3, $sp, 8
    ld.d $t0, $sp, 48
    li.d $t1, 3
    bne $t0, $t1, path_failure
    move $a0, $s0
    la.pcrel $a1, child_name
    move $a2, $s0
    la.pcrel $a3, renamed_name
    invoke 38
    expect 0
    move $a0, $s0
    la.pcrel $a1, child_name
    li.d $a2, 493
    invoke 34
    expect 0
    move $a0, $s0
    la.pcrel $a1, child_data
    li.d $a2, 194
    li.d $a3, 384
    invoke 56
    expect 5
    li.d $a0, 5
    invoke 57
    expect 0
    move $a0, $s1
    la.pcrel $a1, data_name
    li.d $a2, 0
    invoke 56
    expect 5
    li.d $a0, 5
    move $a1, $sp
    invoke 80
    expect 0
    ld.d $t0, $sp, 8
    bne $t0, $s3, path_failure
    li.d $a0, 5
    invoke 57
    expect 0
    move $a0, $sp
    li.d $a1, 512
    invoke 17
    expect 24
    addi.d $t0, $sp, 16
    ld.bu $t0, $t0, 0
    li.d $t1, 114
    bne $t0, $t1, path_failure
    la.pcrel $a0, link_target
    move $a1, $s1
    la.pcrel $a2, link_name
    invoke 36
    expect 0
    move $a0, $s1
    la.pcrel $a1, link_name
    move $a2, $sp
    li.d $a3, 0
    invoke 79
    expect 0
    ld.d $t0, $sp, 8
    bne $t0, $s3, path_failure
    move $a0, $s1
    la.pcrel $a1, link_name
    move $a2, $sp
    li.d $a3, 256
    invoke 79
    expect 0
    ld.w $t0, $sp, 16
    li.d $t1, 0170000
    and $t0, $t0, $t1
    li.d $t1, 0120000
    bne $t0, $t1, path_failure
    move $a0, $s1
    la.pcrel $a1, link_name
    move $a2, $sp
    li.d $a3, 4
    invoke 78
    expect 4
    ld.w $t0, $sp, 0
    li.d $t1, 0x722f2e2e
    bne $t0, $t1, path_failure
    move $a0, $s1
    la.pcrel $a1, link_name
    li.d $a2, 131072
    invoke 56
    expect -40
    move $a0, $s1
    la.pcrel $a1, link_name
    li.d $a2, 193
    li.d $a3, 384
    invoke 56
    expect -17
    la.pcrel $a0, loop_name
    move $a1, $s1
    la.pcrel $a2, loop_name
    invoke 36
    expect 0
    move $a0, $s1
    la.pcrel $a1, loop_name
    li.d $a2, 0
    invoke 56
    expect -40
    move $a0, $s1
    la.pcrel $a1, not_directory
    li.d $a2, 0
    invoke 56
    expect -20
    move $a0, $s1
    la.pcrel $a1, data_name
    move $a2, $s0
    la.pcrel $a3, hard_name
    li.d $a4, 0
    invoke 37
    expect 0
    move $a0, $s1
    la.pcrel $a1, data_name
    li.d $a2, 0
    invoke 35
    expect 0
    move $a0, $s0
    la.pcrel $a1, hard_name
    move $a2, $sp
    li.d $a3, 0
    invoke 79
    expect 0
    ld.d $t0, $sp, 8
    bne $t0, $s3, path_failure
    move $a0, $s0
    la.pcrel $a1, renamed_name
    li.d $a2, 0
    invoke 35
    expect -21
    move $a0, $s0
    la.pcrel $a1, renamed_name
    li.d $a2, 512
    invoke 35
    expect -39
    move $a0, $sp
    li.d $a1, 1
    invoke 17
    expect -34
    li.d $a0, 0
    li.d $a1, 512
    invoke 17
    expect -14
    li.d $a0, 999
    la.pcrel $a1, data_name
    li.d $a2, 0
    invoke 56
    expect -9
    li.d $a0, 999
    la.pcrel $a1, archive_data
    li.d $a2, 0
    invoke 56
    expect 5
    li.d $a0, 5
    invoke 57
    expect 0
    la.pcrel $a0, parent_name
    invoke 49
    expect 0
    move $a0, $sp
    li.d $a1, 512
    invoke 17
    expect 16
    move $a0, $s1
    invoke 50
    expect 0
    move $a0, $s1
    invoke 57
    expect 0
    move $a0, $sp
    li.d $a1, 512
    invoke 17
    expect 24
    la.pcrel $a0, slash
    invoke 49
    expect 0
    move $a0, $s0
    la.pcrel $a1, renamed_link
    li.d $a2, 0x220000
    invoke 56
    bltz $a0, path_failure
    move $s4, $a0
    move $a0, $s4
    move $a1, $sp
    invoke 80
    expect 0
    ld.w $t0, $sp, 16
    li.d $t1, 0170000
    and $t0, $t0, $t1
    li.d $t1, 0120000
    bne $t0, $t1, path_failure
    ld.d $s5, $sp, 8
    move $a0, $s0
    la.pcrel $a1, renamed_link
    li.d $a2, 0
    invoke 35
    expect 0
    move $a0, $s4
    move $a1, $sp
    invoke 80
    expect 0
    ld.d $t0, $sp, 8
    bne $t0, $s5, path_failure
    ld.w $t0, $sp, 20
    bnez $t0, path_failure
    move $a0, $s4
    la.pcrel $a1, empty_path
    move $a2, $sp
    li.d $a3, 512
    invoke 78
    expect 15
    ld.w $t0, $sp, 0
    li.d $t1, 0x722f2e2e
    bne $t0, $t1, path_failure
    move $a0, $s4
    invoke 57
    expect 0
    move $a0, $s0
    la.pcrel $a1, renamed_loop
    li.d $a2, 0
    invoke 35
    expect 0
    move $a0, $s2
    invoke 57
    expect 0
    move $a0, $s0
    la.pcrel $a1, hard_name
    li.d $a2, 0
    invoke 35
    expect 0
    move $a0, $s0
    la.pcrel $a1, child_data
    li.d $a2, 0
    invoke 35
    expect 0
    move $a0, $s0
    la.pcrel $a1, child_name
    li.d $a2, 512
    invoke 35
    expect 0
    move $a0, $s0
    la.pcrel $a1, renamed_name
    li.d $a2, 512
    invoke 35
    expect 0
    move $a0, $s0
    invoke 57
    expect 0
    li.d $a0, -100
    la.pcrel $a1, root_path
    li.d $a2, 512
    invoke 35
    expect 0
    la.pcrel $a0, var_path
    invoke 49
    expect 0
    la.pcrel $a0, parent_name
    invoke 49
    expect 0
    move $a0, $sp
    li.d $a1, 512
    invoke 17
    expect 2
    li.d $a0, 42
    invoke 94
path_failure:
    addi.d $a0, $s8, 100
    invoke 94
    b path_failure
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
empty_path: .asciz ""
payload: .asciz "xyz"
.section .note.GNU-stack,"",@progbits
