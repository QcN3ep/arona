// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

#ifndef LINUX_FS_TEST_H
#define LINUX_FS_TEST_H

extern long fs_syscall(long number, const long arguments[6]);
extern void fs_fail(unsigned line);
#define P(value) ((long)(value))
#define SC(number, ...) fs_syscall(number, (long[6]){__VA_ARGS__})
#define CHECK(condition) do { if (!(condition)) fs_fail(__LINE__); } while (0)

#endif
