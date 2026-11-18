# SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

.section .text
.global _start
_start:
    la.pcrel $s0, mutable_data
    li.d $t0, 73
    st.d $t0, $s0, 0
    la.pcrel $a0, uname_buffer
    li.d $a7, 160
    syscall 0
    bnez $a0, failure
    la.pcrel $t0, uname_buffer
    ld.bu $t0, $t0, 0
    li.d $t1, 76
    bne $t0, $t1, failure
    li.d $a0, 0
    li.d $a1, 4096
    li.d $a2, 7
    li.d $a3, 0x22
    li.d $a4, -1
    li.d $a5, 0
    li.d $a7, 222
    syscall 0
    bltz $a0, failure
    move $s1, $a0
    la.pcrel $t0, code
    ld.d $t0, $t0, 0
    st.d $t0, $s1, 0
    ibar 0
    jirl $ra, $s1, 0
    li.d $t0, 77
    bne $a0, $t0, failure
    li.d $a0, 17
    li.d $a1, 0
    li.d $a2, 0
    li.d $a3, 0
    li.d $a4, 0
    li.d $a7, 220
    syscall 0
    bltz $a0, failure
    beqz $a0, child
    move $s2, $a0
    addi.d $a1, $sp, -16
    li.d $a2, 0
    li.d $a3, 0
    li.d $a7, 260
    syscall 0
    bne $a0, $s2, failure
    ld.w $t0, $sp, -16
    li.d $t1, 10752
    bne $t0, $t1, failure
    ld.d $t0, $s0, 0
    li.d $t1, 73
    bne $t0, $t1, failure
    b success
child:
    li.d $t0, 91
    st.d $t0, $s0, 0
    jirl $ra, $s1, 0
    li.d $t0, 77
    bne $a0, $t0, failure
success:
    li.d $a0, 42
    li.d $a7, 94
    syscall 0
failure:
    li.d $a0, 1
    li.d $a7, 94
    syscall 0
.balign 8
code:
    addi.d $a0, $zero, 77
    jr $ra
mutable_data: .quad 0
uname_buffer: .zero 392
.section .note.GNU-stack,"",@progbits
