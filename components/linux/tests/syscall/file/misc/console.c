// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

#include "misc.h"

static const char path[] = "/dev/console";
static const char message[] = "Console device write\n";
static unsigned char byte;

int console_main(void) {
    static struct statx info;
    /* The device node reports a character device without a filesystem entry. */
    CHECK(misc_statx(path, 0, &info) == 0);
    CHECK((info.mode & 0170000) == 0020000);
    CHECK((info.mode & 0777) == 0600);
    CHECK(info.links == 1);

    long descriptor = misc_open(path, O_RDWR, 0);
    CHECK(descriptor >= 0);
    CHECK(SC(SYS_STATX, descriptor, P(""), AT_EMPTY_PATH, 0x7ff, P(&info)) == 0);
    CHECK((info.mode & 0170000) == 0020000);

    /* The console is a terminal, so terminal ioctls answer on it. */
    static struct termios settings;
    CHECK(misc_ioctl(descriptor, TCGETS, &settings) == 0);
    static struct winsize window;
    CHECK(misc_ioctl(descriptor, TIOCGWINSZ, &window) == 0);

    /* Writes reach the console device and report their length. */
    CHECK(misc_write(descriptor, message, sizeof(message) - 1) == (long)sizeof(message) - 1);
    CHECK(SC(SYS_READ, descriptor, P(&byte), 0) == 0);

    /* A non-blocking console read reports EAGAIN while no input is queued. */
    long nonblocking = misc_open(path, O_RDWR | O_NONBLOCK, 0);
    CHECK(nonblocking >= 0);
    CHECK_ERR(misc_read(nonblocking, &byte, 1), EAGAIN);

    /* Read-only descriptors reject writes and the path rejects O_DIRECTORY. */
    long reader = misc_open(path, O_RDONLY, 0);
    CHECK(reader >= 0);
    CHECK_ERR(misc_write(reader, &byte, 1), EBADF);
    CHECK_ERR(misc_open(path, O_RDONLY | O_DIRECTORY, 0), ENOTDIR);

    CHECK(misc_close(reader) == 0);
    CHECK(misc_close(nonblocking) == 0);
    CHECK(misc_close(descriptor) == 0);
    return 42;
}
