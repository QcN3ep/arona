// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

// POSIX message queue scenarios for /bin/linux-mqueue.

extern long mqueue_syscall(long number, const long arguments[6]);

#define P(value) ((long)(value))
#define SC(number, ...) mqueue_syscall(number, (long[6]){__VA_ARGS__})
#define CHECK(condition) do { if (!(condition)) mqueue_fail(__LINE__); } while (0)

#define O_RDONLY 0
#define O_WRONLY 1
#define O_CREAT 0x40
#define O_EXCL 0x80
#define O_TRUNC 0x200
#define O_NONBLOCK 0x800
#define O_DIRECTORY 0x10000
#define O_CLOEXEC 0x80000
#define AT_FDCWD (-100)
#define AT_REMOVEDIR 0x200

#define MQ_OPEN 180
#define MQ_UNLINK 181
#define MQ_TIMEDSEND 182
#define MQ_TIMEDRECEIVE 183
#define MQ_NOTIFY 184
#define MQ_GETSETATTR 185

#define EINTR 4
#define EBADF 9
#define EAGAIN 11
#define EACCES 13
#define EFAULT 14
#define EBUSY 16
#define EEXIST 17
#define EISDIR 21
#define EINVAL 22
#define ENOENT 2
#define ENOTDIR 20
#define EMSGSIZE 90
#define EOPNOTSUPP 95
#define ETIMEDOUT 110
#define ENAMETOOLONG 36
#define EROFS 30

#define SIGEV_SIGNAL 0
#define SIGEV_NONE 1
#define SIGEV_THREAD 2
#define SIGUSR1 10
#define SIGCHLD 17

struct timespec { long seconds, nanoseconds; };
struct mq_attr { long flags, maxmsg, msgsize, curmsgs, reserved[4]; };
struct sigevent {
    unsigned long value;
    int signo;
    int notify;
    unsigned char padding[48];
};
struct signal_action { unsigned long handler, flags, mask; };

static volatile unsigned signals;
static char payload[4096];
static char path[300];

static void mqueue_fail(unsigned line) {
    char text[] = "Mqueue assertion line 0000\n";
    for (int i = 25; i >= 22; --i) { text[i] = '0' + line % 10; line /= 10; }
    SC(64, 1, P(text), sizeof(text) - 1);
    SC(94, 1);
    for (;;) {}
}

static void handler(int number) {
    if (number == SIGUSR1) ++signals;
}

static void install_handler(void) {
    struct signal_action action = {P(handler), 0, 0};
    CHECK(SC(134, SIGUSR1, P(&action), 0, 8) == 0);
}

static long now_ns(void) {
    struct timespec time;
    CHECK(SC(113, 1, P(&time)) == 0);
    return time.seconds * 1000000000 + time.nanoseconds;
}

static void pause_ns(long nanoseconds) {
    struct timespec duration = {0, nanoseconds};
    long result = SC(101, P(&duration), 0);
    CHECK(result == 0 || result == -EINTR);
}

static void timeout_at(struct timespec *time, long nanoseconds) {
    CHECK(SC(113, 0, P(time)) == 0);
    time->nanoseconds += nanoseconds;
    while (time->nanoseconds >= 1000000000) {
        ++time->seconds;
        time->nanoseconds -= 1000000000;
    }
}

static struct mq_attr attributes(long maxmsg, long msgsize) {
    struct mq_attr attr = {0, maxmsg, msgsize, 0, {0, 0, 0, 0}};
    return attr;
}

static long open_queue(const char *name, long flags, long mode, const struct mq_attr *attr) {
    return SC(MQ_OPEN, P(name), flags, mode, P(attr));
}

static long send(long fd, const void *data, long length, long priority) {
    return SC(MQ_TIMEDSEND, fd, P(data), length, priority, 0);
}

static long send_timed(long fd, const void *data, long length, long priority,
                       const struct timespec *deadline) {
    return SC(MQ_TIMEDSEND, fd, P(data), length, priority, P(deadline));
}

static long receive(long fd, void *data, long length, unsigned *priority) {
    return SC(MQ_TIMEDRECEIVE, fd, P(data), length, P(priority), 0);
}

static long receive_timed(long fd, void *data, long length, unsigned *priority,
                          const struct timespec *deadline) {
    return SC(MQ_TIMEDRECEIVE, fd, P(data), length, P(priority), P(deadline));
}

static long read_file(const char *name, char *bytes, long length) {
    long fd = SC(56, AT_FDCWD, P(name), O_RDONLY | O_CLOEXEC, 0);
    if (fd < 0) return fd;
    long count = SC(63, fd, P(bytes), length - 1);
    CHECK(count >= 0);
    SC(57, fd);
    bytes[count] = 0;
    return count;
}

static int contains(const char *text, const char *needle) {
    for (long i = 0; text[i]; ++i) {
        long j = 0;
        while (needle[j] && text[i + j] == needle[j]) ++j;
        if (!needle[j]) return 1;
    }
    return 0;
}

static int directory_has(const char *wanted) {
    char bytes[2048];
    long fd = SC(56, AT_FDCWD, P("/dev/mqueue"), O_RDONLY | O_DIRECTORY | O_CLOEXEC, 0);
    CHECK(fd >= 0);
    long count = SC(61, fd, P(bytes), sizeof(bytes));
    CHECK(count >= 0);
    SC(57, fd);
    long offset = 0;
    while (offset < count) {
        unsigned short record = *(unsigned short *)(bytes + offset + 16);
        CHECK(record >= 20);
        const char *name = bytes + offset + 19;
        long i = 0;
        while (name[i]) {
            if (name[i] != wanted[i]) break;
            ++i;
        }
        if (name[i] == 0 && wanted[i] == 0) return 1;
        offset += record;
    }
    return 0;
}

static void wait_child(long child) {
    int status = -1;
    CHECK(SC(260, child, P(&status), 0, 0) == child);
    CHECK(status == 0);
}

static long fork_child(void) {
    long child = SC(220, SIGCHLD, 0, 0, 0, 0);
    CHECK(child >= 0);
    return child;
}

static void basic(void) {
    struct mq_attr attr = attributes(4, 128);
    long fd = open_queue("/mq-basic", O_CREAT | O_RDONLY, 0600, &attr);
    CHECK(fd >= 0);
    struct mq_attr got;
    CHECK(SC(MQ_GETSETATTR, fd, 0, P(&got)) == 0);
    CHECK(got.maxmsg == 4 && got.msgsize == 128 && got.curmsgs == 0 && got.flags == 0);

    CHECK(open_queue("/mq-basic", O_CREAT | O_EXCL | O_RDONLY, 0600, &attr) == -EEXIST);
    CHECK(open_queue("/mq-absent", O_RDONLY, 0, 0) == -ENOENT);
    CHECK(open_queue("mq-noslash", O_CREAT | O_RDONLY, 0600, &attr) == -EINVAL);
    CHECK(open_queue("/mq/nested", O_CREAT | O_RDONLY, 0600, &attr) == -EINVAL);
    CHECK(open_queue("", O_CREAT | O_RDONLY, 0600, &attr) == -ENOENT);
    CHECK(open_queue(0, O_CREAT | O_RDONLY, 0600, &attr) == -EFAULT);
    for (long i = 0; i < 298; ++i) path[i] = 'q';
    path[0] = '/';
    path[298] = 0;
    CHECK(open_queue(path, O_CREAT | O_RDONLY, 0600, &attr) == -ENAMETOOLONG);

    struct mq_attr flags = {O_NONBLOCK, 0, 0, 0, {0, 0, 0, 0}};
    CHECK(SC(MQ_GETSETATTR, fd, P(&flags), P(&got)) == 0);
    CHECK((got.flags & O_NONBLOCK) == 0);
    CHECK(SC(MQ_GETSETATTR, fd, 0, P(&got)) == 0 && (got.flags & O_NONBLOCK) != 0);
    struct mq_attr invalid = {0x400, 0, 0, 0, {0, 0, 0, 0}};
    CHECK(SC(MQ_GETSETATTR, fd, P(&invalid), 0) == -EINVAL);
    struct mq_attr clear = {0, 0, 0, 0, {0, 0, 0, 0}};
    CHECK(SC(MQ_GETSETATTR, fd, P(&clear), 0) == 0);

    long writer = open_queue("/mq-basic", O_WRONLY | O_NONBLOCK, 0, 0);
    CHECK(writer >= 0);
    CHECK(send(writer, "a", 1, 1) == 0);
    CHECK(SC(MQ_GETSETATTR, fd, 0, P(&got)) == 0 && got.curmsgs == 1);

    CHECK(read_file("/dev/mqueue/mq-basic", payload, sizeof(payload)) > 0);
    CHECK(contains(payload, "QSIZE:1"));
    CHECK(read_file("/dev/mqueue/mq-missing", payload, sizeof(payload)) == -ENOENT);

    char stat[128];
    CHECK(SC(79, AT_FDCWD, P("/dev/mqueue"), P(stat), 0) == 0);
    CHECK((*(unsigned *)(stat + 16) & 0170000) == 0040000);
    CHECK(SC(79, AT_FDCWD, P("/dev/mqueue/mq-basic"), P(stat), 0) == 0);
    CHECK((*(unsigned *)(stat + 16) & 0170000) == 0100000);
    CHECK(SC(79, AT_FDCWD, P("/dev/mqueue/mq-missing"), P(stat), 0) == -ENOENT);
    CHECK(directory_has("mq-basic"));
    CHECK(directory_has("."));

    long missing_dir = SC(56, AT_FDCWD, P("/dev/mqueue"), O_WRONLY, 0);
    CHECK(missing_dir == -EISDIR);
    long write_attribute = SC(56, AT_FDCWD, P("/dev/mqueue/mq-basic"), O_WRONLY, 0);
    CHECK(write_attribute == -EROFS);

    unsigned priority = 0;
    char message[128];
    CHECK(receive(fd, message, 128, &priority) == 1 && message[0] == 'a' && priority == 1);
    CHECK(SC(181, P("/mq-basic")) == 0);
    CHECK(SC(181, P("/mq-basic")) == -ENOENT);
    CHECK(open_queue("/mq-basic", O_RDONLY, 0, 0) == -ENOENT);
    CHECK(send(writer, "b", 1, 0) == 0);
    CHECK(receive(fd, message, 128, &priority) == 1 && message[0] == 'b');
    CHECK(SC(57, writer) == 0 && SC(57, fd) == 0);
}

static void priority(void) {
    struct mq_attr attr = attributes(4, 64);
    long writer = open_queue("/mq-priority", O_CREAT | O_WRONLY | O_NONBLOCK, 0600, &attr);
    long reader = open_queue("/mq-priority", O_RDONLY | O_NONBLOCK, 0600, &attr);
    CHECK(writer >= 0 && reader >= 0);
    CHECK(send(writer, "low", 3, 1) == 0);
    CHECK(send(writer, "high1", 5, 5) == 0);
    CHECK(send(writer, "high2", 5, 5) == 0);
    CHECK(send(writer, "mid", 3, 3) == 0);
    CHECK(send(writer, "full", 4, 2) == -EAGAIN);

    unsigned priority = 0;
    char message[64];
    CHECK(receive(reader, message, 64, &priority) == 5 && priority == 5);
    CHECK(message[0] == 'h' && message[4] == '1');
    CHECK(receive(reader, message, 64, &priority) == 5 && priority == 5);
    CHECK(message[4] == '2');
    CHECK(receive(reader, message, 64, &priority) == 3 && priority == 3);
    CHECK(message[0] == 'm');
    CHECK(receive(reader, message, 64, &priority) == 3 && priority == 1);
    CHECK(receive(reader, message, 64, &priority) == -EAGAIN);

    CHECK(send(writer, payload, 65, 0) == -EMSGSIZE);
    CHECK(send(writer, "x", 1, 32768) == -EINVAL);
    CHECK(SC(MQ_TIMEDSEND, writer, 0, 1, 0, 0) == -EFAULT);
    CHECK(SC(MQ_TIMEDRECEIVE, reader, 0, 64, P(&priority), 0) == -EFAULT);
    CHECK(SC(MQ_TIMEDRECEIVE, reader, P(message), 64, 0, 0) == -EFAULT);
    CHECK(receive(reader, message, 63, &priority) == -EMSGSIZE);
    CHECK(receive(writer, message, 64, &priority) == -EBADF);
    CHECK(send(reader, "x", 1, 0) == -EBADF);
    CHECK(send(99, "x", 1, 0) == -EBADF);
    CHECK(SC(MQ_GETSETATTR, 99, 0, P(&attr)) == -EBADF);
    CHECK(SC(MQ_NOTIFY, 99, 0) == -EBADF);

    CHECK(SC(181, P("/mq-priority")) == 0);
    CHECK(SC(57, writer) == 0 && SC(57, reader) == 0);
}

static void block(void) {
    install_handler();
    struct mq_attr attr = attributes(1, 16);
    long reader = open_queue("/mq-block", O_CREAT | O_RDONLY, 0600, &attr);
    long writer = open_queue("/mq-block", O_WRONLY, 0600, &attr);
    long nonblock = open_queue("/mq-block", O_RDONLY | O_NONBLOCK, 0600, &attr);
    CHECK(reader >= 0 && writer >= 0 && nonblock >= 0);

    unsigned priority = 0;
    char message[16];
    CHECK(receive(nonblock, message, 16, &priority) == -EAGAIN);

    struct timespec deadline;
    timeout_at(&deadline, 100000000);
    long start = now_ns();
    CHECK(receive_timed(reader, message, 16, &priority, &deadline) == -ETIMEDOUT);
    CHECK(now_ns() - start >= 50000000);

    CHECK(send(writer, "x", 1, 0) == 0);
    timeout_at(&deadline, 100000000);
    start = now_ns();
    CHECK(send_timed(writer, "y", 1, 0, &deadline) == -ETIMEDOUT);
    CHECK(now_ns() - start >= 50000000);
    CHECK(receive(reader, message, 16, &priority) == 1 && message[0] == 'x');

    long parent = SC(172);
    long child = fork_child();
    if (!child) {
        CHECK(receive(reader, message, 16, &priority) == 1 && message[0] == 'F');
        SC(94, 0);
        for (;;) {}
    }
    pause_ns(50000000);
    CHECK(send(writer, "F", 1, 0) == 0);
    wait_child(child);

    CHECK(send(writer, "G", 1, 0) == 0);
    child = fork_child();
    if (!child) {
        CHECK(receive(reader, message, 16, &priority) == 1 && message[0] == 'G');
        SC(94, 0);
        for (;;) {}
    }
    pause_ns(50000000);
    CHECK(send(writer, "H", 1, 0) == 0);
    wait_child(child);
    CHECK(receive(reader, message, 16, &priority) == 1 && message[0] == 'H');

    unsigned before = signals;
    child = fork_child();
    if (!child) {
        pause_ns(50000000);
        CHECK(SC(129, parent, SIGUSR1) == 0);
        SC(94, 0);
        for (;;) {}
    }
    timeout_at(&deadline, 2000000000);
    CHECK(receive_timed(reader, message, 16, &priority, &deadline) == -EINTR);
    CHECK(signals == before + 1);
    wait_child(child);

    CHECK(SC(181, P("/mq-block")) == 0);
    CHECK(SC(57, reader) == 0 && SC(57, writer) == 0 && SC(57, nonblock) == 0);
}

static void notify(void) {
    install_handler();
    struct mq_attr attr = attributes(4, 32);
    long reader = open_queue("/mq-notify", O_CREAT | O_RDONLY, 0600, &attr);
    long writer = open_queue("/mq-notify", O_WRONLY, 0600, &attr);
    CHECK(reader >= 0 && writer >= 0);
    char message[32];
    unsigned priority = 0;

    struct sigevent event = {0, SIGUSR1, SIGEV_SIGNAL, {0}};
    CHECK(SC(MQ_NOTIFY, reader, P(&event)) == 0);
    CHECK(SC(MQ_NOTIFY, writer, P(&event)) == -EBUSY);
    CHECK(read_file("/dev/mqueue/mq-notify", payload, sizeof(payload)) > 0);
    CHECK(contains(payload, "NOTIFY:0"));
    CHECK(contains(payload, "SIGEV:10"));

    unsigned before = signals;
    long child = fork_child();
    if (!child) {
        pause_ns(50000000);
        CHECK(send(writer, "N", 1, 7) == 0);
        SC(94, 0);
        for (;;) {}
    }
    pause_ns(200000000);
    CHECK(signals == before + 1);
    CHECK(receive(reader, message, 32, &priority) == 1 && message[0] == 'N' && priority == 7);
    wait_child(child);
    CHECK(read_file("/dev/mqueue/mq-notify", payload, sizeof(payload)) > 0);
    CHECK(contains(payload, "NOTIFY:0"));
    CHECK(contains(payload, "SIGEV:0"));

    struct sigevent none = {0, 0, SIGEV_NONE, {0}};
    CHECK(SC(MQ_NOTIFY, reader, P(&none)) == 0);
    before = signals;
    child = fork_child();
    if (!child) {
        pause_ns(30000000);
        CHECK(send(writer, "M", 1, 0) == 0);
        SC(94, 0);
        for (;;) {}
    }
    pause_ns(100000000);
    CHECK(signals == before);
    CHECK(receive(reader, message, 32, &priority) == 1 && message[0] == 'M');
    wait_child(child);

    CHECK(SC(MQ_NOTIFY, reader, P(&event)) == 0);
    CHECK(SC(MQ_NOTIFY, reader, 0) == 0);
    before = signals;
    child = fork_child();
    if (!child) {
        pause_ns(30000000);
        CHECK(send(writer, "D", 1, 0) == 0);
        SC(94, 0);
        for (;;) {}
    }
    pause_ns(100000000);
    CHECK(signals == before);
    CHECK(receive(reader, message, 32, &priority) == 1 && message[0] == 'D');
    wait_child(child);

    struct sigevent thread = {0, 0, SIGEV_THREAD, {0}};
    CHECK(SC(MQ_NOTIFY, reader, P(&thread)) == -EOPNOTSUPP);
    struct sigevent unsupported = {0, 0, 3, {0}};
    CHECK(SC(MQ_NOTIFY, reader, P(&unsupported)) == -EOPNOTSUPP);
    struct sigevent zero = {0, 0, SIGEV_SIGNAL, {0}};
    CHECK(SC(MQ_NOTIFY, reader, P(&zero)) == -EINVAL);
    struct sigevent kind = {0, SIGUSR1, 7, {0}};
    CHECK(SC(MQ_NOTIFY, reader, P(&kind)) == -EINVAL);

    CHECK(SC(181, P("/mq-notify")) == 0);
    CHECK(SC(57, reader) == 0 && SC(57, writer) == 0);
}

static void path_view(void) {
    struct mq_attr attr = attributes(2, 32);
    long fd = open_queue("/mq-path", O_CREAT | O_RDONLY, 0600, &attr);
    CHECK(fd >= 0);
    CHECK(send(open_queue("/mq-path", O_WRONLY, 0, 0), "P", 1, 0) == 0);

    char stat[128];
    CHECK(SC(79, AT_FDCWD, P("/dev/mqueue/mq-path"), P(stat), 0) == 0);
    CHECK((*(unsigned *)(stat + 16) & 0170000) == 0100000);
    CHECK(*(long *)(stat + 48) > 0);
    CHECK(SC(79, AT_FDCWD, P("/dev/mqueue/mq-path/more"), P(stat), 0) == -ENOTDIR);

    long directory = SC(56, AT_FDCWD, P("/dev/mqueue"), O_RDONLY | O_DIRECTORY, 0);
    CHECK(directory >= 0);
    CHECK(SC(61, directory, P(payload), sizeof(payload)) > 0);
    SC(57, directory);
    CHECK(SC(56, AT_FDCWD, P("/dev/mqueue/mq-path"), O_DIRECTORY, 0) == -ENOTDIR);
    CHECK(SC(56, AT_FDCWD, P("/dev/mqueue/mq-path/"), O_RDONLY, 0) == -ENOTDIR);

    CHECK(SC(35, AT_FDCWD, P("/dev/mqueue/mq-path"), AT_REMOVEDIR) == -ENOTDIR);
    CHECK(SC(35, AT_FDCWD, P("/dev/mqueue/mq-path"), 0) == 0);
    CHECK(SC(35, AT_FDCWD, P("/dev/mqueue/mq-path"), 0) == -ENOENT);
    CHECK(SC(35, AT_FDCWD, P("/dev/mqueue"), 0) == -EBUSY);
    CHECK(open_queue("/mq-path", O_RDONLY, 0, 0) == -ENOENT);
    CHECK(receive(fd, payload, 32, (unsigned *)stat) == 1 && payload[0] == 'P');
    CHECK(SC(57, fd) == 0);
}

static void lifecycle(void) {
    struct mq_attr attr = attributes(4, 32);
    long reader = open_queue("/mq-life", O_CREAT | O_RDONLY, 0600, &attr);
    long writer = open_queue("/mq-life", O_WRONLY | O_NONBLOCK, 0600, &attr);
    CHECK(reader >= 0 && writer >= 0);

    long copy = SC(23, reader);
    CHECK(copy >= 0 && SC(57, reader) == 0);
    struct mq_attr flags = {O_NONBLOCK, 0, 0, 0, {0, 0, 0, 0}};
    CHECK(SC(MQ_GETSETATTR, copy, P(&flags), 0) == 0);
    CHECK(send(writer, "D", 1, 0) == 0);
    char message[32];
    unsigned priority = 0;
    CHECK(receive(copy, message, 32, &priority) == 1 && message[0] == 'D');

    CHECK(SC(181, P("/mq-life")) == 0);
    CHECK(open_queue("/mq-life", O_RDONLY, 0, 0) == -ENOENT);
    CHECK(send(writer, "U", 1, 0) == 0);
    CHECK(receive(copy, message, 32, &priority) == 1 && message[0] == 'U');

    CHECK(send(writer, "F", 1, 0) == 0);
    long child = fork_child();
    if (!child) {
        CHECK(SC(57, writer) == 0);
        CHECK(receive(copy, message, 32, &priority) == 1 && message[0] == 'F');
        SC(57, copy);
        SC(94, 0);
        for (;;) {}
    }
    CHECK(SC(57, writer) == 0);
    wait_child(child);
    CHECK(SC(57, copy) == 0);

    long fresh = open_queue("/mq-life", O_CREAT | O_RDONLY, 0600, &attr);
    CHECK(fresh >= 0);
    struct mq_attr got;
    CHECK(SC(MQ_GETSETATTR, fresh, 0, P(&got)) == 0 && got.curmsgs == 0);
    CHECK(SC(57, fresh) == 0);
    CHECK(SC(181, P("/mq-life")) == 0);

    long linked = open_queue("/mq-keep", O_CREAT | O_WRONLY, 0600, &attr);
    CHECK(linked >= 0 && send(linked, "K", 1, 0) == 0);
    CHECK(SC(57, linked) == 0);
    long reopened = open_queue("/mq-keep", O_RDONLY, 0, 0);
    CHECK(reopened >= 0);
    CHECK(SC(MQ_GETSETATTR, reopened, 0, P(&got)) == 0 && got.curmsgs == 1);
    CHECK(receive(reopened, message, 32, &priority) == 1 && message[0] == 'K');
    CHECK(SC(57, reopened) == 0);
    CHECK(SC(181, P("/mq-keep")) == 0);
}

static void permission(void) {
    struct mq_attr attr = attributes(2, 32);
    long previous_umask = SC(166, 0);
    CHECK(previous_umask >= 0);
    long owner = open_queue("/mq-perm", O_CREAT | O_RDONLY, 0600, &attr);
    CHECK(owner >= 0);
    CHECK(open_queue("/mq-perm", O_WRONLY, 0, 0) >= 0);
    long child = fork_child();
    if (!child) {
        CHECK(SC(146, 1234) == 0);
        CHECK(open_queue("/mq-perm", O_RDONLY, 0, 0) == -EACCES);
        CHECK(open_queue("/mq-perm", O_WRONLY, 0, 0) == -EACCES);
        CHECK(SC(MQ_UNLINK, P("/mq-perm")) == -EACCES);
        SC(94, 0);
        for (;;) {}
    }
    wait_child(child);

    CHECK(open_queue("/mq-perm-world", O_CREAT | O_RDONLY, 0606, &attr) >= 0);
    child = fork_child();
    if (!child) {
        CHECK(SC(144, 1234) == 0);
        CHECK(SC(146, 1234) == 0);
        long read = open_queue("/mq-perm-world", O_RDONLY, 0, 0);
        long write = open_queue("/mq-perm-world", O_WRONLY, 0, 0);
        CHECK(read >= 0 && write >= 0);
        SC(57, read);
        SC(57, write);
        SC(94, 0);
        for (;;) {}
    }
    wait_child(child);

    CHECK(SC(181, P("/mq-perm-world")) == 0);
    CHECK(SC(181, P("/mq-perm")) == 0);
    CHECK(SC(57, owner) == 0);
    CHECK(SC(166, previous_umask) == 0);
}

int mqueue_main(const char *mode) {
    if (mode[0] == 'b' && mode[1] == 'a') basic();
    else if (mode[0] == 'p' && mode[1] == 'r') priority();
    else if (mode[0] == 'b' && mode[1] == 'l') block();
    else if (mode[0] == 'n') notify();
    else if (mode[0] == 'p' && mode[1] == 'a') path_view();
    else if (mode[0] == 'l') lifecycle();
    else if (mode[0] == 'p' && mode[1] == 'e') permission();
    else mqueue_fail(__LINE__);
    return 42;
}
