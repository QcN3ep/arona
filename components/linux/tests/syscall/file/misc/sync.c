// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

#include "misc.h"

static const char path[] = "/var/misc-sync";
static unsigned char payload[8192];
static unsigned char buffer[8192];

static void fill(void) {
    for (unsigned index = 0; index < sizeof(payload); ++index) {
        payload[index] = (unsigned char)('a' + index % 26);
    }
}

int sync_main(void) {
    fill();
    /* A clean tree still succeeds. */
    CHECK(SC(SYS_SYNC) == 0);

    long descriptor = misc_open(path, O_RDWR | O_CREAT | O_TRUNC, 0600);
    CHECK(descriptor >= 0);
    CHECK(misc_write_all(descriptor, payload, sizeof(payload)) == (long)sizeof(payload));
    CHECK(SC(SYS_SYNC) == 0);
    CHECK(misc_close(descriptor) == 0);

    /* The flushed contents are readable through a new descriptor. */
    descriptor = misc_open(path, O_RDONLY, 0);
    CHECK(descriptor >= 0);
    CHECK(misc_read_all(descriptor, buffer, sizeof(buffer)) == (long)sizeof(buffer));
    for (unsigned index = 0; index < sizeof(buffer); ++index) {
        CHECK(buffer[index] == payload[index]);
    }
    CHECK(misc_read(descriptor, buffer, 1) == 0);
    CHECK(misc_close(descriptor) == 0);

    /* Truncation and unlinking are flushed by the same call. */
    descriptor = misc_open(path, O_RDWR, 0);
    CHECK(descriptor >= 0);
    CHECK(SC(SYS_FTRUNCATE, descriptor, 512) == 0);
    CHECK(SC(SYS_SYNC) == 0);
    static struct statx info;
    CHECK(misc_statx(path, 0, &info) == 0);
    CHECK(info.size == 512);
    CHECK(misc_close(descriptor) == 0);
    CHECK(SC(SYS_UNLINKAT, AT_FDCWD, P(path), 0) == 0);
    CHECK(SC(SYS_SYNC) == 0);

    /* Repeated calls stay successful. */
    CHECK(SC(SYS_SYNC) == 0);
    CHECK(SC(SYS_SYNC) == 0);
    return 42;
}
