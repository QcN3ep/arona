// SPDX-License-Identifier: MPL-2.0

use runtime::trap::UserState;

use crate::elf::Machine;

// REFERENCE: https://github.com/torvalds/linux/blob/master/include/uapi/asm-generic/unistd.h
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
pub(super) const UMASK: usize = 166;
pub(super) const EXIT: usize = 93;
pub(super) const EXIT_GROUP: usize = 94;
pub(super) const BRK: usize = 214;
pub(super) const MUNMAP: usize = 215;
pub(super) const MMAP: usize = 222;
pub(super) const MPROTECT: usize = 226;

pub(super) struct Errno(isize);

pub(super) const ENOENT: Errno = Errno(2);
pub(super) const EIO: Errno = Errno(5);
pub(super) const EBADF: Errno = Errno(9);
pub(super) const ENOMEM: Errno = Errno(12);
pub(super) const EACCES: Errno = Errno(13);
pub(super) const EFAULT: Errno = Errno(14);
pub(super) const EEXIST: Errno = Errno(17);
pub(super) const ENOTDIR: Errno = Errno(20);
pub(super) const EISDIR: Errno = Errno(21);
pub(super) const EINVAL: Errno = Errno(22);
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
