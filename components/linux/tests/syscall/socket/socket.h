// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

#ifndef LINUX_SOCKET_TEST_H
#define LINUX_SOCKET_TEST_H

extern long net_syscall(long number, const long arguments[6]);
extern void socket_fail(unsigned line);
#define P(value) ((long)(value))
#define SC(number, ...) net_syscall(number, (long[6]){__VA_ARGS__})
#define CHECK(condition) do { if (!(condition)) socket_fail(__LINE__); } while (0)
#define fail socket_fail

struct address { unsigned short family; unsigned char port[2]; unsigned char ip[4]; unsigned char zero[8]; };
struct pollfd { int fd; short events, revents; };
struct timespec { long seconds, nanoseconds; };
struct iovec { void *base; unsigned long length; };
struct message {
    void *name;
    unsigned name_length, padding;
    struct iovec *iov;
    unsigned long iov_length;
    void *control;
    unsigned long control_length;
    unsigned flags, tail;
};

#endif
