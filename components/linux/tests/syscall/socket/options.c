// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

#include "socket.h"

static char payload[65536];

static void set(long fd, int level, int name, int value) {
    CHECK(SC(208, fd, level, name, P(&value), 4) == 0);
}

static int get(long fd, int level, int name) {
    int value = -1;
    unsigned length = 4;
    CHECK(SC(209, fd, level, name, P(&value), P(&length)) == 0 && length == 4);
    return value;
}

static void timeout(long fd, int name, long micros) {
    struct timespec value = {micros / 1000000, micros % 1000000};
    CHECK(SC(208, fd, 1, name, P(&value), 16) == 0);
}

static long now(void) {
    struct timespec value;
    CHECK(SC(113, 1, P(&value)) == 0);
    return value.seconds * 1000000000 + value.nanoseconds;
}

static void pause(void) {
    struct timespec value = {0, 50000000};
    CHECK(SC(101, P(&value), 0) == 0);
}

static void receive_timeout(void) {
    long fd = SC(198, 2, 2, 0);
    CHECK(fd >= 0);
    timeout(fd, 20, 30000);
    struct timespec value = {0};
    unsigned length = sizeof(value);
    CHECK(SC(209, fd, 1, 20, P(&value), P(&length)) == 0);
    CHECK(value.seconds == 0 && value.nanoseconds == 30000 && length == 16);
    value.nanoseconds = 1000000;
    CHECK(SC(208, fd, 1, 20, P(&value), 16) == -33);
    CHECK(SC(208, fd, 1, 20, 0, 16) == -14);
    CHECK(SC(208, fd, 1, 20, P(&value), 8) == -22);
    struct pollfd poll = {fd, 1, 0};
    struct timespec zero = {0};
    CHECK(SC(73, P(&poll), 1, P(&zero), 0, 8) == 0);
    CHECK(SC(207, fd, P(payload), 1, 0x40, 0, 0) == -11);
    long start = now();
    CHECK(SC(207, fd, P(payload), 1, 0, 0, 0) == -11);
    CHECK(now() - start >= 10000000);
    long copy = SC(23, fd);
    CHECK(copy >= 0);
    length = 16;
    CHECK(SC(209, copy, 1, 20, P(&value), P(&length)) == 0 && value.nanoseconds == 30000);
    set(fd, 1, 8, 2048);
    CHECK(get(copy, 1, 8) == 4096);
    set(copy, 1, 8, 0);
    CHECK(get(fd, 1, 8) == 256);
    struct address address = {2, {0}, {127, 0, 0, 1}, {0}};
    length = sizeof(address);
    CHECK(SC(200, fd, P(&address), sizeof(address)) == 0);
    CHECK(SC(204, fd, P(&address), P(&length)) == 0);
    CHECK(SC(206, fd, P(payload), 512, 0, P(&address), sizeof(address)) == 512);
    CHECK(SC(207, fd, P(payload), 512, 0x40, 0, 0) == -11);
    set(fd, 1, 8, 2048);
    CHECK(SC(206, fd, P(payload), 512, 0, P(&address), sizeof(address)) == 512);
    CHECK(SC(207, fd, P(payload), 512, 0, 0, 0) == 512);
    CHECK(SC(208, fd, 1, 7, P(&length), 4) == -95);
    CHECK(SC(209, fd, 6, 4, P(&value), P(&length)) == -92);
    CHECK(SC(57, copy) == 0 && SC(57, fd) == 0);
}

static void stream_options(void) {
    long listener = SC(198, 2, 1, 0);
    struct address address = {2, {0}, {127, 0, 0, 1}, {0}};
    unsigned length = sizeof(address);
    CHECK(SC(200, listener, P(&address), sizeof(address)) == 0);
    CHECK(SC(204, listener, P(&address), P(&length)) == 0);
    CHECK(SC(201, listener, 1) == 0);
    timeout(listener, 20, 30000);
    CHECK(SC(202, listener, 0, 0) == -11);
    long client = SC(198, 2, 1, 0);
    CHECK(SC(203, client, P(&address), sizeof(address)) == 0);
    long peer = SC(202, listener, 0, 0);
    CHECK(peer >= 0);
    struct timespec value = {0};
    length = sizeof(value);
    CHECK(SC(209, peer, 1, 20, P(&value), P(&length)) == 0 && value.nanoseconds == 30000);
    set(client, 1, 9, 1);
    set(client, 6, 4, 7);
    set(client, 6, 5, 3);
    set(client, 6, 6, 4);
    CHECK(get(client, 1, 9) != 0 && get(client, 6, 4) == 7);
    CHECK(get(client, 6, 5) == 3 && get(client, 6, 6) == 4);
    int invalid = 0;
    CHECK(SC(208, client, 6, 4, P(&invalid), 4) == -22);
    CHECK(get(client, 6, 4) == 7);
    CHECK(get(client, 1, 7) > 0 && get(client, 1, 8) > 0);
    CHECK(SC(208, client, 1, 8, P(&length), 4) == -95);
    for (unsigned i = 0; i < sizeof(payload); ++i) payload[i] = 'Q';
    unsigned long queued = 0;
    for (;;) {
        long count = SC(206, client, P(payload), sizeof(payload), 0x40 | 0x4000, 0, 0);
        if (count == -11) break;
        CHECK(count > 0 && (unsigned long)count <= sizeof(payload));
        queued += count;
    }
    CHECK(queued > 0);
    timeout(client, 21, 30000);
    for (;;) {
        long start = now();
        long count = SC(206, client, P(payload), sizeof(payload), 0x4000, 0, 0);
        if (count == -11) { CHECK(now() - start >= 10000000); break; }
        CHECK(count > 0 && count <= 65536);
        queued += count;
    }
    long child = SC(220, 17, 0, 0, 0, 0);
    CHECK(child >= 0);
    if (!child) {
        CHECK(SC(57, client) == 0 && SC(57, listener) == 0);
        timeout(peer, 20, 0);
        pause();
        unsigned long received = 0;
        for (;;) {
            long count = SC(63, peer, P(payload), sizeof(payload));
            CHECK(count >= 0);
            if (!count) break;
            for (long i = 0; i < count; ++i) CHECK(payload[i] == 'Q');
            received += count;
        }
        CHECK(received > queued);
        SC(94, 0);
        for (;;) {}
    }
    CHECK(SC(57, peer) == 0);
    timeout(client, 21, 2000000);
    struct iovec input[] = {{payload, 32768}, {payload + 32768, 32768}};
    long count = SC(66, client, P(input), 2);
    CHECK(count > 0 && count <= 65536);
    CHECK(SC(210, client, 1) == 0);
    int status = -1;
    CHECK(SC(260, child, P(&status), 0, 0) == child && status == 0);
    CHECK(SC(57, client) == 0 && SC(57, listener) == 0);
}

void socket_options(void) {
    receive_timeout();
    stream_options();
}
