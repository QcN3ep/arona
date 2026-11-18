// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

// Guest checks for the in-memory /tmp filesystem and the mount calls.

#include "fs.h"

#define AT_FDCWD (-100)
#define AT_REMOVEDIR 0x200
#define O_RDONLY 0
#define O_WRONLY 1
#define O_RDWR 2
#define O_CREAT 0x40
#define O_EXCL 0x80
#define O_TRUNC 0x200
#define EPERM 1
#define ENOENT 2
#define EACCES 13
#define EOPNOTSUPP 95
#define ENOSPC 28
#define ENOMEM 12
#define EBUSY 16
#define ENODEV 19
#define EEXIST 17
#define EINVAL 22
#define HUGE (2 * 1024 * 1024L)
#define MAP_SHARED 1
#define MAP_PRIVATE 2
#define MAP_FIXED 0x10
#define MAP_ANONYMOUS 0x20
#define MAP_HUGETLB 0x40000

void fs_fail(unsigned line) {
    char text[] = "Filesystem assertion line 0000\n";
    char *digits = text + sizeof(text) - 6;
    for (int index = 3; index >= 0; --index) {
        digits[index] = '0' + line % 10;
        line /= 10;
    }
    SC(64, 1, P(text), sizeof(text) - 1);
    SC(94, 1);
    for (;;) {}
}

static void write_all(long fd, const char *bytes, long length) {
    while (length > 0) {
        long count = SC(64, fd, P(bytes), length);
        CHECK(count > 0 && count <= length);
        bytes += count;
        length -= count;
    }
}

static void read_exact(long fd, char *bytes, long length) {
    while (length > 0) {
        long count = SC(63, fd, P(bytes), length);
        CHECK(count > 0 && count <= length);
        bytes += count;
        length -= count;
    }
}

static long open_file(const char *path, long flags, long mode) {
    return SC(56, AT_FDCWD, P(path), flags, mode);
}

static void temporary(void) {
    long fd = open_file("/tmp/arona-fs", O_RDWR | O_CREAT | O_TRUNC, 0600);
    CHECK(fd >= 0);
    write_all(fd, "hello", 5);
    CHECK(SC(62, fd, 0, 0) == 0);
    char content[8] = {0};
    CHECK(SC(63, fd, P(content), 5) == 5);
    CHECK(content[0] == 'h' && content[1] == 'e' && content[4] == 'o');
    CHECK(SC(46, fd, 2) == 0);
    CHECK(SC(62, fd, 0, 0) == 0);
    char buffer[8] = {0};
    CHECK(SC(63, fd, P(buffer), 8) == 2);
    CHECK(buffer[0] == 'h' && buffer[1] == 'e');
    CHECK(SC(57, fd) == 0);

    // The file survives closing and reopening because tmpfs keeps it in memory.
    fd = open_file("/tmp/arona-fs", O_RDONLY, 0);
    CHECK(fd >= 0);
    CHECK(SC(63, fd, P(buffer), 8) == 2);
    CHECK(buffer[0] == 'h' && buffer[1] == 'e');
    CHECK(SC(57, fd) == 0);

    // Writing after the end of file leaves a zero-filled hole.
    fd = open_file("/tmp/arona-fs", O_RDWR | O_TRUNC, 0);
    CHECK(fd >= 0);
    CHECK(SC(62, fd, 4, 0) == 4);
    write_all(fd, "X", 1);
    CHECK(SC(62, fd, 0, 2) == 5);
    CHECK(SC(62, fd, 0, 0) == 0);
    char sparse[5] = {0};
    read_exact(fd, sparse, 5);
    CHECK(sparse[0] == 0 && sparse[3] == 0 && sparse[4] == 'X');
    CHECK(SC(57, fd) == 0);

    CHECK(SC(34, AT_FDCWD, P("/tmp/arona-dir"), 0755) == 0);
    CHECK(SC(38, AT_FDCWD, P("/tmp/arona-fs"), AT_FDCWD, P("/tmp/arona-dir/file")) == 0);
    fd = open_file("/tmp/arona-dir/file", O_RDONLY, 0);
    CHECK(fd >= 0);
    CHECK(SC(57, fd) == 0);
    CHECK(SC(35, AT_FDCWD, P("/tmp/arona-dir/file"), 0) == 0);
    CHECK(SC(35, AT_FDCWD, P("/tmp/arona-dir"), AT_REMOVEDIR) == 0);
    // Renaming moved the file, so the old name no longer exists.
    CHECK(SC(35, AT_FDCWD, P("/tmp/arona-fs"), 0) == -ENOENT);
    CHECK(open_file("/tmp/arona-fs", O_RDONLY, 0) == -ENOENT);

    // An open session keeps its inode alive after the last name is removed.
    fd = open_file("/tmp/arona-unlinked", O_RDWR | O_CREAT | O_TRUNC, 0600);
    CHECK(fd >= 0);
    CHECK(SC(35, AT_FDCWD, P("/tmp/arona-unlinked"), 0) == 0);
    CHECK(open_file("/tmp/arona-unlinked", O_RDONLY, 0) == -ENOENT);
    write_all(fd, "kept", 4);
    CHECK(SC(62, fd, 0, 0) == 0);
    char live[4] = {0};
    read_exact(fd, live, 4);
    CHECK(live[0] == 'k' && live[3] == 't');
    CHECK(SC(57, fd) == 0);
}

static void mounted_tmpfs(const char *point, const char *file) {
    CHECK(SC(34, AT_FDCWD, P(point), 0755) == 0);
    CHECK(SC(40, P("tmpfs"), P(point), P("tmpfs"), 0, 0) == 0);
    long fd = open_file(file, O_WRONLY | O_CREAT | O_TRUNC, 0640);
    CHECK(fd >= 0);
    write_all(fd, "mounted", 7);
    CHECK(SC(57, fd) == 0);
    fd = open_file(file, O_RDONLY, 0);
    CHECK(fd >= 0);
    char content[7] = {0};
    read_exact(fd, content, 7);
    CHECK(content[0] == 'm' && content[6] == 'd');
    CHECK(SC(57, fd) == 0);
    // Mounting over the same point reports a busy mount.
    CHECK(SC(40, P("tmpfs"), P(point), P("tmpfs"), 0, 0) == -EBUSY);
    CHECK(SC(39, P(point), 0) == 0);
    CHECK(open_file(file, O_RDONLY, 0) == -ENOENT);
    // The directory beneath the mount becomes visible again.
    CHECK(SC(34, AT_FDCWD, P(point), 0755) == -EEXIST);
    CHECK(SC(35, AT_FDCWD, P(point), AT_REMOVEDIR) == 0);
}

static void nested(void) {
    CHECK(SC(34, AT_FDCWD, P("/tmp/arona-nest"), 0755) == 0);
    CHECK(SC(40, P("tmpfs"), P("/tmp/arona-nest"), P("tmpfs"), 0, 0) == 0);
    CHECK(SC(34, AT_FDCWD, P("/tmp/arona-nest/inner"), 0755) == 0);
    CHECK(SC(40, P("tmpfs"), P("/tmp/arona-nest/inner"), P("tmpfs"), 0, 0) == 0);
    long fd = open_file("/tmp/arona-nest/inner/file", O_RDWR | O_CREAT, 0600);
    CHECK(fd >= 0);
    write_all(fd, "inner", 5);
    CHECK(SC(57, fd) == 0);
    // The outer mount cannot be removed while a nested mount exists.
    CHECK(SC(39, P("/tmp/arona-nest"), 0) == -EBUSY);
    CHECK(SC(39, P("/tmp/arona-nest/inner"), 0) == 0);
    CHECK(open_file("/tmp/arona-nest/inner/file", O_RDONLY, 0) == -ENOENT);
    CHECK(SC(39, P("/tmp/arona-nest"), 0) == 0);
    // Unmounting exposes the directory that was hidden beneath the mount.
    CHECK(SC(34, AT_FDCWD, P("/tmp/arona-nest"), 0755) == -EEXIST);
    CHECK(SC(35, AT_FDCWD, P("/tmp/arona-nest"), AT_REMOVEDIR) == 0);
}

static void errors(void) {
    // The boot archive is read-only, so a user mount must not shadow it.
    CHECK(SC(40, P("tmpfs"), P("/bin"), P("tmpfs"), 0, 0) == -EPERM);
    CHECK(SC(40, P("tmpfs"), P("/etc"), P("tmpfs"), 0, 0) == -EPERM);
    CHECK(SC(40, P("tmpfs"), P("/"), P("tmpfs"), 0, 0) == -EPERM);
    CHECK(SC(40, P("hugetlbfs"), P("/etc"), P("hugetlbfs"), 0, 0) == -EPERM);
    CHECK(SC(34, AT_FDCWD, P("/tmp/arona-plain"), 0755) == 0);
    CHECK(SC(40, P("tmpfs"), P("/tmp/arona-missing"), P("tmpfs"), 0, 0) == -ENOENT);
    CHECK(SC(40, P("ext4"), P("/tmp/arona-plain"), P("ext4"), 0, 0) == -ENODEV);
    // Read-only and bind-style flags are not implemented for memory mounts.
    CHECK(SC(40, P("tmpfs"), P("/tmp/arona-plain"), P("tmpfs"), 1, 0) == -EINVAL);
    // MS_NOSUID, MS_NODEV, and MS_NOEXEC are rejected until the namespace
    // enforces them instead of silently accepting a no-op.
    CHECK(SC(40, P("tmpfs"), P("/tmp/arona-plain"), P("tmpfs"), 2, 0) == -EINVAL);
    CHECK(SC(40, P("tmpfs"), P("/tmp/arona-plain"), P("tmpfs"), 4, 0) == -EINVAL);
    CHECK(SC(40, P("tmpfs"), P("/tmp/arona-plain"), P("tmpfs"), 8, 0) == -EINVAL);
    // The flag word must be validated as a full 32-bit value; high bits are not
    // truncated into a supported flag.
    CHECK(SC(40, P("tmpfs"), P("/tmp/arona-plain"), P("tmpfs"), 0x100000000L, 0) == -EINVAL);
    CHECK(SC(40, P("tmpfs"), P("/tmp/arona-plain"), P("tmpfs"), 4096, 0) == -EINVAL);
    CHECK(SC(40, P("tmpfs"), P("/tmp/arona-plain"), P("tmpfs"), 0, P("size=1m")) == -EINVAL);
    // Umounting a directory that is not a mount point is invalid.
    CHECK(SC(39, P("/tmp/arona-plain"), 0) == -EINVAL);
    CHECK(SC(39, P("/tmp/arona-missing"), 0) == -ENOENT);
    CHECK(SC(39, P("/tmp"), 1) == -EINVAL);
    // The startup /tmp mount belongs to the file service, so only mounts created
    // through the mount call can be removed here.
    CHECK(SC(39, P("/tmp"), 0) == -EINVAL);
    CHECK(SC(35, AT_FDCWD, P("/tmp/arona-plain"), AT_REMOVEDIR) == 0);
}

static void hugepages(void) {
    const char *name = "/dev/hugepages/arona-huge";
    long cleanup = SC(35, AT_FDCWD, P(name), 0);
    CHECK(cleanup == 0 || cleanup == -ENOENT);
    long fd = open_file(name, O_RDWR | O_CREAT | O_TRUNC, 0600);
    CHECK(fd >= 0);
    // Truncation must use whole 2 MiB units.
    CHECK(SC(46, fd, HUGE + 1) == -EINVAL);
    CHECK(SC(46, fd, HUGE) == 0);
    CHECK(SC(62, fd, 0, 2) == HUGE);

    // A shared file mapping receives the block storage directly.
    long address = SC(222, 0, HUGE, 3, MAP_SHARED, fd, 0);
    CHECK(address > 0 && address % HUGE == 0);
    *(volatile unsigned char *)address = 'A';
    *(volatile unsigned char *)(address + HUGE - 1) = 'B';
    char byte = 0;
    CHECK(SC(62, fd, 0, 0) == 0);
    CHECK(SC(63, fd, P(&byte), 1) == 1 && byte == 'A');
    CHECK(SC(62, fd, HUGE - 1, 0) == HUGE - 1);
    CHECK(SC(63, fd, P(&byte), 1) == 1 && byte == 'B');
    CHECK(SC(62, fd, 4, 0) == 4);
    write_all(fd, "C", 1);
    CHECK(*(volatile unsigned char *)(address + 4) == 'C');

    // Unmapping releases the address space view but not the file contents.
    CHECK(SC(215, address, HUGE) == 0);
    address = SC(222, 0, HUGE, 3, MAP_SHARED, fd, 0);
    CHECK(address > 0 && address % HUGE == 0);
    CHECK(*(volatile unsigned char *)address == 'A');
    CHECK(*(volatile unsigned char *)(address + 4) == 'C');
    CHECK(SC(215, address, HUGE) == 0);

    // Address, length, offset, and flag encodings are validated.
    CHECK(SC(222, 0, HUGE, 3, MAP_SHARED, fd, 4096) == -EINVAL);
    long rounded_file = SC(222, 4096, 4096, 3, MAP_SHARED, fd, 0);
    CHECK(rounded_file > 0 && rounded_file % HUGE == 0);
    CHECK(SC(215, rounded_file, HUGE) == 0);
    CHECK(SC(222, 4096, HUGE, 3, MAP_FIXED | MAP_SHARED, fd, 0) == -EINVAL);
    CHECK(SC(222, 0, HUGE, 3, MAP_SHARED | (22 << 26), fd, 0) == -EINVAL);
    long rounded_anon = SC(222, 4096, HUGE + 4096, 3, MAP_PRIVATE | MAP_ANONYMOUS | MAP_HUGETLB, -1, 0);
    CHECK(rounded_anon > 0 && rounded_anon % HUGE == 0);
    CHECK(SC(226, rounded_anon, 2 * HUGE, 1) == 0);
    CHECK(SC(226, rounded_anon, 2 * HUGE, 3) == 0);
    CHECK(SC(215, rounded_anon, 2 * HUGE) == 0);
    CHECK(SC(222, 4096, HUGE, 3, MAP_FIXED | MAP_ANONYMOUS | MAP_HUGETLB, -1, 0) == -EINVAL);

    // An attacker-controlled length cannot reach the block vector: the mapping
    // record budget bounds one mapping, and the whole address range is checked
    // before any block is materialized.
    CHECK(SC(222, 0, (long)((1UL << 62)), 3, MAP_PRIVATE | MAP_ANONYMOUS | MAP_HUGETLB, -1, 0) == -ENOMEM);
    const long near_end = (long)(~0UL & ~(HUGE - 1));
    long overflow = SC(222, near_end, 2 * HUGE, 3, MAP_FIXED | MAP_ANONYMOUS | MAP_HUGETLB, -1, 0);
    CHECK(overflow == -ENOMEM || overflow == -EINVAL);
    // The service remains usable after both rejected requests.
    long alive = SC(222, 0, HUGE, 3, MAP_PRIVATE | MAP_ANONYMOUS | MAP_HUGETLB, -1, 0);
    CHECK(alive > 0);
    CHECK(*(volatile unsigned char *)alive == 0);
    CHECK(SC(215, alive, HUGE) == 0);

    // Anonymous huge mappings are shared across fork and reused after unmap.
    long shared = SC(222, 0, HUGE, 3, MAP_SHARED | MAP_ANONYMOUS | MAP_HUGETLB, -1, 0);
    CHECK(shared > 0 && shared % HUGE == 0);
    *(volatile unsigned char *)shared = 'P';
    long child = SC(220, 17, 0, 0, 0, 0);
    CHECK(child >= 0);
    if (child == 0) {
        CHECK(*(volatile unsigned char *)shared == 'P');
        *(volatile unsigned char *)(shared + 4096) = 'S';
        SC(94, 0);
        for (;;) {}
    }
    long status = 0;
    CHECK(SC(260, child, P(&status), 0, 0) == child && status == 0);
    CHECK(*(volatile unsigned char *)(shared + 4096) == 'S');
    CHECK(SC(215, shared, HUGE) == 0);
    for (int iteration = 0; iteration < 16; ++iteration) {
        long reused = SC(222, 0, HUGE, 3, MAP_PRIVATE | MAP_ANONYMOUS | MAP_HUGETLB, -1, 0);
        CHECK(reused > 0 && reused % HUGE == 0);
        *(volatile unsigned char *)reused = (unsigned char)iteration;
        CHECK(*(volatile unsigned char *)reused == (unsigned char)iteration);
        CHECK(SC(215, reused, HUGE) == 0);
    }

    // Accessing the part of a shared mapping beyond end of file raises SIGBUS.
    const long beyond_address = 0x20000000L;
    long beyond = SC(222, beyond_address, 2 * HUGE, 3, MAP_SHARED | MAP_FIXED, fd, 0);
    CHECK(beyond > 0 && beyond % HUGE == 0);
    child = SC(220, 17, 0, 0, 0, 0);
    CHECK(child >= 0);
    if (child == 0) {
        volatile unsigned char value = *(volatile unsigned char *)(beyond + HUGE);
        (void)value;
        SC(94, 0);
        for (;;) {}
    }
    status = 0;
    CHECK(SC(260, child, P(&status), 0, 0) == child && (status & 127) == 7);
    CHECK(SC(215, beyond, 2 * HUGE) == 0);

    // Descriptor access modes bound the mapping permissions, and MAP_PRIVATE
    // file mappings are not implemented because they would alias file frames.
    long readable = open_file(name, O_RDONLY, 0);
    CHECK(readable >= 0);
    CHECK(SC(222, 0, HUGE, 3, MAP_SHARED, readable, 0) == -EACCES);
    long read_only = SC(222, 0, HUGE, 1, MAP_SHARED, readable, 0);
    CHECK(read_only > 0 && read_only % HUGE == 0);
    CHECK(SC(226, read_only, HUGE, 3) == -EACCES);
    CHECK(SC(215, read_only, HUGE) == 0);
    CHECK(SC(57, readable) == 0);
    long writable = open_file(name, O_WRONLY, 0);
    CHECK(writable >= 0);
    CHECK(SC(222, 0, HUGE, 3, MAP_SHARED, writable, 0) == -EACCES);
    CHECK(SC(222, 0, HUGE, 3, MAP_PRIVATE, writable, 0) == -EACCES);
    CHECK(SC(57, writable) == 0);
    CHECK(SC(222, 0, HUGE, 3, MAP_PRIVATE, fd, 0) == -EOPNOTSUPP);
    CHECK(SC(57, fd) == 0);
    CHECK(SC(35, AT_FDCWD, P(name), 0) == 0);
    CHECK(open_file(name, O_RDONLY, 0) == -ENOENT);

    // A private huge block is copied on fork, so the child write stays private.
    long private = SC(222, 0, HUGE, 3, MAP_PRIVATE | MAP_ANONYMOUS | MAP_HUGETLB, -1, 0);
    CHECK(private > 0 && private % HUGE == 0);
    *(volatile unsigned char *)private = 'M';
    child = SC(220, 17, 0, 0, 0, 0);
    CHECK(child >= 0);
    if (child == 0) {
        CHECK(*(volatile unsigned char *)private == 'M');
        *(volatile unsigned char *)private = 'C';
        SC(94, 0);
        for (;;) {}
    }
    status = 0;
    CHECK(SC(260, child, P(&status), 0, 0) == child && status == 0);
    CHECK(*(volatile unsigned char *)private == 'M');
    CHECK(SC(215, private, HUGE) == 0);

    // A MAP_SHARED huge file mapping keeps its block across fork, so writes through
    // either process are visible to the other and reach the file.
    const char *fork_name = "/dev/hugepages/arona-huge-fork";
    long fork_cleanup = SC(35, AT_FDCWD, P(fork_name), 0);
    CHECK(fork_cleanup == 0 || fork_cleanup == -ENOENT);
    long fork_fd = open_file(fork_name, O_RDWR | O_CREAT | O_TRUNC, 0600);
    CHECK(fork_fd >= 0);
    CHECK(SC(46, fork_fd, HUGE) == 0);
    long file_shared = SC(222, 0, HUGE, 3, MAP_SHARED, fork_fd, 0);
    CHECK(file_shared > 0 && file_shared % HUGE == 0);
    *(volatile unsigned char *)(file_shared + 8) = 'F';
    child = SC(220, 17, 0, 0, 0, 0);
    CHECK(child >= 0);
    if (child == 0) {
        CHECK(*(volatile unsigned char *)(file_shared + 8) == 'F');
        *(volatile unsigned char *)(file_shared + 9) = 'H';
        SC(94, 0);
        for (;;) {}
    }
    status = 0;
    CHECK(SC(260, child, P(&status), 0, 0) == child && status == 0);
    CHECK(*(volatile unsigned char *)(file_shared + 9) == 'H');
    char fork_byte = 0;
    CHECK(SC(62, fork_fd, 8, 0) == 8);
    CHECK(SC(63, fork_fd, P(&fork_byte), 1) == 1 && fork_byte == 'F');
    CHECK(SC(62, fork_fd, 9, 0) == 9);
    CHECK(SC(63, fork_fd, P(&fork_byte), 1) == 1 && fork_byte == 'H');
    *(volatile unsigned char *)(file_shared + 10) = 'P';
    CHECK(SC(62, fork_fd, 10, 0) == 10);
    CHECK(SC(63, fork_fd, P(&fork_byte), 1) == 1 && fork_byte == 'P');
    CHECK(SC(215, file_shared, HUGE) == 0);
    CHECK(SC(57, fork_fd) == 0);
    CHECK(SC(35, AT_FDCWD, P(fork_name), 0) == 0);

    // Anonymous huge mappings above 2 MiB are allocated and mapped per block.
    long wide = SC(222, 0, 2 * HUGE, 3, MAP_PRIVATE | MAP_ANONYMOUS | MAP_HUGETLB, -1, 0);
    CHECK(wide > 0 && wide % HUGE == 0);
    *(volatile unsigned char *)wide = 'L';
    *(volatile unsigned char *)(wide + 2 * HUGE - 1) = 'R';
    CHECK(*(volatile unsigned char *)wide == 'L');
    CHECK(*(volatile unsigned char *)(wide + 2 * HUGE - 1) == 'R');
    CHECK(SC(215, wide, 2 * HUGE) == 0);

    // A failed block allocation does not commit the extended file size.
    const char *limited_name = "/dev/hugepages/arona-huge-limit";
    long limited = open_file(limited_name, O_RDWR | O_CREAT | O_TRUNC, 0600);
    CHECK(limited >= 0);
    CHECK(SC(46, limited, 32 * HUGE) == 0);
    char marker = 'x';
    for (long index = 0; index < 32; ++index) {
        CHECK(SC(62, limited, index * HUGE, 0) == index * HUGE);
        write_all(limited, &marker, 1);
    }
    CHECK(SC(62, limited, 0, 2) == 32 * HUGE);
    CHECK(SC(62, limited, 32 * HUGE, 0) == 32 * HUGE);
    CHECK(SC(64, limited, P(&marker), 1) == -ENOSPC);
    CHECK(SC(62, limited, 0, 2) == 32 * HUGE);

    // A failed MAP_FIXED replacement keeps the previous mapping: the replacement
    // file has no materialized blocks and the volume is full, so materialization
    // fails before the target range is unmapped.
    const char *fresh_name = "/dev/hugepages/arona-huge-fresh";
    long fresh_cleanup = SC(35, AT_FDCWD, P(fresh_name), 0);
    CHECK(fresh_cleanup == 0 || fresh_cleanup == -ENOENT);
    long fresh = open_file(fresh_name, O_RDWR | O_CREAT | O_TRUNC, 0600);
    CHECK(fresh >= 0);
    CHECK(SC(46, fresh, 2 * HUGE) == 0);
    const long fixed_address = 0x30000000L;
    long replaced = SC(222, fixed_address, HUGE, 3, MAP_SHARED | MAP_FIXED, limited, 0);
    CHECK(replaced == fixed_address);
    *(volatile unsigned char *)replaced = 'K';
    CHECK(SC(222, fixed_address, 2 * HUGE, 3, MAP_SHARED | MAP_FIXED, fresh, 0) == -ENOSPC);
    CHECK(*(volatile unsigned char *)fixed_address == 'K');
    char fixed_byte = 0;
    CHECK(SC(62, limited, 0, 0) == 0);
    CHECK(SC(63, limited, P(&fixed_byte), 1) == 1 && fixed_byte == 'K');
    CHECK(SC(215, fixed_address, HUGE) == 0);
    CHECK(SC(57, fresh) == 0);
    CHECK(SC(35, AT_FDCWD, P(fresh_name), 0) == 0);

    CHECK(SC(57, limited) == 0);
    CHECK(SC(35, AT_FDCWD, P(limited_name), 0) == 0);

    // Truncating a mapped block cannot refund its volume charge. A fresh volume
    // fits 32 blocks, so the 33rd mapping reports ENOSPC even though every file
    // was truncated back to zero; unmapping then lets statfs report the space again.
    const char *ledger_name = "/tmp/arona-huge-ledger/ledger";
    CHECK(SC(34, AT_FDCWD, P("/tmp/arona-huge-ledger"), 0755) == 0);
    CHECK(SC(40, P("none"), P("/tmp/arona-huge-ledger"), P("hugetlbfs"), 0, 0) == 0);
    long ledger = open_file(ledger_name, O_RDWR | O_CREAT | O_TRUNC, 0600);
    CHECK(ledger >= 0);
    long ledger_addresses[40];
    long ledger_count = 0;
    for (long iteration = 0; iteration < 40; ++iteration) {
        CHECK(SC(46, ledger, HUGE) == 0);
        long mapped = SC(222, 0, HUGE, 3, MAP_SHARED, ledger, 0);
        if (mapped < 0) {
            CHECK(mapped == -ENOSPC);
            break;
        }
        ledger_addresses[ledger_count++] = mapped;
        CHECK(SC(46, ledger, 0) == 0);
    }
    CHECK(ledger_count == 32);
    for (long index = 0; index < ledger_count; ++index) {
        CHECK(SC(215, ledger_addresses[index], HUGE) == 0);
    }
    long stats[15];
    CHECK(SC(44, ledger, P(stats)) == 0);
    CHECK(stats[3] >= 16000);

    // A partial huge unmap is rejected before earlier runs are removed.
    const long mixed = 0x24000000L;
    CHECK(SC(222, mixed, 4096, 3, MAP_FIXED | MAP_ANONYMOUS | MAP_PRIVATE, -1, 0) == mixed);
    CHECK(SC(46, ledger, 2 * HUGE) == 0);
    long huge_part = SC(222, mixed + HUGE, HUGE, 3, MAP_SHARED | MAP_FIXED, ledger, 0);
    CHECK(huge_part == mixed + HUGE);
    CHECK(SC(215, mixed, HUGE + 4096) == -EINVAL);
    *(volatile unsigned char *)mixed = 'Z';
    CHECK(*(volatile unsigned char *)mixed == 'Z');
    CHECK(SC(215, huge_part, HUGE) == 0);
    CHECK(SC(215, mixed, 4096) == 0);
    CHECK(SC(46, ledger, 0) == 0);
    CHECK(SC(57, ledger) == 0);
    CHECK(SC(35, AT_FDCWD, P(ledger_name), 0) == 0);
    CHECK(SC(39, P("/tmp/arona-huge-ledger"), 0) == 0);
    CHECK(SC(35, AT_FDCWD, P("/tmp/arona-huge-ledger"), AT_REMOVEDIR) == 0);

    // Mount and unmount a huge page volume through the Linux names.
    CHECK(SC(34, AT_FDCWD, P("/tmp/arona-huge-mnt"), 0755) == 0);
    CHECK(SC(40, P("none"), P("/tmp/arona-huge-mnt"), P("hugetlbfs"), 0, 0) == 0);
    long mounted = open_file("/tmp/arona-huge-mnt/file", O_RDWR | O_CREAT | O_TRUNC, 0600);
    CHECK(mounted >= 0);
    CHECK(SC(46, mounted, HUGE) == 0);
    CHECK(SC(57, mounted) == 0);
    CHECK(SC(39, P("/tmp/arona-huge-mnt"), 0) == 0);
    CHECK(SC(35, AT_FDCWD, P("/tmp/arona-huge-mnt"), AT_REMOVEDIR) == 0);
    // The startup /dev/hugepages mount belongs to the service, not to mount(2).
    CHECK(SC(39, P("/dev/hugepages"), 0) == -EINVAL);

    write_all(1, "Linux hugepages tests passed\n", 29);
}

// A mapped block stays charged until the last descriptor, name, and kernel mapping
// reference are gone. Every release path must retire the block instead of refunding it.
static void huge_release_accounting(void) {
    const char *point = "/tmp/arona-huge-release";
    const char *file = "/tmp/arona-huge-release/file";
    CHECK(SC(34, AT_FDCWD, P(point), 0755) == 0);
    CHECK(SC(40, P("none"), P(point), P("hugetlbfs"), 0, 0) == 0);

    long addresses[40];
    long stats[15];
    long count = 0;
    // A fresh volume holds 32 blocks. Closing and unlinking a file whose block is
    // still mapped must not make the block available again.
    for (long iteration = 0; iteration < 40; ++iteration) {
        long fd = open_file(file, O_RDWR | O_CREAT | O_TRUNC, 0600);
        CHECK(fd >= 0);
        CHECK(SC(46, fd, HUGE) == 0);
        long mapped = SC(222, 0, HUGE, 3, MAP_SHARED, fd, 0);
        if (mapped < 0) {
            CHECK(mapped == -ENOSPC);
            CHECK(iteration == 32);
            CHECK(SC(57, fd) == 0);
            CHECK(SC(35, AT_FDCWD, P(file), 0) == 0);
            break;
        }
        CHECK(mapped > 0 && mapped % HUGE == 0);
        addresses[count++] = mapped;
        if (iteration % 2 == 0) {
            CHECK(SC(35, AT_FDCWD, P(file), 0) == 0);
            CHECK(SC(57, fd) == 0);
        } else {
            CHECK(SC(57, fd) == 0);
            CHECK(SC(35, AT_FDCWD, P(file), 0) == 0);
        }
    }
    CHECK(count == 32);
    long observer = open_file(file, O_RDWR | O_CREAT | O_TRUNC, 0600);
    CHECK(observer >= 0);
    CHECK(SC(44, observer, P(stats)) == 0);
    CHECK(stats[3] == 0);
    for (long index = 0; index < count; ++index) {
        CHECK(SC(215, addresses[index], HUGE) == 0);
    }
    CHECK(SC(44, observer, P(stats)) == 0);
    CHECK(stats[3] >= 16000);
    CHECK(SC(57, observer) == 0);
    CHECK(SC(35, AT_FDCWD, P(file), 0) == 0);
    CHECK(SC(39, P(point), 0) == 0);
    CHECK(SC(35, AT_FDCWD, P(point), AT_REMOVEDIR) == 0);

    // Replacing the last name by rename releases through the same retirement. The
    // startup volume sits directly on the boot archive, so renaming inside it is not
    // confused with a cross-mount rename.
    const char *victim = "/dev/hugepages/arona-release-victim";
    const char *source = "/dev/hugepages/arona-release-source";
    count = 0;
    for (long iteration = 0; iteration < 40; ++iteration) {
        long fd = open_file(victim, O_RDWR | O_CREAT | O_TRUNC, 0600);
        CHECK(fd >= 0);
        CHECK(SC(46, fd, HUGE) == 0);
        long mapped = SC(222, 0, HUGE, 3, MAP_SHARED, fd, 0);
        if (mapped < 0) {
            CHECK(mapped == -ENOSPC);
            CHECK(iteration == 32);
            CHECK(SC(57, fd) == 0);
            CHECK(SC(35, AT_FDCWD, P(victim), 0) == 0);
            break;
        }
        CHECK(mapped > 0 && mapped % HUGE == 0);
        addresses[count++] = mapped;
        CHECK(SC(57, fd) == 0);
        long replacement = open_file(source, O_RDWR | O_CREAT | O_TRUNC, 0600);
        CHECK(replacement >= 0);
        CHECK(SC(57, replacement) == 0);
        CHECK(SC(38, AT_FDCWD, P(source), AT_FDCWD, P(victim)) == 0);
    }
    CHECK(count == 32);
    observer = open_file(source, O_RDWR | O_CREAT | O_TRUNC, 0600);
    CHECK(observer >= 0);
    CHECK(SC(44, observer, P(stats)) == 0);
    CHECK(stats[3] == 0);
    for (long index = 0; index < count; ++index) {
        CHECK(SC(215, addresses[index], HUGE) == 0);
    }
    CHECK(SC(44, observer, P(stats)) == 0);
    CHECK(stats[3] >= 16000);
    CHECK(SC(57, observer) == 0);
    CHECK(SC(35, AT_FDCWD, P(source), 0) == 0);
    long victim_cleanup = SC(35, AT_FDCWD, P(victim), 0);
    CHECK(victim_cleanup == 0 || victim_cleanup == -ENOENT);

    write_all(1, "Linux hugepage release tests passed\n", 36);
}

static void memory_budget(void) {
    const char *points[3] = {
        "/tmp/arona-budget-0",
        "/tmp/arona-budget-1",
        "/tmp/arona-budget-2",
    };
    for (int index = 0; index < 3; ++index) {
        CHECK(SC(34, AT_FDCWD, P(points[index]), 0755) == 0);
    }
    // The startup /tmp and /dev/hugepages volumes already reserve half of the
    // global 256 MiB budget, so only two more 64 MiB volumes fit.
    CHECK(SC(40, P("none"), P(points[0]), P("tmpfs"), 0, 0) == 0);
    CHECK(SC(40, P("none"), P(points[1]), P("tmpfs"), 0, 0) == 0);
    CHECK(SC(40, P("none"), P(points[2]), P("tmpfs"), 0, 0) == -ENOSPC);
    CHECK(SC(39, P(points[1]), 0) == 0);
    CHECK(SC(40, P("none"), P(points[2]), P("tmpfs"), 0, 0) == 0);
    CHECK(SC(39, P(points[0]), 0) == 0);
    CHECK(SC(39, P(points[2]), 0) == 0);
    for (int index = 0; index < 3; ++index) {
        CHECK(SC(35, AT_FDCWD, P(points[index]), AT_REMOVEDIR) == 0);
    }
}

// A 64 MiB mapping requests 32 blocks. Another process runs getpid in a loop;
// because the mapping waits asynchronously, that process completes many syscalls
// while the blocks are materialized. A synchronous round trip per block would
// stall the single Linux service loop and leave the counter unchanged.
static void large_mapping(void) {
    const char *point = "/tmp/arona-large";
    const char *file = "/tmp/arona-large/file";
    CHECK(SC(34, AT_FDCWD, P(point), 0755) == 0);
    CHECK(SC(40, P("none"), P(point), P("hugetlbfs"), 0, 0) == 0);
    long fd = open_file(file, O_RDWR | O_CREAT | O_TRUNC, 0600);
    CHECK(fd >= 0);
    const long length = 32 * HUGE;
    CHECK(SC(46, fd, length) == 0);

    long shared = SC(222, 0, 4096, 3, MAP_SHARED | MAP_ANONYMOUS, -1, 0);
    CHECK(shared > 0);
    volatile long *ready = (volatile long *)shared;
    volatile long *go = ready + 1;
    volatile long *stop = ready + 2;
    volatile unsigned long *counter = (volatile unsigned long *)(ready + 3);
    *ready = 0;
    *go = 0;
    *stop = 0;
    *counter = 0;
    long child = SC(220, 17, 0, 0, 0, 0);
    CHECK(child >= 0);
    if (child == 0) {
        *ready = 1;
        while (!*go) {}
        while (!*stop) {
            CHECK(SC(172) > 0);
            (*counter)++;
        }
        SC(94, 0);
        for (;;) {}
    }
    while (!*ready) {}
    *go = 1;
    long address = SC(222, 0, length, 3, MAP_SHARED, fd, 0);
    long observed = (long)*counter;
    *stop = 1;
    CHECK(address > 0 && address % HUGE == 0);
    CHECK(observed >= 4);

    for (long index = 0; index < 32; ++index) {
        *(volatile unsigned char *)(address + index * HUGE) = (unsigned char)(index + 1);
        *(volatile unsigned char *)(address + index * HUGE + HUGE - 1) =
            (unsigned char)(255 - index);
    }
    char byte = 0;
    for (long index = 0; index < 32; ++index) {
        CHECK(SC(62, fd, index * HUGE, 0) == index * HUGE);
        CHECK(SC(63, fd, P(&byte), 1) == 1 && byte == (char)(index + 1));
        CHECK(SC(62, fd, index * HUGE + HUGE - 1, 0) == index * HUGE + HUGE - 1);
        CHECK(SC(63, fd, P(&byte), 1) == 1 && byte == (char)(255 - index));
    }
    CHECK(SC(215, address, length) == 0);
    long status = 0;
    CHECK(SC(260, child, P(&status), 0, 0) == child && status == 0);
    CHECK(SC(215, shared, 4096) == 0);
    CHECK(SC(57, fd) == 0);
    CHECK(SC(35, AT_FDCWD, P(file), 0) == 0);
    CHECK(SC(39, P(point), 0) == 0);
    CHECK(SC(35, AT_FDCWD, P(point), AT_REMOVEDIR) == 0);
}

static void denied_mount(void) {
    const char *point = "/tmp/arona-denied-root";
    CHECK(SC(34, AT_FDCWD, P(point), 0755) == 0);
    // The process was launched without the mount capability even though it is
    // root, so both mount and umount2 are rejected before reaching the service.
    CHECK(SC(40, P("none"), P(point), P("tmpfs"), 0, 0) == -EPERM);
    CHECK(SC(39, P("/tmp"), 0) == -EPERM);
    CHECK(SC(35, AT_FDCWD, P(point), AT_REMOVEDIR) == 0);
}

static void mounted_links(void) {
    const char *point = "/tmp/arona-links";
    const char *first = "/tmp/arona-links/first";
    const char *second = "/tmp/arona-links/second";
    const char *third = "/tmp/arona-links/third";
    CHECK(SC(34, AT_FDCWD, P(point), 0755) == 0);
    CHECK(SC(40, P("tmpfs"), P(point), P("tmpfs"), 0, 0) == 0);
    long fd = open_file(first, O_RDWR | O_CREAT, 0600);
    CHECK(fd >= 0);
    write_all(fd, "rename", 6);
    CHECK(SC(57, fd) == 0);
    // renameat and linkat stay inside the mounted tmpfs, not crossing its parent.
    CHECK(SC(38, AT_FDCWD, P(first), AT_FDCWD, P(second)) == 0);
    CHECK(open_file(first, O_RDONLY, 0) == -ENOENT);
    CHECK(SC(37, AT_FDCWD, P(second), AT_FDCWD, P(third)) == 0);
    fd = open_file(third, O_RDONLY, 0);
    CHECK(fd >= 0);
    char content[6] = {0};
    read_exact(fd, content, 6);
    CHECK(content[0] == 'r' && content[5] == 'e');
    CHECK(SC(57, fd) == 0);
    CHECK(SC(39, P(point), 0) == 0);
    CHECK(SC(35, AT_FDCWD, P(point), AT_REMOVEDIR) == 0);
}


int fs_main(int argc, char **argv) {
    if (argc > 1 && argv[1][0] == 'd') {
        denied_mount();
        return 42;
    }
    temporary();
    hugepages();
    huge_release_accounting();
    mounted_tmpfs("/tmp/arona-mnt", "/tmp/arona-mnt/file");
    mounted_tmpfs("/var/arona-mnt", "/var/arona-mnt/file");
    mounted_links();
    nested();
    errors();
    memory_budget();
    large_mapping();
    // Credential checks reject mounting and unmounting for non-root callers.
    CHECK(SC(146, 1000) == 0);
    CHECK(SC(40, P("tmpfs"), P("/tmp/arona-denied"), P("tmpfs"), 0, 0) == -EPERM);
    CHECK(SC(39, P("/tmp"), 0) == -EPERM);
    return 42;
}
