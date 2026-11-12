// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

#include "../mount/fs.h"

static char pty_buffer[4096];

void pty_limits(void) {
    long master = SC(56, -100, P("/dev/ptmx"), 0x802, 0);
    CHECK(master >= 0);
    int unlocked = 0;
    CHECK(SC(29, master, 0x40045431, P(&unlocked)) == 0);
    long slave = SC(29, master, 0x5441, 0x802);
    CHECK(slave >= 0);
    unsigned char settings[36];
    CHECK(SC(29, slave, 0x5401, P(settings)) == 0);
    settings[12] &= ~8; // Disable echo so input capacity is independent of output.
    CHECK(SC(29, slave, 0x5402, P(settings)) == 0);
    for (unsigned index = 0; index < sizeof(pty_buffer); ++index) pty_buffer[index] = 'x';
    long written = 0;
    while (written < 4096) {
        long count = SC(64, master, P(pty_buffer), 4096 - written);
        CHECK(count > 0);
        written += count;
    }
    char erase = 127, newline = '\n';
    CHECK(SC(64, master, P(&erase), 1) == 1);
    CHECK(SC(64, master, P(&newline), 1) == 1);
    CHECK(SC(63, slave, P(pty_buffer), 4096) == 4095);
    CHECK(pty_buffer[4094] == '\n');

    // Empty EOF records also consume input capacity.
    for (unsigned index = 0; index < sizeof(pty_buffer); ++index) pty_buffer[index] = 4;
    written = 0;
    while (written < 4096) {
        long count = SC(64, master, P(pty_buffer), 4096 - written);
        CHECK(count > 0);
        written += count;
    }
    CHECK(SC(64, master, P(pty_buffer), 1) == -11);
    CHECK(SC(63, slave, P(pty_buffer), 1) == 0);
    CHECK(SC(64, master, P(pty_buffer), 1) == 1);
    CHECK(SC(29, slave, 0x540b, 0) == 0);

    // A child drains slave output while its parent changes attributes.
    CHECK(SC(64, slave, P("kept"), 4) == 4);
    long child = SC(220, 17, 0, 0, 0, 0);
    CHECK(child >= 0);
    if (child == 0) {
        CHECK(SC(63, master, P(pty_buffer), 4) == 4);
        CHECK(pty_buffer[0] == 'k' && pty_buffer[3] == 't');
        SC(94, 0);
        for (;;) {}
    }
    CHECK(SC(29, slave, 0x5403, P(settings)) == 0);
    long status = 0;
    CHECK(SC(260, child, P(&status), 0, 0) == child && status == 0);
    CHECK(SC(64, slave, P("also"), 4) == 4);
    child = SC(220, 17, 0, 0, 0, 0);
    CHECK(child >= 0);
    if (child == 0) {
        CHECK(SC(63, master, P(pty_buffer), 4) == 4);
        CHECK(pty_buffer[0] == 'a' && pty_buffer[3] == 'o');
        SC(94, 0);
        for (;;) {}
    }
    CHECK(SC(29, slave, 0x5404, P(settings)) == 0);
    CHECK(SC(260, child, P(&status), 0, 0) == child && status == 0);
    CHECK(SC(57, slave) == 0);
    CHECK(SC(57, master) == 0);
}
