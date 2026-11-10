# SPDX-License-Identifier: MPL-2.0

.section .text
.global _start
.option norelax
_start:
    andi t0, sp, 15
    bnez t0, failure
    ld t0, 0(sp)
    li t1, 2
    bne t0, t1, failure
    ld t0, 24(sp)
    bnez t0, failure
    ld t0, 32(sp)
    lbu t0, 0(t0)
    li t1, 65
    bne t0, t1, failure
    ld t0, 40(sp)
    bnez t0, failure
    addi t0, sp, 48
    li s2, 0
auxv:
    ld t1, 0(t0)
    ld t2, 8(t0)
    addi t0, t0, 16
    beqz t1, auxv_done
    li t3, 3
    beq t1, t3, aux_phdr
    li t3, 4
    beq t1, t3, aux_phent
    li t3, 5
    beq t1, t3, aux_phnum
    li t3, 6
    beq t1, t3, aux_pagesz
    li t3, 9
    beq t1, t3, aux_entry
    li t3, 31
    beq t1, t3, aux_execfn
    j auxv
aux_phdr:
    lw t3, 0(t2)
    beqz t3, failure
    ori s2, s2, 1
    j auxv
aux_phent:
    li t3, 56
    bne t2, t3, failure
    ori s2, s2, 2
    j auxv
aux_phnum:
    beqz t2, failure
    ori s2, s2, 4
    j auxv
aux_pagesz:
    li t3, 4096
    bne t2, t3, failure
    ori s2, s2, 8
    j auxv
aux_entry:
    la t3, _start
    bne t2, t3, failure
    ori s2, s2, 16
    j auxv
aux_execfn:
    lbu t3, 0(t2)
    li t4, 47
    bne t3, t4, failure
    ori s2, s2, 32
    j auxv
auxv_done:
    li t0, 63
    bne s2, t0, failure
    la t0, zeroed
    ld t0, 0(t0)
    bnez t0, failure
    ld t0, 16(sp)
    lbu s3, 0(t0)
    li t1, 102
    beq s3, t1, fault
    li t1, 114
    beq s3, t1, readonly
    li t1, 105
    beq s3, t1, illegal
    li t1, 103
    beq s3, t1, group
    li t1, 119
    beq s3, t1, files_write
    li t1, 118
    beq s3, t1, files_vector
    li t1, 83
    beq s3, t1, system_basics
    li t1, 84
    beq s3, t1, files_terminal
    li t1, 80
    beq s3, t1, files_pipe
    li t1, 82
    beq s3, t1, files_poll
    li t1, 77
    beq s3, t1, mapping_bus_fault
    li t1, 70
    beq s3, t1, file_mappings
    li t1, 112
    beq s3, t1, files_path
    li t1, 109
    beq s3, t1, files_metadata
    li t1, 100
    beq s3, t1, files_directory
    li t1, 65
    beq s3, t1, files_append
    li t1, 66
    beq s3, t1, files_append
    li t1, 54
    bltu s3, t1, memory_fault
    jal ra, heap
    li t1, 115
    beq s3, t1, shrunk
    li t1, 120
    beq s3, t1, execute_heap
    jal ra, mappings
    jal ra, files
    li a7, -1
    ecall
    li t0, -38
    bne a0, t0, failure
    li a0, 99
    li a1, 0
    li a2, 1
    li a7, 64
    ecall
    li t0, -9
    bne a0, t0, failure
    li a0, 1
    li a1, 0
    li a2, 1
    li a7, 64
    ecall
    li t0, -14
    bne a0, t0, failure
    li a0, 1
    li a1, -4096
    li a2, 1
    li a7, 64
    ecall
    li t0, -14
    bne a0, t0, failure
    li a0, 1
    li a1, 0
    li a2, 0
    li a7, 64
    ecall
    bnez a0, failure
    li a0, 1
    li a1, 0
    li a2, -1
    li a7, 64
    ecall
    li t0, -22
    bne a0, t0, failure
    la s0, message
    li s1, message_end - message
    li s2, 0
write:
    li a0, 1
    mv a1, s0
    mv a2, s1
    li a7, 64
    ecall
    blez a0, failure
    bgtu a0, s1, failure
    add s0, s0, a0
    sub s1, s1, a0
    addi s2, s2, 1
    bnez s1, write
    li t0, 2
    bltu s2, t0, failure
    li a0, 298
    li a7, 93
    ecall
failure:
    li a0, 1
    li a7, 94
    ecall
    j failure
fault:
    ld t0, 0(zero)
    j failure
readonly:
    la t0, message
    sb zero, 0(t0)
    j failure
illegal:
    .word 0
    j failure
group:
    li a0, 43
    li a7, 94
    ecall
    j failure
shrunk:
    li t0, 4096
    add t0, s4, t0
    lbu t0, 0(t0)
    j failure
execute_heap:
    jr s4

heap:
    li a0, 0
    li a7, 214
    ecall
    mv s4, a0
    la t0, zeroed_end
    li t1, 4095
    add t0, t0, t1
    li t1, -4096
    and t0, t0, t1
    bltu s4, t0, failure
    addi a0, s4, -1
    ecall
    bne a0, s4, failure
    li a0, -1
    ecall
    bne a0, s4, failure
    addi s5, s4, 1
    mv a0, s5
    ecall
    bne a0, s5, failure
    lbu t0, 0(s4)
    bnez t0, failure
    li t0, 90
    sb t0, 0(s4)
    li t0, 107
    sb t0, 128(s4)
    addi s5, s4, 129
    mv a0, s5
    ecall
    bne a0, s5, failure
    lbu t0, 128(s4)
    li t1, 107
    bne t0, t1, failure
    li s5, 8193
    add s5, s4, s5
    mv a0, s5
    ecall
    bne a0, s5, failure
    li t0, 4096
    add t0, s4, t0
    li t1, 12288
    add t1, s4, t1
heap_new_zero:
    lbu t2, 0(t0)
    bnez t2, failure
    addi t0, t0, 1
    bne t0, t1, heap_new_zero
    li a0, 0x7ffffffffffff000
    ecall
    bne a0, s5, failure
    li a0, 0
    ecall
    bne a0, s5, failure
    lbu t0, 0(s4)
    li t1, 90
    bne t0, t1, failure
    li t0, 4096
    add s6, s4, t0
    li t1, 60
    sb t1, 0(s6)
    add s7, s6, t0
    li t1, 125
    sb t1, 0(s7)
    addi a0, s6, 1
    ecall
    addi t0, s6, 1
    bne a0, t0, failure
    mv a0, s5
    ecall
    bne a0, s5, failure
    lbu t0, 0(s7)
    bnez t0, failure
    lbu t0, 0(s6)
    li t1, 60
    bne t0, t1, failure
    mv a0, s4
    ecall
    bne a0, s4, failure
    li a0, 0
    ecall
    bne a0, s4, failure
    mv a0, s5
    ecall
    bne a0, s5, failure
    mv t0, s4
    li t1, 12288
    add t1, s4, t1
heap_reused_zero:
    lbu t2, 0(t0)
    bnez t2, failure
    addi t0, t0, 1
    bne t0, t1, heap_reused_zero
    addi a0, s4, 1
    ecall
    addi t0, s4, 1
    bne a0, t0, failure
    ret

.section .rodata
message:
    .ascii "Linux ABI guest write\nLinux partial write: "
    .rept 256
    .ascii "#"
    .endr
    .ascii "\n"
message_end:

.section .bss
.balign 4096
zeroed:
    .zero 4097
zeroed_end:

.section .note.GNU-stack,"",@progbits
