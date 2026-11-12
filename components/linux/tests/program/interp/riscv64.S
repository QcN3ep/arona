# SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

# Interpreter entry: confirm that AT_PHDR/AT_ENTRY describe the main image and
# that AT_BASE names this interpreter mapping.

.section .text
.global _start
_start:
    auipc t0, 0
    ld t1, 0(sp)
    addi t1, t1, 3
    slli t1, t1, 3
    add t1, sp, t1
    li a2, 0
    li a3, 0
    li a4, 0
1:
    ld t2, 0(t1)
    ld t3, 8(t1)
    addi t1, t1, 16
    beqz t2, 2f
    li t4, 3
    beq t2, t4, 3f
    li t4, 7
    beq t2, t4, 4f
    li t4, 9
    beq t2, t4, 5f
    j 1b
3:
    mv a2, t3
    j 1b
4:
    mv a4, t3
    j 1b
5:
    mv a3, t3
    j 1b
2:
    # The main entry follows the program headers in the checked fixture layout.
    beqz a2, 6f
    beqz a4, 6f
    lw t2, 0(a2)
    li t3, 1
    bne t2, t3, 6f
    sub t2, a3, a2
    li t3, 0xc0
    bne t2, t3, 6f
    addi t2, t0, -0x100
    bne a4, t2, 6f
    li a0, 42
    li a7, 93
    ecall
6:
    li a0, 1
    li a7, 93
    ecall
.section .note.GNU-stack,"",@progbits
