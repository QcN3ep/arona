# SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

# Entry point for a static PIE: confirm the startup auxv describes this image.

.section .text
.global _start
_start:
    pcaddi $t0, 0
    ld.d $t1, $sp, 0
    addi.d $t1, $t1, 3
    slli.d $t1, $t1, 3
    add.d $t1, $sp, $t1
    move $a2, $zero
    move $a3, $zero
1:
    ld.d $t2, $t1, 0
    ld.d $t3, $t1, 8
    addi.d $t1, $t1, 16
    beqz $t2, 2f
    li.w $t4, 3
    beq $t2, $t4, 3f
    li.w $t4, 9
    beq $t2, $t4, 4f
    b 1b
3:
    move $a2, $t3
    b 1b
4:
    move $a3, $t3
    b 1b
2:
    # AT_PHDR must address the program header table and AT_ENTRY this entry.
    beqz $a2, 5f
    ld.w $t2, $a2, 0
    li.w $t3, 1
    bne $t2, $t3, 5f
    bne $a3, $t0, 5f
    li.w $a0, 42
    li.w $a7, 93
    syscall 0
5:
    li.w $a0, 1
    li.w $a7, 93
    syscall 0
.section .note.GNU-stack,"",@progbits
