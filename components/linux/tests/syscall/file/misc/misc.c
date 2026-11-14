// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

#include "misc.h"

extern int access_main(void);
extern int utimens_main(void);
extern int sync_main(void);
extern int sendfile_main(void);
extern int rlimit_main(void);
extern int console_main(void);

void misc_fail(unsigned long line) {
    char text[] = "Fs misc assertion line 0000\n";
    unsigned newline = sizeof(text) - 2;
    for (unsigned index = 1; index <= 4; ++index) {
        text[newline - index] = (char)('0' + line % 10);
        line /= 10;
    }
    SC(SYS_WRITE, 1, P(text), sizeof(text) - 1);
    SC(SYS_EXIT_GROUP, 1);
    for (;;) {}
}

long misc_write_all(long fd, const void *buffer, unsigned long length) {
    const unsigned char *bytes = buffer;
    unsigned long done = 0;
    while (done < length) {
        long count = misc_write(fd, bytes + done, length - done);
        if (count <= 0) return count;
        done += (unsigned long)count;
    }
    return (long)done;
}

long misc_read_all(long fd, void *buffer, unsigned long length) {
    unsigned char *bytes = buffer;
    unsigned long done = 0;
    while (done < length) {
        long count = misc_read(fd, bytes + done, length - done);
        if (count <= 0) return count;
        done += (unsigned long)count;
    }
    return (long)done;
}

long misc_open(const char *path, long flags, long mode) {
    return SC(SYS_OPENAT, AT_FDCWD, P(path), flags, mode);
}

long misc_read(long fd, void *buffer, unsigned long length) {
    return SC(SYS_READ, fd, P(buffer), length);
}

long misc_write(long fd, const void *buffer, unsigned long length) {
    return SC(SYS_WRITE, fd, P(buffer), length);
}

long misc_close(long fd) {
    return SC(SYS_CLOSE, fd);
}

long misc_statx(const char *path, long flags, void *info) {
    return SC(SYS_STATX, AT_FDCWD, P(path), flags, 0x7ff, P(info));
}

long misc_ioctl(long fd, long command, void *argument) {
    return SC(SYS_IOCTL, fd, command, P(argument));
}

static int same(const char *left, const char *right) {
    while (*left != 0 && *left == *right) {
        ++left;
        ++right;
    }
    return *left == *right;
}

int misc_main(const char *mode) {
    if (same(mode, "access")) return access_main();
    if (same(mode, "utimens")) return utimens_main();
    if (same(mode, "sync")) return sync_main();
    if (same(mode, "sendfile")) return sendfile_main();
    if (same(mode, "rlimit")) return rlimit_main();
    if (same(mode, "console")) return console_main();
    return 1;
}
