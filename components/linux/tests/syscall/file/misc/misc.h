// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

#ifndef ARONA_LINUX_FS_MISC_TEST_H
#define ARONA_LINUX_FS_MISC_TEST_H

extern long misc_syscall(long number, const long arguments[6]);
extern void misc_fail(unsigned long line);

#define P(value) ((long)(value))
#define SC(number, ...) misc_syscall(number, (long[6]){__VA_ARGS__})
#define CHECK(condition) do { if (!(condition)) misc_fail(__LINE__); } while (0)
#define CHECK_ERR(expression, error) \
    do { long value_ = (long)(expression); if (value_ != -(error)) misc_fail(__LINE__); } while (0)
#define INFINITY_ (~0UL)

struct timespec { long seconds, nanoseconds; };
struct rlimit { unsigned long current, maximum; };
struct statx_time { long seconds; unsigned nanoseconds; unsigned reserved; };
struct statx {
    unsigned mask, block_size;
    unsigned long attributes;
    unsigned links, uid, gid;
    unsigned short mode, spare;
    unsigned long inode, size, blocks, attributes_mask;
    struct statx_time accessed, birth, changed, modified;
    unsigned rdevice_major, rdevice_minor;
    unsigned device_major, device_minor;
    unsigned long mount;
    unsigned long spare2[13];
};
struct termios { unsigned char bytes[36]; };
struct winsize { unsigned short rows, columns, x, y; };

// REFERENCE: https://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git/tree/include/uapi/asm-generic/unistd.h?h=v6.12
#define SYS_FCNTL 25
#define SYS_IOCTL 29
#define SYS_MKDIRAT 34
#define SYS_UNLINKAT 35
#define SYS_SYMLINKAT 36
#define SYS_FTRUNCATE 46
#define SYS_FACCESSAT 48
#define SYS_OPENAT 56
#define SYS_CLOSE 57
#define SYS_PIPE2 59
#define SYS_LSEEK 62
#define SYS_READ 63
#define SYS_WRITE 64
#define SYS_SENDFILE 71
#define SYS_SYNC 81
#define SYS_UTIMENSAT 88
#define SYS_EXIT_GROUP 94
#define SYS_CLOCK_GETTIME 113
#define SYS_RT_SIGACTION 134
#define SYS_SETRESUID 147
#define SYS_SETRESGID 149
#define SYS_GETPID 172
#define SYS_GETUID 174
#define SYS_GETEUID 175
#define SYS_GETEGID 177
#define SYS_SOCKET 198
#define SYS_BIND 200
#define SYS_LISTEN 201
#define SYS_ACCEPT 202
#define SYS_CONNECT 203
#define SYS_GETSOCKNAME 204
#define SYS_CLONE 220
#define SYS_MMAP 222
#define SYS_MUNMAP 215
#define SYS_BRK 214
#define SYS_WAIT4 260
#define SYS_PRLIMIT64 261
#define SYS_STATX 291
#define SYS_FACCESSAT2 439


// REFERENCE: https://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git/tree/include/uapi/asm-generic/fcntl.h?h=v6.12
#define O_RDONLY 0
#define O_WRONLY 1
#define O_RDWR 2
#define O_CREAT 0x40
#define O_TRUNC 0x200
#define O_APPEND 0x400
#define O_NONBLOCK 0x800
#define O_DIRECTORY 0x10000
#define F_SETFL 4
#define F_SETPIPE_SZ 1031

#define AT_FDCWD (-100)
#define AT_SYMLINK_NOFOLLOW 0x100
#define AT_EACCESS 0x200
#define AT_EMPTY_PATH 0x1000

#define UTIME_NOW 0x3fffffff
#define UTIME_OMIT 0x3ffffffe

#define R_OK 4
#define W_OK 2
#define X_OK 1
#define F_OK 0

#define RLIMIT_CPU 0
#define RLIMIT_FSIZE 1
#define RLIMIT_STACK 3
#define RLIMIT_NOFILE 7
#define RLIMIT_AS 9

#define TIOCGWINSZ 0x5413
#define TCGETS 0x5401

#define MAP_PRIVATE 2
#define MAP_ANONYMOUS 0x20
#define PROT_READ_WRITE 3

#define WNOHANG 1
#define SIGCHLD 17

// REFERENCE: https://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git/tree/include/uapi/asm-generic/errno-base.h?h=v6.12
#define EPERM 1
#define ENOENT 2
#define ESRCH 3
#define EINTR 4
#define EIO 5
#define EBADF 9
#define EAGAIN 11
#define ENOMEM 12
#define EACCES 13
#define EFAULT 14
#define EEXIST 17
#define ENOTDIR 20
#define EINVAL 22
#define EMFILE 24
#define EFBIG 27
#define EROFS 30
#define EPIPE 32
#define EOPNOTSUPP 95

long misc_open(const char *path, long flags, long mode);
long misc_write_all(long fd, const void *buffer, unsigned long length);
long misc_read_all(long fd, void *buffer, unsigned long length);
long misc_read(long fd, void *buffer, unsigned long length);
long misc_write(long fd, const void *buffer, unsigned long length);
long misc_close(long fd);
long misc_statx(const char *path, long flags, void *info);
long misc_ioctl(long fd, long command, void *argument);

#endif
