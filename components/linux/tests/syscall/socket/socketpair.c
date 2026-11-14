// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

#include "socket.h"

#define SOCKETPAIR 199
#define STREAM 1
#define DATAGRAM 2
#define NONBLOCK 0x800
#define CLOEXEC 0x80000
#define PEEK 2
#define TRUNC 0x20
#define DONTWAIT 0x40
#define NOSIGNAL 0x4000
#define SHUT_RD 0
#define SHUT_WR 1
#define SHUT_BOTH 2

static unsigned char payload[131072];
static unsigned char scratch[131072];
static volatile unsigned signals;

static void close_fd(long fd) { CHECK(SC(57, fd) == 0); }

static void pause_ns(long ns) {
    struct timespec duration = {0, ns};
    CHECK(SC(101, P(&duration), 0) == 0);
}

static int same(const unsigned char *left, const unsigned char *right, unsigned long count) {
    for (unsigned long index = 0; index < count; ++index) {
        if (left[index] != right[index]) return 0;
    }
    return 1;
}

static void make_pair(int type, int flags, int pair[2]) {
    CHECK(SC(SOCKETPAIR, 1, type | flags, 0, P(pair)) == 0);
    CHECK(pair[0] >= 0 && pair[1] >= 0 && pair[0] != pair[1]);
}

static short poll_wait(long fd, short events, long seconds) {
    struct pollfd poll = {fd, events, 0};
    struct timespec duration = {seconds, 0};
    CHECK(SC(73, P(&poll), 1, P(&duration), 0, 8) == 1);
    return poll.revents;
}

static short poll_now(long fd, short events) {
    struct pollfd poll = {fd, events, 0};
    struct timespec zero = {0, 0};
    long count = SC(73, P(&poll), 1, P(&zero), 0, 8);
    CHECK(count == 0 || count == 1);
    return poll.revents;
}

static void handler(int number) {
    if (number == 13) ++signals;
}

static void stream_io(void) {
    int pair[2];
    make_pair(STREAM, 0, pair);
    for (unsigned i = 0; i < sizeof(payload); ++i) payload[i] = i * 29;
    CHECK(SC(64, pair[0], P(payload), 4096) == 4096);
    unsigned char *buffer = scratch;
    CHECK(SC(63, pair[1], P(buffer), 1000) == 1000);
    CHECK(same(buffer, payload, 1000));
    CHECK(SC(63, pair[1], P(buffer), 4096) == 3096);
    CHECK(same(buffer, payload + 1000, 3096));
    CHECK(SC(207, pair[1], P(buffer), 1, DONTWAIT, 0, 0) == -11);
    // Both directions carry independent byte streams.
    CHECK(SC(64, pair[1], P("reply"), 5) == 5);
    CHECK(SC(63, pair[0], P(buffer), 5) == 5);
    CHECK(same(buffer, (const unsigned char *)"reply", 5));
    // A zero-length write does not publish a byte.
    CHECK(SC(64, pair[0], P(payload), 0) == 0);
    CHECK(poll_now(pair[1], 1) == 0);
    // Streaming boundaries are short transfers, not lost data.
    CHECK(SC(64, pair[0], P(payload), 70000) == 65536);
    CHECK(SC(63, pair[1], P(buffer), 1000) == 1000);
    CHECK(same(buffer, payload, 1000));
    CHECK(SC(63, pair[1], P(buffer), 65536) == 64536);
    CHECK(same(buffer, payload + 1000, 64536));
    // Duplicated descriptors share the same receive queue.
    CHECK(SC(64, pair[0], P("12"), 2) == 2);
    long copy = SC(23, pair[1]);
    CHECK(copy >= 0);
    CHECK(SC(63, copy, P(buffer), 1) == 1 && buffer[0] == '1');
    CHECK(SC(63, pair[1], P(buffer), 1) == 1 && buffer[0] == '2');
    close_fd(copy);
    close_fd(pair[0]);
    close_fd(pair[1]);
}

static void stream_peek(void) {
    int pair[2];
    make_pair(STREAM, 0, pair);
    CHECK(SC(64, pair[0], P("abcd"), 4) == 4);
    unsigned char buffer[8] = {0};
    CHECK(SC(207, pair[1], P(buffer), 2, PEEK, 0, 0) == 2);
    CHECK(same(buffer, (const unsigned char *)"ab", 2));
    CHECK(SC(63, pair[1], P(buffer), 4) == 4);
    CHECK(same(buffer, (const unsigned char *)"abcd", 4));
    CHECK(SC(207, pair[1], P(buffer), 1, DONTWAIT, 0, 0) == -11);
    close_fd(pair[0]);
    close_fd(pair[1]);
}

static void stream_eof(void) {
    int pair[2];
    make_pair(STREAM, 0, pair);
    CHECK(SC(64, pair[1], P("data"), 4) == 4);
    close_fd(pair[1]);
    unsigned char buffer[8] = {0};
    CHECK(SC(63, pair[0], P(buffer), 8) == 4);
    CHECK(same(buffer, (const unsigned char *)"data", 4));
    CHECK(SC(63, pair[0], P(buffer), 8) == 0);
    CHECK(SC(206, pair[0], P("x"), 1, NOSIGNAL, 0, 0) == -32);
    short events = poll_now(pair[0], 1 | 4);
    CHECK(events & 1);
    CHECK(events & 16);
    CHECK((events & 4) == 0);
    close_fd(pair[0]);

    // A broken stream write raises SIGPIPE unless MSG_NOSIGNAL is supplied.
    struct { long handler; unsigned long flags, mask; } action = {P(handler), 0, 0};
    CHECK(SC(134, 13, P(&action), 0, 8) == 0);
    make_pair(STREAM, 0, pair);
    close_fd(pair[1]);
    unsigned before = signals;
    CHECK(SC(64, pair[0], P("x"), 1) == -32);
    CHECK(signals == before + 1);
    CHECK(SC(206, pair[0], P("x"), 1, NOSIGNAL, 0, 0) == -32);
    CHECK(signals == before + 1);
    close_fd(pair[0]);
}

static void stream_shutdown(void) {
    int pair[2];
    make_pair(STREAM, 0, pair);
    CHECK(SC(64, pair[0], P("abc"), 3) == 3);
    CHECK(SC(210, pair[0], SHUT_WR) == 0);
    unsigned char buffer[8] = {0};
    CHECK(SC(63, pair[1], P(buffer), 8) == 3);
    CHECK(same(buffer, (const unsigned char *)"abc", 3));
    CHECK(SC(63, pair[1], P(buffer), 8) == 0);
    CHECK(SC(206, pair[0], P("x"), 1, NOSIGNAL, 0, 0) == -32);
    CHECK(SC(64, pair[1], P("xyz"), 3) == 3);
    CHECK(SC(63, pair[0], P(buffer), 3) == 3);
    CHECK(SC(210, pair[0], SHUT_RD) == 0);
    CHECK(SC(63, pair[0], P(buffer), 1) == 0);
    CHECK(SC(206, pair[1], P("x"), 1, NOSIGNAL, 0, 0) == -32);
    CHECK(SC(210, pair[1], SHUT_BOTH) == 0);
    short events = poll_now(pair[1], 1 | 4);
    CHECK(events & 1);
    CHECK(events & 16);
    CHECK(SC(210, pair[1], 3) == -22);
    close_fd(pair[0]);
    close_fd(pair[1]);
}

static void stream_nonblocking(void) {
    int pair[2];
    make_pair(STREAM, NONBLOCK, pair);
    CHECK((SC(25, pair[0], 3) & NONBLOCK) != 0);
    CHECK((SC(25, pair[1], 3) & NONBLOCK) != 0);
    unsigned char buffer[8];
    CHECK(SC(63, pair[0], P(buffer), 1) == -11);
    CHECK(SC(207, pair[0], P(buffer), 1, DONTWAIT, 0, 0) == -11);
    CHECK(SC(64, pair[0], P(payload), 65536) == 65536);
    CHECK(SC(64, pair[0], P(payload), 1) == -11);
    CHECK(SC(206, pair[0], P(payload), 1, DONTWAIT | NOSIGNAL, 0, 0) == -11);
    long copy = SC(23, pair[0]);
    CHECK(copy >= 0);
    CHECK(SC(25, copy, 4, 0) == 0);
    CHECK((SC(25, pair[0], 3) & NONBLOCK) == 0);
    CHECK((SC(25, copy, 3) & NONBLOCK) == 0);
    CHECK(SC(25, pair[0], 4, NONBLOCK) == 0);
    CHECK(SC(63, pair[1], P(buffer), 1) == 1);
    CHECK(SC(64, pair[0], P(payload), 1) == 1);
    close_fd(copy);
    close_fd(pair[0]);
    close_fd(pair[1]);
}

static void stream_poll(void) {
    int pair[2];
    make_pair(STREAM, 0, pair);
    CHECK((poll_now(pair[0], 1 | 4) & 4) != 0);
    CHECK((poll_now(pair[1], 1) & 1) == 0);
    CHECK(SC(64, pair[0], P("p"), 1) == 1);
    short events = poll_now(pair[1], 1);
    CHECK(events & 1);
    CHECK((poll_now(pair[1], 4) & 4) != 0);
    CHECK(SC(210, pair[0], SHUT_WR) == 0);
    events = poll_now(pair[1], 1);
    CHECK(events & 1);
    CHECK(SC(63, pair[1], P(payload), 1) == 1);
    CHECK(SC(63, pair[1], P(payload), 1) == 0);
    close_fd(pair[0]);
    events = poll_now(pair[1], 1 | 4);
    CHECK(events & 1);
    CHECK(events & 16);
    CHECK((events & 4) == 0);
    close_fd(pair[1]);
}

static void stream_fork(void) {
    int pair[2];
    make_pair(STREAM, 0, pair);
    long copy = SC(23, pair[1]);
    CHECK(copy >= 0);
    long child = SC(220, 17, 0, 0, 0, 0);
    CHECK(child >= 0);
    if (!child) {
        close_fd(pair[0]);
        close_fd(copy);
        pause_ns(50000000);
        CHECK(SC(64, pair[1], P("K"), 1) == 1);
        close_fd(pair[1]);
        SC(94, 0);
        for (;;) {}
    }
    close_fd(pair[1]);
    close_fd(copy);
    short events = poll_wait(pair[0], 1, 2);
    CHECK(events & 1);
    unsigned char buffer[8];
    CHECK(SC(63, pair[0], P(buffer), 1) == 1 && buffer[0] == 'K');
    CHECK(SC(63, pair[0], P(buffer), 1) == 0);
    int status = -1;
    CHECK(SC(260, child, P(&status), 0, 0) == child);
    CHECK(status == 0);
    close_fd(pair[0]);
}

static void datagram(void) {
    int pair[2];
    make_pair(DATAGRAM, 0, pair);
    unsigned char *buffer = scratch;
    CHECK(SC(206, pair[0], P("one"), 3, 0, 0, 0) == 3);
    CHECK(SC(206, pair[0], P("two"), 3, 0, 0, 0) == 3);
    CHECK(SC(207, pair[1], P(buffer), 8, 0, 0, 0) == 3);
    CHECK(same(buffer, (const unsigned char *)"one", 3));
    CHECK(SC(207, pair[1], P(buffer), 8, 0, 0, 0) == 3);
    CHECK(same(buffer, (const unsigned char *)"two", 3));
    // Peeking preserves one complete datagram.
    CHECK(SC(206, pair[0], P("peek"), 4, 0, 0, 0) == 4);
    CHECK(SC(207, pair[1], P(buffer), 2, PEEK, 0, 0) == 2);
    CHECK(same(buffer, (const unsigned char *)"pe", 2));
    CHECK(SC(207, pair[1], P(buffer), 8, 0, 0, 0) == 4);
    CHECK(same(buffer, (const unsigned char *)"peek", 4));
    // A short receive discards the rest unless PEEK is set.
    CHECK(SC(206, pair[0], P("trunc"), 5, 0, 0, 0) == 5);
    CHECK(SC(207, pair[1], P(buffer), 2, TRUNC, 0, 0) == 5);
    CHECK(same(buffer, (const unsigned char *)"tr", 2));
    CHECK(SC(207, pair[1], P(buffer), 1, DONTWAIT, 0, 0) == -11);
    // Empty datagrams remain distinct messages.
    CHECK(SC(206, pair[0], P(buffer), 0, 0, 0, 0) == 0);
    CHECK(SC(207, pair[1], P(buffer), 0, 0, 0, 0) == 0);
    CHECK(SC(207, pair[1], P(buffer), 1, DONTWAIT, 0, 0) == -11);
    // The whole payload is one datagram and oversized sends are rejected.
    for (unsigned i = 0; i < 65507; ++i) payload[i] = i * 7;
    CHECK(SC(206, pair[0], P(payload), 65507, DONTWAIT, 0, 0) == 65507);
    CHECK(SC(206, pair[0], P(buffer), 64, DONTWAIT, 0, 0) == -11);
    CHECK(SC(207, pair[1], P(buffer), 65536, 0, 0, 0) == 65507);
    CHECK(same(buffer, payload, 65507));
    CHECK(SC(206, pair[0], P(payload), 65508, DONTWAIT, 0, 0) == -90);
    // An unnamed peer reports a zero-length source address.
    CHECK(SC(206, pair[0], P("x"), 1, 0, 0, 0) == 1);
    unsigned length = 16;
    CHECK(SC(207, pair[1], P(buffer), 8, 0, P(payload), P(&length)) == 1);
    CHECK(length == 0);
    close_fd(pair[1]);
    CHECK(SC(206, pair[0], P("x"), 1, DONTWAIT | NOSIGNAL, 0, 0) == -32);
    CHECK(SC(207, pair[0], P(buffer), 1, DONTWAIT, 0, 0) == 0);
    close_fd(pair[0]);
}

static void errors(void) {
    int pair[2] = {-1, -1};
    CHECK(SC(SOCKETPAIR, 2, STREAM, 0, P(pair)) == -97);
    CHECK(SC(SOCKETPAIR, 1, 3, 0, P(pair)) == -93);
    CHECK(SC(SOCKETPAIR, 1, STREAM, 6, P(pair)) == -93);
    CHECK(SC(SOCKETPAIR, 1, STREAM | 0x40000000, 0, P(pair)) == -22);
    CHECK(SC(SOCKETPAIR, 1, STREAM, 0, 0) == -14);
    CHECK(SC(SOCKETPAIR, 1, STREAM, 0, P(pair)) == 0);
    CHECK(SC(210, pair[0], 3) == -22);
    CHECK(SC(203, pair[0], 0, 0) == -106);
    CHECK(SC(201, pair[0], 8) == -95);
    CHECK(SC(200, pair[0], 0, 0) == -95);
    unsigned char address[16] = {0};
    unsigned length = sizeof(address);
    CHECK(SC(204, pair[0], P(address), P(&length)) == 0);
    CHECK(length == 2 && address[0] == 1 && address[1] == 0);
    length = sizeof(address);
    CHECK(SC(205, pair[0], P(address), P(&length)) == 0);
    CHECK(length == 2 && address[0] == 1 && address[1] == 0);
    CHECK(SC(206, pair[0], P("x"), 1, 1, 0, 0) == -95);
    CHECK(SC(207, pair[0], P(address), 1, 1, 0, 0) == -95);
    CHECK(SC(206, 1, P("x"), 1, 0, 0, 0) == -88);
    int value = 0;
    length = 4;
    CHECK(SC(209, pair[0], 1, 3, P(&value), P(&length)) == 0 && length == 4 && value == 1);
    length = 4;
    CHECK(SC(209, pair[0], 1, 4, P(&value), P(&length)) == 0 && length == 4 && value == 0);
    CHECK(SC(208, pair[0], 1, 2, P(&value), 4) == -95);
    CHECK(SC(209, pair[0], 1, 99, P(&value), P(&length)) == -92);
    close_fd(pair[0]);
    close_fd(pair[1]);

    CHECK(SC(SOCKETPAIR, 1, STREAM | CLOEXEC, 0, P(pair)) == 0);
    CHECK(SC(25, pair[0], 1) == 1 && SC(25, pair[1], 1) == 1);
    close_fd(pair[0]);
    close_fd(pair[1]);
}

static void stream_vectored(void) {
    int pair[2];
    make_pair(STREAM, 0, pair);
    struct iovec input[] = {{(void *)"ab", 2}, {(void *)"cd", 2}};
    struct message send = {0, 0, 0, input, 2, 0, 0, 0, 0};
    CHECK(SC(211, pair[0], P(&send), 0) == 4);
    unsigned char buffer[4] = {0};
    struct iovec first[] = {{buffer, 1}, {buffer + 1, 3}};
    struct message receive = {0, 0, 0, first, 2, 0, 0, 0, 0};
    CHECK(SC(212, pair[1], P(&receive), 0) == 4);
    CHECK(receive.name_length == 0 && receive.flags == 0);
    CHECK(same(buffer, (const unsigned char *)"abcd", 4));
    CHECK(SC(64, pair[0], P("wxyz"), 4) == 4);
    unsigned char peek[2] = {0};
    struct iovec second[] = {{peek, 2}};
    struct message peek_message = {0, 0, 0, second, 1, 0, 0, 0, 0};
    CHECK(SC(212, pair[1], P(&peek_message), PEEK) == 2);
    CHECK(same(peek, (const unsigned char *)"wx", 2));
    CHECK(SC(212, pair[1], P(&receive), 0) == 4);
    CHECK(same(buffer, (const unsigned char *)"wxyz", 4));
    close_fd(pair[0]);
    close_fd(pair[1]);
}

static void stream_timeout(void) {
    int pair[2];
    make_pair(STREAM, 0, pair);
    struct { long seconds, microseconds; } timeout = {0, 10000};
    CHECK(SC(208, pair[0], 1, 20, P(&timeout), sizeof(timeout)) == 0);
    unsigned char byte = 0;
    CHECK(SC(63, pair[0], P(&byte), 1) == -11);
    CHECK(SC(208, pair[0], 1, 21, P(&timeout), sizeof(timeout)) == 0);
    CHECK(SC(64, pair[0], P(payload), 65536) == 65536);
    CHECK(SC(64, pair[0], P(&byte), 1) == -11);
    close_fd(pair[0]);
    close_fd(pair[1]);
}

static void datagram_vectored(void) {
    int pair[2];
    make_pair(DATAGRAM, 0, pair);
    int kind = 0;
    unsigned length = 4;
    CHECK(SC(209, pair[0], 1, 3, P(&kind), P(&length)) == 0 && kind == 2);
    struct iovec input[] = {{(void *)"ab", 2}, {(void *)"cde", 3}};
    struct message send = {0, 0, 0, input, 2, 0, 0, 0, 0};
    CHECK(SC(211, pair[0], P(&send), 0) == 5);
    unsigned char small[2] = {0};
    struct iovec output[] = {{small, 2}};
    struct message receive = {0, 0, 0, output, 1, 0, 0, 0, 0};
    CHECK(SC(212, pair[1], P(&receive), TRUNC) == 5);
    CHECK(small[0] == 'a' && small[1] == 'b');
    CHECK((receive.flags & TRUNC) != 0);
    CHECK(SC(207, pair[1], P(small), 1, DONTWAIT, 0, 0) == -11);
    close_fd(pair[0]);
    close_fd(pair[1]);
}

void socketpair_main(void) {
    stream_io();
    stream_peek();
    stream_eof();
    stream_shutdown();
    stream_nonblocking();
    stream_poll();
    stream_fork();
    stream_vectored();
    stream_timeout();
    datagram();
    datagram_vectored();
    errors();
}
