// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

// Guest checks for shared mappings of files on the /dev/shm tmpfs.
//
// The scenarios cover repeated mappings of one file, parent and child
// processes observing each other's writes, write-back on msync and munmap,
// SIGBUS after ftruncate shrinks the file, and the shared mapping access
// rules for read-only and write-only descriptors.

#include "shm.h"

#define AT_FDCWD (-100)
#define O_RDONLY 0
#define O_WRONLY 1
#define O_RDWR 2
#define O_CREAT 0x40
#define O_TRUNC 0x200
#define O_DIRECTORY 0x10000
#define PROT_READ 1
#define PROT_WRITE 2
#define MAP_SHARED 1
#define MAP_PRIVATE 2
#define MS_SYNC 4
#define SIGCHLD 17
#define SIGBUS 7
#define EACCES 13
#define EINVAL 22
#define PAGE 4096
#define LENGTH (2 * PAGE)

void shm_fail(unsigned line) {
    char text[] = "Shared memory assertion line 0000\n";
    char *digits = text + sizeof(text) - 6;
    for (int index = 3; index >= 0; --index) {
        digits[index] = '0' + line % 10;
        line /= 10;
    }
    SC(64, 1, P(text), sizeof(text) - 1);
    SC(94, 1);
    for (;;) {}
}

static void write_all(long fd, const unsigned char *bytes, long length) {
    while (length > 0) {
        long count = SC(64, fd, P(bytes), length);
        CHECK(count > 0 && count <= length);
        bytes += count;
        length -= count;
    }
}

static void test_directory(void) {
    long fd = SC(56, AT_FDCWD, P("/dev/shm"), O_RDONLY | O_DIRECTORY, 0);
    CHECK(fd >= 0);
    CHECK(SC(57, fd) == 0);
}

static void read_all(long fd, unsigned char *bytes, long length) {
    while (length > 0) {
        long count = SC(63, fd, P(bytes), length);
        CHECK(count > 0 && count <= length);
        bytes += count;
        length -= count;
    }
}

static void read_at(long fd, unsigned char *bytes, long length, long offset) {
    while (length > 0) {
        long count = SC(67, fd, P(bytes), length, offset);
        CHECK(count > 0 && count <= length);
        bytes += count;
        length -= count;
        offset += count;
    }
}

static long open_file(const char *path, long flags, long mode) {
    long fd = SC(56, AT_FDCWD, P(path), flags, mode);
    CHECK(fd >= 0);
    return fd;
}

static void close_file(long fd) {
    CHECK(SC(57, fd) == 0);
}

static void resize_file(long fd, long length) {
    CHECK(SC(46, fd, length) == 0);
}

static volatile unsigned char *shared_map(long fd, long length, long protection) {
    long address = SC(222, 0, length, protection, MAP_SHARED, fd, 0);
    CHECK(address > 0);
    return (volatile unsigned char *)address;
}

static void unmap(void *address, long length) {
    CHECK(SC(215, P(address), length) == 0);
}

static void temporary(void) {
    test_directory();
    long fd = open_file("/dev/shm/arona-shm", O_RDWR | O_CREAT | O_TRUNC, 0600);
    write_all(fd, (const unsigned char *)"shared", 6);
    CHECK(SC(62, fd, 0, 0) == 0);
    unsigned char content[6] = {0};
    read_all(fd, content, 6);
    CHECK(content[0] == 's' && content[5] == 'd');
    resize_file(fd, LENGTH);

    volatile unsigned char *first = shared_map(fd, LENGTH, PROT_READ | PROT_WRITE);
    volatile unsigned char *second = shared_map(fd, LENGTH, PROT_READ | PROT_WRITE);
    CHECK(first != second);
    CHECK(first[0] == 's' && second[0] == 's');
    first[0] = 0x11;
    first[PAGE + 5] = 0x22;
    // Both mappings reference the same page frames, so writes are immediately visible.
    CHECK(second[0] == 0x11);
    CHECK(second[PAGE + 5] == 0x22);
    second[100] = 0x33;
    CHECK(first[100] == 0x33);

    CHECK(SC(227, P(first), LENGTH, MS_SYNC) == 0);
    unsigned char bytes[LENGTH];
    read_at(fd, bytes, LENGTH, 0);
    CHECK(bytes[0] == 0x11 && bytes[100] == 0x33 && bytes[PAGE + 5] == 0x22);

    // Data reaches the file when a mapping is removed, even through another mapping.
    first[200] = 0x44;
    unmap((void *)first, LENGTH);
    CHECK(second[200] == 0x44);
    unmap((void *)second, LENGTH);
    read_at(fd, bytes, LENGTH, 0);
    CHECK(bytes[200] == 0x44);
    close_file(fd);
}

static void forked(void) {
    long fd = open_file("/dev/shm/arona-shm-fork", O_RDWR | O_CREAT | O_TRUNC, 0600);
    resize_file(fd, PAGE);
    volatile unsigned char *parent = shared_map(fd, PAGE, PROT_READ | PROT_WRITE);
    parent[0] = 0x5a;
    parent[10] = 0xa5;

    long pid = SC(220, SIGCHLD, 0, 0, 0, 0);
    CHECK(pid >= 0);
    if (pid == 0) {
        // The child maps the file through its own descriptor and reaches the same pages.
        long child_fd = open_file("/dev/shm/arona-shm-fork", O_RDWR, 0);
        volatile unsigned char *child = shared_map(child_fd, PAGE, PROT_READ | PROT_WRITE);
        CHECK(child[0] == 0x5a);
        CHECK(child[10] == 0xa5);
        child[20] = 0x77;
        close_file(child_fd);
        SC(94, 0);
        for (;;) {}
    }
    int status = 0;
    CHECK(SC(260, pid, P(&status), 0, 0) == pid);
    CHECK((status & 0x7f) == 0);
    CHECK(((status >> 8) & 0xff) == 0);
    CHECK(parent[20] == 0x77);

    unmap((void *)parent, PAGE);
    unsigned char byte = 0;
    read_at(fd, &byte, 1, 20);
    CHECK(byte == 0x77);
    close_file(fd);
}

static void truncated(void) {
    long fd = open_file("/dev/shm/arona-shm-truncate", O_RDWR | O_CREAT | O_TRUNC, 0600);
    resize_file(fd, LENGTH);
    volatile unsigned char *mapping = shared_map(fd, LENGTH, PROT_READ | PROT_WRITE);
    mapping[0] = 1;
    mapping[PAGE] = 2;
    CHECK(SC(227, P(mapping), LENGTH, MS_SYNC) == 0);

    long pid = SC(220, SIGCHLD, 0, 0, 0, 0);
    CHECK(pid >= 0);
    if (pid == 0) {
        // Shrinking the file invalidates the pages that now lie beyond the end.
        resize_file(fd, PAGE);
        CHECK(mapping[0] == 1);
        (void)mapping[PAGE];
        SC(94, 0);
        for (;;) {}
    }
    int status = 0;
    CHECK(SC(260, pid, P(&status), 0, 0) == pid);
    CHECK((status & 0x7f) == SIGBUS);

    unsigned char byte = 0;
    read_at(fd, &byte, 1, 0);
    CHECK(byte == 1);
    // Growing the file again makes the range accessible and zero-filled.
    resize_file(fd, LENGTH);
    CHECK(mapping[PAGE] == 0);
    mapping[PAGE] = 0x42;
    CHECK(SC(227, P(mapping), LENGTH, MS_SYNC) == 0);
    read_at(fd, &byte, 1, PAGE);
    CHECK(byte == 0x42);
    unmap((void *)mapping, LENGTH);
    close_file(fd);
}

static void errors(void) {
    long fd = open_file("/dev/shm/arona-shm-errors", O_RDWR | O_CREAT | O_TRUNC, 0600);
    resize_file(fd, PAGE);
    close_file(fd);

    long read_only = open_file("/dev/shm/arona-shm-errors", O_RDONLY, 0);
    CHECK(SC(222, 0, PAGE, PROT_READ | PROT_WRITE, MAP_SHARED, read_only, 0) == -EACCES);
    CHECK(SC(222, 0, PAGE, PROT_WRITE, MAP_SHARED, read_only, 0) == -EACCES);
    long readonly_mapping = SC(222, 0, PAGE, PROT_READ, MAP_SHARED, read_only, 0);
    CHECK(readonly_mapping > 0);
    CHECK(SC(226, readonly_mapping, PAGE, PROT_READ | PROT_WRITE) == -EACCES);
    unmap((void *)readonly_mapping, PAGE);
    // Private writable mappings copy pages, so read access to the descriptor is enough.
    long private = SC(222, 0, PAGE, PROT_READ | PROT_WRITE, MAP_PRIVATE, read_only, 0);
    CHECK(private > 0);
    ((volatile unsigned char *)private)[0] = 0x66;
    unmap((void *)private, PAGE);
    unsigned char byte = 1;
    read_at(read_only, &byte, 1, 0);
    CHECK(byte == 0);
    close_file(read_only);

    long write_only = open_file("/dev/shm/arona-shm-errors", O_WRONLY, 0);
    CHECK(SC(222, 0, PAGE, PROT_READ, MAP_SHARED, write_only, 0) == -EACCES);
    CHECK(SC(222, 0, PAGE, PROT_READ, MAP_PRIVATE, write_only, 0) == -EACCES);
    close_file(write_only);

    long writable = open_file("/dev/shm/arona-shm-errors", O_RDWR, 0);
    CHECK(SC(222, 0, PAGE, PROT_READ, MAP_SHARED, writable, 1) == -EINVAL);
    CHECK(SC(222, 0, PAGE, PROT_READ, 0, writable, 0) == -EINVAL);
    close_file(writable);
}

static void coherent(const char *path) {
    long fd = open_file(path, O_RDWR | O_CREAT | O_TRUNC, 0600);
    resize_file(fd, PAGE);
    volatile unsigned char *mapping = shared_map(fd, PAGE, PROT_READ | PROT_WRITE);
    mapping[0] = 0x51;
    unsigned char byte = 0;
    read_at(fd, &byte, 1, 0);
    CHECK(byte == 0x51);
    byte = 0x62;
    CHECK(SC(68, fd, P(&byte), 1, 1) == 1);
    CHECK(mapping[1] == 0x62);
    CHECK(SC(227, P(mapping), PAGE, MS_SYNC) == 0);
    read_at(fd, &byte, 1, 1);
    CHECK(byte == 0x62);
    mapping[100] = 0x77;
    mapping[PAGE - 1] = 0x78;
    resize_file(fd, 100);
    resize_file(fd, PAGE);
    CHECK(mapping[0] == 0x51 && mapping[1] == 0x62);
    CHECK(mapping[100] == 0 && mapping[PAGE - 1] == 0);
    close_file(fd);
    CHECK(SC(35, AT_FDCWD, P(path), 0) == 0);
    long replacement = open_file(path, O_RDWR | O_CREAT | O_TRUNC, 0600);
    resize_file(replacement, PAGE);
    volatile unsigned char *fresh = shared_map(replacement, PAGE, PROT_READ | PROT_WRITE);
    CHECK(fresh[0] == 0);
    fresh[0] = 0x33;
    CHECK(mapping[0] == 0x51);
    unmap((void *)mapping, PAGE);
    unmap((void *)fresh, PAGE);
    close_file(replacement);
    replacement = open_file(path, O_RDONLY, 0);
    read_at(replacement, &byte, 1, 0);
    CHECK(byte == 0x33);
    close_file(replacement);
    CHECK(SC(35, AT_FDCWD, P(path), 0) == 0);
}

static void file_futex(void) {
    const char *path = "/dev/shm/arona-futex";
    long fd = open_file(path, O_RDWR | O_CREAT | O_TRUNC, 0600);
    resize_file(fd, PAGE);
    volatile unsigned *first = (volatile unsigned *)shared_map(fd, PAGE, 3);
    volatile unsigned *second = (volatile unsigned *)shared_map(fd, PAGE, 3);
    long child = SC(220, SIGCHLD, 0, 0, 0, 0);
    CHECK(child >= 0);
    if (child == 0) {
        long timeout[2] = {10, 0};
        second[1] = 1;
        CHECK(SC(98, P(second), 0, 0, P(timeout)) == 0);
        SC(94, 0);
        for (;;) {}
    }
    while (first[1] == 0) CHECK(SC(124) == 0);
    long wake = 0;
    for (unsigned attempt = 0; attempt < 10000 && wake == 0; ++attempt) {
        wake = SC(98, P(first), 1, 1);
        CHECK(wake >= 0);
        if (wake == 0) CHECK(SC(124) == 0);
    }
    CHECK(wake == 1);
    int status = 0;
    CHECK(SC(260, child, P(&status), 0, 0) == child && status == 0);
    unmap((void *)first, PAGE);
    unmap((void *)second, PAGE);
    close_file(fd);
    CHECK(SC(35, AT_FDCWD, P(path), 0) == 0);
}

int shm_main(void) {
    temporary();
    forked();
    truncated();
    errors();
    coherent("/dev/shm/arona-coherent");
    coherent("/var/arona-coherent");
    file_futex();
    return 42;
}
