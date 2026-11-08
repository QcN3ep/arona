// SPDX-License-Identifier: MPL-2.0

use runtime::trap::UserState;

use crate::elf::Machine;

// REFERENCE: https://github.com/torvalds/linux/blob/master/include/uapi/asm-generic/unistd.h
pub(super) const WRITE: usize = 64;
pub(super) const EXIT: usize = 93;
pub(super) const EXIT_GROUP: usize = 94;
pub(super) const BRK: usize = 214;
pub(super) const MUNMAP: usize = 215;
pub(super) const MMAP: usize = 222;
pub(super) const MPROTECT: usize = 226;

pub(super) struct Errno(isize);

pub(super) const EIO: Errno = Errno(5);
pub(super) const EBADF: Errno = Errno(9);
pub(super) const ENOMEM: Errno = Errno(12);
pub(super) const EACCES: Errno = Errno(13);
pub(super) const EFAULT: Errno = Errno(14);
pub(super) const EEXIST: Errno = Errno(17);
pub(super) const EINVAL: Errno = Errno(22);
pub(super) const ENOSYS: Errno = Errno(38);
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
