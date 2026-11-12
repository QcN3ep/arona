# SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

.section .text

.macro invoke number
    li.d $a7, \number
    syscall 0
.endm

.macro expect value
    li.d $s8, \@
    li.d $t0, \value
    bne $a0, $t0, virtual_failure
.endm

.macro open_virtual path, flags=0
    li.d $a0, -100
    la.pcrel $a1, \path
    li.d $a2, \flags
    invoke 56
.endm

.macro read_fd fd, length
    move $a0, \fd
    move $a1, $sp
    li.d $a2, \length
    invoke 63
.endm

.global files_virtual
files_virtual:
    addi.d $sp, $sp, -512

    # /proc/self/status exposes the running process description.
    open_virtual status_path
    expect 0
    move $s0, $a0
    read_fd $s0, 256
    blez $a0, virtual_failure
    move $s1, $a0
    li.d $t0, 11
    sltu $t1, $s1, $t0
    bnez $t1, virtual_failure
    ld.d $t0, $sp, 0
    li.d $t1, 0x696c093a656d614e
    bne $t0, $t1, virtual_failure
    ld.d $t0, $sp, 8
    li.d $t1, 0x747365742d78756e
    bne $t0, $t1, virtual_failure
    ld.bu $t0, $sp, 16
    li.d $t1, 10
    bne $t0, $t1, virtual_failure
    add.d $t0, $sp, $s1
    addi.d $t0, $t0, -11
    ld.d $t1, $t0, 0
    li.d $t2, 0x3a73646165726854
    bne $t1, $t2, virtual_failure
    ld.bu $t1, $t0, 8
    li.d $t2, 9
    bne $t1, $t2, virtual_failure
    ld.bu $t1, $t0, 9
    li.d $t2, 49
    bne $t1, $t2, virtual_failure
    ld.bu $t1, $t0, 10
    li.d $t2, 10
    bne $t1, $t2, virtual_failure

    move $a0, $s0
    li.d $a1, 6
    li.d $a2, 0
    invoke 62
    expect 6
    read_fd $s0, 5
    expect 5
    ld.w $t0, $sp, 0
    li.d $t1, 0x756e696c
    bne $t0, $t1, virtual_failure
    ld.bu $t0, $sp, 4
    li.d $t1, 120
    bne $t0, $t1, virtual_failure
    move $a0, $s0
    li.d $a1, 0
    li.d $a2, 2
    invoke 62
    bne $a0, $s1, virtual_failure
    move $a0, $s0
    li.d $a1, 0
    li.d $a2, 0
    invoke 62
    expect 0

    move $a0, $s0
    addi.d $a1, $sp, 256
    invoke 80
    expect 0
    ld.w $t0, $sp, 272
    li.d $t1, 0100444
    bne $t0, $t1, virtual_failure
    ld.w $t0, $sp, 276
    li.d $t1, 1
    bne $t0, $t1, virtual_failure
    ld.d $t0, $sp, 304
    bne $t0, $s1, virtual_failure
    move $a0, $s0
    invoke 57
    expect 0

    # /proc/self/cmdline starts with the program path and is NUL separated.
    open_virtual cmdline_path
    expect 0
    move $s0, $a0
    read_fd $s0, 64
    blez $a0, virtual_failure
    ld.d $t0, $sp, 0
    li.d $t1, 0x6e696c2f6e69622f
    bne $t0, $t1, virtual_failure
    ld.bu $t0, $sp, 15
    bnez $t0, virtual_failure
    move $a0, $s0
    invoke 57
    expect 0

    # /proc/version identifies the emulated Linux release.
    open_virtual version_path
    expect 0
    move $s0, $a0
    read_fd $s0, 128
    blez $a0, virtual_failure
    ld.d $t0, $sp, 0
    li.d $t1, 0x65762078756e694c
    bne $t0, $t1, virtual_failure
    move $a0, $s0
    li.d $a1, 0
    li.d $a2, 2
    invoke 62
    blez $a0, virtual_failure
    move $a0, $s0
    li.d $a1, 0
    li.d $a2, 0
    invoke 62
    expect 0
    move $a0, $s0
    invoke 57
    expect 0

    # The remaining virtual files are readable snapshots.
    open_virtual mounts_path
    expect 0
    move $s0, $a0
    read_fd $s0, 128
    blez $a0, virtual_failure
    move $a0, $s0
    invoke 57
    expect 0
    open_virtual osrelease_path
    expect 0
    move $s0, $a0
    read_fd $s0, 32
    blez $a0, virtual_failure
    move $a0, $s0
    invoke 57
    expect 0

    # /sys/devices/system/cpu/online describes the single online CPU.
    open_virtual online_path
    expect 0
    move $s0, $a0
    read_fd $s0, 2
    expect 2
    ld.hu $t0, $sp, 0
    li.d $t1, 0x0a30
    bne $t0, $t1, virtual_failure
    read_fd $s0, 1
    expect 0
    move $a0, $s0
    li.d $a1, 100
    li.d $a2, 0
    invoke 62
    expect 100
    read_fd $s0, 1
    expect 0
    move $a0, $s0
    li.d $a1, 0
    li.d $a2, 1
    invoke 62
    expect 100
    move $a0, $s0
    li.d $a1, 0
    li.d $a2, 0
    invoke 62
    expect 0
    read_fd $s0, 2
    expect 2
    move $a0, $s0
    li.d $a1, -1
    li.d $a2, 2
    invoke 62
    expect 1
    read_fd $s0, 1
    expect 1
    ld.bu $t0, $sp, 0
    li.d $t1, 10
    bne $t0, $t1, virtual_failure

    move $a0, $s0
    addi.d $a1, $sp, 256
    invoke 80
    expect 0
    ld.w $t0, $sp, 272
    li.d $t1, 0100444
    bne $t0, $t1, virtual_failure
    ld.w $t0, $sp, 276
    li.d $t1, 1
    bne $t0, $t1, virtual_failure
    ld.d $t0, $sp, 304
    li.d $t1, 2
    bne $t0, $t1, virtual_failure

    addi.d $t0, $sp, 384
    st.w $s0, $t0, 0
    li.d $t1, 1
    st.h $t1, $t0, 4
    st.h $zero, $t0, 6
    st.d $zero, $sp, 400
    st.d $zero, $sp, 408
    addi.d $a0, $sp, 384
    li.d $a1, 1
    addi.d $a2, $sp, 400
    li.d $a3, 0
    li.d $a4, 8
    invoke 73
    expect 1
    ld.hu $t0, $sp, 390
    li.d $t1, 1
    bne $t0, $t1, virtual_failure

    move $a0, $s0
    la.pcrel $a1, dummy
    li.d $a2, 1
    invoke 64
    expect -30
    move $a0, $s0
    invoke 57
    expect 0

    # Invalid accesses report the read-only or missing nature of the mount.
    open_virtual status_path, 1
    expect -30
    open_virtual status_path, 0x10000
    expect -20
    open_virtual missing_path
    expect -2

    li.d $a0, 42
    invoke 94
virtual_failure:
    li.d $a0, 3
    invoke 94
    b virtual_failure

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
