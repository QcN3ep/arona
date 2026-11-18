// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

// Exercise the Linux namespace through paths, directory handles, and mounts.

extern long vfs_syscall(long number, const long arguments[6]);
#define P(value) ((long)(value))
#define SC(number, ...) vfs_syscall(number, (long[6]){__VA_ARGS__})
#define CHECK(condition) do { if (!(condition)) fail(__LINE__); } while (0)
#define AT_FDCWD (-100)
#define O_DIRECTORY 0x10000
#define O_PATH 0x200000

struct stat {
    unsigned long device, inode;
    unsigned mode, links, uid, gid;
    unsigned long rdev, pad;
    long size;
    int block_size, pad2;
    long blocks;
    long times[6];
    unsigned unused[2];
};
struct statfs { unsigned long kind, block_size, blocks, free, available, files, free_files, id, name_length, fragment, flags, spare[4]; };
static unsigned char buffer[4096];

static void fail(unsigned line) {
    char text[] = "VFS assertion line 0000\n";
    for (int index = 22; index >= 19; --index) { text[index] = '0' + line % 10; line /= 10; }
    SC(64, 1, P(text), sizeof(text) - 1);
    SC(94, 1);
    for (;;) {}
}
static int equal(const char *a, const char *b) { while (*a && *a == *b) { a++; b++; } return *a == *b; }
static int contains(const char *text, long length, const char *needle) {
    for (long i = 0; i < length; i++) { long j = 0; while (i + j < length && needle[j] && text[i+j] == needle[j]) j++; if (!needle[j]) return 1; }
    return 0;
}
static long openat(long directory, const char *path, long flags) { return SC(56, directory, P(path), flags, 0600); }
static long open(const char *path, long flags) { long fd = openat(AT_FDCWD, path, flags); CHECK(fd >= 0); return fd; }
static void close(long fd) { CHECK(SC(57, fd) == 0); }
static struct stat statat(long directory, const char *path, int flags) { struct stat info; CHECK(SC(79, directory, P(path), P(&info), flags) == 0); return info; }
static struct stat fstat(long fd) { struct stat info; CHECK(SC(80, fd, P(&info)) == 0); return info; }
static int entry(long fd, const char *name, int kind, unsigned long inode) {
    CHECK(SC(62, fd, 0, 0) == 0);
    for (;;) {
        long count = SC(61, fd, P(buffer), 128);
        CHECK(count >= 0);
        if (!count) return 0;
        long offset = 0;
        while (offset < count) {
            unsigned short length = buffer[offset + 16] | (buffer[offset + 17] << 8);
            CHECK(length >= 24 && offset + length <= count);
            if (equal((char *)buffer + offset + 19, name)) {
                unsigned long found = 0;
                for (int i = 0; i < 8; i++) found |= (unsigned long)buffer[offset + i] << (i * 8);
                CHECK(buffer[offset + 18] == kind);
                CHECK(found == inode);
                return 1;
            }
            offset += length;
        }
    }
}
static void directories(void) {
    long root = open("/", O_DIRECTORY);
    for (const char **path = (const char *[]){"dev", "proc", "sys", 0}; *path; path++) {
        struct stat info = statat(root, *path, 0);
        CHECK((info.mode & 0170000) == 0040000);
        CHECK(entry(root, *path, 4, info.inode));
    }
    long dev = open("/dev", O_DIRECTORY);
    CHECK(SC(34, AT_FDCWD, P("/"), 0755) == -17);
    CHECK(SC(34, AT_FDCWD, P("/dev"), 0755) == -17);
    CHECK(SC(35, AT_FDCWD, P("/dev"), 0x200) == -16);
    CHECK(SC(49, P("/dev")) == 0);
    char cwd[128];
    CHECK(SC(17, P(cwd), sizeof(cwd)) == 5 && equal(cwd, "/dev"));
    long null = open("./null", 2);
    struct stat info = statat(dev, "null", 0), actual = fstat(null);
    CHECK(info.inode == actual.inode && info.device == actual.device && info.rdev == actual.rdev);
    CHECK((info.mode & 0170000) == 0020000);
    CHECK(entry(dev, "null", 2, info.inode));
    CHECK(SC(64, null, P("x"), 1) == 1 && SC(63, null, P(buffer), 1) == 0);
    close(null);
    CHECK(SC(50, root) == 0);
    long shm = openat(dev, "shm", O_DIRECTORY); CHECK(shm >= 0);
    CHECK(SC(50, shm) == 0);
    CHECK(SC(49, P("..")) == 0);
    CHECK(SC(17, P(cwd), sizeof(cwd)) == 5 && equal(cwd, "/dev"));
    info = statat(dev, "shm", 0);
    CHECK(entry(dev, "shm", 4, info.inode));
    CHECK(SC(49, P("/")) == 0);
    close(shm); close(dev); close(root);
}
static void links(void) {
    long proc = open("/proc", O_DIRECTORY);
    struct stat info = statat(proc, "self", 0x100);
    CHECK((info.mode & 0170000) == 0120000);
    CHECK(entry(proc, "self", 10, info.inode));
    CHECK(SC(78, proc, P("self"), P(buffer), sizeof(buffer)) > 0);
    long self = openat(proc, "self", O_DIRECTORY); CHECK(self >= 0);
    info = statat(self, "stat", 0);
    CHECK(entry(self, "stat", 8, info.inode));
    long stat = openat(self, "stat", 0); CHECK(stat >= 0); CHECK(SC(63, stat, P(buffer), sizeof(buffer)) > 0); close(stat);
    CHECK(SC(49, P("/proc/self")) == 0);
    long exe = open("exe", O_PATH | 0x20000);
    struct stat link = fstat(exe);
    CHECK((link.mode & 0170000) == 0120000);
    long length = SC(78, exe, P(""), P(buffer), sizeof(buffer));
    CHECK(length == 14 && contains((char *)buffer, length, "/bin/linux-vfs"));
    close(exe);
    CHECK(openat(self, "exe", 0x20000) == -40);
    CHECK(openat(self, "stat/child", 0) == -20);
    CHECK(openat(self, "absent", 0) == -2);
    CHECK(SC(49, P("/")) == 0);
    CHECK(SC(36, P("/proc/self/stat"), AT_FDCWD, P("/tmp/vfs-proc")) == 0);
    stat = open("/tmp/vfs-proc", 0); CHECK(SC(63, stat, P(buffer), sizeof(buffer)) > 0); close(stat);
    CHECK(SC(36, P("/dev"), AT_FDCWD, P("/tmp/vfs-dev")) == 0);
    long null = open("/tmp/vfs-dev/../dev/null", 2); close(null);
    CHECK(SC(36, P("vfs-cycle"), AT_FDCWD, P("/tmp/vfs-cycle")) == 0);
    CHECK(openat(AT_FDCWD, "/tmp/vfs-cycle", 0) == -40);
    for (const char **path = (const char *[]){"/tmp/vfs-proc", "/tmp/vfs-dev", "/tmp/vfs-cycle", 0}; *path; path++) CHECK(SC(35, AT_FDCWD, P(*path), 0) == 0);
    close(self); close(proc);
}
static void directory_state(void) {
    long proc = open("/proc", O_DIRECTORY);
    CHECK(SC(61, proc, P(buffer), 1) == -22);
    CHECK(SC(61, proc, P(buffer), 24) == 24);
    CHECK(equal((char *)buffer + 19, "."));
    long alias = SC(23, proc); CHECK(alias >= 0);
    CHECK(SC(61, alias, P(buffer), 24) == 24);
    CHECK(equal((char *)buffer + 19, ".."));
    close(alias);
    long child = SC(220, 17, 0, 0, 0, 0); CHECK(child >= 0);
    if (child == 0) { SC(93, 0); for (;;) {} }
    char name[24]; char digits[24]; int count = 0;
    for (long id = child; id; id /= 10) digits[count++] = '0' + id % 10;
    for (int i = 0; i < count; i++) name[i] = digits[count - i - 1];
    name[count] = 0;
    struct stat info = statat(proc, name, 0);
    CHECK(entry(proc, name, 4, info.inode));
    int status = -1;
    CHECK(SC(260, child, P(&status), 0, 0) == child && status == 0);
    CHECK(SC(62, proc, 0, 0) == 0);
    CHECK(SC(79, proc, P(name), P(&info), 0) == -2);
    CHECK(!entry(proc, name, 4, 0));
    long path = openat(proc, "self/stat", O_PATH | 3 | 0x80000000);
    CHECK(path >= 0);
    CHECK(SC(63, path, P(buffer), 1) == -9);
    info = fstat(path); CHECK((info.mode & 0170000) == 0100000);
    struct statfs stats; CHECK(SC(44, path, P(&stats)) == 0 && stats.kind == 0x9fa0);
    close(path);
    CHECK(openat(proc, "self/stat/", 0) == -20);
    close(proc);
}
static void native_handles(void) {
    CHECK(SC(34, AT_FDCWD, P("/tmp/vfs-dir/"), 0700) == 0);
    long dir = open("/tmp/vfs-dir", O_DIRECTORY);
    CHECK(SC(38, AT_FDCWD, P("/tmp/vfs-dir"), AT_FDCWD, P("/tmp/vfs-renamed")) == 0);
    CHECK(SC(50, dir) == 0);
    char cwd[128]; CHECK(SC(17, P(cwd), sizeof(cwd)) > 0 && equal(cwd, "/tmp/vfs-renamed"));
    long file = openat(dir, "file", 2 | 0x40); CHECK(file >= 0); close(file);
    file = open("/tmp/vfs-renamed/file", 0); close(file);
    CHECK(SC(35, dir, P("file"), 0) == 0);
    CHECK(SC(49, P("/")) == 0); close(dir);
    CHECK(SC(35, AT_FDCWD, P("/tmp/vfs-renamed"), 0x200) == 0);
}
static void mounts(void) {
    struct statfs a, b;
    for (const char **path = (const char *[]){"/proc", "/sys", "/dev", "/dev/pts", "/dev/mqueue", "/dev/shm", "/dev/hugepages", 0}; *path; path++) {
        long fd = open(*path, O_DIRECTORY);
        CHECK(SC(43, P(*path), P(&a)) == 0);
        CHECK(SC(44, fd, P(&b)) == 0);
        CHECK(a.kind == b.kind && a.id == b.id && a.block_size == b.block_size);
        close(fd);
    }
    CHECK(SC(34, AT_FDCWD, P("/tmp/vfs-mount"), 0700) == 0);
    CHECK(SC(40, P("tmpfs"), P("/tmp/vfs-mount"), P("tmpfs"), 0, 0) == 0);
    long fd = open("/proc/mounts", 0);
    long count = SC(63, fd, P(buffer), sizeof(buffer)); CHECK(count > 0);
    CHECK(contains((char *)buffer, count, " /tmp/vfs-mount tmpfs "));
    CHECK(contains((char *)buffer, count, " /var ext4 "));
    CHECK(contains((char *)buffer, count, " /dev/shm tmpfs "));
    close(fd);
    long tmp = open("/tmp", O_DIRECTORY);
    struct stat info = statat(tmp, "vfs-mount", 0);
    CHECK(entry(tmp, "vfs-mount", 4, info.inode)); close(tmp);
    CHECK(SC(39, P("/tmp/vfs-mount"), 0) == 0);
    fd = open("/proc/mounts", 0); count = SC(63, fd, P(buffer), sizeof(buffer)); CHECK(count > 0);
    CHECK(!contains((char *)buffer, count, " /tmp/vfs-mount tmpfs ")); close(fd);
    CHECK(SC(35, AT_FDCWD, P("/tmp/vfs-mount"), 0x200) == 0);
}
static void execution(void) {
    long child = SC(220, 17, 0, 0, 0, 0); CHECK(child >= 0);
    if (child == 0) {
        const char *args[] = {"linux-vfs", "again", 0};
        SC(221, P("bin/linux-vfs"), P(args), 0);
        fail(__LINE__);
    }
    int status = -1;
    CHECK(SC(260, child, P(&status), 0, 0) == child && status == (42 << 8));
}
int vfs_main(const char *mode) {
    if (mode && equal(mode, "done")) return 42;
    if (mode && equal(mode, "again")) {
        const char *args[] = {"linux-vfs", "done", 0};
        SC(221, P("/proc/self/exe"), P(args), 0);
        fail(__LINE__);
    }
    directories(); links(); directory_state(); native_handles(); mounts(); execution();
    return 42;
}
void *memcpy(void *output, const void *input, unsigned long length) {
    unsigned char *to = output; const unsigned char *from = input;
    for (unsigned long i = 0; i < length; i++) to[i] = from[i];
    return output;
}
