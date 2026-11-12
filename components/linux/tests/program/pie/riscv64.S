# SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

# Entry point for a static PIE: confirm the startup auxv describes this image.

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
1:
    ld t2, 0(t1)
    ld t3, 8(t1)
    addi t1, t1, 16
    beqz t2, 2f
    li t4, 3
    beq t2, t4, 3f
    li t4, 9
    beq t2, t4, 4f
    j 1b
3:
    mv a2, t3
    j 1b
4:
    mv a3, t3
    j 1b
2:
    # AT_PHDR must address the program header table and AT_ENTRY this entry.
    beqz a2, 5f
    lw t2, 0(a2)
    li t3, 1
    bne t2, t3, 5f
    bne a3, t0, 5f
    li a0, 42
    li a7, 93
    ecall
5:
    li a0, 1
    li a7, 93
    ecall
.section .note.GNU-stack,"",@progbits
