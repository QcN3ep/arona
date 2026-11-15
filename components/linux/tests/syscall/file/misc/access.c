// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

#include "misc.h"

static const char owner_path[] = "/var/misc-owner";
static const char group_path[] = "/var/misc-group";
static const char other_path[] = "/var/misc-other";
static const char exec_path[] = "/var/misc-exec";
static const char none_path[] = "/var/misc-none";
static const char root_path[] = "/var/misc-root";
static const char link_path[] = "/var/misc-private";
static const char absent_path[] = "/var/misc-absent";

int access_main(void) {
    // The superuser bypasses read and write checks but still needs an execute bit.
    CHECK(SC(SYS_FACCESSAT, AT_FDCWD, P(owner_path), R_OK) == 0);
    CHECK(SC(SYS_FACCESSAT, AT_FDCWD, P(owner_path), W_OK) == 0);
    CHECK(SC(SYS_FACCESSAT, AT_FDCWD, P(owner_path), F_OK) == 0);
    CHECK_ERR(SC(SYS_FACCESSAT, AT_FDCWD, P(owner_path), X_OK), EACCES);
    CHECK(SC(SYS_FACCESSAT, AT_FDCWD, P(exec_path), X_OK) == 0);
    CHECK(SC(SYS_FACCESSAT, AT_FDCWD, P(none_path), R_OK) == 0);
    CHECK(SC(SYS_FACCESSAT2, AT_FDCWD, P("/etc/linux-data"), R_OK, 0) == 0);
    CHECK_ERR(SC(SYS_FACCESSAT2, AT_FDCWD, P("/etc/linux-data"), X_OK, 0), EACCES);
    CHECK(SC(SYS_FACCESSAT, AT_FDCWD, P("/var"), X_OK) == 0);

    // Path resolution and argument validation.
    CHECK_ERR(SC(SYS_FACCESSAT, AT_FDCWD, P(absent_path), F_OK), ENOENT);
    CHECK_ERR(SC(SYS_FACCESSAT, AT_FDCWD, P("/var/misc-owner/inner"), F_OK), ENOTDIR);
    CHECK_ERR(SC(SYS_FACCESSAT, AT_FDCWD, P(owner_path), 8), EINVAL);
    CHECK_ERR(SC(SYS_FACCESSAT, AT_FDCWD, 0, F_OK), EFAULT);
    CHECK_ERR(SC(SYS_FACCESSAT2, AT_FDCWD, P(owner_path), R_OK, 0x8000), EINVAL);
    CHECK_ERR(SC(SYS_FACCESSAT2, AT_FDCWD, P(""), R_OK, AT_EMPTY_PATH), EBADF);
    // A non-empty path ignores AT_EMPTY_PATH, like Linux.
    CHECK(SC(SYS_FACCESSAT2, AT_FDCWD, P(owner_path), R_OK, AT_EMPTY_PATH) == 0);

    // Descriptor checks through AT_EMPTY_PATH.
    long descriptor = misc_open(root_path, O_RDONLY, 0);
    CHECK(descriptor >= 0);
    CHECK(SC(SYS_FACCESSAT2, descriptor, P(""), R_OK | W_OK, AT_EMPTY_PATH) == 0);
    CHECK(SC(SYS_FACCESSAT2, descriptor, P(""), F_OK, AT_EMPTY_PATH) == 0);
    CHECK_ERR(SC(SYS_FACCESSAT2, descriptor, P(""), R_OK, 0), ENOENT);

    // Check the effective identity while the real identity stays privileged.
    CHECK(SC(SYS_SETRESGID, 0, 456, 0) == 0);
    CHECK(SC(SYS_SETRESUID, 0, 123, 0) == 0);
    CHECK(SC(SYS_GETUID) == 0);
    CHECK(SC(SYS_GETEUID) == 123);
    CHECK(SC(SYS_GETEGID) == 456);

    // Checks without AT_EACCESS consult the real identity, which is still root.
    CHECK(SC(SYS_FACCESSAT2, AT_FDCWD, P(root_path), R_OK | W_OK, 0) == 0);
    CHECK(SC(SYS_FACCESSAT2, AT_FDCWD, P(none_path), R_OK, 0) == 0);
    CHECK_ERR(SC(SYS_FACCESSAT2, AT_FDCWD, P(group_path), X_OK, 0), EACCES);

    // faccessat ignores its flag argument, unlike faccessat2.
    CHECK(SC(SYS_FACCESSAT, AT_FDCWD, P(root_path), R_OK, AT_EACCESS) == 0);

    // AT_EACCESS consults the effective identity instead.
    CHECK(SC(SYS_FACCESSAT2, AT_FDCWD, P(owner_path), R_OK | W_OK, AT_EACCESS) == 0);
    CHECK_ERR(SC(SYS_FACCESSAT2, AT_FDCWD, P(owner_path), X_OK, AT_EACCESS), EACCES);
    CHECK(SC(SYS_FACCESSAT2, AT_FDCWD, P(group_path), R_OK, AT_EACCESS) == 0);
    CHECK_ERR(SC(SYS_FACCESSAT2, AT_FDCWD, P(group_path), W_OK, AT_EACCESS), EACCES);
    CHECK(SC(SYS_FACCESSAT2, AT_FDCWD, P(other_path), R_OK, AT_EACCESS) == 0);
    CHECK_ERR(SC(SYS_FACCESSAT2, AT_FDCWD, P(other_path), W_OK, AT_EACCESS), EACCES);
    CHECK_ERR(SC(SYS_FACCESSAT2, AT_FDCWD, P(none_path), R_OK, AT_EACCESS), EACCES);
    CHECK(SC(SYS_FACCESSAT2, AT_FDCWD, P(none_path), F_OK, AT_EACCESS) == 0);
    CHECK(SC(SYS_FACCESSAT2, AT_FDCWD, P(exec_path), X_OK, AT_EACCESS) == 0);
    CHECK(SC(SYS_FACCESSAT2, AT_FDCWD, P("/var"), X_OK, AT_EACCESS) == 0);
    CHECK_ERR(SC(SYS_FACCESSAT2, AT_FDCWD, P(root_path), R_OK, AT_EACCESS), EACCES);

    // AT_SYMLINK_NOFOLLOW checks the link itself.
    CHECK_ERR(
        SC(SYS_FACCESSAT2, AT_FDCWD, P(link_path), R_OK, AT_EACCESS), EACCES);
    CHECK(SC(SYS_FACCESSAT2, AT_FDCWD, P(link_path), R_OK,
             AT_EACCESS | AT_SYMLINK_NOFOLLOW) == 0);
    CHECK_ERR(
        SC(SYS_FACCESSAT2, descriptor, P(""), R_OK, AT_EACCESS | AT_EMPTY_PATH), EACCES);
    CHECK_ERR(
        SC(SYS_FACCESSAT2, descriptor, P(""), W_OK, AT_EACCESS | AT_EMPTY_PATH), EACCES);
    CHECK_ERR(
        SC(SYS_FACCESSAT2, descriptor, P(""), X_OK, AT_EACCESS | AT_EMPTY_PATH), EACCES);
    CHECK(SC(SYS_FACCESSAT2, descriptor, P(""), F_OK, AT_EACCESS | AT_EMPTY_PATH) == 0);

    CHECK(SC(SYS_SETRESUID, 0, 0, 0) == 0);
    CHECK(SC(SYS_SETRESGID, 0, 0, 0) == 0);
    CHECK(SC(SYS_GETEUID) == 0);
    CHECK(misc_close(descriptor) == 0);
    return 42;
}
