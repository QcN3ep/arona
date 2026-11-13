// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

#include "socket.h"

static unsigned char bytes[65508];

static void close_fd(long fd) { CHECK(SC(57, fd) == 0); }

static long udp(struct address *endpoint) {
    long fd = SC(198, 2, 2, 17);
    CHECK(fd >= 0);
    *endpoint = (struct address){2, {0, 0}, {127, 0, 0, 1}, {0}};
    CHECK(SC(200, fd, P(endpoint), sizeof(*endpoint)) == 0);
    unsigned length = sizeof(*endpoint);
    CHECK(SC(204, fd, P(endpoint), P(&length)) == 0);
    CHECK(length == sizeof(*endpoint) && (endpoint->port[0] || endpoint->port[1]));
    return fd;
}

static void wait_ready(long fd, short events) {
    struct pollfd entry = {fd, events, 0};
    struct timespec duration = {10, 0};
    CHECK(SC(73, P(&entry), 1, P(&duration), 0, 8) == 1);
    CHECK(entry.revents & events);
}

static void receive_faults(long fd, long sender, struct address *target) {
    struct iovec input[] = {{"one", 3}, {0, 1}};
    struct message message = {target, sizeof(*target), 0, input, 2, 0, 0, 0, 0};
    CHECK(SC(211, sender, P(&message), 0) == -14);
    char data[8] = {0};
    CHECK(SC(207, fd, P(data), sizeof(data), 0x40, 0, 0) == -11);
    CHECK(SC(206, sender, P("abcdef"), 6, 0, P(target), sizeof(*target)) == 6);
    wait_ready(fd, 1);
    CHECK(SC(212, fd, 0, 0) == -14);
    message.iov_length = 1025;
    CHECK(SC(212, fd, P(&message), 0) == -22);
    struct iovec invalid_source = {data, sizeof(data)};
    message = (struct message){(void *)1, 16, 0, &invalid_source, 1, 0, 0, 0, 0};
    CHECK(SC(212, fd, P(&message), 0) == -14);
    CHECK(SC(207, fd, 0, 6, 0, 0, 0) == -14);
    wait_ready(fd, 1);
    CHECK(SC(207, fd, P(data), 3, 2, 0, 0) == 3);
    CHECK(data[0] == 'a' && data[2] == 'c');
    CHECK(SC(207, fd, P(data), 3, 2 | 0x20, 0, 0) == 6);
    struct address source;
    struct iovec output[] = {{data, 1}, {data + 1, 2}};
    message = (struct message){&source, 4, 0, output, 2, 0, 0, 0x7fffffff, 0};
    CHECK(SC(212, fd, P(&message), 2) == 3);
    CHECK(message.name_length == 16 && message.flags == 0x20 && message.control_length == 0);
    CHECK(source.family == 2);
    message.name_length = sizeof(source);
    CHECK(SC(212, fd, P(&message), 0x20) == 6 && message.flags == 0x20);
    CHECK(SC(207, fd, P(data), 1, 0x40, 0, 0) == -11);
    // A failed scatter keeps the whole datagram available for a valid retry.
    CHECK(SC(206, sender, P("ABC"), 3, 0, P(target), sizeof(*target)) == 3);
    output[0] = (struct iovec){data, 1};
    output[1] = (struct iovec){0, 2};
    CHECK(SC(65, fd, P(output), 2) == -14);
    output[1] = (struct iovec){data + 1, 2};
    CHECK(SC(65, fd, P(output), 2) == 3);
    CHECK(data[0] == 'A' && data[1] == 'B' && data[2] == 'C');
    CHECK(SC(207, fd, P(data), 1, 0x40, 0, 0) == -11);
    // A successful short scatter consumes this datagram, including its uncopied tail.
    CHECK(SC(206, sender, P("ABC"), 3, 0, P(target), sizeof(*target)) == 3);
    CHECK(SC(65, fd, P(output), 1) == 1 && data[0] == 'A');
    CHECK(SC(207, fd, P(data), 1, 0x40, 0, 0) == -11);
    // Unsupported control data and flags leave the queued datagram untouched.
    CHECK(SC(206, sender, P("Z"), 1, 0, P(target), sizeof(*target)) == 1);
    message.control = data;
    message.control_length = 1;
    CHECK(SC(212, fd, P(&message), 0) == -95);
    CHECK(SC(211, sender, P(&message), 0) == -95);
    CHECK(SC(207, fd, P(data), 1, 0x100, 0, 0) == -95);
    CHECK(SC(207, fd, P(data), 1, 0, 0, 0) == 1 && data[0] == 'Z');
    CHECK(SC(206, sender, 0, 0, 0, P(target), sizeof(*target)) == 0);
    wait_ready(fd, 1);
    CHECK(SC(63, fd, 0, 0) == 0);
    CHECK(SC(65, fd, 0, 0) == 0);
    wait_ready(fd, 1);
    CHECK(SC(207, fd, 0, 0, 2, 0, 0) == 0);
    wait_ready(fd, 1);
    CHECK(SC(207, fd, 0, 0, 0, 0, 0) == 0);
    CHECK(SC(207, fd, P(data), 1, 0x40, 0, 0) == -11);
}

static void datagram_processes(long receiver, long sender, struct address target, struct address source) {
    long child = SC(220, 17, 0, 0, 0, 0);
    CHECK(child >= 0);
    if (!child) {
        close_fd(receiver);
        CHECK(SC(203, sender, P(&target), sizeof(target)) == 0);
        // Connected UDP also permits selecting a new peer and an explicit destination.
        CHECK(SC(203, sender, P(&target), sizeof(target)) == 0);
        for (unsigned size = 0;; size = size ? 65507 : 1) {
            for (unsigned i = 0; i < size; ++i) bytes[i] = (unsigned char)(i * 29);
            struct iovec input[] = {{bytes, size / 2}, {bytes + size / 2, size - size / 2}};
            struct message message = {0, 0, 0, input, 2, 0, 0, 0, 0};
            CHECK(SC(211, sender, P(&message), 0x4000) == size);
            char ack;
            CHECK(SC(63, sender, P(&ack), 1) == 1 && ack == 'K');
            if (size == 65507) break;
        }
        unsigned short family = 0;
        CHECK(SC(203, sender, P(&family), sizeof(family)) == 0);
        unsigned length = sizeof(target);
        CHECK(SC(205, sender, P(&target), P(&length)) == -107);
        CHECK(SC(64, sender, P("x"), 1) == -89);
        close_fd(sender);
        SC(94, 0);
        for (;;) {}
    }
    close_fd(sender);
    for (unsigned size = 0;; size = size ? 65507 : 1) {
        wait_ready(receiver, 1);
        struct address peer = {0};
        unsigned length = sizeof(peer);
        CHECK(SC(207, receiver, P(bytes), sizeof(bytes), 0, P(&peer), P(&length)) == size);
        CHECK(length == sizeof(peer) && peer.family == 2);
        CHECK(peer.port[0] == source.port[0] && peer.port[1] == source.port[1]);
        for (unsigned i = 0; i < size; ++i) CHECK(bytes[i] == (unsigned char)(i * 29));
        struct iovec reply[] = {{"K", 1}, {0, 0}};
        struct message message = {&peer, sizeof(peer), 0, reply, 2, 0, 0, 0, 0};
        CHECK(SC(211, receiver, P(&message), 0) == 1);
        if (size == 65507) break;
    }
    int status = -1;
    CHECK(SC(260, child, P(&status), 0, 0) == child && status == 0);
}

void datagram_main(void) {
    struct address target, source;
    long receiver = udp(&target), sender = udp(&source);
    int kind = 0;
    unsigned kind_length = sizeof(kind);
    CHECK(SC(209, receiver, 1, 3, P(&kind), P(&kind_length)) == 0 && kind == 2);
    CHECK(SC(201, receiver, 1) == -95);
    unsigned length = sizeof(target);
    CHECK(SC(205, receiver, P(&target), P(&length)) == -107);
    CHECK(SC(206, sender, 0, 0, 0, 0, 0) == -89);
    CHECK(SC(206, sender, P(bytes), sizeof(bytes), 0, P(&target), sizeof(target)) == -90);
    receive_faults(receiver, sender, &target);
    datagram_processes(receiver, sender, target, source);
    close_fd(receiver);
}

static void phase(long file, char value, int write) {
    for (;;) {
        char state = value;
        long count = SC(write ? 68 : 67, file, P(&state), 1, 0);
        if (count == -12) continue;
        CHECK(count == 1);
        if (write || state == value) return;
        SC(124);
    }
}

void datagram_memory(void) {
    long file = SC(56, -100, P("/var/linux-socket-memory"), 2, 0);
    CHECK(file >= 0);
    struct address target;
    long fd = udp(&target);
    for (unsigned i = 0; i < 65507; ++i) bytes[i] = (unsigned char)(i * 31);
    CHECK(SC(206, fd, P(bytes), 65507, 0, P(&target), sizeof(target)) == 65507);
    wait_ready(fd, 1);
    phase(file, 1, 1);
    phase(file, 2, 0);
    CHECK(SC(207, fd, P(bytes), sizeof(bytes), 0, 0, 0) == -12);
    phase(file, 3, 1);
    phase(file, 4, 0);
    struct iovec output[] = {{bytes, 100}, {bytes + 100, sizeof(bytes) - 100}};
    struct message message = {0, 0, 0, output, 2, 0, 0, 0, 0};
    CHECK(SC(212, fd, P(&message), 0) == 65507);
    CHECK(message.flags == 0);
    for (unsigned i = 0; i < 65507; ++i) CHECK(bytes[i] == (unsigned char)(i * 31));
    CHECK(SC(207, fd, P(bytes), sizeof(bytes), 0x40, 0, 0) == -11);
    close_fd(file);
    close_fd(fd);
}
