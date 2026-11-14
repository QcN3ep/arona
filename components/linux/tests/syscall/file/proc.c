// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

// Guest scenarios for `/proc` statistics: maps, smaps, meminfo, uptime, exe,
// and per-process cmdline and stat lookups.

extern long test_syscall(long number, const long arguments[6]);

#define P(value) ((long)(value))
#define SC(number, ...) test_syscall(number, (long[6]){__VA_ARGS__})
#define CHECK(condition) do { if (!(condition)) proc_fail(__LINE__); } while (0)

struct timespec {
    long seconds;
    long nanoseconds;
};

// REFERENCE: https://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git/tree/include/uapi/asm-generic/stat.h?h=v6.12
struct stat {
    unsigned long device;
    unsigned long inode;
    unsigned int mode;
    unsigned int links;
    unsigned int uid;
    unsigned int gid;
    unsigned long special;
    unsigned long pad;
    long size;
    int block_size;
    int pad2;
    long blocks;
    long accessed_seconds;
    unsigned long accessed_nanoseconds;
    long modified_seconds;
    unsigned long modified_nanoseconds;
    long changed_seconds;
    unsigned long changed_nanoseconds;
    unsigned int unused0;
    unsigned int unused1;
};
_Static_assert(sizeof(struct stat) == 128, "unexpected stat layout");

static char page[4096];
static char link_target[512];
static char path[64];

void proc_fail(unsigned line) {
    char text[] = "Live assertion line 0000\n";
    for (int index = 23; index >= 20; --index) {
        text[index] = '0' + line % 10;
        line /= 10;
    }
    SC(64, 1, P(text), sizeof(text) - 1);
    SC(94, 3);
    for (;;) {}
}

static unsigned long text_length(const char *text) {
    unsigned long length = 0;
    while (text[length]) length++;
    return length;
}

static int text_equals(const char *left, const char *right) {
    while (*left && *left == *right) {
        left++;
        right++;
    }
    return *left == *right;
}

static int range_equals(const char *text, long length, const char *value) {
    long index = 0;
    while (index < length && value[index] && text[index] == value[index]) index++;
    return index == length && value[index] == 0;
}

static long find(const char *haystack, long length, const char *needle, long needle_length) {
    for (long start = 0; start + needle_length <= length; ++start) {
        long index = 0;
        while (index < needle_length && haystack[start + index] == needle[index]) index++;
        if (index == needle_length) return start;
    }
    return -1;
}

static long find_text(const char *haystack, long length, const char *needle) {
    return find(haystack, length, needle, (long)text_length(needle));
}

static long first_number(const char *text, long length) {
    long index = 0;
    while (index < length && (text[index] < '0' || text[index] > '9')) index++;
    long value = 0;
    while (index < length && text[index] >= '0' && text[index] <= '9') {
        value = value * 10 + (text[index] - '0');
        index++;
    }
    return value;
}

static long open_path(const char *name) { return SC(56, -100, P(name), 0, 0); }

static long read_fd(long fd, void *buffer, long length) {
    return SC(63, fd, P(buffer), length);
}

static void close_fd(long fd) { CHECK(SC(57, fd) == 0); }

static long read_file(const char *name, long *length) {
    long fd = open_path(name);
    CHECK(fd >= 0);
    *length = read_fd(fd, page, sizeof(page));
    CHECK(*length > 0);
    return fd;
}

static void test_exe_link(void) {
    long length = SC(78, -100, P("/proc/self/exe"), P(link_target), sizeof(link_target));
    CHECK(length > 0);
    CHECK(range_equals(link_target, length, "/bin/linux-test"));
    CHECK(link_target[length] == 0);
    CHECK(SC(78, -100, P("/proc/self/exe"), P(link_target), 8) == 8);
    CHECK(SC(78, -100, P("/proc/self/exe"), P(link_target), 0) == -22);
    CHECK(SC(78, -100, P("/proc/self/absent-exe"), P(link_target), sizeof(link_target)) == -2);
    CHECK(SC(78, -100, P("/proc/self/maps"), P(link_target), 16) == -22);
}

static void test_maps(void) {
    long length;
    long fd = read_file("/proc/self/maps", &length);
    CHECK(length > 32);
    CHECK(page[8] == '-');
    CHECK(page[17] == ' ');
    CHECK(page[21] == 'p');
    CHECK(page[31] == ' ');
    CHECK(find_text(page, length, "/bin/") >= 0);
    long end = SC(62, fd, 0, 2);
    CHECK(end == length);
    struct stat info;
    CHECK(SC(80, fd, P(&info)) == 0);
    CHECK(info.size == end);
    CHECK(SC(63, fd, P(page), 16) == 0);
    CHECK(SC(64, fd, P("x"), 1) == -30);
    close_fd(fd);
}

static void test_smaps(void) {
    long length;
    long fd = read_file("/proc/self/smaps", &length);
    CHECK(find_text(page, length, "Size:") >= 0);
    CHECK(find_text(page, length, "Rss:") >= 0);
    CHECK(find_text(page, length, "/bin/") >= 0);
    close_fd(fd);
}

static void test_meminfo(void) {
    long length;
    long fd = read_file("/proc/meminfo", &length);
    CHECK(range_equals(page, 9, "MemTotal:"));
    CHECK(first_number(page + 9, length - 9) > 0);
    CHECK(find_text(page, length, "MemFree:") >= 0);
    CHECK(find_text(page, length, "MemAvailable:") >= 0);
    close_fd(fd);
}

static void test_uptime(void) {
    long length;
    long fd = read_file("/proc/uptime", &length);
    CHECK(page[length - 1] == '\n');
    long dot = find_text(page, length, ".");
    long space = find_text(page, length, " ");
    CHECK(dot > 0);
    CHECK(space > dot);
    CHECK(range_equals(page + length - 5, 5, "0.00\n"));
    close_fd(fd);
}

static void test_stat_self(void) {
    long length;
    long fd = read_file("/proc/self/stat", &length);
    CHECK(first_number(page, length) == SC(172));
    CHECK(find_text(page, length, "linux-test") >= 0);
    long close = find_text(page, length, ") ");
    CHECK(close >= 0);
    char state = page[close + 2];
    CHECK(state == 'R' || state == 'S' || state == 'T' || state == 'Z');
    close_fd(fd);
}

static void build_pid_path(char *out, long pid, const char *suffix) {
    const char *prefix = "/proc/";
    while (*prefix) *out++ = *prefix++;
    if (pid == 0) {
        *out++ = '0';
    } else {
        char digits[24];
        int count = 0;
        unsigned long value = (unsigned long)pid;
        while (value) {
            digits[count++] = (char)('0' + value % 10);
            value /= 10;
        }
        while (count) *out++ = digits[--count];
    }
    while (*suffix) *out++ = *suffix++;
    *out = 0;
}

static void child_loop(void) {
    struct timespec duration = {0, 200000000};
    SC(101, P(&duration), 0);
    SC(94, 0);
    for (;;) {}
}

static void test_other_process(void) {
    long child = SC(220, 17, 0, 0, 0, 0);
    CHECK(child >= 0);
    if (child == 0) child_loop();
    build_pid_path(path, child, "/cmdline");
    long fd = open_path(path);
    CHECK(fd >= 0);
    long length = read_fd(fd, page, sizeof(page));
    CHECK(length > 0);
    CHECK(text_equals(page, "/bin/linux-test"));
    CHECK(text_equals(page + text_length(page) + 1, "Live"));
    close_fd(fd);
    build_pid_path(path, child, "/stat");
    fd = open_path(path);
    CHECK(fd >= 0);
    length = read_fd(fd, page, sizeof(page));
    CHECK(length > 0);
    CHECK(first_number(page, length) == child);
    CHECK(find_text(page, length, "linux-test") >= 0);
    long close = find_text(page, length, ") ");
    CHECK(close >= 0);
    char state = page[close + 2];
    CHECK(state == 'R' || state == 'S' || state == 'T' || state == 'Z');
    close_fd(fd);
    int status = -1;
    CHECK(SC(260, child, P(&status), 0, 0) == child);
    CHECK(status == 0);
    build_pid_path(path, child, "/stat");
    CHECK(open_path(path) == -2);
    CHECK(open_path("/proc/999999/cmdline") == -2);
    CHECK(open_path("/proc/999999/stat") == -2);
}

void proc_stats_main(const char *mode) {
    (void)mode;
    test_exe_link();
    test_maps();
    test_smaps();
    test_meminfo();
    test_uptime();
    test_stat_self();
    test_other_process();
}
