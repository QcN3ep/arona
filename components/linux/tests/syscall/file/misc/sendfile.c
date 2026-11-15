// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

#include "misc.h"

static const char source_path[] = "/var/misc-send-src";
static const char target_path[] = "/var/misc-send-dst";
static const char big_path[] = "/var/misc-send-big";
static unsigned char letters[26];
static unsigned char big[8192];
static unsigned char buffer[8192];
static int pipe_fds[2];

struct in_address {
    unsigned short family;
    unsigned char port[2];
    unsigned char address[4];
    unsigned char zero[8];
};

static void fill(void) {
    for (unsigned index = 0; index < sizeof(letters); ++index) {
        letters[index] = (unsigned char)('a' + index);
    }
    for (unsigned index = 0; index < sizeof(big); ++index) {
        big[index] = (unsigned char)('a' + index % 26);
    }
}

static void transfer_to_file(void) {
    long source = misc_open(source_path, O_RDWR | O_CREAT | O_TRUNC, 0600);
    CHECK(source >= 0);
    CHECK(misc_write_all(source, letters, sizeof(letters)) == (long)sizeof(letters));
    long target = misc_open(target_path, O_RDWR | O_CREAT | O_TRUNC, 0600);
    CHECK(target >= 0);

    /* The explicit offset transfers the requested range and advances. */
    long offset = 5;
    CHECK(SC(SYS_SENDFILE, target, source, P(&offset), 10) == 10);
    CHECK(offset == 15);
    CHECK(SC(SYS_LSEEK, target, 0, 0) == 0);
    CHECK(misc_read(target, buffer, 10) == 10);
    for (unsigned index = 0; index < 10; ++index) {
        CHECK(buffer[index] == (unsigned char)('f' + index));
    }

    /* Reading past the end transfers nothing. */
    offset = 1000;
    CHECK(SC(SYS_SENDFILE, target, source, P(&offset), 10) == 0);
    CHECK(offset == 1000);

    /* A zero count is a successful no-op and leaves the offset alone. */
    offset = 7;
    CHECK(SC(SYS_SENDFILE, target, source, P(&offset), 0) == 0);
    CHECK(offset == 7);

    /* Without an offset pointer the source file position advances instead. */
    CHECK(SC(SYS_LSEEK, source, 0, 0) == 0);
    CHECK(SC(SYS_FTRUNCATE, target, 0) == 0);
    CHECK(SC(SYS_LSEEK, target, 0, 0) == 0);
    CHECK(SC(SYS_SENDFILE, target, source, 0, 26) == 26);
    CHECK(SC(SYS_LSEEK, source, 0, 1) == 26);
    CHECK(SC(SYS_LSEEK, target, 0, 0) == 0);
    CHECK(misc_read_all(target, buffer, 26) == 26);
    for (unsigned index = 0; index < 26; ++index) {
        CHECK(buffer[index] == letters[index]);
    }

    /* Boundary values and unsupported descriptor kinds. */
    offset = -1;
    CHECK_ERR(SC(SYS_SENDFILE, target, source, P(&offset), 10), EINVAL);
    CHECK_ERR(SC(SYS_SENDFILE, target, source, 0, -1), EINVAL);
    CHECK_ERR(SC(SYS_SENDFILE, target, 999, 0, 8), EBADF);
    CHECK_ERR(SC(SYS_SENDFILE, 999, source, 0, 8), EBADF);
    long directory = misc_open("/var", O_RDONLY | O_DIRECTORY, 0);
    CHECK(directory >= 0);
    CHECK_ERR(SC(SYS_SENDFILE, target, directory, 0, 8), EINVAL);
    long reader = misc_open(target_path, O_RDONLY, 0);
    CHECK(reader >= 0);
    CHECK_ERR(SC(SYS_SENDFILE, reader, source, 0, 8), EBADF);

    CHECK(misc_close(reader) == 0);
    CHECK(misc_close(directory) == 0);
    CHECK(misc_close(target) == 0);
    CHECK(misc_close(source) == 0);
}

static void check_pipe_bytes(unsigned long length, unsigned long source_offset) {
    CHECK(misc_read(pipe_fds[0], buffer, length) == (long)length);
    for (unsigned long index = 0; index < length; ++index) {
        CHECK(buffer[index] == big[source_offset + index]);
    }
}

static void reader_closes(long source, int explicit_offset);

static void transfer_to_pipe(void) {
    long source = misc_open(big_path, O_RDWR | O_CREAT | O_TRUNC, 0600);
    CHECK(source >= 0);
    CHECK(misc_write_all(source, big, sizeof(big)) == (long)sizeof(big));
    CHECK(SC(SYS_LSEEK, source, 0, 0) == 0);
    CHECK(SC(SYS_PIPE2, P(pipe_fds), 0) == 0);
    CHECK(SC(SYS_FCNTL, pipe_fds[0], F_SETPIPE_SZ, 4096) == 4096);
    CHECK(SC(SYS_FCNTL, pipe_fds[1], F_SETPIPE_SZ, 4096) == 4096);
    CHECK(SC(SYS_FCNTL, pipe_fds[1], F_SETFL, O_NONBLOCK) == 0);

    /* A full chunk is transferred, and a larger request reports a short count. */
    CHECK(SC(SYS_LSEEK, source, 0, 0) == 0);
    long offset = 0;
    CHECK(SC(SYS_SENDFILE, pipe_fds[1], source, P(&offset), (long)sizeof(big)) == 4096);
    CHECK(offset == 4096);
    check_pipe_bytes(4096, 0);
    CHECK(SC(SYS_SENDFILE, pipe_fds[1], source, P(&offset), (long)sizeof(big) - 4096) == 4096);
    CHECK(offset == 8192);
    check_pipe_bytes(4096, 4096);

    /* A full non-blocking pipe reports EAGAIN when nothing was written. */
    CHECK(SC(SYS_SENDFILE, pipe_fds[1], source, 0, 4096) == 4096);
    CHECK_ERR(SC(SYS_SENDFILE, pipe_fds[1], source, 0, 4096), EAGAIN);
    check_pipe_bytes(4096, 0);

    /* The read end of a pipe is not a destination. */
    CHECK_ERR(SC(SYS_SENDFILE, pipe_fds[0], source, 0, 16), EBADF);

    CHECK(misc_close(pipe_fds[0]) == 0);
    CHECK(misc_close(pipe_fds[1]) == 0);
    reader_closes(source, 0);
    reader_closes(source, 1);
    CHECK(misc_close(source) == 0);
}

static void reader_closes(long source, int explicit_offset) {
    CHECK(SC(SYS_PIPE2, P(pipe_fds), 0) == 0);
    CHECK(SC(SYS_FCNTL, pipe_fds[1], F_SETPIPE_SZ, 4096) == 4096);
    CHECK(SC(SYS_LSEEK, source, explicit_offset ? 123 : 0, 0) >= 0);
    long child = SC(SYS_CLONE, SIGCHLD, 0);
    CHECK(child >= 0);
    if (child == 0) {
        CHECK(misc_close(pipe_fds[1]) == 0);
        CHECK(misc_read(pipe_fds[0], buffer, 1) == 1 && buffer[0] == big[0]);
        CHECK(misc_close(pipe_fds[0]) == 0);
        SC(SYS_EXIT_GROUP, 0);
        for (;;) {}
    }
    CHECK(misc_close(pipe_fds[0]) == 0);
    long offset = 0;
    long pointer = explicit_offset ? P(&offset) : 0;
    long count = SC(SYS_SENDFILE, pipe_fds[1], source, pointer, sizeof(big));
    CHECK(count >= 4096 && count <= 4097);
    CHECK(SC(SYS_LSEEK, source, 0, 1) == (explicit_offset ? 123 : count));
    CHECK(!explicit_offset || offset == count);
    CHECK_ERR(SC(SYS_SENDFILE, pipe_fds[1], source, pointer, 1), EPIPE);
    CHECK(SC(SYS_LSEEK, source, 0, 1) == (explicit_offset ? 123 : count));
    CHECK(!explicit_offset || offset == count);
    int status = -1;
    CHECK(SC(SYS_WAIT4, child, P(&status), 0, 0) == child && status == 0);
    CHECK(misc_close(pipe_fds[1]) == 0);
}

static long prepare_listener(struct in_address *address) {
    long listener = SC(SYS_SOCKET, 2, 1, 0);
    CHECK(listener >= 0);
    struct in_address local = {2, {0, 0}, {127, 0, 0, 1}, {0, 0, 0, 0, 0, 0, 0, 0}};
    CHECK(SC(SYS_BIND, listener, P(&local), sizeof(local)) == 0);
    CHECK(SC(SYS_LISTEN, listener, 1) == 0);
    unsigned length = sizeof(local);
    CHECK(SC(SYS_GETSOCKNAME, listener, P(&local), P(&length)) == 0);
    *address = local;
    return listener;
}

static void transfer_to_socket(void) {
    long source = misc_open(big_path, O_RDWR | O_CREAT | O_TRUNC, 0600);
    CHECK(source >= 0);
    CHECK(misc_write_all(source, letters, sizeof(letters)) == (long)sizeof(letters));
    struct in_address address;
    long listener = prepare_listener(&address);
    long child = SC(SYS_CLONE, SIGCHLD, 0);
    CHECK(child >= 0);
    if (child == 0) {
        long client = SC(SYS_SOCKET, 2, 1, 0);
        if (client < 0) misc_fail(__LINE__);
        if (SC(SYS_CONNECT, client, P(&address), sizeof(address)) != 0) misc_fail(__LINE__);
        unsigned long total = 0;
        while (total < sizeof(letters)) {
            long count = misc_read(client, buffer + total, sizeof(letters) - total);
            if (count <= 0) misc_fail(__LINE__);
            total += (unsigned long)count;
        }
        for (unsigned index = 0; index < sizeof(letters); ++index) {
            if (buffer[index] != letters[index]) misc_fail(__LINE__);
        }
        if (misc_close(client) != 0) misc_fail(__LINE__);
        SC(SYS_EXIT_GROUP, 0);
        for (;;) {}
    }
    long accepted = SC(SYS_ACCEPT, listener, 0, 0);
    CHECK(accepted >= 0);
    CHECK(SC(SYS_LSEEK, source, 0, 0) == 0);
    CHECK(SC(SYS_SENDFILE, accepted, source, 0, (long)sizeof(letters)) == (long)sizeof(letters));
    CHECK(misc_close(accepted) == 0);
    CHECK_ERR(SC(SYS_SENDFILE, accepted, source, 0, 8), EBADF);
    CHECK(misc_close(listener) == 0);
    int status = -1;
    CHECK(SC(SYS_WAIT4, child, P(&status), 0, 0) == child);
    CHECK(status == 0);
    CHECK(misc_close(source) == 0);
}

int sendfile_main(void) {
    fill();
    SC(SYS_UNLINKAT, AT_FDCWD, P(source_path), 0);
    SC(SYS_UNLINKAT, AT_FDCWD, P(target_path), 0);
    SC(SYS_UNLINKAT, AT_FDCWD, P(big_path), 0);
    transfer_to_file();
    transfer_to_pipe();
    transfer_to_socket();
    return 42;
}
