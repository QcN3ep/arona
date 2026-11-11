// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

use crate::elf::Machine;
use runtime::trap::UserState;

pub(super) const SIGALTSTACK: usize = 132;
pub(super) const RT_SIGSUSPEND: usize = 133;
pub(super) const RT_SIGACTION: usize = 134;
pub(super) const RT_SIGPROCMASK: usize = 135;
pub(super) const RT_SIGPENDING: usize = 136;
pub(super) const RT_SIGTIMEDWAIT: usize = 137;
pub(super) const RT_SIGRETURN: usize = 139;
pub(super) const KILL: usize = 129;
pub(super) const TKILL: usize = 130;
pub(super) const TGKILL: usize = 131;
pub(super) const EINTR: Errno = Errno(4);
pub(crate) const ESRCH: Errno = Errno(3);
pub(crate) const EPERM: Errno = Errno(1);

// REFERENCE: https://github.com/torvalds/linux/blob/master/include/uapi/asm-generic/unistd.h
pub(super) const UNAME: usize = 160;
pub(super) const CLOCK_GETTIME: usize = 113;
pub(super) const CLOCK_GETRES: usize = 114;
pub(super) const GETTIMEOFDAY: usize = 169;
pub(super) const NANOSLEEP: usize = 101;
pub(super) const CLOCK_NANOSLEEP: usize = 115;
pub(super) const GETRANDOM: usize = 278;
pub(super) const IOCTL: usize = 29;
pub(crate) const ENOTTY: Errno = Errno(25);
pub(super) const PIPE2: usize = 59;
pub(super) const PPOLL: usize = 73;
pub(super) const PSELECT6: usize = 72;
pub(super) const EAGAIN: Errno = Errno(11);
pub(super) const EPIPE: Errno = Errno(32);
pub(super) const EXECVE: usize = 221;
pub(super) const ENOEXEC: Errno = Errno(8);
pub(super) const WAITID: usize = 95;
pub(super) const WAIT4: usize = 260;
pub(super) const ECHILD: Errno = Errno(10);
pub(super) const FUTEX: usize = 98;
pub(super) const ETIMEDOUT: Errno = Errno(110);
pub(super) const GETPID: usize = 172;
pub(super) const GETPPID: usize = 173;
pub(super) const GETUID: usize = 174;
pub(super) const SETGROUPS: usize = 159;
pub(super) const SETRESGID: usize = 149;
pub(super) const SETRESUID: usize = 147;
pub(super) const SETUID: usize = 146;
pub(super) const SETREUID: usize = 145;
pub(super) const SETREGID: usize = 143;
pub(super) const SETGID: usize = 144;
pub(super) const GETEUID: usize = 175;
pub(super) const GETGID: usize = 176;
pub(super) const GETEGID: usize = 177;
pub(super) const GETRESUID: usize = 148;
pub(super) const GETRESGID: usize = 150;
pub(super) const GETGROUPS: usize = 158;
pub(super) const SETPGID: usize = 154;
pub(super) const GETPGID: usize = 155;
pub(super) const GETSID: usize = 156;
pub(super) const SETSID: usize = 157;
pub(super) const GETTID: usize = 178;
pub(super) const SET_TID_ADDRESS: usize = 96;
pub(super) const CLONE: usize = 220;
pub(super) const SCHED_YIELD: usize = 124;
pub(super) const GETCWD: usize = 17;
pub(super) const MKDIRAT: usize = 34;
pub(super) const UNLINKAT: usize = 35;
pub(super) const SYMLINKAT: usize = 36;
pub(super) const LINKAT: usize = 37;
pub(super) const RENAMEAT: usize = 38;
pub(super) const CHDIR: usize = 49;
pub(super) const FCHDIR: usize = 50;
pub(super) const READLINKAT: usize = 78;
pub(super) const RENAMEAT2: usize = 276;
pub(super) const DUP: usize = 23;
pub(super) const DUP3: usize = 24;
pub(super) const FCNTL: usize = 25;
pub(super) const FTRUNCATE: usize = 46;
pub(super) const OPENAT: usize = 56;
pub(super) const CLOSE: usize = 57;
pub(super) const GETDENTS64: usize = 61;
pub(super) const LSEEK: usize = 62;
pub(super) const READ: usize = 63;
pub(super) const WRITE: usize = 64;
pub(super) const READV: usize = 65;
pub(super) const WRITEV: usize = 66;
pub(super) const PREAD64: usize = 67;
pub(super) const PWRITE64: usize = 68;
// REFERENCE: https://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git/tree/fs/read_write.c?h=v6.12
pub(super) const PREADV: usize = 69;
pub(super) const PWRITEV: usize = 70;
pub(super) const FSYNC: usize = 82;
pub(super) const FDATASYNC: usize = 83;
pub(super) const NEWFSTATAT: usize = 79;
pub(super) const FSTAT: usize = 80;
pub(super) const STATFS: usize = 43;
pub(super) const FSTATFS: usize = 44;
pub(super) const STATX: usize = 291;
pub(super) const EVENTFD2: usize = 19;
pub(super) const UMASK: usize = 166;
pub(super) const EXIT: usize = 93;
pub(super) const EXIT_GROUP: usize = 94;
pub(super) const BRK: usize = 214;
pub(super) const MUNMAP: usize = 215;
pub(super) const MMAP: usize = 222;
pub(super) const MADVISE: usize = 233;
pub(super) const ENODEV: Errno = Errno(19);
pub(super) const MPROTECT: usize = 226;
pub(super) const MSYNC: usize = 227;

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) struct Errno(isize);

pub(super) const ENOENT: Errno = Errno(2);
pub(crate) const EIO: Errno = Errno(5);
pub(crate) const ENXIO: Errno = Errno(6);
pub(super) const EBADF: Errno = Errno(9);
pub(crate) const ENOMEM: Errno = Errno(12);
pub(crate) const EACCES: Errno = Errno(13);
pub(crate) const EFAULT: Errno = Errno(14);
pub(super) const EEXIST: Errno = Errno(17);
pub(super) const ENOTDIR: Errno = Errno(20);
pub(super) const EBUSY: Errno = Errno(16);
pub(super) const EXDEV: Errno = Errno(18);
pub(super) const ENOTEMPTY: Errno = Errno(39);
pub(super) const ELOOP: Errno = Errno(40);
pub(super) const ERANGE: Errno = Errno(34);
pub(super) const EISDIR: Errno = Errno(21);
pub(crate) const EINVAL: Errno = Errno(22);
pub(super) const EMFILE: Errno = Errno(24);
pub(super) const EFBIG: Errno = Errno(27);
pub(super) const ENOSPC: Errno = Errno(28);
pub(super) const ESPIPE: Errno = Errno(29);
pub(super) const EROFS: Errno = Errno(30);
pub(super) const ENAMETOOLONG: Errno = Errno(36);
pub(super) const ENOSYS: Errno = Errno(38);
pub(super) const EOVERFLOW: Errno = Errno(75);
pub(super) const EOPNOTSUPP: Errno = Errno(95);

pub(super) type SyscallResult = Result<usize, Errno>;

impl Machine {
    pub(crate) fn initial(self, entry: usize, stack: usize) -> UserState {
        let mut state = UserState {
            pc: entry,
            ..UserState::default()
        };
        state.registers[match self {
            Self::Riscv64 => 2,
            Self::LoongArch64 => 3,
        }] = stack;
        state
    }

    pub(super) fn thread_state(self, state: &mut UserState, stack: usize, tls: Option<usize>) {
        state.registers[match self {
            Self::Riscv64 => 2,
            Self::LoongArch64 => 3,
        }] = stack;
        if let Some(tls) = tls {
            state.registers[match self {
                Self::Riscv64 => 4,
                Self::LoongArch64 => 2,
            }] = tls;
        }
    }

    pub(super) fn syscall(self, state: &UserState) -> (usize, [usize; 6]) {
        let (number, first) = match self {
            Self::Riscv64 => (17, 10),
            Self::LoongArch64 => (11, 4),
        };
        (
            state.registers[number],
            state.registers[first..first + 6].try_into().unwrap(),
        )
    }

    pub(super) fn complete(self, state: &mut UserState, successor: usize, result: SyscallResult) {
        state.pc = successor;
        state.registers[match self {
            Self::Riscv64 => 10,
            Self::LoongArch64 => 4,
        }] = result.unwrap_or_else(|errno| -errno.0 as usize);
    }
}

pub(super) const TIMERFD_CREATE: usize = 85;
pub(super) const TIMERFD_SETTIME: usize = 86;
pub(super) const TIMERFD_GETTIME: usize = 87;

pub(super) const SIGNALFD4: usize = 74;
