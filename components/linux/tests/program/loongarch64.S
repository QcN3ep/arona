# SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

.section .text
.global _start
_start:
    andi $t0, $sp, 15
    bnez $t0, failure
    ld.d $t0, $sp, 0
    li.d $t1, 2
    bne $t0, $t1, failure
    ld.d $t0, $sp, 24
    bnez $t0, failure
    ld.d $t0, $sp, 32
    ld.bu $t0, $t0, 0
    li.d $t1, 65
    bne $t0, $t1, failure
    ld.d $t0, $sp, 40
    bnez $t0, failure
    addi.d $t0, $sp, 48
    li.d $s2, 0
auxv:
    ld.d $t1, $t0, 0
    ld.d $t2, $t0, 8
    addi.d $t0, $t0, 16
    beqz $t1, auxv_done
    li.d $t3, 3
    beq $t1, $t3, aux_phdr
    li.d $t3, 4
    beq $t1, $t3, aux_phent
    li.d $t3, 5
    beq $t1, $t3, aux_phnum
    li.d $t3, 6
    beq $t1, $t3, aux_pagesz
    li.d $t3, 9
    beq $t1, $t3, aux_entry
    li.d $t3, 31
    beq $t1, $t3, aux_execfn
    b auxv
aux_phdr:
    ld.w $t3, $t2, 0
    beqz $t3, failure
    ori $s2, $s2, 1
    b auxv
aux_phent:
    li.d $t3, 56
    bne $t2, $t3, failure
    ori $s2, $s2, 2
    b auxv
aux_phnum:
    beqz $t2, failure
    ori $s2, $s2, 4
    b auxv
aux_pagesz:
    li.d $t3, 4096
    bne $t2, $t3, failure
    ori $s2, $s2, 8
    b auxv
aux_entry:
    la.local $t3, _start
    bne $t2, $t3, failure
    ori $s2, $s2, 16
    b auxv
aux_execfn:
    ld.bu $t3, $t2, 0
    li.d $t4, 47
    bne $t3, $t4, failure
    ori $s2, $s2, 32
    b auxv
auxv_done:
    li.d $t0, 63
    bne $s2, $t0, failure
    la.local $t0, zeroed
    ld.d $t0, $t0, 0
    bnez $t0, failure
    ld.d $t0, $sp, 16
    ld.bu $s3, $t0, 0
    li.d $t1, 102
    beq $s3, $t1, fault
    li.d $t1, 114
    beq $s3, $t1, readonly
    li.d $t1, 105
    beq $s3, $t1, illegal
    li.d $t1, 101
    beq $s3, $t1, exceptions
    li.d $t1, 103
    beq $s3, $t1, group
    li.d $t1, 119
    beq $s3, $t1, files_write
    li.d $t1, 118
    beq $s3, $t1, files_vector
    li.d $t1, 69
    beq $s3, $t1, exec_process
    li.d $t1, 90
    beq $s3, $t1, exec_replaced
    li.d $t1, 73
    beq $s3, $t1, process_identity
    li.d $t1, 85
    beq $s3, $t1, identity_replaced
    li.d $t1, 75
    beq $s3, $t1, fork_process
    li.d $t1, 86
    beq $s3, $t1, vfork_process
    li.d $t1, 74
    beq $s3, $t1, futex_basics
    li.d $t1, 72
    beq $s3, $t1, threads
    li.d $t1, 78
    beq $s3, $t1, signals
    li.d $t1, 83
    beq $s3, $t1, system_basics
    li.d $t1, 79
    beq $s3, $t1, configured_exit
    li.d $t1, 67
    beq $s3, $t1, process_terminal
    li.d $t1, 84
    beq $s3, $t1, files_terminal
    li.d $t1, 80
    beq $s3, $t1, files_pipe
    li.d $t1, 82
    beq $s3, $t1, files_poll
    li.d $t1, 86
    beq $s3, $t1, files_virtual
    li.d $t1, 77
    beq $s3, $t1, mapping_bus_fault
    li.d $t1, 70
    beq $s3, $t1, file_mappings
    li.d $t1, 112
    beq $s3, $t1, files_path
    li.d $t1, 109
    beq $s3, $t1, files_metadata
    li.d $t1, 100
    beq $s3, $t1, files_directory
    li.d $t1, 65
    beq $s3, $t1, files_append
    li.d $t1, 66
    beq $s3, $t1, files_append
    li.d $t1, 54
    bltu $s3, $t1, memory_fault
    bl heap
    li.d $t1, 115
    beq $s3, $t1, shrunk
    li.d $t1, 120
    beq $s3, $t1, execute_heap
    bl mappings
    bl files
    li.d $a7, -1
    syscall 0
    li.d $t0, -38
    bne $a0, $t0, failure
    li.d $a0, 99
    li.d $a1, 0
    li.d $a2, 1
    li.d $a7, 64
    syscall 0
    li.d $t0, -9
    bne $a0, $t0, failure
    li.d $a0, 1
    li.d $a1, 0
    li.d $a2, 1
    li.d $a7, 64
    syscall 0
    li.d $t0, -14
    bne $a0, $t0, failure
    li.d $a0, 1
    li.d $a1, -4096
    li.d $a2, 1
    li.d $a7, 64
    syscall 0
    li.d $t0, -14
    bne $a0, $t0, failure
    li.d $a0, 1
    li.d $a1, 0
    li.d $a2, 0
    li.d $a7, 64
    syscall 0
    bnez $a0, failure
    li.d $a0, 1
    li.d $a1, 0
    li.d $a2, -1
    li.d $a7, 64
    syscall 0
    li.d $t0, -22
    bne $a0, $t0, failure
    la.local $s0, message
    la.local $s1, message_end
    sub.d $s1, $s1, $s0
    li.d $s2, 0
write:
    li.d $a0, 1
    move $a1, $s0
    move $a2, $s1
    li.d $a7, 64
    syscall 0
    blez $a0, failure
    bltu $s1, $a0, failure
    add.d $s0, $s0, $a0
    sub.d $s1, $s1, $a0
    addi.d $s2, $s2, 1
    bnez $s1, write
    li.d $t0, 2
    bltu $s2, $t0, failure
    li.d $a0, 298
    li.d $a7, 93
    syscall 0
failure:
    li.d $a0, 1
    li.d $a7, 94
    syscall 0
    b failure
fault:
    ld.d $t0, $zero, 0
    b failure
readonly:
    la.local $t0, message
    st.b $zero, $t0, 0
    b failure
illegal:
    .word 0
    b failure
exceptions:
    break 0
    b failure
group:
    li.d $a0, 43
    li.d $a7, 94
    syscall 0
    b failure
shrunk:
    li.d $t0, 4096
    add.d $t0, $s4, $t0
    ld.bu $t0, $t0, 0
    b failure
execute_heap:
    jr $s4

heap:
    li.d $a0, 0
    li.d $a7, 214
    syscall 0
    move $s4, $a0
    la.local $t0, zeroed_end
    li.d $t1, 4095
    add.d $t0, $t0, $t1
    li.d $t1, -4096
    and $t0, $t0, $t1
    bltu $s4, $t0, failure
    addi.d $a0, $s4, -1
    syscall 0
    bne $a0, $s4, failure
    li.d $a0, -1
    syscall 0
    bne $a0, $s4, failure
    addi.d $s5, $s4, 1
    move $a0, $s5
    syscall 0
    bne $a0, $s5, failure
    ld.bu $t0, $s4, 0
    bnez $t0, failure
    li.d $t0, 90
    st.b $t0, $s4, 0
    li.d $t0, 107
    st.b $t0, $s4, 128
    addi.d $s5, $s4, 129
    move $a0, $s5
    syscall 0
    bne $a0, $s5, failure
    ld.bu $t0, $s4, 128
    li.d $t1, 107
    bne $t0, $t1, failure
    li.d $s5, 8193
    add.d $s5, $s4, $s5
    move $a0, $s5
    syscall 0
    bne $a0, $s5, failure
    li.d $t0, 4096
    add.d $t0, $s4, $t0
    li.d $t1, 12288
    add.d $t1, $s4, $t1
heap_new_zero:
    ld.bu $t2, $t0, 0
    bnez $t2, failure
    addi.d $t0, $t0, 1
    bne $t0, $t1, heap_new_zero
    li.d $a0, 0x7ffffffffffff000
    syscall 0
    bne $a0, $s5, failure
    li.d $a0, 0
    syscall 0
    bne $a0, $s5, failure
    ld.bu $t0, $s4, 0
    li.d $t1, 90
    bne $t0, $t1, failure
    li.d $t0, 4096
    add.d $s6, $s4, $t0
    li.d $t1, 60
    st.b $t1, $s6, 0
    add.d $s7, $s6, $t0
    li.d $t1, 125
    st.b $t1, $s7, 0
    addi.d $a0, $s6, 1
    syscall 0
    addi.d $t0, $s6, 1
    bne $a0, $t0, failure
    move $a0, $s5
    syscall 0
    bne $a0, $s5, failure
    ld.bu $t0, $s7, 0
    bnez $t0, failure
    ld.bu $t0, $s6, 0
    li.d $t1, 60
    bne $t0, $t1, failure
    move $a0, $s4
    syscall 0
    bne $a0, $s4, failure
    li.d $a0, 0
    syscall 0
    bne $a0, $s4, failure
    move $a0, $s5
    syscall 0
    bne $a0, $s5, failure
    move $t0, $s4
    li.d $t1, 12288
    add.d $t1, $s4, $t1
heap_reused_zero:
    ld.bu $t2, $t0, 0
    bnez $t2, failure
    addi.d $t0, $t0, 1
    bne $t0, $t1, heap_reused_zero
    addi.d $a0, $s4, 1
    syscall 0
    addi.d $t0, $s4, 1
    bne $a0, $t0, failure
    jr $ra

configured_exit:
    li.d $a0, 0
    li.d $a7, 94
    syscall 0
    b configured_exit
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
