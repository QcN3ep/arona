// SPDX-License-Identifier: MPL-2.0

//! 文件描述符与共享的打开状态。

use alloc::{collections::BTreeMap, rc::Rc};
use core::cell::Cell;

pub(crate) const O_APPEND: u32 = 0x400;
pub(crate) const O_NONBLOCK: u32 = 0x800;
pub(crate) const O_DSYNC: u32 = 0x1000;

pub(crate) struct FileTable<'a> {
    entries: BTreeMap<usize, Descriptor<'a>>,
}

pub(crate) struct Descriptor<'a> {
    pub description: Rc<OpenFileDescription<'a>>,
    pub close_on_exec: bool,
}

pub(crate) enum OpenFileDescription<'a> {
    Output {
        session: devmgr::uart::Session,
        flags: Cell<u32>,
    },
    Directory {
        session: fs::Directory<'a>,
        flags: Cell<u32>,
    },
    Regular {
        file: fs::File<'a>,
        flags: Cell<u32>,
        offset: Cell<usize>,
        busy: Cell<bool>,
    },
}

pub(crate) struct FilePosition<'a> {
    description: Rc<OpenFileDescription<'a>>,
}

impl<'a> FileTable<'a> {
    pub fn new(output: devmgr::uart::Session) -> Self {
        let description = Rc::new(OpenFileDescription::Output {
            session: output,
            flags: Cell::new(1),
        });
        Self {
            entries: [
                (
                    1,
                    Descriptor {
                        description: Rc::clone(&description),
                        close_on_exec: false,
                    },
                ),
                (
                    2,
                    Descriptor {
                        description,
                        close_on_exec: false,
                    },
                ),
            ]
            .into_iter()
            .collect(),
        }
    }

    pub fn get(&self, fd: usize) -> Option<&Descriptor<'a>> {
        self.entries.get(&fd)
    }

    pub fn get_mut(&mut self, fd: usize) -> Option<&mut Descriptor<'a>> {
        self.entries.get_mut(&fd)
    }

    pub fn insert(&mut self, descriptor: Descriptor<'a>, minimum: usize) -> Option<usize> {
        let mut available = minimum;
        for &fd in self.entries.range(minimum..).map(|(fd, _)| fd) {
            if fd != available {
                break;
            }
            available += 1;
        }
        if available > i32::MAX as usize {
            return None;
        }
        self.entries.insert(available, descriptor);
        Some(available)
    }

    pub fn replace(&mut self, fd: usize, descriptor: Descriptor<'a>) {
        self.entries.insert(fd, descriptor);
    }

    pub fn close(&mut self, fd: usize) -> bool {
        self.entries.remove(&fd).is_some()
    }
}

impl<'a> Descriptor<'a> {
    pub fn regular(file: fs::File<'a>, flags: u32, close_on_exec: bool) -> Self {
        Self {
            description: Rc::new(OpenFileDescription::Regular {
                file,
                flags: Cell::new(flags),
                offset: Cell::new(0),
                busy: Cell::new(false),
            }),
            close_on_exec,
        }
    }

    pub fn duplicate(&self, close_on_exec: bool) -> Self {
        Self {
            description: Rc::clone(&self.description),
            close_on_exec,
        }
    }
}

impl<'a> OpenFileDescription<'a> {
    pub fn flags(&self) -> u32 {
        match self {
            Self::Output { flags, .. }
            | Self::Directory { flags, .. }
            | Self::Regular { flags, .. } => flags.get(),
        }
    }

    pub fn set_flags(&self, value: u32) {
        let (Self::Output { flags, .. }
        | Self::Directory { flags, .. }
        | Self::Regular { flags, .. }) = self;
        flags.set((flags.get() & !(O_APPEND | O_NONBLOCK)) | (value & (O_APPEND | O_NONBLOCK)));
    }

    pub fn position(self: &Rc<Self>) -> Option<FilePosition<'a>> {
        let Self::Regular { busy, .. } = self.as_ref() else {
            return None;
        };
        if busy.replace(true) {
            return None;
        }
        Some(FilePosition {
            description: Rc::clone(self),
        })
    }
}

impl FilePosition<'_> {
    pub fn get(&self) -> usize {
        let OpenFileDescription::Regular { offset, .. } = self.description.as_ref() else {
            unreachable!()
        };
        offset.get()
    }

    pub fn set(&self, position: usize) {
        let OpenFileDescription::Regular { offset, .. } = self.description.as_ref() else {
            unreachable!()
        };
        offset.set(position);
    }
}

impl Drop for FilePosition<'_> {
    fn drop(&mut self) {
        let OpenFileDescription::Regular { busy, .. } = self.description.as_ref() else {
            unreachable!()
        };
        busy.set(false);
    }
}
