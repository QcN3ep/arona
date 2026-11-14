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
#define EBUSY 16
#define ENODEV 19
#define EEXIST 17
#define EINVAL 22

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
    CHECK(SC(34, AT_FDCWD, P("/tmp/arona-plain"), 0755) == 0);
    CHECK(SC(40, P("tmpfs"), P("/tmp/arona-missing"), P("tmpfs"), 0, 0) == -ENOENT);
    CHECK(SC(40, P("ext4"), P("/tmp/arona-plain"), P("ext4"), 0, 0) == -ENODEV);
    // Read-only and bind-style flags are not implemented for memory mounts.
    CHECK(SC(40, P("tmpfs"), P("/tmp/arona-plain"), P("tmpfs"), 1, 0) == -EINVAL);
    CHECK(SC(40, P("tmpfs"), P("/tmp/arona-plain"), P("tmpfs"), 4096, 0) == -EINVAL);
    CHECK(SC(40, P("tmpfs"), P("/tmp/arona-plain"), P("tmpfs"), 0, P("size=1m")) == -EINVAL);
    // Umounting a directory that is not a mount point is invalid.
    CHECK(SC(39, P("/tmp/arona-plain"), 0) == -EINVAL);
    CHECK(SC(39, P("/tmp/arona-missing"), 0) == -ENOENT);
    CHECK(SC(39, P("/tmp"), 1) == -EINVAL);
    // Mounting over the boot archive would shadow system files for every process.
    CHECK(SC(40, P("tmpfs"), P("/bin"), P("tmpfs"), 0, 0) == -EPERM);
    CHECK(SC(40, P("tmpfs"), P("/etc"), P("tmpfs"), 0, 0) == -EPERM);
    CHECK(SC(40, P("tmpfs"), P("/"), P("tmpfs"), 0, 0) == -EPERM);
    // The startup /tmp mount belongs to the file service, so only mounts created
    // through the mount call can be removed here.
    CHECK(SC(39, P("/tmp"), 0) == -EINVAL);
    CHECK(SC(35, AT_FDCWD, P("/tmp/arona-plain"), AT_REMOVEDIR) == 0);
}

int fs_main(void) {
    temporary();
    mounted_tmpfs("/tmp/arona-mnt", "/tmp/arona-mnt/file");
    mounted_tmpfs("/var/arona-mnt", "/var/arona-mnt/file");
    nested();
    errors();
    // Credential checks reject mounting and unmounting for non-root callers.
    CHECK(SC(146, 1000) == 0);
    CHECK(SC(40, P("tmpfs"), P("/tmp/arona-denied"), P("tmpfs"), 0, 0) == -EPERM);
    CHECK(SC(39, P("/tmp"), 0) == -EPERM);
    return 42;
}
