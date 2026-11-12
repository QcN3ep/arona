# SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

# Interpreter entry: confirm that AT_PHDR/AT_ENTRY describe the main image and
# that AT_BASE names this interpreter mapping.

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
    move $a4, $zero
1:
    ld.d $t2, $t1, 0
    ld.d $t3, $t1, 8
    addi.d $t1, $t1, 16
    beqz $t2, 2f
    li.w $t4, 3
    beq $t2, $t4, 3f
    li.w $t4, 7
    beq $t2, $t4, 4f
    li.w $t4, 9
    beq $t2, $t4, 5f
    b 1b
3:
    move $a2, $t3
    b 1b
4:
    move $a4, $t3
    b 1b
5:
    move $a3, $t3
    b 1b
2:
    # The main entry follows the program headers in the checked fixture layout.
    beqz $a2, 6f
    beqz $a4, 6f
    ld.w $t2, $a2, 0
    li.w $t3, 1
    bne $t2, $t3, 6f
    sub.d $t2, $a3, $a2
    li.w $t3, 0xc0
    bne $t2, $t3, 6f
    addi.d $t2, $t0, -0x100
    bne $a4, $t2, 6f
    li.w $a0, 42
    li.w $a7, 93
    syscall 0
6:
    li.w $a0, 1
    li.w $a7, 93
    syscall 0
.section .note.GNU-stack,"",@progbits
