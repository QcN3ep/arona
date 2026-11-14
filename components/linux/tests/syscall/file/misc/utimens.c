// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

#include "misc.h"

static const char path[] = "/var/misc-utimens";
static const char link_path[] = "/var/misc-utimens-link";
static const char directory_path[] = "/var/misc-utimens-dir";

static void remove_path(const char *name, long directory) {
    long result = SC(SYS_UNLINKAT, AT_FDCWD, P(name), directory);
    CHECK(result == 0 || result == -ENOENT);
}

int utimens_main(void) {
    static struct statx info;
    struct timespec times[2];
    remove_path(link_path, 0);
    remove_path(directory_path, 0x200);
    remove_path(path, 0);

    long descriptor = misc_open(path, O_RDWR | O_CREAT | O_TRUNC, 0600);
    CHECK(descriptor >= 0);
    CHECK(misc_write(descriptor, "time", 4) == 4);

    /* Explicit timestamps are stored with their nanoseconds. */
    times[0].seconds = 1000;
    times[0].nanoseconds = 0;
    times[1].seconds = 2000;
    times[1].nanoseconds = 123456789;
    CHECK(SC(SYS_UTIMENSAT, AT_FDCWD, P(path), P(times), 0) == 0);
    CHECK(misc_statx(path, 0, &info) == 0);
    CHECK(info.accessed.seconds == 1000 && info.accessed.nanoseconds == 0);
    CHECK(info.modified.seconds == 2000 && info.modified.nanoseconds == 123456789);
    CHECK(info.changed.seconds > 0);
    CHECK(info.size == 4);

    /* UTIME_NOW and UTIME_OMIT apply per field. */
    struct timespec before;
    CHECK(SC(SYS_CLOCK_GETTIME, 0, P(&before)) == 0);
    times[0].seconds = 0x7fffffff;
    times[0].nanoseconds = UTIME_NOW;
    times[1].seconds = 99;
    times[1].nanoseconds = UTIME_OMIT;
    CHECK(SC(SYS_UTIMENSAT, AT_FDCWD, P(path), P(times), 0) == 0);
    CHECK(misc_statx(path, 0, &info) == 0);
    CHECK(info.modified.seconds == 2000 && info.modified.nanoseconds == 123456789);
    CHECK(info.accessed.seconds >= before.seconds);
    struct timespec after;
    CHECK(SC(SYS_CLOCK_GETTIME, 0, P(&after)) == 0);
    CHECK(info.accessed.seconds <= after.seconds);

    /* A null timestamp array sets both fields to the current time. */
    CHECK(SC(SYS_UTIMENSAT, AT_FDCWD, P(path), 0, 0) == 0);
    CHECK(misc_statx(path, 0, &info) == 0);
    CHECK(info.accessed.seconds >= before.seconds);
    CHECK(info.modified.seconds >= before.seconds);

    /* Directory-relative updates use the directory descriptor as the anchor. */
    long directory = misc_open("/var", O_RDONLY | O_DIRECTORY, 0);
    CHECK(directory >= 0);
    times[0].seconds = 3000;
    times[0].nanoseconds = 1;
    times[1].seconds = 4000;
    times[1].nanoseconds = 2;
    CHECK(SC(SYS_UTIMENSAT, directory, P("misc-utimens"), P(times), 0) == 0);
    CHECK(misc_statx(path, 0, &info) == 0);
    CHECK(info.accessed.seconds == 3000 && info.accessed.nanoseconds == 1);
    CHECK(info.modified.seconds == 4000 && info.modified.nanoseconds == 2);
    CHECK_ERR(SC(SYS_UTIMENSAT, descriptor, P("misc-utimens"), P(times), 0), ENOTDIR);

    /* AT_SYMLINK_NOFOLLOW updates the link rather than its target. */
    CHECK(SC(SYS_SYMLINKAT, P(path), AT_FDCWD, P(link_path)) == 0);
    times[0].seconds = 5000;
    times[0].nanoseconds = 0;
    times[1].seconds = 6000;
    times[1].nanoseconds = 0;
    CHECK(SC(SYS_UTIMENSAT, AT_FDCWD, P(link_path), P(times), AT_SYMLINK_NOFOLLOW) == 0);
    CHECK(misc_statx(link_path, 0, &info) == 0);
    CHECK(info.modified.seconds == 4000);
    CHECK(misc_statx(link_path, AT_SYMLINK_NOFOLLOW, &info) == 0);
    CHECK(info.accessed.seconds == 5000 && info.modified.seconds == 6000);

    /* AT_EMPTY_PATH updates the descriptor target. */
    times[0].seconds = 7000;
    times[0].nanoseconds = 0;
    times[1].seconds = 8000;
    times[1].nanoseconds = 0;
    CHECK(SC(SYS_UTIMENSAT, descriptor, P(""), P(times), AT_EMPTY_PATH) == 0);
    CHECK(misc_statx(path, 0, &info) == 0);
    CHECK(info.accessed.seconds == 7000 && info.modified.seconds == 8000);

    /* Omitting both fields leaves every timestamp untouched. */
    struct statx previous = info;
    times[0].nanoseconds = UTIME_OMIT;
    times[1].nanoseconds = UTIME_OMIT;
    CHECK(SC(SYS_UTIMENSAT, AT_FDCWD, P(path), P(times), 0) == 0);
    CHECK(misc_statx(path, 0, &info) == 0);
    CHECK(info.accessed.seconds == previous.accessed.seconds);
    CHECK(info.modified.seconds == previous.modified.seconds);
    CHECK(info.changed.seconds == previous.changed.seconds);
    CHECK(info.changed.nanoseconds == previous.changed.nanoseconds);

    /* Invalid timestamp values, flags, and paths. */
    struct timespec invalid[2] = {{0, 1000000000}, {0, 0}};
    CHECK_ERR(SC(SYS_UTIMENSAT, AT_FDCWD, P(path), P(invalid), 0), EINVAL);
    invalid[0].seconds = -1;
    invalid[0].nanoseconds = 0;
    CHECK_ERR(SC(SYS_UTIMENSAT, AT_FDCWD, P(path), P(invalid), 0), EINVAL);
    invalid[0].seconds = 0;
    invalid[0].nanoseconds = -1;
    CHECK_ERR(SC(SYS_UTIMENSAT, AT_FDCWD, P(path), P(invalid), 0), EINVAL);
    CHECK_ERR(SC(SYS_UTIMENSAT, AT_FDCWD, P(path), P(times), 8), EINVAL);
    CHECK_ERR(SC(SYS_UTIMENSAT, AT_FDCWD, P("/var/misc-absent"), P(times), 0), ENOENT);
    CHECK_ERR(SC(SYS_UTIMENSAT, AT_FDCWD, P(""), P(times), 0), ENOENT);
    CHECK_ERR(SC(SYS_UTIMENSAT, AT_FDCWD, P(path), 8, 0), EFAULT);

    /* The read-only archive rejects timestamp updates. */
    times[0].seconds = 1;
    times[0].nanoseconds = 0;
    times[1].seconds = 1;
    times[1].nanoseconds = 0;
    CHECK_ERR(SC(SYS_UTIMENSAT, AT_FDCWD, P("/etc/linux-data"), P(times), 0), EROFS);

    /* Directories accept timestamp updates as well. */
    CHECK(SC(SYS_MKDIRAT, AT_FDCWD, P(directory_path), 0755) == 0);
    times[0].seconds = 11;
    times[0].nanoseconds = 0;
    times[1].seconds = 12;
    times[1].nanoseconds = 0;
    CHECK(SC(SYS_UTIMENSAT, AT_FDCWD, P(directory_path), P(times), 0) == 0);
    CHECK(misc_statx(directory_path, 0, &info) == 0);
    CHECK(info.accessed.seconds == 11 && info.modified.seconds == 12);

    CHECK(misc_close(directory) == 0);
    CHECK(misc_close(descriptor) == 0);
    return 42;
}
