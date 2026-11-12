# SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

.section .text
.option norelax

.macro invoke number
    li a7, \number
    ecall
.endm

.macro expect value
    li s11, \@
    li t0, \value
    bne a0, t0, virtual_failure
.endm

.macro open_virtual path, flags=0
    li a0, -100
    la a1, \path
    li a2, \flags
    invoke 56
.endm

.macro read_fd fd, length
    mv a0, \fd
    mv a1, sp
    li a2, \length
    invoke 63
.endm

.global files_virtual
files_virtual:
    addi sp, sp, -512

    # /proc/self/status exposes the running process description.
    open_virtual status_path
    expect 0
    mv s0, a0
    read_fd s0, 256
    blez a0, virtual_failure
    mv s1, a0
    li t0, 11
    bltu s1, t0, virtual_failure
    ld t0, 0(sp)
    li t1, 0x696c093a656d614e
    bne t0, t1, virtual_failure
    ld t0, 8(sp)
    li t1, 0x747365742d78756e
    bne t0, t1, virtual_failure
    lbu t0, 16(sp)
    li t1, 10
    bne t0, t1, virtual_failure
    add t0, sp, s1
    addi t0, t0, -11
    ld t1, 0(t0)
    li t2, 0x3a73646165726854
    bne t1, t2, virtual_failure
    lbu t1, 8(t0)
    li t2, 9
    bne t1, t2, virtual_failure
    lbu t1, 9(t0)
    li t2, 49
    bne t1, t2, virtual_failure
    lbu t1, 10(t0)
    li t2, 10
    bne t1, t2, virtual_failure

    mv a0, s0
    li a1, 6
    li a2, 0
    invoke 62
    expect 6
    read_fd s0, 5
    expect 5
    lw t0, 0(sp)
    li t1, 0x756e696c
    bne t0, t1, virtual_failure
    lbu t0, 4(sp)
    li t1, 120
    bne t0, t1, virtual_failure
    mv a0, s0
    li a1, 0
    li a2, 2
    invoke 62
    bne a0, s1, virtual_failure
    mv a0, s0
    li a1, 0
    li a2, 0
    invoke 62
    expect 0

    mv a0, s0
    addi a1, sp, 256
    invoke 80
    expect 0
    lw t0, 272(sp)
    li t1, 0100444
    bne t0, t1, virtual_failure
    lw t0, 276(sp)
    li t1, 1
    bne t0, t1, virtual_failure
    ld t0, 304(sp)
    bne t0, s1, virtual_failure
    mv a0, s0
    invoke 57
    expect 0

    # /proc/self/cmdline starts with the program path and is NUL separated.
    open_virtual cmdline_path
    expect 0
    mv s0, a0
    read_fd s0, 64
    blez a0, virtual_failure
    ld t0, 0(sp)
    li t1, 0x6e696c2f6e69622f
    bne t0, t1, virtual_failure
    lbu t0, 15(sp)
    bnez t0, virtual_failure
    mv a0, s0
    invoke 57
    expect 0

    # /proc/version identifies the emulated Linux release.
    open_virtual version_path
    expect 0
    mv s0, a0
    read_fd s0, 128
    blez a0, virtual_failure
    ld t0, 0(sp)
    li t1, 0x65762078756e694c
    bne t0, t1, virtual_failure
    mv a0, s0
    li a1, 0
    li a2, 2
    invoke 62
    blez a0, virtual_failure
    mv a0, s0
    li a1, 0
    li a2, 0
    invoke 62
    expect 0
    mv a0, s0
    invoke 57
    expect 0

    # The remaining virtual files are readable snapshots.
    open_virtual mounts_path
    expect 0
    mv s0, a0
    read_fd s0, 128
    blez a0, virtual_failure
    mv a0, s0
    invoke 57
    expect 0
    open_virtual osrelease_path
    expect 0
    mv s0, a0
    read_fd s0, 32
    blez a0, virtual_failure
    mv a0, s0
    invoke 57
    expect 0

    # /sys/devices/system/cpu/online describes the single online CPU.
    open_virtual online_path
    expect 0
    mv s0, a0
    read_fd s0, 2
    expect 2
    lhu t0, 0(sp)
    li t1, 0x0a30
    bne t0, t1, virtual_failure
    read_fd s0, 1
    expect 0
    mv a0, s0
    li a1, 100
    li a2, 0
    invoke 62
    expect 100
    read_fd s0, 1
    expect 0
    mv a0, s0
    li a1, 0
    li a2, 1
    invoke 62
    expect 100
    mv a0, s0
    li a1, 0
    li a2, 0
    invoke 62
    expect 0
    read_fd s0, 2
    expect 2
    mv a0, s0
    li a1, -1
    li a2, 2
    invoke 62
    expect 1
    read_fd s0, 1
    expect 1
    lbu t0, 0(sp)
    li t1, 10
    bne t0, t1, virtual_failure

    mv a0, s0
    addi a1, sp, 256
    invoke 80
    expect 0
    lw t0, 272(sp)
    li t1, 0100444
    bne t0, t1, virtual_failure
    lw t0, 276(sp)
    li t1, 1
    bne t0, t1, virtual_failure
    ld t0, 304(sp)
    li t1, 2
    bne t0, t1, virtual_failure

    addi t0, sp, 384
    sw s0, 0(t0)
    li t1, 1
    sh t1, 4(t0)
    sh zero, 6(t0)
    sd zero, 400(sp)
    sd zero, 408(sp)
    addi a0, sp, 384
    li a1, 1
    addi a2, sp, 400
    li a3, 0
    li a4, 8
    invoke 73
    expect 1
    lhu t0, 390(sp)
    li t1, 1
    bne t0, t1, virtual_failure

    mv a0, s0
    la a1, dummy
    li a2, 1
    invoke 64
    expect -30
    mv a0, s0
    invoke 57
    expect 0

    # Invalid accesses report the read-only or missing nature of the mount.
    open_virtual status_path, 1
    expect -30
    open_virtual status_path, 0x10000
    expect -20
    open_virtual missing_path
    expect -2

    li a0, 42
    invoke 94
virtual_failure:
    li a0, 3
    invoke 94
    j virtual_failure

.section .rodata
status_path: .asciz "/proc/self/status"
cmdline_path: .asciz "/proc/self/cmdline"
version_path: .asciz "/proc/version"
mounts_path: .asciz "/proc/mounts"
online_path: .asciz "/sys/devices/system/cpu/online"
osrelease_path: .asciz "/sys/kernel/osrelease"
missing_path: .asciz "/proc/self/absent"
dummy: .asciz "x"

.section .note.GNU-stack,"",@progbits
