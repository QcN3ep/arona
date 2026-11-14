// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

extern long shm_syscall(long number, const long arguments[6]);

void *memset(void *destination, int value, unsigned long length) {
    unsigned char *bytes = destination;
    for (unsigned long index = 0; index < length; ++index) {
        bytes[index] = (unsigned char)value;
    }
    return destination;
}

void *memcpy(void *destination, const void *source, unsigned long length) {
    unsigned char *output = destination;
    const unsigned char *input = source;
    for (unsigned long index = 0; index < length; ++index) {
        output[index] = input[index];
    }
    return destination;
}

#define P(value) ((long)(value))
#define SC(number, ...) shm_syscall(number, (long[6]){__VA_ARGS__})
#define CHECK(condition) do { if (!(condition)) shm_fail(__LINE__); } while (0)

#define PAGE 4096L
#define SHMMAX (1L << 30)

#define IPC_CREAT 0x200
#define IPC_EXCL 0x400
#define IPC_RMID 0
#define IPC_SET 1
#define IPC_STAT 2
#define IPC_INFO 3
#define SHM_INFO 14

#define SHM_RDONLY 0x1000
#define SHM_RND 0x2000
#define SHM_REMAP 0x4000

#define KEY1 0x5a5a1001L
#define KEY2 0x5a5a1002L
#define KEY3 0x5a5a1003L
#define KEY4 0x5a5a1004L
#define KEY5 0x5a5a1005L

struct ipc_perm {
    int key;
    unsigned uid, gid, cuid, cgid, mode;
    unsigned short seq, padding;
    unsigned long unused1, unused2;
};

struct shmid_ds {
    struct ipc_perm perm;
    unsigned long segsz;
    long atime, dtime, ctime;
    int cpid, lpid;
    unsigned long nattch, unused4, unused5;
};

struct shminfo {
    int max, min, mni, seg, all;
};

struct shm_info {
    int used_ids;
    unsigned long total, resident, swapped, attempts, successes;
};

static void shm_fail(unsigned line) {
    char text[] = "Shared memory assertion line 0000\n";
    for (int index = 32; index >= 29; --index) {
        text[index] = '0' + line % 10;
        line /= 10;
    }
    SC(64, 1, P(text), sizeof(text) - 1);
    SC(94, 1);
    for (;;) {}
}

static void expect(long actual, long expected, unsigned line) {
    if (actual != expected) {
        shm_fail(line);
    }
}

static long wait_child(long child) {
    int status = -1;
    expect(SC(260, child, P(&status), 0, 0), child, __LINE__);
    return status;
}

static void basic(void) {
    long id = SC(194, KEY1, 8192, IPC_CREAT | 0600);
    CHECK(id >= 0);
    CHECK(SC(194, KEY1, 4096, IPC_CREAT | 0600) == id);
    CHECK(SC(194, KEY1, 8192, IPC_CREAT | IPC_EXCL | 0600) == -17);
    CHECK(SC(194, KEY1, 16384, 0600) == -22);
    CHECK(SC(194, 0x5a5a2001L, 4096, 0) == -2);
    CHECK(SC(194, 0, 0, 0600) == -22);
    CHECK(SC(194, 0, SHMMAX + 1, 0600) == -22);

    char *first = (char *)SC(196, id, 0, 0);
    CHECK(first > 0);
    for (long index = 0; index < 8192; ++index) {
        CHECK(first[index] == 0);
    }
    first[0] = 0x41;
    first[4095] = 0x42;
    first[4096] = 0x43;

    char *second = (char *)SC(196, id, 0, 0);
    CHECK(second > 0 && second != first);
    CHECK(second[0] == 0x41 && second[4095] == 0x42 && second[4096] == 0x43);
    second[1] = 0x44;
    CHECK(first[1] == 0x44);

    struct shmid_ds stat = {0};
    CHECK(SC(195, id, IPC_STAT, P(&stat)) == 0);
    CHECK(stat.perm.key == (int)KEY1);
    CHECK(stat.perm.mode == 0600);
    CHECK(stat.segsz == 8192);
    CHECK(stat.nattch == 2);
    CHECK(stat.cpid == (int)SC(172));
    CHECK(stat.lpid == (int)SC(172));

    stat.perm.mode = 0640;
    CHECK(SC(195, id, IPC_SET, P(&stat)) == 0);
    struct shmid_ds changed = {0};
    CHECK(SC(195, id, IPC_STAT, P(&changed)) == 0);
    CHECK(changed.perm.mode == 0640);

    CHECK(SC(195, id, IPC_STAT, 0x10) == -14);
    CHECK(SC(195, id, IPC_SET, 0x10) == -14);
    CHECK(SC(195, id, IPC_INFO, 0x10) == -14);
    CHECK(SC(195, id, 999, 0) == -22);
    CHECK(SC(195, 123456, IPC_STAT, P(&stat)) == -22);
    CHECK(SC(196, 123456, 0, 0) == -22);

    CHECK(SC(196, id, P(first) + 1, 0) == -22);
    char *rounded = (char *)SC(196, id, 0x20000123, SHM_RND);
    CHECK(rounded == (char *)0x20000000);
    CHECK(rounded[0] == 0x41 && rounded[4096] == 0x43);
    rounded[2] = 0x45;
    CHECK(first[2] == 0x45);
    CHECK(SC(196, id, 0x20000123, SHM_RND) == -22);
    CHECK(SC(196, id, 0x21000000, SHM_REMAP) == -22);
    char *fixed = (char *)SC(196, id, 0x22000000, 0);
    CHECK(fixed == (char *)0x22000000);
    CHECK(fixed[0] == 0x41);
    fixed[3] = 0x46;
    CHECK(first[3] == 0x46);
    CHECK(SC(197, P(fixed)) == 0);
    CHECK(SC(197, P(rounded)) == 0);
    CHECK(SC(197, P(rounded)) == -22);

    CHECK(SC(197, P(first)) == 0);
    CHECK(SC(197, P(first)) == -22);
    CHECK(SC(197, P((char *)first + 1)) == -22);
    CHECK(SC(197, 0x10000) == -22);
    CHECK(SC(195, id, IPC_STAT, P(&stat)) == 0);
    CHECK(stat.nattch == 1);
    CHECK(SC(197, P(second)) == 0);
    CHECK(SC(195, id, IPC_STAT, P(&stat)) == 0);
    CHECK(stat.nattch == 0);

    long huge = SC(194, 0, SHMMAX, 0600);
    CHECK(huge >= 0);
    CHECK(SC(196, huge, 0, 0) == -12);
    CHECK(SC(195, huge, IPC_RMID, 0) == 0);

    struct shminfo info = {0};
    CHECK(SC(195, 0, IPC_INFO, P(&info)) >= 0);
    CHECK(info.max == SHMMAX);
    CHECK(info.min == 1);
    CHECK(info.mni >= 1 && info.all >= 1);
    struct shm_info summary = {0};
    CHECK(SC(195, 0, SHM_INFO, P(&summary)) >= 0);
    CHECK(summary.used_ids >= 1);
    CHECK(summary.total >= 8192);

    CHECK(SC(195, id, IPC_RMID, 0) == 0);
    CHECK(SC(194, KEY1, 4096, 0600) == -2);
    CHECK(SC(195, id, IPC_STAT, P(&stat)) == -22);
}

static void lifecycle(void) {
    long id = SC(194, KEY2, 4096, IPC_CREAT | 0600);
    CHECK(id >= 0);
    char *mapping = (char *)SC(196, id, 0, 0);
    CHECK(mapping > 0);
    mapping[0] = 0x4c;
    CHECK(SC(195, id, IPC_RMID, 0) == 0);
    CHECK(SC(196, id, 0, 0) == -43);
    CHECK(SC(194, KEY2, 4096, 0600) == -2);
    CHECK(mapping[0] == 0x4c);
    mapping[0] = 0x4d;
    mapping[1] = 0x4e;
    struct shmid_ds stat = {0};
    CHECK(SC(195, id, IPC_STAT, P(&stat)) == 0);
    CHECK(stat.nattch == 1);
    // A new segment may take the released key while the old one is attached.
    long replacement = SC(194, KEY2, 4096, IPC_CREAT | 0600);
    CHECK(replacement >= 0 && replacement != id);
    CHECK(SC(197, P(mapping)) == 0);
    CHECK(SC(194, KEY2, 4096, 0600) == replacement);
    CHECK(SC(195, replacement, IPC_RMID, 0) == 0);
    CHECK(SC(195, id, IPC_STAT, P(&stat)) == -22);
    CHECK(SC(196, id, 0, 0) == -22);
}

static void shared_fork(void) {
    long id = SC(194, 0, 8192, 0600);
    CHECK(id >= 0);
    char *mapping = (char *)SC(196, id, 0, 0);
    CHECK(mapping > 0);
    mapping[0] = 0x11;
    mapping[4096] = 0x22;
    long child = SC(220, 17, 0, 0, 0, 0);
    CHECK(child >= 0);
    if (child == 0) {
        CHECK(mapping[0] == 0x11 && mapping[4096] == 0x22);
        mapping[0] = 0x33;
        char *second = (char *)SC(196, id, 0, 0);
        CHECK(second > 0);
        CHECK(second[0] == 0x33 && second[4096] == 0x22);
        second[1] = 0x44;
        CHECK(mapping[1] == 0x44);
        CHECK(SC(197, P(second)) == 0);
        SC(94, 42);
    }
    CHECK(wait_child(child) == 42 << 8);
    CHECK(mapping[0] == 0x33 && mapping[1] == 0x44);
    CHECK(SC(197, P(mapping)) == 0);
    CHECK(SC(195, id, IPC_RMID, 0) == 0);
}

static void permissions(void) {
    long restricted = SC(194, KEY3, 4096, IPC_CREAT | 0400);
    CHECK(restricted >= 0);
    long group = SC(194, KEY4, 4096, IPC_CREAT | 0040);
    CHECK(group >= 0);
    long child = SC(220, 17, 0, 0, 0, 0);
    CHECK(child >= 0);
    if (child == 0) {
        CHECK(SC(146, 1000) == 0);
        struct shmid_ds stat = {0};
        CHECK(SC(196, restricted, 0, 0) == -13);
        CHECK(SC(196, restricted, 0, SHM_RDONLY) == -13);
        CHECK(SC(194, KEY3, 4096, 0666) == -13);
        CHECK(SC(195, restricted, IPC_STAT, P(&stat)) == -13);
        CHECK(SC(195, restricted, IPC_RMID, 0) == -1);
        CHECK(SC(195, group, IPC_RMID, 0) == -1);
        char *mapping = (char *)SC(196, group, 0, SHM_RDONLY);
        CHECK(mapping > 0);
        CHECK(mapping[0] == 0);
        CHECK(SC(197, P(mapping)) == 0);
        CHECK(SC(196, group, 0, 0) == -13);

        long own = SC(194, KEY5, 4096, IPC_CREAT | 0400);
        CHECK(own >= 0);
        CHECK(SC(196, own, 0, 0) == -13);
        mapping = (char *)SC(196, own, 0, SHM_RDONLY);
        CHECK(mapping > 0);
        CHECK(SC(197, P(mapping)) == 0);
        struct shmid_ds set = {0};
        set.perm.mode = 0600;
        CHECK(SC(195, own, IPC_SET, P(&set)) == -13);
        CHECK(SC(195, own, IPC_STAT, P(&stat)) == 0);
        CHECK(stat.perm.mode == 0400);
        CHECK(SC(195, own, IPC_RMID, 0) == 0);
        SC(94, 42);
    }
    CHECK(wait_child(child) == 42 << 8);
    CHECK(SC(195, restricted, IPC_RMID, 0) == 0);
    CHECK(SC(195, group, IPC_RMID, 0) == 0);
    CHECK(SC(194, KEY3, 4096, 0600) == -2);
}

static void read_only(void) {
    long id = SC(194, 0, 4096, 0600);
    CHECK(id >= 0);
    char *mapping = (char *)SC(196, id, 0, SHM_RDONLY);
    CHECK(mapping > 0);
    CHECK(mapping[0] == 0);
    CHECK(SC(226, P(mapping), 4096, 1 | 2) == -13);
    mapping[0] = 1;
    shm_fail(__LINE__);
}

static void mapping_changes(void) {
    long id = SC(194, 0, 4096, 0600);
    CHECK(id >= 0);
    char *mapping = (char *)SC(196, id, 0, 0);
    CHECK(mapping > 0);
    mapping[0] = 0x5a;
    // munmap leaves no record: attaching again at the same fixed address works.
    CHECK(SC(215, P(mapping), 4096) == 0);
    char *again = (char *)SC(196, id, P(mapping), 0);
    CHECK(again == mapping);
    CHECK(again[0] == 0x5a);
    struct shmid_ds stat = {0};
    CHECK(SC(195, id, IPC_STAT, P(&stat)) == 0);
    CHECK(stat.nattch == 1);
    // A fixed replacement must not detach the System V mapping behind its back.
    CHECK(SC(222, P(again), 4096, 3, 0x32, -1, 0) == -22);
    // mremap moves the mapping away from the recorded attachment address.
    char *moved = (char *)SC(216, P(again), 4096, 4096, 3, 0x30000000);
    CHECK(moved == (char *)0x30000000);
    CHECK(SC(197, P(again)) == -22);
    CHECK(SC(197, P(moved)) == -22);
    CHECK(SC(195, id, IPC_RMID, 0) == 0);
}

void shm_main(const char *mode) {
    if (*mode == 'b') {
        basic();
    } else if (*mode == 'f') {
        shared_fork();
    } else if (*mode == 'l') {
        lifecycle();
    } else if (*mode == 'p') {
        permissions();
    } else if (*mode == 'r') {
        read_only();
    } else if (*mode == 'm') {
        mapping_changes();
    } else {
        shm_fail(__LINE__);
    }
    SC(94, 42);
    for (;;) {}
}
