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
.global remap_mappings
remap_mappings:
    addi.d $sp, $sp, -64

    # Clear the anonymous test window.
    li.d $a0, 0x1000000000
    li.d $a1, 65536
    invoke 215
    li.d $a0, 0x1000000000
    li.d $a1, 8192
    li.d $a2, 3
    li.d $a3, 0x32
    li.d $a4, -1
    li.d $a5, 0
    invoke 222
    li.d $t0, 0x1000000000
    bne $a0, $t0, mapping_failure
    move $s0, $a0
    li.d $t1, 0x11
    st.b $t1, $s0, 0
    li.d $t0, 4096
    add.d $t2, $s0, $t0
    li.d $t1, 0x22
    st.b $t1, $t2, 0

    # Grow in place and observe zeroed pages.
    move $a0, $s0
    li.d $a1, 8192
    li.d $a2, 16384
    li.d $a3, 0
    li.d $a4, 0
    invoke 216
    bne $a0, $s0, mapping_failure
    ld.bu $t0, $s0, 0
    li.d $t1, 0x11
    bne $t0, $t1, mapping_failure
    li.d $t0, 4096
    add.d $t2, $s0, $t0
    ld.bu $t0, $t2, 0
    li.d $t1, 0x22
    bne $t0, $t1, mapping_failure
    li.d $t0, 8192
    add.d $t2, $s0, $t0
    ld.bu $t0, $t2, 0
    bnez $t0, mapping_failure
    li.d $t1, 0x33
    st.b $t1, $t2, 0

    # Shrink in place and check the tail is unmapped.
    move $a0, $s0
    li.d $a1, 16384
    li.d $a2, 8192
    li.d $a3, 0
    li.d $a4, 0
    invoke 216
    bne $a0, $s0, mapping_failure
    ld.bu $t0, $s0, 0
    li.d $t1, 0x11
    bne $t0, $t1, mapping_failure
    li.d $t0, 8192
    add.d $a0, $s0, $t0
    li.d $a1, 4096
    li.d $a2, 4
    invoke 227
    expect -12
    move $a0, $s0
    li.d $a1, 16384
    li.d $a2, 8192
    li.d $a3, 0
    li.d $a4, 0
    invoke 216
    expect -14

    # Shrink to one page and block in-place growth.
    move $a0, $s0
    li.d $a1, 8192
    li.d $a2, 4096
    li.d $a3, 0
    li.d $a4, 0
    invoke 216
    bne $a0, $s0, mapping_failure
    li.d $a0, 0x1000001000
    li.d $a1, 4096
    li.d $a2, 3
    li.d $a3, 0x32
    li.d $a4, -1
    li.d $a5, 0
    invoke 222
    li.d $t0, 0x1000001000
    bne $a0, $t0, mapping_failure
    li.d $t1, 0x44
    st.b $t1, $a0, 0
    move $a0, $s0
    li.d $a1, 4096
    li.d $a2, 8192
    li.d $a3, 0
    li.d $a4, 0
    invoke 216
    expect -12
    ld.bu $t0, $s0, 0
    li.d $t1, 0x11
    bne $t0, $t1, mapping_failure
    li.d $t0, 0x1000001000
    ld.bu $t0, $t0, 0
    li.d $t1, 0x44
    bne $t0, $t1, mapping_failure

    # MAYMOVE relocates and preserves the data.
    move $a0, $s0
    li.d $a1, 4096
    li.d $a2, 8192
    li.d $a3, 1
    li.d $a4, 0
    invoke 216
    bltz $a0, mapping_failure
    beq $a0, $s0, mapping_failure
    move $s1, $a0
    ld.bu $t0, $s1, 0
    li.d $t1, 0x11
    bne $t0, $t1, mapping_failure
    li.d $t0, 4096
    add.d $t2, $s1, $t0
    ld.bu $t0, $t2, 0
    bnez $t0, mapping_failure
    move $a0, $s0
    li.d $a1, 4096
    li.d $a2, 4
    invoke 227
    expect -12
    move $a0, $s1
    li.d $a1, 8192
    li.d $a2, 8192
    li.d $a3, 0
    li.d $a4, 0
    invoke 216
    bne $a0, $s1, mapping_failure
    li.d $t0, 0x1000001000
    ld.bu $t0, $t0, 0
    li.d $t1, 0x44
    bne $t0, $t1, mapping_failure

    # MREMAP_FIXED moves over an existing destination mapping.
    li.d $a0, 0x1000100000
    li.d $a1, 8192
    invoke 215
    li.d $a0, 0x1000100000
    li.d $a1, 4096
    li.d $a2, 3
    li.d $a3, 0x32
    li.d $a4, -1
    li.d $a5, 0
    invoke 222
    li.d $t0, 0x1000100000
    bne $a0, $t0, mapping_failure
    li.d $t1, 0x55
    st.b $t1, $a0, 0
    move $a0, $s1
    li.d $a1, 8192
    li.d $a2, 8192
    li.d $a3, 3
    li.d $a4, 0x1000100000
    invoke 216
    li.d $t0, 0x1000100000
    bne $a0, $t0, mapping_failure
    move $s2, $a0
    ld.bu $t0, $s2, 0
    li.d $t1, 0x11
    bne $t0, $t1, mapping_failure
    li.d $t0, 4096
    add.d $t2, $s2, $t0
    ld.bu $t0, $t2, 0
    bnez $t0, mapping_failure
    move $a0, $s1
    li.d $a1, 8192
    li.d $a2, 4
    invoke 227
    expect -12

    # Reject invalid flags, sizes, addresses, and overlapping destinations.
    move $a0, $s2
    li.d $a1, 8192
    li.d $a2, 8192
    li.d $a3, 8
    li.d $a4, 0
    invoke 216
    expect -22
    move $a0, $s2
    li.d $a1, 8192
    li.d $a2, 8192
    li.d $a3, 2
    li.d $a4, 0
    invoke 216
    expect -22
    move $a0, $s2
    li.d $a1, 8192
    li.d $a2, 8192
    li.d $a3, 3
    move $a4, $s2
    invoke 216
    expect -22
    move $a0, $s2
    li.d $a1, 8192
    li.d $a2, 8192
    li.d $a3, 3
    li.d $t0, 4096
    add.d $a4, $s2, $t0
    invoke 216
    expect -22
    move $a0, $s2
    li.d $a1, 8192
    li.d $a2, 8192
    li.d $a3, 3
    li.d $t0, 1
    add.d $a4, $s2, $t0
    invoke 216
    expect -22
    move $a0, $s2
    li.d $a1, 8192
    li.d $a2, 8192
    li.d $a3, 3
    li.d $a4, 0
    invoke 216
    expect -22
    li.d $t0, 1
    add.d $a0, $s2, $t0
    li.d $a1, 8192
    li.d $a2, 8192
    li.d $a3, 1
    li.d $a4, 0
    invoke 216
    expect -22
    move $a0, $s2
    li.d $a1, 8192
    li.d $a2, 0
    li.d $a3, 1
    li.d $a4, 0
    invoke 216
    expect -22
    move $a0, $s2
    li.d $a1, 0
    li.d $a2, 8192
    li.d $a3, 1
    li.d $a4, 0
    invoke 216
    expect -22
    move $a0, $s2
    li.d $a1, 8192
    li.d $a2, 8192
    li.d $a3, 5
    li.d $a4, 0
    invoke 216
    expect -95
    li.d $a0, 0x1000400000
    li.d $a1, 4096
    li.d $a2, 8192
    li.d $a3, 1
    li.d $a4, 0
    invoke 216
    expect -14
    move $a0, $s2
    li.d $a1, 8192
    li.d $a2, 8192
    li.d $a3, 3
    li.d $a4, 0x4000000000
    invoke 216
    expect -22
    ld.bu $t0, $s2, 0
    li.d $t1, 0x11
    bne $t0, $t1, mapping_failure
    move $a0, $s2
    li.d $a1, 8192
    li.d $a2, 4
    invoke 227
    expect 0

    # MREMAP_FIXED growth zero-fills the extension.
    li.d $a0, 0x1000050000
    li.d $a1, 65536
    invoke 215
    move $a0, $s2
    li.d $a1, 8192
    li.d $a2, 16384
    li.d $a3, 3
    li.d $a4, 0x1000050000
    invoke 216
    li.d $t0, 0x1000050000
    bne $a0, $t0, mapping_failure
    move $s3, $a0
    ld.bu $t0, $s3, 0
    li.d $t1, 0x11
    bne $t0, $t1, mapping_failure
    li.d $t0, 4096
    add.d $t2, $s3, $t0
    ld.bu $t0, $t2, 0
    bnez $t0, mapping_failure
    li.d $t0, 8192
    add.d $t2, $s3, $t0
    ld.bu $t0, $t2, 0
    bnez $t0, mapping_failure
    li.d $t0, 12288
    add.d $t2, $s3, $t0
    ld.bu $t0, $t2, 0
    bnez $t0, mapping_failure
    move $a0, $s2
    li.d $a1, 8192
    li.d $a2, 4
    invoke 227
    expect -12

    # MREMAP_FIXED shrink moves only the retained prefix.
    li.d $a0, 0x1000060000
    li.d $a1, 65536
    invoke 215
    move $a0, $s3
    li.d $a1, 16384
    li.d $a2, 8192
    li.d $a3, 3
    li.d $a4, 0x1000060000
    invoke 216
    li.d $t0, 0x1000060000
    bne $a0, $t0, mapping_failure
    move $s4, $a0
    ld.bu $t0, $s4, 0
    li.d $t1, 0x11
    bne $t0, $t1, mapping_failure
    li.d $t0, 4096
    add.d $t2, $s4, $t0
    ld.bu $t0, $t2, 0
    bnez $t0, mapping_failure
    move $a0, $s3
    li.d $a1, 16384
    li.d $a2, 4
    invoke 227
    expect -12
    move $a0, $s4
    li.d $a1, 8192
    invoke 215
    expect 0

    # Shrink, move, and reject growth for private file mappings.
    li.d $a0, 0x1000200000
    li.d $a1, 65536
    invoke 215
    li.d $a0, -100
    la.pcrel $a1, data_path
    li.d $a2, 0
    invoke 56
    bltz $a0, mapping_failure
    move $s5, $a0
    li.d $a0, 0x1000200000
    li.d $a1, 12288
    li.d $a2, 3
    li.d $a3, 0x12
    move $a4, $s5
    li.d $a5, 0
    invoke 222
    li.d $t0, 0x1000200000
    bne $a0, $t0, mapping_failure
    move $s6, $a0
    ld.bu $t0, $s6, 0
    li.d $t1, 48
    bne $t0, $t1, mapping_failure
    li.d $t0, 4096
    add.d $t2, $s6, $t0
    ld.bu $t0, $t2, 0
    li.d $t1, 48
    bne $t0, $t1, mapping_failure
    li.d $t0, 8192
    add.d $t2, $s6, $t0
    ld.bu $t0, $t2, 0
    li.d $t1, 90
    bne $t0, $t1, mapping_failure
    ld.bu $t0, $t2, 1
    bnez $t0, mapping_failure
    move $a0, $s6
    li.d $a1, 8193
    li.d $a2, 8192
    li.d $a3, 0
    li.d $a4, 0
    invoke 216
    bne $a0, $s6, mapping_failure
    ld.bu $t0, $s6, 0
    li.d $t1, 48
    bne $t0, $t1, mapping_failure
    li.d $t0, 8192
    add.d $a0, $s6, $t0
    li.d $a1, 4096
    li.d $a2, 4
    invoke 227
    expect -12
    move $a0, $s6
    li.d $a1, 8192
    li.d $a2, 12288
    li.d $a3, 0
    li.d $a4, 0
    invoke 216
    expect -12
    ld.bu $t0, $s6, 0
    li.d $t1, 48
    bne $t0, $t1, mapping_failure
    # A file mapping cannot grow through MREMAP_FIXED either.
    li.d $a0, 0x1000350000
    li.d $a1, 4096
    li.d $a2, 3
    li.d $a3, 0x32
    li.d $a4, -1
    li.d $a5, 0
    invoke 222
    li.d $t0, 0x1000350000
    bne $a0, $t0, mapping_failure
    li.d $t1, 0x66
    st.b $t1, $a0, 0
    move $a0, $s6
    li.d $a1, 8192
    li.d $a2, 12288
    li.d $a3, 3
    li.d $a4, 0x1000350000
    invoke 216
    expect -12
    li.d $t0, 0x1000350000
    ld.bu $t0, $t0, 0
    li.d $t1, 0x66
    bne $t0, $t1, mapping_failure
    ld.bu $t0, $s6, 0
    li.d $t1, 48
    bne $t0, $t1, mapping_failure

    # Moving a file mapping preserves its contents.
    li.d $a0, 0x1000300000
    li.d $a1, 8192
    invoke 215
    move $a0, $s6
    li.d $a1, 8192
    li.d $a2, 8192
    li.d $a3, 3
    li.d $a4, 0x1000300000
    invoke 216
    li.d $t0, 0x1000300000
    bne $a0, $t0, mapping_failure
    move $s7, $a0
    ld.bu $t0, $s7, 0
    li.d $t1, 48
    bne $t0, $t1, mapping_failure
    li.d $t0, 4096
    add.d $t2, $s7, $t0
    ld.bu $t0, $t2, 0
    li.d $t1, 48
    bne $t0, $t1, mapping_failure
    move $a0, $s6
    li.d $a1, 8192
    li.d $a2, 4
    invoke 227
    expect -12

    move $a0, $s5
    invoke 57
    expect 0
    move $a0, $s7
    li.d $a1, 8192
    invoke 215
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
    move $s1, $a0
    li.d $a0, 0x1000500000
    li.d $a1, 16384
    invoke 215
    move $a0, $s1
    li.d $a1, 16384
    li.d $a2, 16384
    li.d $a3, 3
    li.d $a4, 0x1000500000
    invoke 216
    li.d $t0, 0x1000500000
    bne $a0, $t0, mapping_failure
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
