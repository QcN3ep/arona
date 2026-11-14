// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

// MBR partition block devices exposed as /dev/vda, /dev/vda1, and /dev/vda2.

extern long block_syscall(long number, const long arguments[6]);
#define P(value) ((long)(value))
#define SC(number, ...) block_syscall(number, (long[6]){__VA_ARGS__})
#define CHECK(condition) do { if (!(condition)) block_fail(__LINE__); } while (0)

#define AT_FDCWD (-100)
#define O_RDONLY 0
#define O_RDWR 2
#define SEEK_SET 0
#define SEEK_END 2
#define BLKGETSIZE64 0x80081272
#define BLKSSZGET 0x1268
#define S_IFMT 0170000
#define S_IFBLK 0060000
#define ENOENT 2
#define EINVAL 22

struct stat {
    unsigned long device, inode;
    unsigned mode, links, uid, gid;
    unsigned long rdev, pad;
    long size, blocks;
    long accessed_seconds, accessed_nanoseconds;
    long modified_seconds, modified_nanoseconds;
    long changed_seconds, changed_nanoseconds;
    long reserved[2];
};

static char buffer[4096];

static unsigned long read_le32(const char *bytes) {
    return (unsigned char)bytes[0] | (unsigned char)bytes[1] << 8
        | (unsigned char)bytes[2] << 16 | (unsigned long)(unsigned char)bytes[3] << 24;
}

static unsigned long read_le64(const char *bytes) {
    return read_le32(bytes) | read_le32(bytes + 4) << 32;
}

static void write_text(const char *text, unsigned long length) {
    SC(64, 1, P(text), length);
}

static void block_fail(unsigned line) {
    char text[] = "Block assertion line 0000\n";
    for (int i = 25; i >= 22; --i) {
        text[i] = '0' + line % 10;
        line /= 10;
    }
    write_text(text, sizeof(text) - 1);
    SC(94, 1);
    for (;;) {}
}

static long open_path(const char *path, int flags) {
    return SC(56, AT_FDCWD, P(path), flags, 0);
}

static unsigned long device_size(long fd) {
    unsigned long size = 0;
    CHECK(SC(29, fd, BLKGETSIZE64, P(&size)) == 0);
    return size;
}

static void check_basic(const char *path, unsigned long size, unsigned long start) {
    long fd = open_path(path, O_RDONLY);
    CHECK(fd >= 0);
    CHECK(device_size(fd) == size);
    int sector = 0;
    CHECK(SC(29, fd, BLKSSZGET, P(&sector)) == 0);
    CHECK(sector == 512);
    struct stat info;
    CHECK(SC(80, fd, P(&info)) == 0);
    CHECK((info.mode & S_IFMT) == S_IFBLK);
    CHECK(SC(63, fd, P(buffer), 8) == 8);
    CHECK(SC(67, fd, P(buffer), 8, 0) == 8);
    if (start == 0) {
        CHECK(buffer[0] == 'A');
    } else {
        CHECK(buffer[0] == 'A' && buffer[7] == (start == 0x200000 ? '2' : '1'));
    }
    CHECK(SC(57, fd) == 0);
}

void block_main(void) {
    long whole = open_path("/dev/vda", O_RDONLY);
    CHECK(whole >= 0);
    CHECK(device_size(whole) == 0x400000UL);
    int sector = 0;
    CHECK(SC(29, whole, BLKSSZGET, P(&sector)) == 0);
    CHECK(sector == 512);
    CHECK(SC(67, whole, P(buffer), 512, 0) == 512);
    CHECK((unsigned char)buffer[510] == 0x55 && (unsigned char)buffer[511] == 0xaa);
    CHECK(read_le32(&buffer[446 + 8]) == 2048);
    CHECK(read_le32(&buffer[446 + 12]) == 2048);
    CHECK(read_le32(&buffer[462 + 8]) == 4096);
    CHECK(read_le32(&buffer[462 + 12]) == 4096);

    long first = open_path("/dev/vda1", O_RDONLY);
    long second = open_path("/dev/vda2", O_RDWR);
    CHECK(first >= 0 && second >= 0);
    CHECK(device_size(first) == 0x100000UL);
    CHECK(device_size(second) == 0x200000UL);
    CHECK(SC(57, first) == 0);

    struct stat info;
    CHECK(SC(80, second, P(&info)) == 0);
    CHECK((info.mode & S_IFMT) == S_IFBLK);
    CHECK(SC(67, second, P(buffer), 8, 0) == 8);
    CHECK(read_le64(buffer) == 0x32502d414e4f5241UL);

    CHECK(SC(68, second, P("BLK-WRITE"), 9, 4096) == 9);
    CHECK(SC(67, second, P(buffer), 9, 4096) == 9);
    CHECK(read_le64(buffer) == 0x544952572d4b4c42UL && buffer[8] == 'E');

    // Partial writes preserve the untouched bytes in both adjacent sectors.
    for (unsigned index = 0; index < 1024; ++index) buffer[index] = (char)(index * 7 + 3);
    CHECK(SC(68, second, P(buffer), 1024, 8192) == 1024);
    CHECK(SC(68, second, P("PATCH"), 5, 8192 + 510) == 5);
    CHECK(SC(67, second, P(buffer), 1024, 8192) == 1024);
    for (unsigned index = 0; index < 1024; ++index) {
        CHECK((unsigned char)buffer[index] == (index >= 510 && index < 515
            ? (unsigned char)"PATCH"[index - 510] : (unsigned char)(index * 7 + 3)));
    }

    long position = SC(62, second, 510, SEEK_SET);
    CHECK(position == 510);
    CHECK(SC(67, second, P(buffer), 4, 510) == 4);

    CHECK(SC(62, second, 0x200000, SEEK_SET) == 0x200000);
    CHECK(SC(62, second, 0x200001, SEEK_SET) == -EINVAL);
    CHECK(SC(62, second, -1, SEEK_SET) == -EINVAL);
    CHECK(SC(67, second, P(buffer), 16, 0x200000) == 0);
    CHECK(SC(68, second, P("write"), 8, 0x1ffffe) == -EINVAL);
    CHECK(SC(62, second, 0, SEEK_END) == 0x200000);

    long missing = open_path("/dev/vda3", O_RDONLY);
    CHECK(missing == -ENOENT);
    CHECK(SC(57, second) == 0);

    long reopened = open_path("/dev/vda2", O_RDONLY);
    CHECK(reopened >= 0);
    CHECK(SC(67, reopened, P(buffer), 9, 4096) == 9);
    CHECK(read_le64(buffer) == 0x544952572d4b4c42UL);
    CHECK(SC(57, reopened) == 0);

    first = open_path("/dev/vda1", O_RDONLY);
    CHECK(first >= 0);
    CHECK(SC(67, first, P(buffer), 8, 0) == 8);
    CHECK(read_le64(buffer) == 0x31502d414e4f5241UL);
    CHECK(SC(57, first) == 0);
    CHECK(SC(57, whole) == 0);

    check_basic("/dev/vda", 0x400000UL, 0);
    check_basic("/dev/vda1", 0x100000UL, 0x100000UL);
    check_basic("/dev/vda2", 0x200000UL, 0x200000UL);

    write_text("Linux block tests passed\n", 25);
    SC(94, 42);
    for (;;) {}
}
