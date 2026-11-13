// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

#include "socket.h"

extern void datagram_main(void);
extern void datagram_memory(void);
extern void socket_options(void);
extern void socket_ipv6(void);
static unsigned char payload[131072];
static volatile unsigned signals;

void socket_fail(unsigned line) {
    char text[] = "Socket assertion line 0000\n";
    for (int i = 25; i >= 22; --i) { text[i] = '0' + line % 10; line /= 10; }
    SC(64, 1, P(text), sizeof(text) - 1);
    SC(94, 1);
    for (;;) {}
}

static void pause_ns(long ns) {
    struct timespec duration = {0, ns};
    CHECK(SC(101, P(&duration), 0) == 0);
}

static void close_fd(long fd) { CHECK(SC(57, fd) == 0); }
static long socket_fd(int flags) {
    long fd = SC(198, 2, 1 | flags, 0);
    CHECK(fd >= 0);
    return fd;
}

static int option(long fd, int level, int name) {
    int value = -1;
    unsigned length = sizeof(value);
    CHECK(SC(209, fd, level, name, P(&value), P(&length)) == 0);
    CHECK(length == sizeof(value));
    return value;
}

static void set_option(long fd, int level, int name, int value) {
    CHECK(SC(208, fd, level, name, P(&value), sizeof(value)) == 0);
    CHECK(option(fd, level, name) == value);
}

static struct address loopback(unsigned port) {
    struct address address = {2, {port >> 8, port}, {127, 0, 0, 1}, {0}};
    return address;
}

static long listener(struct address *address) {
    long fd = socket_fd(0);
    set_option(fd, 1, 2, 1);
    CHECK(SC(200, fd, P(address), sizeof(*address)) == 0);
    unsigned length = sizeof(*address);
    CHECK(SC(204, fd, P(address), P(&length)) == 0);
    CHECK(length == sizeof(*address));
    CHECK(address->port[0] || address->port[1]);
    CHECK(SC(201, fd, 8) == 0);
    return fd;
}

static short ready(long fd, short events) {
    struct pollfd poll = {fd, events, 0};
    struct timespec duration = {10, 0};
    CHECK(SC(73, P(&poll), 1, P(&duration), 0, 8) == 1);
    CHECK(poll.revents & (events | 8 | 16));
    return poll.revents;
}

static void wait_child(long child) {
    int status = -1;
    CHECK(SC(260, child, P(&status), 0, 0) == child);
    CHECK(status == 0);
}

static void client(void) {
    struct address address = loopback(23140);
    long fd = -1;
    for (int attempt = 0; attempt < 100; ++attempt) {
        fd = socket_fd(0x800 | 0x80000);
        CHECK(SC(25, fd, 1, 0) == 1);
        long result = SC(203, fd, P(&address), sizeof(address));
        CHECK(result == 0 || result == -115 || result == -111);
        if (result == -115) {
            ready(fd, 4);
            long copy = SC(23, fd);
            CHECK(copy >= 0);
            result = SC(203, copy, P(&address), sizeof(address));
            close_fd(copy);
        }
        if (result == 0) break;
        CHECK(result == -111);
        close_fd(fd);
        fd = -1;
        pause_ns(10000000);
    }
    CHECK(fd >= 0);
    CHECK(SC(203, fd, P(&address), sizeof(address)) == -106);
    CHECK(option(fd, 1, 3) == 1);
    set_option(fd, 6, 1, 1);
    unsigned length = sizeof(address);
    CHECK(SC(205, fd, P(&address), P(&length)) == 0);
    CHECK(address.port[0] == (23140 >> 8) && address.port[1] == (23140 & 255));
    long copy = SC(23, fd);
    CHECK(copy >= 0);
    CHECK(SC(25, copy, 4, 0) == 0);
    CHECK((SC(25, fd, 3) & 0x800) == 0);
    close_fd(copy);
    for (unsigned i = 0; i < sizeof(payload); ++i) payload[i] = i * 37;
    unsigned offset = 0;
    while (offset < sizeof(payload)) {
        unsigned remaining = sizeof(payload) - offset;
        struct iovec input[] = {{payload + offset, remaining / 2}, {payload + offset + remaining / 2, remaining - remaining / 2}};
        struct message message = {0, 0, 0, input, 2, 0, 0, 0, 0};
        long count = offset == 0 ? SC(211, fd, P(&message), 0x4000) : SC(66, fd, P(input), 2);
        CHECK(count > 0 && count <= 65536);
        offset += count;
    }
    CHECK(SC(210, fd, 1) == 0);
    unsigned char response[2] = {0};
    CHECK(SC(63, fd, P(response), 2) == 2);
    CHECK(response[0] == 'O' && response[1] == 'K');
    ready(fd, 1);
    length = sizeof(address);
    CHECK(SC(207, fd, P(response), 2, 0, P(&address), P(&length)) == 0);
    CHECK(length == 0);
    close_fd(fd);
}

static void server(void) {
    struct address address = loopback(23140);
    long fd = listener(&address);
    CHECK(SC(25, fd, 4, 0x800) == 0);
    CHECK(SC(202, fd, 0, 0) == -11);
    CHECK(SC(25, fd, 4, 0) == 0);
    unsigned length = sizeof(address);
    long accepted = SC(242, fd, P(&address), P(&length), 0x800 | 0x80000);
    CHECK(accepted >= 0 && length == sizeof(address) && address.family == 2);
    CHECK(SC(25, accepted, 1, 0) == 1);
    CHECK(SC(25, accepted, 3, 0) & 0x800);
    CHECK((SC(25, fd, 3, 0) & 0x800) == 0);
    close_fd(fd);
    CHECK(SC(25, accepted, 4, 0) == 0);
    unsigned total = 0;
    for (;;) {
        struct iovec output[] = {{payload, 3072}, {payload + 3072, 5120}};
        struct message message = {&address, sizeof(address), 0, output, 2, 0, 0, 0, 0};
        long count = total == 0 ? SC(212, accepted, P(&message), 0) : SC(65, accepted, P(output), 2);
        if (total == 0) CHECK(message.name_length == 0);
        CHECK(message.flags == 0);
        CHECK(count >= 0);
        if (!count) break;
        for (long i = 0; i < count; ++i) CHECK(payload[i] == (unsigned char)((total + i) * 37));
        total += count;
    }
    CHECK(total == sizeof(payload));
    CHECK(SC(64, accepted, P("OK"), 2) == 2);
    CHECK(SC(210, accepted, 1) == 0);
    close_fd(accepted);
}

static void refused(void) {
    long fd = socket_fd(0x800);
    unsigned char kind[2] = {0, 0x7f};
    unsigned length = 1;
    CHECK(SC(209, fd, 1, 3, P(kind), P(&length)) == 0);
    CHECK(length == 1 && kind[0] == 1 && kind[1] == 0x7f);
    length = 0;
    CHECK(SC(209, fd, 1, 3, 0, P(&length)) == 0 && length == 0);
    struct address address = loopback(23141);
    long result = SC(203, fd, P(&address), sizeof(address));
    CHECK(result == -115 || result == -111);
    if (result == -115) {
        CHECK(ready(fd, 4) & (4 | 8));
        CHECK(SC(203, fd, P(&address), sizeof(address)) == -111);
    }
    CHECK(option(fd, 1, 4) == 0);
    close_fd(fd);
    CHECK(SC(200, 1, P(&address), sizeof(address)) == -88);
    CHECK(SC(198, 1, 1, 0) == -97);
}

static void fork_socket(void) {
    struct address address = loopback(0);
    long fd = listener(&address);
    long copy = SC(23, fd);
    CHECK(copy >= 0);
    long child = SC(220, 17, 0, 0, 0, 0);
    CHECK(child >= 0);
    if (!child) {
        close_fd(fd);
        CHECK(SC(25, copy, 4, 0x800) == 0);
        close_fd(copy);
        long client = socket_fd(0);
        CHECK(SC(203, client, P(&address), sizeof(address)) == 0);
        CHECK(SC(64, client, P("X"), 1) == 1);
        close_fd(client);
        SC(94, 0);
        for (;;) {}
    }
    close_fd(fd);
    ready(copy, 1);
    CHECK(SC(25, copy, 3, 0) & 0x800);
    long accepted = SC(202, copy, 0, 0);
    CHECK(accepted >= 0);
    CHECK((SC(25, accepted, 3, 0) & 0x800) == 0);
    close_fd(copy);
    CHECK(SC(63, accepted, 0, 1) == -14);
    int pipe[2];
    CHECK(SC(59, P(pipe), 0) == 0);
    CHECK(SC(64, pipe[1], P("P"), 1) == 1);
    struct pollfd mixed[] = {{accepted, 1, 0}, {pipe[0], 1, 0}, {1, 4, 0}};
    struct timespec zero = {0, 0};
    CHECK(SC(73, P(mixed), 3, P(&zero), 0, 8) == 3);
    CHECK((mixed[0].revents & 1) && (mixed[1].revents & 1) && (mixed[2].revents & 4));
    CHECK(accepted < 64);
    unsigned long readset = 1UL << accepted;
    CHECK(SC(72, accepted + 1, P(&readset), 0, 0, P(&zero), 0) == 1);
    CHECK(readset == (1UL << accepted));
    CHECK(SC(63, accepted, 0, 0) == 0);
    char byte;
    CHECK(SC(207, accepted, P(&byte), 1, 2, 0, 0) == 1 && byte == 'X');
    CHECK(SC(63, accepted, P(&byte), 1) == 1 && byte == 'X');
    wait_child(child);
    CHECK(SC(63, accepted, P(&byte), 1) == 0);
    close_fd(accepted);
    close_fd(pipe[0]);
    close_fd(pipe[1]);
}

static void exec_socket(const char *mode) {
    if (mode[1] != 'x') {
        long closed = mode[1] - '0', retained = mode[2] - '0';
        CHECK(SC(25, closed, 1) == -9);
        CHECK(SC(25, retained, 1) == 0);
        CHECK(option(retained, 1, 3) == 1);
        close_fd(retained);
        return;
    }
    long fd = socket_fd(0x80000), copy = SC(23, fd);
    CHECK(fd < 10 && copy >= 0 && copy < 10);
    char argument[] = {'e', '0' + fd, '0' + copy, 0};
    const char *arguments[] = {"/bin/linux-socket", argument, 0};
    const char *environment[] = {0};
    SC(221, P(arguments[0]), P(arguments), P(environment));
    CHECK(0);
}

static void handler(int number) { if (number == 10 || number == 13) ++signals; }

static void signal_socket(unsigned long flags, int timed) {
    struct { long handler; unsigned long flags, mask; } action = {P(handler), flags, 0};
    CHECK(SC(134, 10, P(&action), 0, 8) == 0);
    CHECK(SC(134, 13, P(&action), 0, 8) == 0);
    struct address address = loopback(0);
    long fd = listener(&address);
    int pipe[2];
    CHECK(SC(59, P(pipe), 0) == 0);
    long parent = SC(172);
    long child = SC(220, 17, 0, 0, 0, 0);
    CHECK(child >= 0);
    if (!child) {
        close_fd(fd);
        close_fd(pipe[1]);
        long client = socket_fd(0);
        CHECK(SC(203, client, P(&address), sizeof(address)) == 0);
        char byte;
        CHECK(SC(63, pipe[0], P(&byte), 1) == 1);
        pause_ns(100000000);
        CHECK(SC(129, parent, 10) == 0);
        pause_ns(100000000);
        CHECK(SC(64, client, P("S"), 1) == 1);
        close_fd(client);
        SC(94, 0);
        for (;;) {}
    }
    close_fd(pipe[0]);
    long accepted = SC(202, fd, 0, 0);
    CHECK(accepted >= 0);
    close_fd(fd);
    if (timed) {
        long duration[] = {2, 0};
        CHECK(SC(208, accepted, 1, 20, P(duration), sizeof(duration)) == 0);
    }
    CHECK(SC(64, pipe[1], P("R"), 1) == 1);
    char byte = 0;
    unsigned previous = signals;
    long result = SC(63, accepted, P(&byte), 1);
    if (!flags || timed) { CHECK(result == -4); result = SC(63, accepted, P(&byte), 1); }
    CHECK(result == 1 && byte == 'S' && signals == previous + 1);
    wait_child(child);
    CHECK(SC(210, accepted, 1) == 0);
    previous = signals;
    struct iovec input = {(void *)"X", 1};
    struct message message = {0, 0, 0, &input, 1, 0, 0, 0, 0};
    CHECK(SC(211, accepted, P(&message), 0x4000) == -32);
    CHECK(signals == previous);
    CHECK(SC(64, accepted, P("X"), 1) == -32);
    CHECK(signals == previous + 1);
    close_fd(accepted);
    close_fd(pipe[1]);
}

static void shared_reads(void) {
    struct address address = loopback(0);
    long fd = listener(&address);
    int pipe[2];
    CHECK(SC(59, P(pipe), 0) == 0);
    long writer = SC(220, 17, 0, 0, 0, 0);
    CHECK(writer >= 0);
    if (!writer) {
        close_fd(fd);
        close_fd(pipe[1]);
        long client = socket_fd(0);
        CHECK(SC(203, client, P(&address), sizeof(address)) == 0);
        char byte;
        CHECK(SC(63, pipe[0], P(&byte), 1) == 1);
        pause_ns(100000000);
        CHECK(SC(64, client, P("XX"), 2) == 2);
        close_fd(client);
        SC(94, 0);
        for (;;) {}
    }
    long accepted = SC(202, fd, 0, 0);
    CHECK(accepted >= 0);
    close_fd(fd);
    close_fd(pipe[0]);
    long reader = SC(220, 17, 0, 0, 0, 0);
    CHECK(reader >= 0);
    if (!reader) {
        close_fd(pipe[1]);
        char byte;
        CHECK(SC(63, accepted, P(&byte), 1) == 1 && byte == 'X');
        close_fd(accepted);
        SC(94, 0);
        for (;;) {}
    }
    CHECK(SC(64, pipe[1], P("R"), 1) == 1);
    close_fd(pipe[1]);
    char byte;
    CHECK(SC(63, accepted, P(&byte), 1) == 1 && byte == 'X');
    wait_child(reader);
    wait_child(writer);
    CHECK(SC(63, accepted, P(&byte), 1) == 0);
    close_fd(accepted);
}

static void lifecycle(void) {
    shared_reads();
    struct address address = loopback(0);
    long fd = listener(&address);
    int pipe[2];
    CHECK(SC(59, P(pipe), 0) == 0);
    long child = SC(220, 17, 0, 0, 0, 0);
    CHECK(child >= 0);
    if (!child) {
        close_fd(pipe[0]);
        CHECK(SC(64, pipe[1], P("R"), 1) == 1);
        SC(202, fd, 0, 0);
        CHECK(0);
    }
    close_fd(pipe[1]);
    char byte;
    CHECK(SC(63, pipe[0], P(&byte), 1) == 1);
    pause_ns(100000000);
    CHECK(SC(129, child, 9) == 0);
    int status = 0;
    CHECK(SC(260, child, P(&status), 0, 0) == child && status == 9);
    close_fd(pipe[0]);
    long client = socket_fd(0);
    CHECK(SC(203, client, P(&address), sizeof(address)) == 0);
    long accepted = SC(202, fd, 0, 0);
    CHECK(accepted >= 0);
    close_fd(fd);
    CHECK(SC(64, client, P("R"), 1) == 1);
    ready(accepted, 1);
    CHECK(SC(64, accepted, P("XYZ"), 3) == 3);
    unsigned length = sizeof(address);
    CHECK(SC(207, client, 0, 3, 0, P(&address), P(&length)) == -14);
    CHECK(SC(207, client, P(&byte), 1, 2, P(&address), P(&length)) == 1);
    CHECK(byte == 'X' && length == 0);
    // Closing with unread data aborts the TCP connection.
    close_fd(accepted);
    CHECK(ready(client, 8) & 8);
    char bytes[3] = {0};
    struct iovec output = {bytes, sizeof(bytes)};
    struct message message = {&address, sizeof(address), 0, &output, 1, 0, 0, 0, 0};
    CHECK(SC(212, client, P(&message), 0) == 3);
    CHECK(bytes[0] == 'X' && bytes[1] == 'Y' && bytes[2] == 'Z');
    CHECK(message.name_length == 0 && message.flags == 0);
    length = sizeof(address);
    CHECK(SC(207, client, P(&byte), 1, 0, P(&address), P(&length)) == -104);
    message.name_length = sizeof(address);
    CHECK(SC(212, client, P(&message), 0) == 0 && message.name_length == 0);
    CHECK(SC(207, client, P(&byte), 1, 0, P(&address), P(&length)) == 0);
    CHECK(length == 0);
    close_fd(client);
    // Final close releases the listening endpoint for a new socket.
    fd = listener(&address);
    close_fd(fd);
}

int socket_main(const char *mode) {
    if (*mode == 'i') socket_ipv6();
    else if (*mode == 'o') socket_options();
    else if (*mode == 'm') datagram_memory();
    else if (*mode == 'u') datagram_main();
    else if (*mode == 'c') client();
    else if (*mode == 'r') refused();
    else if (*mode == 'f') fork_socket();
    else if (*mode == 'e') exec_socket(mode);
    else if (*mode == 'l') lifecycle();
    else if (mode[1] == 'e') server();
    else { signal_socket(0, 0); signal_socket(0x10000000, 0); signal_socket(0x10000000, 1); }
    return 42;
}
