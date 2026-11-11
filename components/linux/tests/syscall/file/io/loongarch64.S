# SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

.section .text

.macro invoke number
    li.d $a7, \number
    syscall 0
.endm

.macro expect value
    li.d $s8, \@
    li.d $t0, \value
    bne $a0, $t0, io_failure
.endm

.macro open_file flags, path=write_path, mode=0644
    li.d $a0, -100
    la.pcrel $a1, \path
    li.d $a2, \flags
    li.d $a3, \mode
    invoke 56
.endm

.macro seek fd, offset=0, whence=1
    move $a0, \fd
    li.d $a1, \offset
    li.d $a2, \whence
    invoke 62
.endm

.macro put fd, bytes=pair, length=2
    move $a0, \fd
    la.pcrel $a1, \bytes
    li.d $a2, \length
    invoke 64
.endm

.global files_write
files_write:
    li.d $t0, -8192
    add.d $sp, $sp, $t0
    open_file 0xc2
    expect 0
    move $s0, $a0
    open_file 0xc2
    expect -17
    move $a0, $s0
    invoke 23
    expect 3
    move $s1, $a0
    put $s0, digits, 10
    expect 10
    seek $s1
    expect 10
    move $a0, $s0
    la.pcrel $a1, pair
    li.d $a2, 2
    li.d $a3, 2
    invoke 68
    expect 2
    seek $s1
    expect 10
    move $a0, $s1
    move $a1, $sp
    li.d $a2, 10
    li.d $a3, 0
    invoke 67
    expect 10
    ld.d $t0, $sp, 0
    li.d $t1, 0x3736353442413130
    bne $t0, $t1, io_failure
    seek $s0
    expect 10
    move $a0, $s0
    li.d $a1, 0
    li.d $a2, 4
    invoke 64
    expect -14
    seek $s0
    expect 10
    move $a0, $s0
    li.d $a1, 0
    li.d $a2, 2
    li.d $a3, 0
    invoke 68
    expect -14
    move $a0, $s0
    li.d $a1, 0
    li.d $a2, 0
    invoke 64
    expect 0
    move $a0, $s0
    la.pcrel $a1, pair
    li.d $a2, 2
    li.d $a3, -1
    invoke 68
    expect -22
    move $a0, $s0
    la.pcrel $a1, pair
    li.d $a2, 2
    li.d $a3, 0x7fffffffffffffff
    invoke 68
    expect -27
    move $a0, $s0
    li.d $a1, -1
    invoke 46
    expect -22
    move $a0, $s0
    li.d $a1, 2147483649
    invoke 46
    expect -27
    move $a0, $s0
    li.d $a1, 4
    invoke 46
    expect 0
    seek $s1
    expect 10
    move $a0, $s0
    li.d $a1, 16
    invoke 46
    expect 0
    move $a0, $s1
    move $a1, $sp
    li.d $a2, 16
    li.d $a3, 0
    invoke 67
    expect 16
    ld.w $t0, $sp, 0
    li.d $t1, 0x42413130
    bne $t0, $t1, io_failure
    ld.w $t0, $sp, 4
    bnez $t0, io_failure
    ld.d $t0, $sp, 8
    bnez $t0, io_failure

    open_file 0x401
    expect 4
    move $s2, $a0
    seek $s2, 0, 0
    expect 0
    put $s2, suffix
    expect 2
    seek $s2
    expect 18
    move $a0, $s2
    la.pcrel $a1, pair
    li.d $a2, 2
    li.d $a3, 0
    invoke 68
    expect 2
    seek $s2
    expect 18
    open_file 0x401
    expect 5
    move $s3, $a0
    put $s3, suffix
    expect 2
    seek $s2, 0, 2
    expect 22
    move $a0, $s3
    invoke 57
    expect 0
    move $a0, $s2
    invoke 57
    expect 0

    move $a0, $s1
    li.d $a1, 4
    li.d $a2, 0x400
    invoke 25
    expect 0
    move $a0, $s0
    li.d $a1, 3
    invoke 25
    expect 0x402
    put $s1, suffix
    expect 2
    seek $s0
    expect 24
    move $a0, $s0
    li.d $a1, 4
    li.d $a2, 0
    invoke 25
    expect 0
    move $a0, $s1
    li.d $a1, 3
    invoke 25
    expect 2
    put $s1
    expect 2
    seek $s0
    expect 26
    open_file 0
    expect 4
    move $s2, $a0
    put $s2
    expect -9
    move $a0, $s2
    li.d $a1, 0
    invoke 46
    expect -22
    move $a0, $s2
    invoke 82
    expect 0
    move $a0, $s2
    invoke 57
    expect 0
    open_file 0x42
    expect 4
    move $s2, $a0
    seek $s2, 0, 2
    expect 26
    move $a0, $s2
    invoke 57
    expect 0
    open_file 0x202
    expect 4
    move $s2, $a0
    seek $s2, 0, 2
    expect 0
    seek $s0
    expect 26
    move $a0, $s2
    invoke 57
    expect 0

    li.d $a0, 077
    invoke 166
    expect 022
    open_file 0xc2, new_path, 0666
    expect 4
    move $s2, $a0
    li.d $a0, 022
    invoke 166
    expect 077
    move $a0, $s2
    invoke 57
    expect 0
    open_file 0x101002
    expect 4
    move $s2, $a0
    put $s2
    expect 2
    move $a0, $s2
    invoke 83
    expect 0
    move $a0, $s2
    invoke 57
    expect 0
    open_file 0, directory
    expect 4
    move $s2, $a0
    invoke 82
    expect 0
    move $a0, $s2
    invoke 57
    expect 0
    open_file 1, directory
    expect -21
    li.d $a0, 1
    invoke 82
    expect -22
    li.d $a0, -1
    invoke 83
    expect -9

    li.d $a0, 0
    li.d $a1, 8192
    li.d $a2, 3
    li.d $a3, 0x22
    li.d $a4, -1
    li.d $a5, 0
    invoke 222
    blez $a0, io_failure
    move $s4, $a0
    li.d $t0, 4096
    add.d $s5, $s4, $t0
    li.d $t0, 0x7978
    st.h $t0, $s5, -2
    move $a0, $s5
    li.d $a1, 4096
    li.d $a2, 0
    invoke 226
    expect 0
    move $a0, $s0
    addi.d $a1, $s5, -2
    li.d $a2, 8
    invoke 64
    expect 2
    seek $s0
    expect 28
    move $a0, $s0
    move $a1, $s5
    li.d $a2, 8
    invoke 64
    expect -14
    seek $s0
    expect 28
    move $a0, $s0
    move $a1, $sp
    li.d $a2, 28
    li.d $a3, 0
    invoke 67
    expect 28
    ld.hu $t0, $sp, 0
    li.d $t1, 0x4241
    bne $t0, $t1, io_failure
    ld.hu $t0, $sp, 26
    li.d $t1, 0x7978
    bne $t0, $t1, io_failure
    move $a0, $s0
    invoke 82
    expect 0
    li.d $a0, 42
    invoke 94
    b io_failure

.global files_append
files_append:
    addi.d $sp, $sp, -128
    addi.d $s0, $sp, 64
    addi.d $t0, $sp, 128
.Lfill:
    st.b $s3, $s0, 0
    addi.d $s0, $s0, 1
    bne $s0, $t0, .Lfill
    addi.d $t0, $sp, 64
    li.d $t1, 16
    st.d $t0, $sp, 0
    st.d $t1, $sp, 8
    addi.d $t0, $t0, 16
    st.d $t0, $sp, 16
    st.d $t1, $sp, 24
    addi.d $t0, $t0, 16
    st.d $t0, $sp, 32
    st.d $t1, $sp, 40
    addi.d $t0, $t0, 16
    st.d $t0, $sp, 48
    st.d $t1, $sp, 56
    open_file 0x401, append_path
    expect 0
    move $s0, $a0
    li.d $s1, 8
.Lappend:
    move $a0, $s0
    move $a1, $sp
    li.d $a2, 4
    invoke 66
    expect 64
    addi.d $s1, $s1, -1
    bnez $s1, .Lappend
    li.d $a0, 42
    invoke 94
io_failure:
    addi.d $a0, $s8, 10
    invoke 94
    b io_failure

.section .rodata
 write_path: .asciz "/var/linux-write"
 new_path: .asciz "/var/linux-new"
 append_path: .asciz "/var/linux-append"
 directory: .asciz "/var"
 digits: .ascii "0123456789"
 pair: .ascii "AB"
 suffix: .ascii "xy"

.section .note.GNU-stack,"",@progbits
