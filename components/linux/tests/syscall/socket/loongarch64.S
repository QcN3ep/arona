# SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

.section .text
.global _start
_start:
    ld.d $a0, $sp, 16
    bl socket_main
    li.d $a7, 94
    syscall 0
.global net_syscall
net_syscall:
    move $a7, $a0
    move $t0, $a1
    ld.d $a0, $t0, 0
    ld.d $a1, $t0, 8
    ld.d $a2, $t0, 16
    ld.d $a3, $t0, 24
    ld.d $a4, $t0, 32
    ld.d $a5, $t0, 40
    syscall 0
    jr $ra
