// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

#include "misc.h"

static const char path[] = "/var/misc-rlimit";
static unsigned char block[8192];

static void nofile(void) {
    struct rlimit limit;
    CHECK(SC(SYS_PRLIMIT64, 0, RLIMIT_NOFILE, 0, P(&limit)) == 0);
    CHECK(limit.current == 1024 && limit.maximum == 4096);

    // Lowering the soft limit is reported and takes effect immediately.
    struct rlimit update = {64, 4096};
    CHECK(SC(SYS_PRLIMIT64, 0, RLIMIT_NOFILE, P(&update), P(&limit)) == 0);
    CHECK(limit.current == 1024 && limit.maximum == 4096);
    CHECK(SC(SYS_PRLIMIT64, 0, RLIMIT_NOFILE, 0, P(&limit)) == 0);
    CHECK(limit.current == 64 && limit.maximum == 4096);

    // Count the descriptors that the program already holds below the limit.
    long base = 0;
    for (long fd = 0; fd < 64; ++fd) {
        if (SC(SYS_FCNTL, fd, 1, 0) >= 0) {
            ++base;
        }
    }
    CHECK(base >= 1 && base <= 3);
    long descriptor = misc_open(path, O_RDWR | O_CREAT | O_TRUNC, 0600);
    CHECK(descriptor >= 0);
    CHECK(misc_close(descriptor) == 0);
    long descriptors[64];
    long count = 0;
    for (;;) {
        long allocated = misc_open(path, O_RDONLY, 0);
        if (allocated < 0) {
            CHECK(allocated == -EMFILE);
            break;
        }
        CHECK(count < 64 - base);
        CHECK(allocated < 64);
        descriptors[count++] = allocated;
    }
    CHECK(count == 64 - base);
    for (long index = 0; index < count; ++index) {
        CHECK(misc_close(descriptors[index]) == 0);
    }

    // A zero soft limit blocks every new allocation.
    update.current = 0;
    CHECK(SC(SYS_PRLIMIT64, 0, RLIMIT_NOFILE, P(&update), 0) == 0);
    CHECK_ERR(misc_open(path, O_RDONLY, 0), EMFILE);
    update.current = 1024;
    CHECK(SC(SYS_PRLIMIT64, 0, RLIMIT_NOFILE, P(&update), 0) == 0);
    descriptor = misc_open(path, O_RDONLY, 0);
    CHECK(descriptor >= 0);
    CHECK(misc_close(descriptor) == 0);

    // Validation and identity checks.
    struct rlimit inverted = {2, 1};
    CHECK_ERR(SC(SYS_PRLIMIT64, 0, RLIMIT_NOFILE, P(&inverted), 0), EINVAL);
    CHECK_ERR(SC(SYS_PRLIMIT64, 0, 10, 0, P(&limit)), EINVAL);
    CHECK_ERR(SC(SYS_PRLIMIT64, 99999, RLIMIT_NOFILE, 0, P(&limit)), ESRCH);
    CHECK_ERR(SC(SYS_PRLIMIT64, 0, RLIMIT_NOFILE, 8, 0), EFAULT);
    CHECK_ERR(SC(SYS_PRLIMIT64, 0, RLIMIT_NOFILE, 0, 8), EFAULT);
    CHECK(SC(SYS_PRLIMIT64, SC(SYS_GETPID), RLIMIT_NOFILE, 0, P(&limit)) == 0);
    CHECK(limit.current == 1024);
    struct rlimit raise = {512, 8192};
    CHECK(SC(SYS_SETRESUID, 0, 123, 0) == 0);
    CHECK_ERR(SC(SYS_PRLIMIT64, 0, RLIMIT_NOFILE, P(&raise), 0), EPERM);
    CHECK(SC(SYS_SETRESUID, 0, 0, 0) == 0);
}

static void file_size(void) {
    struct rlimit limit;
    CHECK(SC(SYS_PRLIMIT64, 0, RLIMIT_FSIZE, 0, P(&limit)) == 0);
    CHECK(limit.current == INFINITY_ && limit.maximum == INFINITY_);
    struct rlimit update = {4096, INFINITY_};
    CHECK(SC(SYS_PRLIMIT64, 0, RLIMIT_FSIZE, P(&update), 0) == 0);

    // Writes stop at the limit, and later writes report EFBIG.
    long descriptor = misc_open(path, O_RDWR | O_CREAT | O_TRUNC, 0600);
    CHECK(descriptor >= 0);
    for (unsigned index = 0; index < sizeof(block); ++index) {
        block[index] = (unsigned char)index;
    }
    CHECK(misc_write(descriptor, block, sizeof(block)) == 4096);
    CHECK_ERR(misc_write(descriptor, block, 16), EFBIG);
    CHECK_ERR(SC(SYS_FTRUNCATE, descriptor, 8192), EFBIG);
    CHECK(SC(SYS_FTRUNCATE, descriptor, 4096) == 0);
    CHECK(misc_close(descriptor) == 0);

    update.current = INFINITY_;
    CHECK(SC(SYS_PRLIMIT64, 0, RLIMIT_FSIZE, P(&update), 0) == 0);
    descriptor = misc_open(path, O_RDWR | O_APPEND, 0);
    CHECK(descriptor >= 0);
    CHECK(misc_write_all(descriptor, block, sizeof(block)) == (long)sizeof(block));
    CHECK(misc_close(descriptor) == 0);
}

static void address_space(void) {
    struct rlimit limit;
    CHECK(SC(SYS_PRLIMIT64, 0, RLIMIT_AS, 0, P(&limit)) == 0);
    CHECK(limit.current == INFINITY_);
    struct rlimit update = {16 * 1024 * 1024, INFINITY_};
    CHECK(SC(SYS_PRLIMIT64, 0, RLIMIT_AS, P(&update), 0) == 0);

    // A mapping larger than the limit fails, a small one still succeeds.
    CHECK_ERR(SC(SYS_MMAP, 0, 64 * 1024 * 1024, PROT_READ_WRITE,
                MAP_PRIVATE | MAP_ANONYMOUS, -1, 0), ENOMEM);
    long mapping = SC(SYS_MMAP, 0, 65536, PROT_READ_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    CHECK(mapping > 0);
    CHECK(SC(SYS_MUNMAP, mapping, 65536) == 0);

    // Growing the heap past the limit reports the previous break.
    update.current = 4096;
    CHECK(SC(SYS_PRLIMIT64, 0, RLIMIT_AS, P(&update), 0) == 0);
    long current = SC(SYS_BRK, 0);
    CHECK(SC(SYS_BRK, current + 65536) == current);
    update.current = INFINITY_;
    CHECK(SC(SYS_PRLIMIT64, 0, RLIMIT_AS, P(&update), 0) == 0);
    CHECK(SC(SYS_BRK, current + 4096) == current + 4096);
}

static void stack_and_cpu(void) {
    struct rlimit limit;
    CHECK(SC(SYS_PRLIMIT64, 0, RLIMIT_STACK, 0, P(&limit)) == 0);
    CHECK(limit.current == 8 * 1024 * 1024 && limit.maximum == INFINITY_);
    struct rlimit stack = {1024 * 1024, INFINITY_};
    CHECK(SC(SYS_PRLIMIT64, 0, RLIMIT_STACK, P(&stack), 0) == 0);
    CHECK(SC(SYS_PRLIMIT64, 0, RLIMIT_STACK, 0, P(&limit)) == 0);
    CHECK(limit.current == 1024 * 1024);

    CHECK(SC(SYS_PRLIMIT64, 0, RLIMIT_CPU, 0, P(&limit)) == 0);
    CHECK(limit.current == INFINITY_ && limit.maximum == INFINITY_);
    struct rlimit cpu = {10, 20};
    CHECK(SC(SYS_PRLIMIT64, 0, RLIMIT_CPU, P(&cpu), P(&limit)) == 0);
    CHECK(limit.current == INFINITY_);
    CHECK(SC(SYS_PRLIMIT64, 0, RLIMIT_CPU, 0, P(&limit)) == 0);
    CHECK(limit.current == 10 && limit.maximum == 20);
    struct rlimit inverted = {2, 1};
    CHECK_ERR(SC(SYS_PRLIMIT64, 0, RLIMIT_CPU, P(&inverted), 0), EINVAL);
    cpu.current = INFINITY_;
    cpu.maximum = INFINITY_;
    CHECK(SC(SYS_PRLIMIT64, 0, RLIMIT_CPU, P(&cpu), 0) == 0);
}

int rlimit_main(void) {
    nofile();
    file_size();
    address_space();
    stack_and_cpu();
    CHECK(SC(SYS_UNLINKAT, AT_FDCWD, P(path), 0) == 0);
    return 42;
}
