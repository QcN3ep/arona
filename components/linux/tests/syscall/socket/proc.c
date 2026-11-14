// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

#include "socket.h"

struct address6 {
    unsigned short family;
    unsigned char port[2];
    unsigned flow;
    unsigned char ip[16];
    unsigned scope;
};

union stat_buffer {
    unsigned char bytes[144];
    unsigned long alignment;
};

static char content[16384];
static int length;

static long open_file(const char *path, int flags) {
    return SC(56, -100, P(path), flags, 0);
}

static long read_file(const char *path) {
    long fd = open_file(path, 0);
    CHECK(fd >= 0);
    long total = 0;
    for (;;) {
        long count = SC(63, fd, P(content + total), sizeof(content) - 1 - total);
        CHECK(count >= 0);
        if (!count) { break; }
        total += count;
        CHECK(total < (long)sizeof(content) - 1);
    }
    CHECK(SC(57, fd) == 0);
    content[total] = 0;
    length = (int)total;
    return total;
}

static int contains(const char *needle) {
    int size = 0;
    while (needle[size]) { size++; }
    for (int offset = 0; offset + size <= length; offset++) {
        int index = 0;
        while (index < size && content[offset + index] == needle[index]) { index++; }
        if (index == size) { return 1; }
    }
    return 0;
}

static void port_text(unsigned port, char output[5]) {
    static const char digits[] = "0123456789ABCDEF";
    for (int index = 0; index < 4; index++) {
        output[3 - index] = digits[(port >> (4 * index)) & 15];
    }
    output[4] = 0;
}

static void pause_ns(long nanoseconds) {
    struct timespec duration = {0, nanoseconds};
    CHECK(SC(101, P(&duration), 0) == 0);
}

static void close_fd(long fd) { CHECK(SC(57, fd) == 0); }

static long open_socket(int family, int kind) {
    long fd = SC(198, family, kind, 0);
    CHECK(fd >= 0);
    return fd;
}

static unsigned bind_any(long fd, int family) {
    unsigned port = 0;
    if (family == 10) {
        struct address6 address = {10, {0}, 0, {0}, 0};
        address.ip[15] = 1;
        CHECK(SC(200, fd, P(&address), sizeof(address)) == 0);
        unsigned size = sizeof(address);
        CHECK(SC(204, fd, P(&address), P(&size)) == 0 && size == sizeof(address));
        CHECK(address.family == 10 && address.ip[15] == 1);
        port = (unsigned)(address.port[0] << 8 | address.port[1]);
    } else {
        struct address address = {2, {0, 0}, {127, 0, 0, 1}, {0}};
        CHECK(SC(200, fd, P(&address), sizeof(address)) == 0);
        unsigned size = sizeof(address);
        CHECK(SC(204, fd, P(&address), P(&size)) == 0 && size == sizeof(address));
        CHECK(address.family == 2 && address.ip[0] == 127);
        port = (unsigned)(address.port[0] << 8 | address.port[1]);
    }
    CHECK(port != 0);
    return port;
}

static void expect_row(const char *address, unsigned port, const char *state) {
    char row[96];
    int index = 0;
    while (address[index]) { row[index] = address[index]; index++; }
    int width = index;
    row[index++] = ':';
    char text[5];
    port_text(port, text);
    for (int digit = 0; digit < 4; digit++) { row[index++] = text[digit]; }
    const char *remote = width == 32
        ? " 00000000000000000000000000000000:0000"
        : " 00000000:0000";
    for (int digit = 0; remote[digit]; digit++) { row[index++] = remote[digit]; }
    row[index++] = ' ';
    const char *status = state;
    while (*status) { row[index++] = *status++; }
    row[index] = 0;
    CHECK(contains(row));
}

static void formats(void) {
    read_file("/proc/net/dev");
    CHECK(contains("Inter-|"));
    CHECK(contains("lo:"));
    read_file("/proc/net/route");
    CHECK(contains("Iface\tDestination"));
    CHECK(contains("lo\t0000007F\t00000000\t0001"));
    read_file("/proc/net/unix");
    CHECK(contains("Num       RefCount Protocol Flags"));
}

static void descriptors(void) {
    long fd = open_file("/proc/net/tcp", 0);
    CHECK(fd >= 0);
    union stat_buffer stat;
    CHECK(SC(80, fd, P(&stat)) == 0);
    CHECK(*(unsigned *)(stat.bytes + 16) == 0100444);
    CHECK(*(unsigned short *)(stat.bytes + 20) == 1);
    long size = (long)*(unsigned long *)(stat.bytes + 48);
    CHECK(size > 0);
    CHECK(SC(62, fd, 0, 2) == size);
    CHECK(SC(62, fd, 0, 0) == 0);
    char header[5] = {0};
    CHECK(SC(63, fd, P(header), 4) == 4);
    CHECK(header[0] == ' ' && header[1] == ' ' && header[2] == 's' && header[3] == 'l');
    char byte = 0;
    CHECK(SC(64, fd, P(&byte), 1) == -30);
    close_fd(fd);
}

static void sockets(void) {
    long tcp = open_socket(2, 1);
    unsigned tcp_port = bind_any(tcp, 2);
    CHECK(SC(201, tcp, 8) == 0);
    long tcp6 = open_socket(10, 1);
    unsigned tcp6_port = bind_any(tcp6, 10);
    CHECK(SC(201, tcp6, 8) == 0);
    long udp = open_socket(2, 2);
    unsigned udp_port = bind_any(udp, 2);
    long udp6 = open_socket(10, 2);
    unsigned udp6_port = bind_any(udp6, 10);

    read_file("/proc/net/tcp");
    CHECK(contains("local_address rem_address"));
    expect_row("0100007F", tcp_port, "0A");
    read_file("/proc/net/udp");
    CHECK(contains("local_address rem_address"));
    expect_row("0100007F", udp_port, "07");
    read_file("/proc/net/tcp6");
    expect_row("00000000000000000000000001000000", tcp6_port, "0A");
    read_file("/proc/net/udp6");
    expect_row("00000000000000000000000001000000", udp6_port, "07");

    close_fd(tcp);
    char row[96];
    int index = 0;
    row[index++] = '0';
    row[index++] = '1';
    row[index++] = '0';
    row[index++] = '0';
    row[index++] = '0';
    row[index++] = '0';
    row[index++] = '7';
    row[index++] = 'F';
    row[index++] = ':';
    char text[5];
    port_text(tcp_port, text);
    for (int digit = 0; digit < 4; digit++) { row[index++] = text[digit]; }
    row[index] = 0;
    for (int attempt = 0; attempt < 200; attempt++) {
        read_file("/proc/net/tcp");
        if (!contains(row)) { break; }
        pause_ns(1000000);
    }
    CHECK(!contains(row));

    close_fd(tcp6);
    close_fd(udp);
    close_fd(udp6);
}

static void errors(void) {
    CHECK(open_file("/proc/net/absent", 0) == -2);
    CHECK(open_file("/proc/net", 0) == -20);
    CHECK(open_file("/proc/net/", 0) == -20);
    CHECK(open_file("/proc/net/tcp", 1) == -30);
    CHECK(open_file("/proc/net/tcp", 0x10000) == -20);
}

void socket_proc(void) {
    formats();
    descriptors();
    sockets();
    errors();
}
