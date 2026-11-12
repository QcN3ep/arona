# SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

.section .text
.option norelax
.global _start
_start:
    ld a0, 16(sp)
    call socket_main
    li a7, 94
    ecall
.global net_syscall
net_syscall:
    mv a7, a0
    mv t0, a1
    ld a0, 0(t0)
    ld a1, 8(t0)
    ld a2, 16(t0)
    ld a3, 24(t0)
    ld a4, 32(t0)
    ld a5, 40(t0)
    ecall
    ret
