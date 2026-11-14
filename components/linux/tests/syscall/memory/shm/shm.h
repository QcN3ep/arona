// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

#ifndef LINUX_SHM_TEST_H
#define LINUX_SHM_TEST_H

extern long shm_syscall(long number, const long arguments[6]);
extern void shm_fail(unsigned line);
#define P(value) ((long)(value))
#define SC(number, ...) shm_syscall(number, (long[6]){__VA_ARGS__})
#define CHECK(condition) do { if (!(condition)) shm_fail(__LINE__); } while (0)

#endif
