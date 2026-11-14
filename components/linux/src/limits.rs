// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

//! Process resource limits and their defaults.

/// Every resource accepts this value as "no limit".
pub(crate) const INFINITY: u64 = u64::MAX;

/// The number of resources addressed by `prlimit64`.
pub(crate) const COUNT: usize = 10;

pub(crate) const CPU: usize = 0;
pub(crate) const FSIZE: usize = 1;
pub(crate) const STACK: usize = 3;
pub(crate) const NOFILE: usize = 7;
pub(crate) const AS: usize = 9;

/// The current and maximum value of one resource.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Rlimit {
    pub current: u64,
    pub maximum: u64,
}

impl Rlimit {
    pub fn unbounded() -> Self {
        Self {
            current: INFINITY,
            maximum: INFINITY,
        }
    }
}

/// The resource limits of one process.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Limits {
    values: [Rlimit; COUNT],
}

// REFERENCE: https://man7.org/linux/man-pages/man2/getrlimit.2.html
impl Default for Limits {
    fn default() -> Self {
        let mut values = [Rlimit::unbounded(); COUNT];
        values[STACK] = Rlimit {
            current: 8 * 1024 * 1024,
            maximum: INFINITY,
        };
        values[NOFILE] = Rlimit {
            current: 1024,
            maximum: 4096,
        };
        Self { values }
    }
}

impl Limits {
    pub fn get(&self, resource: usize) -> Option<Rlimit> {
        self.values.get(resource).copied()
    }

    pub fn set(&mut self, resource: usize, value: Rlimit) -> bool {
        match self.values.get_mut(resource) {
            Some(slot) => {
                *slot = value;
                true
            }
            None => false,
        }
    }

    /// Returns the byte limit of a resource, or `None` when it is unbounded.
    fn bytes(&self, resource: usize) -> Option<usize> {
        usize::try_from(self.values[resource].current).ok()
    }

    /// The largest file size writable by the process.
    pub fn file_size(&self) -> Option<usize> {
        self.bytes(FSIZE)
    }

    /// The largest address space the process may map.
    pub fn address_space(&self) -> Option<usize> {
        self.bytes(AS)
    }

    /// The largest file descriptor the process may allocate.
    pub fn file_descriptors(&self) -> usize {
        usize::try_from(self.values[NOFILE].current)
            .unwrap_or(usize::MAX)
            .min(i32::MAX as usize)
    }

    /// The initial stack allowance of a newly executed image.
    pub fn stack(&self) -> usize {
        self.bytes(STACK)
            .unwrap_or(crate::stack::STACK_SIZE)
            .min(crate::stack::STACK_SIZE)
    }

    pub fn cpu(&self) -> Rlimit {
        self.values[CPU]
    }
}
