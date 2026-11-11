# SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

.section .text
.macro invoke number
    li a7, \number
    ecall
.endm
.macro expect value
    li s8, \@
    li t0, \value
    bne a0, t0, child_failure
.endm
.global fork_process
fork_process:
    addi sp, sp, -256
    invoke 172
    mv s0, a0
    mv a0, sp
    li a1, 0
    invoke 59
    expect 0
    lw s1, 0(sp)
    lw s2, 4(sp)
    li a0, 0
    li a1, 4096
    li a2, 3
    li a3, 0x21
    li a4, -1
    li a5, 0
    invoke 222
    blez a0, child_failure
    mv s3, a0
    li t0, 1
    sd t0, 0(s3)
    la s4, private_word
    sd t0, 0(s4)
    li a0, 17
    li a1, 0
    li a2, 0
    li a3, 0
    li a4, 0
    invoke 220
    beqz a0, child
    bltz a0, child_failure
    mv s5, a0
    mv a0, s2
    invoke 57
    expect 0
    mv a0, s1
    addi a1, sp, 16
    li a2, 1
    invoke 63
    expect 1
    lbu t0, 16(sp)
    li t1, 67
    bne t0, t1, child_failure
    mv a0, s5
    li a1, 1
    li a2, 0
    li a3, 0
    invoke 260
    expect -14
    mv a0, s5
    addi a1, sp, 24
    li a2, 0
    li a3, 0
    invoke 260
    bne a0, s5, child_failure
    lw t0, 24(sp)
    li t1, 5888
    bne t0, t1, child_failure
    ld t0, 0(s3)
    li t1, 2
    bne t0, t1, child_failure
    ld t0, 0(s4)
    li t1, 1
    bne t0, t1, child_failure
    mv a0, s5
    li a1, 0
    li a2, 1
    li a3, 0
    invoke 260
    expect -10
    li a0, 17
    li a1, 0
    li a2, 0
    li a3, 0
    li a4, 0
    invoke 220
    beqz a0, busy_child
    bltz a0, child_failure
    mv s1, a0
    mv a0, s1
    li a1, 0
    li a2, 0
    li a3, 1
    invoke 260
    expect -14
    mv a0, s1
    mv a1, sp
    li a2, 0
    addi a3, sp, 80
    invoke 260
    bne a0, s1, child_failure
    ld t0, 80(sp)
    ld t1, 88(sp)
    or t0, t0, t1
    beqz t0, child_failure
    j busy_done
busy_child:
    li t0, 200000
1:
    addi t0, t0, -1
    bnez t0, 1b
    li a0, 0
    invoke 93
busy_done:
    li a0, 42
    invoke 94
child:
    invoke 172
    beq a0, s0, child_failure
    invoke 173
    bne a0, s0, child_failure
    li t0, 2
    sd t0, 0(s3)
    sd t0, 0(s4)
    mv a0, s1
    invoke 57
    expect 0
    li t0, 67
    sb t0, 16(sp)
    mv a0, s2
    addi a1, sp, 16
    li a2, 1
    invoke 64
    expect 1
    li a0, 23
    invoke 94
child_failure:
    addi a0, s8, 100
    invoke 94
    j child_failure
.section .text
.global vfork_process
vfork_process:
    addi sp, sp, -64
    la s4, private_word
    li t0, 1
    sd t0, 0(s4)
    li a0, 0x4111
    li a1, 0
    li a2, 0
    li a3, 0
    li a4, 0
    invoke 220
    beqz a0, vfork_shared_child
    bltz a0, child_failure
    mv s5, a0
    ld t0, 0(s4)
    li t1, 2
    bne t0, t1, child_failure
    mv a0, s5
    addi a1, sp, 24
    li a2, 0
    li a3, 0
    invoke 260
    bne a0, s5, child_failure
    lw t0, 24(sp)
    li t1, 5888
    bne t0, t1, child_failure
    li a0, 0x4111
    li a1, 0
    li a2, 0
    li a3, 0
    li a4, 0
    invoke 220
    beqz a0, vfork_exec_child
    bltz a0, child_failure
    mv s6, a0
    ld t0, 0(s4)
    li t1, 2
    bne t0, t1, child_failure
    mv a0, s6
    addi a1, sp, 24
    li a2, 0
    li a3, 0
    invoke 260
    bne a0, s6, child_failure
    lw t0, 24(sp)
    bnez t0, child_failure
    li a0, 42
    invoke 94
vfork_shared_child:
    la t0, private_word
    li t1, 2
    sd t1, 0(t0)
    li a0, 23
    invoke 94
vfork_exec_child:
    la t0, vfork_path
    la t1, vfork_argv
    sd t0, 0(t1)
    la t0, vfork_mode
    sd t0, 8(t1)
    sd zero, 16(t1)
    la t0, vfork_environment
    la t1, vfork_envp
    sd t0, 0(t1)
    sd zero, 8(t1)
    la a0, vfork_path
    la a1, vfork_argv
    la a2, vfork_envp
    invoke 221
    li a0, 99
    invoke 94
.section .rodata
vfork_path: .asciz "/bin/linux-test"
vfork_mode: .asciz "O"
vfork_environment: .asciz "ARONA_TEST=yes"
.section .bss
.balign 8
private_word: .skip 8
vfork_argv: .skip 24
vfork_envp: .skip 16
.section .note.GNU-stack,"",@progbits
