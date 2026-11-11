# SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

.section .text
.macro invoke number
    li.d $a7, \number
    syscall 0
.endm
.macro expect value
    li.d $s8, \@
    li.d $t0, \value
    bne $a0, $t0, mapping_failure
.endm
.global file_mappings
file_mappings:
    addi.d $sp, $sp, -512
    li.d $a0, -100
    la.pcrel $a1, data_path
    li.d $a2, 0
    invoke 56
    expect 0
    move $s0, $a0
    li.d $a0, 0
    li.d $a1, 12288
    li.d $a2, 3
    li.d $a3, 2
    move $a4, $s0
    li.d $a5, 0
    invoke 222
    bltz $a0, mapping_failure
    move $s1, $a0
    ld.bu $t0, $s1, 0
    li.d $t1, 48
    bne $t0, $t1, mapping_failure
    li.d $t0, 8192
    add.d $t0, $s1, $t0
    ld.bu $t1, $t0, 0
    li.d $t2, 90
    bne $t1, $t2, mapping_failure
    ld.bu $t1, $t0, 1
    bnez $t1, mapping_failure
    li.d $t1, 120
    st.b $t1, $s1, 0
    move $a0, $s1
    li.d $a1, 12288
    li.d $a2, 4
    invoke 227
    expect 0
    move $a0, $s1
    li.d $a1, 4096
    li.d $a2, 1
    invoke 227
    expect 0
    move $a0, $s1
    li.d $a1, 4096
    li.d $a2, 2
    invoke 227
    expect 0
    move $a0, $s1
    li.d $a1, 0
    li.d $a2, 4
    invoke 227
    expect 0
    move $a0, $s1
    li.d $a1, 4096
    li.d $a2, 8
    invoke 227
    expect -22
    addi.d $a0, $s1, 1
    li.d $a1, 4096
    li.d $a2, 4
    invoke 227
    expect -22
    move $a0, $s1
    li.d $a1, 4096
    li.d $a2, 5
    invoke 227
    expect -22
    li.d $a0, 0x40000000
    li.d $a1, 4096
    li.d $a2, 4
    invoke 227
    expect -12
    move $a0, $s0
    li.d $a1, 0
    li.d $a2, 1
    invoke 62
    expect 0
    move $a0, $s0
    move $a1, $sp
    li.d $a2, 1
    li.d $a3, 0
    invoke 67
    expect 1
    ld.bu $t0, $sp, 0
    li.d $t1, 48
    bne $t0, $t1, mapping_failure
    move $a0, $s0
    invoke 57
    expect 0
    move $a0, $s1
    li.d $a1, 12288
    li.d $a2, 0
    invoke 226
    expect 0
    move $a0, $s1
    li.d $a1, 12288
    li.d $a2, 3
    invoke 226
    expect 0
    ld.bu $t0, $s1, 0
    li.d $t1, 120
    bne $t0, $t1, mapping_failure
    move $a0, $s1
    li.d $a1, 12288
    invoke 215
    expect 0
    li.d $a0, -100
    la.pcrel $a1, data_path
    li.d $a2, 0
    invoke 56
    expect 0
    move $s0, $a0
    li.d $a0, 0
    li.d $a1, 4096
    li.d $a2, 1
    li.d $a3, 2
    move $a4, $s0
    li.d $a5, 4096
    invoke 222
    bltz $a0, mapping_failure
    move $s1, $a0
    ld.bu $t0, $s1, 0
    li.d $t1, 48
    bne $t0, $t1, mapping_failure
    move $a0, $s1
    li.d $a1, 4096
    invoke 215
    expect 0
    li.d $a0, 0
    li.d $a1, 4096
    li.d $a2, 3
    li.d $a3, 2
    li.d $a4, 999
    li.d $a5, 0
    invoke 222
    expect -9
    li.d $a0, 0
    li.d $a1, 4096
    li.d $a2, 3
    li.d $a3, 2
    li.d $a4, 1
    li.d $a5, 0
    invoke 222
    expect -19
    li.d $a0, 0
    li.d $a1, 4096
    li.d $a2, 3
    li.d $a3, 1
    move $a4, $s0
    li.d $a5, 0
    invoke 222
    expect -95
    li.d $a0, 0
    li.d $a1, 4096
    li.d $a2, 3
    li.d $a3, 2
    move $a4, $s0
    li.d $a5, 1
    invoke 222
    expect -22
    move $a0, $s0
    invoke 57
    expect 0
    li.d $a0, 0
    li.d $a1, 8192
    li.d $a2, 3
    li.d $a3, 34
    li.d $a4, -1
    li.d $a5, 0
    invoke 222
    bltz $a0, mapping_failure
    move $s1, $a0
    li.d $t0, 77
    st.b $t0, $s1, 0
    move $a0, $s1
    li.d $a1, 8192
    li.d $a2, 4
    invoke 233
    expect 0
    ld.bu $t0, $s1, 0
    li.d $t1, 0
    bne $t0, $t1, mapping_failure
    move $a0, $s1
    li.d $a1, 8192
    invoke 215
    expect 0
    li.d $a0, 0
    li.d $a1, 8192
    li.d $a2, 3
    li.d $a3, 33
    li.d $a4, -1
    li.d $a5, 0
    invoke 222
    bltz $a0, mapping_failure
    move $s1, $a0
    li.d $t0, 77
    st.b $t0, $s1, 0
    move $a0, $s1
    li.d $a1, 8192
    li.d $a2, 4
    invoke 233
    expect 0
    ld.bu $t0, $s1, 0
    li.d $t1, 77
    bne $t0, $t1, mapping_failure
    move $a0, $s1
    li.d $a1, 8192
    invoke 215
    expect 0
    li.d $a0, 0
    li.d $a1, 4096
    li.d $a2, 3
    li.d $a3, 34
    li.d $a4, -1
    li.d $a5, 0
    invoke 222
    bltz $a0, mapping_failure
    move $s2, $a0
    li.d $t0, 85
    st.b $t0, $s2, 0
    move $a0, $s2
    li.d $a1, 4096
    li.d $a2, 3
    li.d $a3, 50
    li.d $a4, -1
    li.d $a5, 0
    invoke 222
    bne $a0, $s2, mapping_failure
    ld.bu $t0, $s2, 0
    bnez $t0, mapping_failure
    li.d $a0, 0
    li.d $a1, 8192
    li.d $a2, 3
    li.d $a3, 34
    li.d $a4, -1
    li.d $a5, 0
    invoke 222
    bltz $a0, mapping_failure
    move $s3, $a0
    li.d $t0, 66
    st.b $t0, $s3, 0
    li.d $t0, 4096
    add.d $t0, $s3, $t0
    li.d $t1, 77
    st.b $t1, $t0, 0
    li.d $a0, 4096
    add.d $a0, $s3, $a0
    li.d $a1, 4096
    li.d $a2, 3
    li.d $a3, 50
    li.d $a4, -1
    li.d $a5, 0
    invoke 222
    li.d $t0, 4096
    add.d $t0, $s3, $t0
    bne $a0, $t0, mapping_failure
    ld.bu $t0, $s3, 0
    li.d $t1, 66
    bne $t0, $t1, mapping_failure
    li.d $t0, 4096
    add.d $t0, $s3, $t0
    ld.bu $t1, $t0, 0
    bnez $t1, mapping_failure
    li.d $a0, 16384
    add.d $a0, $s3, $a0
    li.d $a1, 4096
    li.d $a2, 3
    li.d $a3, 50
    li.d $a4, -1
    li.d $a5, 0
    invoke 222
    li.d $t0, 16384
    add.d $t0, $s3, $t0
    bne $a0, $t0, mapping_failure
    ld.bu $t0, $a0, 0
    bnez $t0, mapping_failure
    move $a0, $s2
    li.d $a1, 4096
    li.d $a2, 3
    li.d $a3, 0x100032
    li.d $a4, -1
    li.d $a5, 0
    invoke 222
    expect -22
    addi.d $a0, $s2, 1
    li.d $a1, 4096
    li.d $a2, 3
    li.d $a3, 50
    li.d $a4, -1
    li.d $a5, 0
    invoke 222
    expect -22
    li.d $a0, -100
    la.pcrel $a1, data_path
    li.d $a2, 0
    invoke 56
    expect 0
    move $s4, $a0
    li.d $a0, 0
    li.d $a1, 4096
    li.d $a2, 1
    li.d $a3, 2
    move $a4, $s4
    li.d $a5, 0
    invoke 222
    bltz $a0, mapping_failure
    move $s5, $a0
    ld.bu $t0, $s5, 0
    li.d $t1, 48
    bne $t0, $t1, mapping_failure
    move $a0, $s5
    li.d $a1, 4096
    li.d $a2, 1
    li.d $a3, 50
    li.d $a4, -1
    li.d $a5, 0
    invoke 222
    bne $a0, $s5, mapping_failure
    ld.bu $t0, $s5, 0
    bnez $t0, mapping_failure
    move $a0, $s4
    invoke 57
    expect 0
    li.d $a0, 42
    invoke 94
.global mapping_bus_fault
mapping_bus_fault:
    addi.d $sp, $sp, -512
    li.d $a0, -100
    la.pcrel $a1, data_path
    li.d $a2, 0
    invoke 56
    expect 0
    move $s0, $a0
    li.d $a0, 0
    li.d $a1, 16384
    li.d $a2, 3
    li.d $a3, 2
    move $a4, $s0
    li.d $a5, 0
    invoke 222
    bltz $a0, mapping_failure
    li.d $t0, 12288
    add.d $t0, $a0, $t0
    ld.bu $t0, $t0, 0
    b mapping_failure
mapping_failure:
    addi.d $a0, $s8, 100
    invoke 94
    b mapping_failure
.section .rodata
data_path: .asciz "/etc/linux-data"
.section .note.GNU-stack,"",@progbits
